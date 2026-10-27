//! Process-local cache for trusted, pre-interface section fragments.
//! Entries cannot be deserialized or adopted from a GeneratedBuilding archive.

use crate::model::{
    ConfiguratorError, GeneratedMember, GeneratedNode, GeneratedSurface, SectionOutline, SemanticId,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::sync::Arc;

const HARD_MAX_SECTIONS: usize = 256;
const HARD_MAX_ENTITIES: usize = 100_000;
const HARD_MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GeneratorCacheLimits {
    pub max_sections: usize,
    pub max_entities: usize,
    /// Logical JSON bytes of key, raw fragment and section identities, not host RAM.
    pub max_payload_bytes: usize,
}
impl Default for GeneratorCacheLimits {
    fn default() -> Self {
        Self {
            max_sections: 64,
            max_entities: 50_000,
            max_payload_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GeneratorCacheStats {
    pub entries: usize,
    pub entities: usize,
    pub payload_bytes: usize,
    pub total_hits: u64,
    pub total_misses: u64,
    pub total_evictions: u64,
    pub total_raw_generation_calls: u64,
}

/// Per-call facts are outside the canonical GeneratedBuilding and its hash.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GenerationDiagnostics {
    pub raw_sections_recomputed: BTreeSet<SemanticId>,
    pub raw_sections_reused: BTreeSet<SemanticId>,
    pub raw_section_generation_calls: usize,
    pub interfaces_regenerated: usize,
    pub full_graph_postprocess: bool,
    pub cache_evictions: usize,
    pub uncached_sections: BTreeSet<SemanticId>,
    pub cache_stats: GeneratorCacheStats,
}

/// A fragment is captured after section_generate and before any interface edit.
/// Fingerprints, ledgers, routing and shared nodes are deliberately absent.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct RawSectionParts {
    pub(crate) nodes: BTreeMap<SemanticId, GeneratedNode>,
    pub(crate) members: BTreeMap<SemanticId, GeneratedMember>,
    pub(crate) surfaces: BTreeMap<SemanticId, GeneratedSurface>,
    pub(crate) outlines: BTreeMap<SemanticId, SectionOutline>,
    pub(crate) generation_limits: BTreeMap<SemanticId, String>,
}
impl RawSectionParts {
    fn entity_count(&self) -> Result<usize, ConfiguratorError> {
        [
            self.nodes.len(),
            self.members.len(),
            self.surfaces.len(),
            self.outlines.len(),
            self.generation_limits.len(),
        ]
        .into_iter()
        .try_fold(0usize, |sum, count| {
            sum.checked_add(count)
                .ok_or_else(|| ConfiguratorError::ResourceLimit {
                    reason: "Cache fragment entity count overflow".into(),
                })
        })
    }
    fn validate_pre_interface(&self, section: &SemanticId) -> Result<(), ConfiguratorError> {
        let owned = BTreeSet::from([section.clone()]);
        if self
            .nodes
            .iter()
            .any(|(key, n)| key != &n.id || n.owners != owned || !n.fingerprint.is_empty())
            || self
                .members
                .iter()
                .any(|(key, m)| key != &m.id || m.owners != owned || !m.fingerprint.is_empty())
            || self.surfaces.iter().any(|(key, s)| {
                key != &s.id || s.section_id != *section || !s.fingerprint.is_empty()
            })
            || self
                .outlines
                .iter()
                .any(|(key, outline)| key != section || outline.section_id != *section)
            || self.generation_limits.keys().any(|key| key != section)
        {
            return Err(ConfiguratorError::GeneratedBindingMismatch {
                message: "Cache fragment is not an unfinalized, single-section raw part".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CacheKey {
    bytes: Vec<u8>,
    sha256: [u8; 32],
}
impl CacheKey {
    /// Caller supplies the complete typed dependency tuple. Full bytes remain
    /// in the key so a digest match alone never authorizes fragment reuse.
    pub(crate) fn from_serializable<T: Serialize>(value: &T) -> Result<Self, ConfiguratorError> {
        let mut writer = BoundedBytes::new(HARD_MAX_PAYLOAD_BYTES);
        if let Err(error) = serde_json::to_writer(&mut writer, value) {
            return Err(if writer.exceeded {
                ConfiguratorError::ResourceLimit {
                    reason: "Cache key exceeds 64 MiB hard limit".into(),
                }
            } else {
                ConfiguratorError::Serialization {
                    message: error.to_string(),
                }
            });
        }
        let bytes = writer.bytes;
        let digest = Sha256::digest(&bytes);
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(&digest);
        Ok(Self { bytes, sha256 })
    }
}

#[derive(Debug)]
pub(crate) struct CacheUse {
    pub(crate) building_id: SemanticId,
    pub(crate) section_id: SemanticId,
    pub(crate) key: CacheKey,
    pub(crate) parts: Arc<RawSectionParts>,
    pub(crate) was_hit: bool,
}

#[derive(Debug)]
struct CacheEntry {
    building_id: SemanticId,
    section_id: SemanticId,
    key: CacheKey,
    parts: Arc<RawSectionParts>,
    entities: usize,
    payload_bytes: usize,
    last_used: u64,
}

/// No Serialize, Deserialize, archive constructor or global mutable singleton.
#[derive(Debug)]
pub struct GeneratorCache {
    limits: GeneratorCacheLimits,
    entries: Vec<CacheEntry>,
    tick: u64,
    total_hits: u64,
    total_misses: u64,
    total_evictions: u64,
    total_raw_generation_calls: u64,
}
impl Default for GeneratorCache {
    fn default() -> Self {
        Self {
            limits: GeneratorCacheLimits::default(),
            entries: Vec::new(),
            tick: 0,
            total_hits: 0,
            total_misses: 0,
            total_evictions: 0,
            total_raw_generation_calls: 0,
        }
    }
}
impl GeneratorCache {
    pub fn new(limits: GeneratorCacheLimits) -> Result<Self, ConfiguratorError> {
        if limits.max_sections > HARD_MAX_SECTIONS
            || limits.max_entities > HARD_MAX_ENTITIES
            || limits.max_payload_bytes > HARD_MAX_PAYLOAD_BYTES
        {
            return Err(ConfiguratorError::ResourceLimit {
                reason: "Cache limits exceed 256 sections, 100000 entities or 64 MiB payload"
                    .into(),
            });
        }
        Ok(Self {
            limits,
            ..Self::default()
        })
    }

    pub fn stats(&self) -> GeneratorCacheStats {
        GeneratorCacheStats {
            entries: self.entries.len(),
            entities: self.entries.iter().map(|entry| entry.entities).sum(),
            payload_bytes: self.entries.iter().map(|entry| entry.payload_bytes).sum(),
            total_hits: self.total_hits,
            total_misses: self.total_misses,
            total_evictions: self.total_evictions,
            total_raw_generation_calls: self.total_raw_generation_calls,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.tick = 0;
        self.total_hits = 0;
        self.total_misses = 0;
        self.total_evictions = 0;
        self.total_raw_generation_calls = 0;
    }

    fn exact_index(
        &self,
        building: &SemanticId,
        section: &SemanticId,
        key: &CacheKey,
    ) -> Option<usize> {
        self.entries.iter().position(|entry| {
            entry.building_id == *building
                && entry.section_id == *section
                && entry.key.sha256 == key.sha256
                && entry.key.bytes == key.bytes
        })
    }

    /// Read only: hits, LRU order and counters change only after a successful
    /// full generation calls commit_success.
    pub(crate) fn lookup(
        &self,
        building: &SemanticId,
        section: &SemanticId,
        key: &CacheKey,
    ) -> Result<Option<Arc<RawSectionParts>>, ConfiguratorError> {
        Ok(self
            .exact_index(building, section, key)
            .map(|index| Arc::clone(&self.entries[index].parts)))
    }

    /// Commit successful raw uses after the final interface and ledger pass.
    /// All serialization, key, hit and producer-call claims are checked before
    /// the first edit. Totals count only successfully committed calls.
    pub(crate) fn commit_success(
        &mut self,
        uses: Vec<CacheUse>,
        diag: &mut GenerationDiagnostics,
    ) -> Result<(), ConfiguratorError> {
        let expected_misses: BTreeSet<_> = uses
            .iter()
            .filter(|item| !item.was_hit)
            .map(|item| item.section_id.clone())
            .collect();
        let expected_calls = uses.iter().filter(|item| !item.was_hit).count();
        if diag.raw_section_generation_calls != expected_calls
            || diag.raw_sections_recomputed != expected_misses
        {
            return Err(ConfiguratorError::GeneratedBindingMismatch {
                message: "Raw section producer instrumentation differs from cache misses".into(),
            });
        }
        let mut seen = BTreeSet::new();
        let mut prepared = Vec::with_capacity(uses.len());
        for use_item in uses {
            use_item
                .parts
                .validate_pre_interface(&use_item.section_id)?;
            if !seen.insert((use_item.building_id.clone(), use_item.section_id.clone())) {
                return Err(ConfiguratorError::GeneratedBindingMismatch {
                    message: "Repeated section in cache commit".into(),
                });
            }
            let old = self.exact_index(&use_item.building_id, &use_item.section_id, &use_item.key);
            if use_item.was_hit != old.is_some() {
                return Err(ConfiguratorError::GeneratedBindingMismatch {
                    message: "Cache hit/miss claim differs from full key lookup".into(),
                });
            }
            if let Some(index) = old {
                if !Arc::ptr_eq(&use_item.parts, &self.entries[index].parts) {
                    return Err(ConfiguratorError::GeneratedBindingMismatch {
                        message: "Cache hit did not reuse the trusted entry".into(),
                    });
                }
            }
            let entities = use_item.parts.entity_count()?;
            let raw_bytes = if let Some(index) = old {
                Some(self.entries[index].payload_bytes)
            } else {
                serialized_size(&*use_item.parts)?
            };
            let payload_bytes = if let Some(size) = raw_bytes {
                if old.is_some() {
                    Some(size)
                } else {
                    size.checked_add(use_item.key.bytes.len())
                        .and_then(|n| n.checked_add(use_item.building_id.as_str().len()))
                        .and_then(|n| n.checked_add(use_item.section_id.as_str().len()))
                }
            } else {
                None
            };
            let cacheable = self.limits.max_sections > 0
                && entities <= self.limits.max_entities
                && payload_bytes.is_some_and(|bytes| bytes <= self.limits.max_payload_bytes);
            prepared.push(PreparedUse {
                use_item,
                entities,
                payload_bytes,
                cacheable,
            });
        }
        // Canonical commit order makes eviction deterministic even if caller
        // supplied sections in a different order.
        prepared.sort_by(|a, b| {
            (
                &a.use_item.building_id,
                &a.use_item.section_id,
                &a.use_item.key.bytes,
            )
                .cmp(&(
                    &b.use_item.building_id,
                    &b.use_item.section_id,
                    &b.use_item.key.bytes,
                ))
        });
        diag.raw_sections_reused.clear();
        diag.uncached_sections.clear();
        diag.cache_evictions = 0;
        for item in prepared {
            let CacheUse {
                building_id,
                section_id,
                key,
                parts,
                was_hit,
            } = item.use_item;
            if was_hit {
                self.total_hits = self.total_hits.saturating_add(1);
                diag.raw_sections_reused.insert(section_id.clone());
            } else {
                self.total_misses = self.total_misses.saturating_add(1);
                self.total_raw_generation_calls = self.total_raw_generation_calls.saturating_add(1);
            }
            if !item.cacheable {
                diag.uncached_sections.insert(section_id);
                continue;
            }
            self.tick = self.tick.saturating_add(1);
            if let Some(index) = self.exact_index(&building_id, &section_id, &key) {
                self.entries[index].last_used = self.tick;
            } else {
                self.entries.push(CacheEntry {
                    building_id,
                    section_id,
                    key,
                    parts,
                    entities: item.entities,
                    payload_bytes: item.payload_bytes.expect("cacheable payload"),
                    last_used: self.tick,
                });
            }
            while self.over_limits() {
                let index = self
                    .entries
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        (
                            a.last_used,
                            &a.building_id,
                            &a.section_id,
                            &a.key.sha256,
                            &a.key.bytes,
                        )
                            .cmp(&(
                                b.last_used,
                                &b.building_id,
                                &b.section_id,
                                &b.key.sha256,
                                &b.key.bytes,
                            ))
                    })
                    .map(|(index, _)| index)
                    .expect("over-limit cache has an entry");
                self.entries.remove(index);
                self.total_evictions = self.total_evictions.saturating_add(1);
                diag.cache_evictions += 1;
            }
        }
        diag.cache_stats = self.stats();
        Ok(())
    }

    fn over_limits(&self) -> bool {
        let stats = self.stats();
        stats.entries > self.limits.max_sections
            || stats.entities > self.limits.max_entities
            || stats.payload_bytes > self.limits.max_payload_bytes
    }
}

struct PreparedUse {
    use_item: CacheUse,
    entities: usize,
    payload_bytes: Option<usize>,
    cacheable: bool,
}

struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl BoundedBytes {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        }
    }
}
impl Write for BoundedBytes {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(data.len())
            .is_none_or(|n| n > self.limit)
        {
            self.exceeded = true;
            return Err(io::Error::new(io::ErrorKind::Other, "cache key byte limit"));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct BoundedCount {
    bytes: usize,
    exceeded: bool,
}
impl Write for BoundedCount {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .checked_add(data.len())
            .is_none_or(|n| n > HARD_MAX_PAYLOAD_BYTES)
        {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "cache payload byte limit",
            ));
        }
        self.bytes += data.len();
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn serialized_size(value: &RawSectionParts) -> Result<Option<usize>, ConfiguratorError> {
    let mut writer = BoundedCount {
        bytes: 0,
        exceeded: false,
    };
    if let Err(error) = serde_json::to_writer(&mut writer, value) {
        if writer.exceeded {
            return Ok(None);
        }
        return Err(ConfiguratorError::Serialization {
            message: error.to_string(),
        });
    }
    Ok(Some(writer.bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> SemanticId {
        SemanticId::new(value).expect("test ID")
    }
    fn use_item(
        section: &str,
        key: CacheKey,
        parts: Arc<RawSectionParts>,
        was_hit: bool,
    ) -> CacheUse {
        CacheUse {
            building_id: id("building"),
            section_id: id(section),
            key,
            parts,
            was_hit,
        }
    }

    #[test]
    fn hard_limit_is_enforced_without_generation() {
        let limits = GeneratorCacheLimits {
            max_sections: 257,
            ..GeneratorCacheLimits::default()
        };
        assert!(GeneratorCache::new(limits).is_err());
    }

    #[test]
    fn full_key_bytes_gate_lookup_and_success_only_commit() {
        let mut cache = GeneratorCache::default();
        let key = CacheKey::from_serializable(&("section", 1u8)).unwrap();
        let parts = Arc::new(RawSectionParts::default());
        assert!(
            cache
                .lookup(&id("building"), &id("section"), &key)
                .unwrap()
                .is_none()
        );
        let mut diagnostics = GenerationDiagnostics::default();
        diagnostics.raw_section_generation_calls = 1;
        diagnostics.raw_sections_recomputed.insert(id("section"));
        cache
            .commit_success(
                vec![use_item("section", key.clone(), Arc::clone(&parts), false)],
                &mut diagnostics,
            )
            .unwrap();
        assert_eq!(cache.stats().total_misses, 1);
        assert_eq!(diagnostics.raw_section_generation_calls, 1);
        assert!(
            cache
                .lookup(&id("building"), &id("section"), &key)
                .unwrap()
                .is_some()
        );
        let before = cache.stats();
        let hit = cache
            .lookup(&id("building"), &id("section"), &key)
            .unwrap()
            .unwrap();
        assert_eq!(cache.stats(), before); // lookup is read-only
        let mut diagnostics = GenerationDiagnostics::default();
        cache
            .commit_success(
                vec![use_item("section", key.clone(), hit, true)],
                &mut diagnostics,
            )
            .unwrap();
        assert_eq!(cache.stats().total_hits, 1);
        let mut same_digest_different_bytes = key;
        same_digest_different_bytes.bytes.push(b' ');
        assert!(
            cache
                .lookup(
                    &id("building"),
                    &id("section"),
                    &same_digest_different_bytes
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn disabled_cache_records_uncached_successful_raw_call() {
        let mut cache = GeneratorCache::new(GeneratorCacheLimits {
            max_sections: 0,
            ..GeneratorCacheLimits::default()
        })
        .unwrap();
        let key = CacheKey::from_serializable(&"input").unwrap();
        let mut diagnostics = GenerationDiagnostics::default();
        diagnostics.raw_section_generation_calls = 1;
        diagnostics.raw_sections_recomputed.insert(id("section"));
        cache
            .commit_success(
                vec![use_item(
                    "section",
                    key,
                    Arc::new(RawSectionParts::default()),
                    false,
                )],
                &mut diagnostics,
            )
            .unwrap();
        assert_eq!(cache.stats().entries, 0);
        assert!(diagnostics.uncached_sections.contains(&id("section")));
        assert_eq!(cache.stats().total_raw_generation_calls, 1);
    }

    #[test]
    fn claimed_miss_without_producer_call_cannot_mutate_cache() {
        let mut cache = GeneratorCache::default();
        let key = CacheKey::from_serializable(&"input").unwrap();
        let mut diagnostics = GenerationDiagnostics::default();
        let before = cache.stats();
        assert!(
            cache
                .commit_success(
                    vec![use_item(
                        "section",
                        key,
                        Arc::new(RawSectionParts::default()),
                        false
                    )],
                    &mut diagnostics
                )
                .is_err()
        );
        assert_eq!(cache.stats(), before);
    }
}
