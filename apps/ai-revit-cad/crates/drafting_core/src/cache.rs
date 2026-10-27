//! Incremental derived drawing state over an immutable authoritative DrawingSource.
use crate::*;
use crate::cache_index::MaintainedIndex;
use crate::cache_tessellation::build_tessellation;
use crate::cache_digest::{ContentDigestTree, DigestDomain, hash_bytes, hex_digest};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheSourceKey {
    pub document_id: String,
    pub runtime_instance_id: String,
    pub document_epoch: u64,
    pub document_revision: u64,
    pub drawing_id: DrawingId,
    pub drawing_revision: u64,
}

// No public constructor or cache entry point accepts this binding. It is issued
// only after the cache reads the immutable authority and its confirmed journal.
struct ConfirmedSourceChanges<'a> {
    previous: CacheSourceKey,
    current: CacheSourceKey,
    changes: &'a [DraftChange],
}

fn source_identity_matches(a: &CacheSourceKey, b: &CacheSourceKey) -> bool {
    a.document_id == b.document_id
        && a.runtime_instance_id == b.runtime_instance_id
        && a.document_epoch == b.document_epoch
        && a.drawing_id == b.drawing_id
}

fn validate_source_binding(
    source: &CacheSourceKey,
    drawing: &DraftDrawing,
    drawing_id: DrawingId,
    epoch: u64,
) -> DraftResult<()> {
    if source.document_id.is_empty()
        || source.runtime_instance_id.is_empty()
        || source.document_epoch != epoch
        || source.drawing_id != drawing_id
        || drawing.id != drawing_id
        || source.drawing_revision != drawing.revision
        || source.drawing_revision > source.document_revision
    {
        return Err(DraftError::new(
            "invalid_cache_source",
            "drawing identity or revision differs from its authoritative source key",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheGenerations {
    pub geometry: u64,
    pub style_visibility: u64,
    pub camera: u64,
    pub overlay: u64,
    /// Prepared CPU payload generation; it does not prove GPU resource reuse.
    pub resources: u64,
}

#[derive(Clone, Debug)]
pub struct CachedEntityRecord {
    pub id: DraftEntityId,
    pub layer_id: LayerId,
    pub geometry: Arc<EntityGeometry>,
    pub style: DraftStyle,
    pub revision: u64,
    pub source: Option<EntityProvenance>,
    pub geometry_generation: u64,
}
impl CachedEntityRecord {
    pub fn kind(&self) -> EntityKind { self.geometry.kind() }
}

#[derive(Clone, Debug)]
pub struct CachedGeometryBatch {
    pub owner_entity_id: DraftEntityId,
    pub geometry_generation: u64,
    pub geometry: Arc<TessellatedGeometry>,
    pub segments: Arc<[ScreenSegment]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CachedPrimitive {
    pub primitive_id: RenderPrimitiveId,
    pub owner_entity_id: DraftEntityId,
    pub start: Point2,
    pub end: Point2,
    pub appearance: DraftStyle,
}

#[derive(Clone, Debug)]
pub struct CachedFrameBase {
    pub batches: Arc<[CachedGeometryBatch]>,
    pub primitives: Arc<[CachedPrimitive]>,
    pub visible_entity_count: usize,
    /// All generated segments before viewport clipping, including cache hits.
    pub generated_segments: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedOverlay {
    pub selected_ids: Arc<[DraftEntityId]>,
    pub hover_id: Option<DraftEntityId>,
}

#[derive(Clone, Debug)]
pub struct CachedFrame {
    pub token: String,
    pub source: CacheSourceKey,
    pub source_fingerprint: String,
    pub source_sha256: String,
    pub source_hash_scope: String,
    pub generations: CacheGenerations,
    pub camera: Camera2D,
    pub pixel_error: f64,
    pub dpi_factor: f64,
    pub base: Arc<CachedFrameBase>,
    pub overlay: Arc<CachedOverlay>,
}

#[derive(Clone, Debug)]
pub struct CachedDrawingMetadata {
    pub source: CacheSourceKey,
    pub name: String,
    pub plane: DrawingPlane,
    pub entity_count: usize,
    pub counts_by_type: BTreeMap<String, usize>,
    pub layers: Arc<[DraftLayer]>,
    pub source_fingerprint: String,
    pub source_sha256: String,
    pub source_hash_scope: String,
}

#[derive(Clone, Debug)]
pub struct CapturedExport {
    pub snapshot: DraftSnapshot,
    pub display: DisplaySnapshot,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DraftingCacheLimits {
    pub max_tessellation_bytes: usize,
    pub max_tessellation_entries: usize,
    pub max_retained_frame_bases: usize,
    pub max_index_nodes: usize,
    pub max_segments_per_curve: usize,
    pub max_generated_segments: usize,
    pub max_timing_samples: usize,
}
impl Default for DraftingCacheLimits {
    fn default() -> Self {
        Self {
            max_tessellation_bytes: 64 * 1024 * 1024,
            max_tessellation_entries: 20_000,
            max_retained_frame_bases: 4,
            max_index_nodes: 4 * MAX_DOCUMENT_ENTITIES,
            max_segments_per_curve: 4096,
            max_generated_segments: 100_000,
            max_timing_samples: 256,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CacheUpdateSummary {
    pub changed_entities: usize,
    pub geometry_invalidations: usize,
    pub changed_layers: usize,
    pub changed_import_assets: usize,
    pub replayed: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CpuTimingSummary {
    pub samples: usize,
    pub last_ms: f64,
    pub median_ms: f64,
    pub p95_ms: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CacheMetrics {
    pub resets: u64,
    pub commits: u64,
    pub replay_noops: u64,
    pub source_entity_visits: u64,
    /// Geometry copied into maintained cache records; excludes explicit export DTO copies.
    pub geometry_clones: u64,
    pub export_snapshot_geometry_clones: u64,
    pub export_analytic_geometry_clones: u64,
    pub geometry_invalidations: u64,
    pub digest_leaf_updates: u64,
    pub digest_node_hashes: u64,
    pub index_updates: u64,
    pub index_nodes: usize,
    pub index_reserved_nodes: usize,
    pub index_queries: u64,
    pub index_nodes_visited: u64,
    pub index_leaves_examined: u64,
    pub tessellation_hits: u64,
    pub tessellation_misses: u64,
    pub tessellation_evictions: u64,
    pub tessellation_entries: usize,
    pub tessellation_bytes: usize,
    pub projected_entities: u64,
    pub projection_hits: u64,
    pub analytic_fallback_segments: u64,
    pub distance_checks: u64,
    pub frames_prepared: u64,
    pub frame_hits: u64,
    pub overlay_updates: u64,
    pub export_materializations: u64,
    pub retained_frame_bases: usize,
    pub generated_segments: usize,
    pub primitive_count: usize,
    pub generations: CacheGenerations,
    pub limits: Option<DraftingCacheLimits>,
    pub reset_cpu: CpuTimingSummary,
    pub sync_cpu: CpuTimingSummary,
    pub prepare_cpu: CpuTimingSummary,
    pub pick_cpu: CpuTimingSummary,
    pub source_lookup_cpu: CpuTimingSummary,
    pub visibility_query_cpu: CpuTimingSummary,
    pub tessellation_cpu: CpuTimingSummary,
    pub projection_cpu: CpuTimingSummary,
    pub content_hash_cpu: CpuTimingSummary,
}

const SOURCE_HASH_SCOPE: &str = "drafting-lab-active-drawing-merkle-sha256-v1";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TessellationKey {
    entity_id: DraftEntityId,
    geometry_generation: u64,
    pixels_per_metre_bits: u64,
    dpi_factor_bits: u64,
    pixel_error_bits: u64,
    max_segments_per_curve: usize,
}

#[derive(Clone, Debug)]
struct TessellationEntry {
    mesh: Arc<TessellatedGeometry>,
    bytes: usize,
    last_used: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct CameraKey {
    center_x: u64,
    center_y: u64,
    pixels_per_metre: u64,
    viewport_width: u64,
    viewport_height: u64,
}

impl CameraKey {
    fn new(camera: &Camera2D) -> Self {
        Self {
            center_x: camera.center.x.to_bits(),
            center_y: camera.center.y.to_bits(),
            pixels_per_metre: camera.pixels_per_metre.to_bits(),
            viewport_width: camera.viewport_width.to_bits(),
            viewport_height: camera.viewport_height.to_bits(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BaseKey {
    visual_generation: u64,
    camera: CameraKey,
    dpi_factor_bits: u64,
    pixel_error_bits: u64,
    max_segments_per_curve: usize,
    max_generated_segments: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct FrameKey {
    source: CacheSourceKey,
    camera: CameraKey,
    camera_revision: u64,
    options_generation: u64,
    pixel_error_bits: u64,
    dpi_factor_bits: u64,
    max_segments_per_curve: usize,
    max_primitives: usize,
    selected_ids: Vec<DraftEntityId>,
    hover_id: Option<DraftEntityId>,
    generations: CacheGenerations,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ProjectionKey {
    entity_id: DraftEntityId,
    geometry_generation: u64,
    camera: CameraKey,
    mesh_pixels_per_metre_bits: u64,
    mesh_dpi_factor_bits: u64,
    mesh_pixel_error_bits: u64,
    mesh_max_segments_per_curve: usize,
}

#[derive(Clone, Debug)]
struct LayerState {
    visible: bool,
    locked: bool,
}

#[derive(Clone, Debug)]
struct EntityDelta {
    id: DraftEntityId,
    old: Option<Arc<CachedEntityRecord>>,
    new: Option<Arc<CachedEntityRecord>>,
    geometry_hash: Option<[u8; 32]>,
    bounds: Option<Bounds2>,
    entity_leaf: Option<[u8; 32]>,
}

#[derive(Clone, Debug)]
struct LayerDelta {
    id: LayerId,
    old: Option<DraftLayer>,
    new: Option<DraftLayer>,
    leaf: Option<[u8; 32]>,
}

#[derive(Clone, Debug)]
struct AssetDelta {
    id: ImportAssetId,
    leaf: Option<[u8; 32]>,
}

/// Incremental, document-bound derived state for one active drawing.
/// Persistent authority remains in the source document. Generic updates obtain
/// their adjacent journal directly from that source; the Lab compatibility path
/// additionally verifies its authoritative receipt and history event.
pub struct DraftingCache {
    limits: DraftingCacheLimits,
    source: Option<CacheSourceKey>,
    metadata: Option<CachedDrawingMetadata>,
    entities: BTreeMap<DraftEntityId, Arc<CachedEntityRecord>>,
    entity_bounds: BTreeMap<DraftEntityId, Bounds2>,
    geometry_hashes: BTreeMap<DraftEntityId, [u8; 32]>,
    leaf_hashes: BTreeMap<(u8, u64), [u8; 32]>,
    layers: BTreeMap<LayerId, DraftLayer>,
    layer_states: BTreeMap<LayerId, LayerState>,
    assets: BTreeMap<ImportAssetId, ImportAsset>,
    digest_tree: ContentDigestTree,
    global_index: MaintainedIndex,
    layer_indexes: BTreeMap<LayerId, MaintainedIndex>,
    counts_by_type: BTreeMap<String, usize>,
    layer_entity_counts: BTreeMap<LayerId, usize>,
    visible_entity_count: usize,
    generations: CacheGenerations,
    visual_generation: u64,
    camera_key: Option<CameraKey>,
    last_base_key: Option<BaseKey>,
    last_frame_key: Option<FrameKey>,
    last_frame: Option<Arc<CachedFrame>>,
    last_frame_options: Option<DisplayOptions>,
    live_frame_bases: Vec<Weak<CachedFrameBase>>,
    projection_cache: BTreeMap<ProjectionKey, Weak<[ScreenSegment]>>,
    tessellations: BTreeMap<TessellationKey, TessellationEntry>,
    tessellation_lru: BTreeSet<(u64, TessellationKey)>,
    tessellation_bytes: usize,
    lru_clock: u64,
    metrics: CacheMetrics,
    reset_samples: VecDeque<f64>,
    sync_samples: VecDeque<f64>,
    prepare_samples: VecDeque<f64>,
    pick_samples: VecDeque<f64>,
    source_lookup_samples: VecDeque<f64>,
    visibility_query_samples: VecDeque<f64>,
    tessellation_samples: VecDeque<f64>,
    projection_samples: VecDeque<f64>,
    content_hash_samples: VecDeque<f64>,
}

pub type IncrementalDraftingCache = DraftingCache;

impl DraftingCache {
    pub fn new(limits: DraftingCacheLimits) -> DraftResult<Self> {
        validate_cache_limits(&limits)?;
        Ok(Self {
            limits: limits.clone(),
            source: None,
            metadata: None,
            entities: BTreeMap::new(),
            entity_bounds: BTreeMap::new(),
            geometry_hashes: BTreeMap::new(),
            leaf_hashes: BTreeMap::new(),
            layers: BTreeMap::new(),
            layer_states: BTreeMap::new(),
            assets: BTreeMap::new(),
            digest_tree: ContentDigestTree::new(),
            global_index: MaintainedIndex::new(),
            layer_indexes: BTreeMap::new(),
            counts_by_type: BTreeMap::new(),
            layer_entity_counts: BTreeMap::new(),
            visible_entity_count: 0,
            generations: CacheGenerations::default(),
            visual_generation: 0,
            camera_key: None,
            last_base_key: None,
            last_frame_key: None,
            last_frame: None,
            last_frame_options: None,
            live_frame_bases: Vec::new(),
            projection_cache: BTreeMap::new(),
            tessellations: BTreeMap::new(),
            tessellation_lru: BTreeSet::new(),
            tessellation_bytes: 0,
            lru_clock: 0,
            metrics: CacheMetrics::default(),
            reset_samples: VecDeque::new(),
            sync_samples: VecDeque::new(),
            prepare_samples: VecDeque::new(),
            pick_samples: VecDeque::new(),
            source_lookup_samples: VecDeque::new(),
            visibility_query_samples: VecDeque::new(),
            tessellation_samples: VecDeque::new(),
            projection_samples: VecDeque::new(),
            content_hash_samples: VecDeque::new(),
        })
    }

    pub fn reset(
        &mut self,
        doc: &(impl DrawingSource + ?Sized),
        drawing_id: DrawingId,
        epoch: u64,
    ) -> DraftResult<()> {
        let started = Instant::now();
        let result = self.reset_inner(doc, drawing_id, epoch);
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        push_timing(&mut self.reset_samples, elapsed, self.limits.max_timing_samples);
        self.metrics.reset_cpu = timing_summary(&self.reset_samples);
        if let Err(error) = result {
            self.invalidate();
            return Err(error);
        }
        self.metrics.resets = self.metrics.resets.saturating_add(1);
        self.metrics.generations = self.generations;
        self.refresh_metrics();
        Ok(())
    }

    fn reset_inner(
        &mut self,
        doc: &(impl DrawingSource + ?Sized),
        drawing_id: DrawingId,
        epoch: u64,
    ) -> DraftResult<()> {
        let source_lookup_started = Instant::now();
        let drawing = doc.drawing(drawing_id).ok_or_else(|| {
            DraftError::new("drawing_not_found", format!("drawing {drawing_id} is absent"))
        })?;
        let source = doc.cache_source_key(drawing_id, epoch)?;
        validate_source_binding(&source, drawing, drawing_id, epoch)?;
        self.record_source_lookup(source_lookup_started.elapsed());

        let mut content_hash_elapsed = Duration::ZERO;
        let mut entities = BTreeMap::new();
        let mut entity_bounds = BTreeMap::new();
        let mut geometry_hashes = BTreeMap::new();
        let mut leaf_hashes = BTreeMap::new();
        let mut layers = BTreeMap::new();
        let mut layer_states = BTreeMap::new();
        let mut assets = BTreeMap::new();
        let mut digest_tree = ContentDigestTree::new();
        let mut counts_by_type = BTreeMap::new();
        let mut layer_entity_counts: BTreeMap<LayerId, usize> =
            drawing.layers.keys().map(|id| (*id, 0)).collect();
        let mut visible_entity_count = 0usize;
        let mut geometry_clones = 0u64;
        let mut digest_leaf_updates = 0u64;
        let mut digest_node_hashes = 0u64;
        let mut global_items = Vec::with_capacity(drawing.entities.len());
        let mut layer_items: BTreeMap<LayerId, Vec<(DraftEntityId, Bounds2)>> =
            drawing.layers.keys().map(|id| (*id, Vec::new())).collect();

        for (id, layer) in &drawing.layers {
            layers.insert(*id, layer.clone());
            layer_states.insert(
                *id,
                LayerState {
                    visible: layer.visible,
                    locked: layer.locked,
                },
            );
            let hash_started = Instant::now();
            let leaf = layer_leaf_hash(layer)?;
            let node_hashes = digest_tree.upsert(DigestDomain::Layer, id.get(), leaf);
            content_hash_elapsed += hash_started.elapsed();
            digest_leaf_updates += 1;
            digest_node_hashes = digest_node_hashes.saturating_add(node_hashes);
            leaf_hashes.insert((DigestDomain::Layer as u8, id.get()), leaf);
        }
        for (id, asset) in &drawing.import_sources {
            assets.insert(*id, asset.clone());
            let hash_started = Instant::now();
            let leaf = asset_leaf_hash(asset)?;
            let node_hashes = digest_tree.upsert(DigestDomain::ImportAsset, id.get(), leaf);
            content_hash_elapsed += hash_started.elapsed();
            digest_leaf_updates += 1;
            digest_node_hashes = digest_node_hashes.saturating_add(node_hashes);
            leaf_hashes.insert((DigestDomain::ImportAsset as u8, id.get()), leaf);
        }
        for (id, entity) in &drawing.entities {
            let hash_started = Instant::now();
            let geometry_hash = geometry_hash(&entity.geometry)?;
            let leaf = entity_leaf_hash(entity, geometry_hash)?;
            let node_hashes = digest_tree.upsert(DigestDomain::Entity, id.get(), leaf);
            content_hash_elapsed += hash_started.elapsed();
            let bounds = entity.geometry.bounds()?;
            digest_leaf_updates += 1;
            digest_node_hashes = digest_node_hashes.saturating_add(node_hashes);
            leaf_hashes.insert((DigestDomain::Entity as u8, id.get()), leaf);
            geometry_hashes.insert(*id, geometry_hash);
            entity_bounds.insert(*id, bounds);
            global_items.push((*id, bounds));
            layer_items
                .get_mut(&entity.layer_id)
                .ok_or_else(|| {
                    DraftError::new(
                        "invalid_cache_source",
                        format!("entity {id} refers to absent layer {}", entity.layer_id),
                    )
                })?
                .push((*id, bounds));
            let kind = kind_key(entity.kind());
            *counts_by_type.entry(kind).or_insert(0) += 1;
            *layer_entity_counts.entry(entity.layer_id).or_insert(0) += 1;
            if drawing
                .layers
                .get(&entity.layer_id)
                .is_some_and(|layer| layer.visible)
            {
                visible_entity_count += 1;
            }
            geometry_clones = geometry_clones.saturating_add(1);
            entities.insert(
                *id,
                Arc::new(CachedEntityRecord {
                    id: *id,
                    layer_id: entity.layer_id,
                    geometry: Arc::new(entity.geometry.clone()),
                    style: entity.style.clone(),
                    revision: entity.revision,
                    source: entity.source.clone(),
                    geometry_generation: 1,
                }),
            );
        }
        let global_index = MaintainedIndex::bulk_load(global_items)?;
        let mut layer_indexes = BTreeMap::new();
        for (id, items) in layer_items {
            if !items.is_empty() {
                layer_indexes.insert(id, MaintainedIndex::bulk_load(items)?);
            }
        }
        let index_nodes = index_node_count(&global_index, &layer_indexes);
        if index_nodes > self.limits.max_index_nodes {
            return Err(DraftError::new(
                "cache_index_budget",
                format!(
                    "rebuilt spatial indexes need {index_nodes} nodes; limit is {}",
                    self.limits.max_index_nodes
                ),
            ));
        }
        let root_hash_started = Instant::now();
        let source_sha256 = source_root_hash(&source, drawing, &digest_tree)?;
        content_hash_elapsed += root_hash_started.elapsed();
        let source_fingerprint = format!("sha256:{source_sha256}");
        let metadata = CachedDrawingMetadata {
            source: source.clone(),
            name: drawing.name.clone(),
            plane: drawing.plane.clone(),
            entity_count: entities.len(),
            counts_by_type: counts_by_type.clone(),
            layers: Arc::from(drawing.layers.values().cloned().collect::<Vec<_>>()),
            source_fingerprint,
            source_sha256,
            source_hash_scope: SOURCE_HASH_SCOPE.to_owned(),
        };
        self.record_content_hash(content_hash_elapsed);

        // Publish only after the full rebuild has succeeded.
        self.source = Some(source);
        self.metadata = Some(metadata);
        self.entities = entities;
        self.entity_bounds = entity_bounds;
        self.geometry_hashes = geometry_hashes;
        self.leaf_hashes = leaf_hashes;
        self.layers = layers;
        self.layer_states = layer_states;
        self.assets = assets;
        self.digest_tree = digest_tree;
        self.global_index = global_index;
        self.layer_indexes = layer_indexes;
        self.counts_by_type = counts_by_type;
        self.layer_entity_counts = layer_entity_counts;
        self.visible_entity_count = visible_entity_count;
        self.generations = CacheGenerations {
            geometry: 1,
            style_visibility: 1,
            camera: 0,
            overlay: 0,
            resources: 0,
        };
        self.visual_generation = 1;
        self.camera_key = None;
        self.last_base_key = None;
        self.last_frame_key = None;
        self.last_frame = None;
        self.last_frame_options = None;
        self.projection_cache.clear();
        self.tessellations.clear();
        self.tessellation_lru.clear();
        self.tessellation_bytes = 0;
        self.metrics.source_entity_visits = self
            .metrics
            .source_entity_visits
            .saturating_add(self.entities.len() as u64);
        self.metrics.geometry_clones = self.metrics.geometry_clones.saturating_add(geometry_clones);
        self.metrics.digest_leaf_updates = self
            .metrics
            .digest_leaf_updates
            .saturating_add(digest_leaf_updates);
        self.metrics.digest_node_hashes = self
            .metrics
            .digest_node_hashes
            .saturating_add(digest_node_hashes);
        self.metrics.index_updates = self
            .metrics
            .index_updates
            .saturating_add((self.entities.len().saturating_mul(2)) as u64);
        self.metrics.index_nodes = index_nodes;
        self.metrics.generations = self.generations;
        Ok(())
    }

    pub fn performance(&self) -> CacheMetrics {
        let mut metrics = self.metrics.clone();
        metrics.generations = self.generations;
        metrics.limits = Some(self.limits.clone());
        metrics.reset_cpu = timing_summary(&self.reset_samples);
        metrics.sync_cpu = timing_summary(&self.sync_samples);
        metrics.prepare_cpu = timing_summary(&self.prepare_samples);
        metrics.pick_cpu = timing_summary(&self.pick_samples);
        metrics.source_lookup_cpu = timing_summary(&self.source_lookup_samples);
        metrics.visibility_query_cpu = timing_summary(&self.visibility_query_samples);
        metrics.tessellation_cpu = timing_summary(&self.tessellation_samples);
        metrics.projection_cpu = timing_summary(&self.projection_samples);
        metrics.content_hash_cpu = timing_summary(&self.content_hash_samples);
        metrics.tessellation_entries = self.tessellations.len();
        metrics.tessellation_bytes = self.tessellation_bytes;
        metrics.retained_frame_bases = self.live_base_count();
        metrics.index_nodes = index_node_count(&self.global_index, &self.layer_indexes);
        metrics.index_reserved_nodes =
            reserved_index_node_count(&self.global_index, &self.layer_indexes);
        metrics
    }

    fn refresh_metrics(&mut self) {
        self.metrics.generations = self.generations;
        self.metrics.index_nodes = index_node_count(&self.global_index, &self.layer_indexes);
        self.metrics.index_reserved_nodes =
            reserved_index_node_count(&self.global_index, &self.layer_indexes);
        self.metrics.tessellation_entries = self.tessellations.len();
        self.metrics.tessellation_bytes = self.tessellation_bytes;
        self.metrics.retained_frame_bases = self.live_base_count();
    }

    fn invalidate(&mut self) {
        self.metadata = None;
        self.last_frame_key = None;
        self.last_frame = None;
        self.last_frame_options = None;
        self.last_base_key = None;
        self.projection_cache.retain(|_, segments| segments.strong_count() > 0);
        self.refresh_metrics();
    }

    fn live_base_count(&self) -> usize {
        self.live_frame_bases
            .iter()
            .filter(|base| base.strong_count() > 0)
            .count()
    }

    fn record_source_lookup(&mut self, elapsed: std::time::Duration) {
        push_timing(
            &mut self.source_lookup_samples,
            elapsed.as_secs_f64() * 1000.0,
            self.limits.max_timing_samples,
        );
        self.metrics.source_lookup_cpu = timing_summary(&self.source_lookup_samples);
    }

    fn record_content_hash(&mut self, elapsed: std::time::Duration) {
        push_timing(
            &mut self.content_hash_samples,
            elapsed.as_secs_f64() * 1000.0,
            self.limits.max_timing_samples,
        );
        self.metrics.content_hash_cpu = timing_summary(&self.content_hash_samples);
    }
}

fn validate_cache_limits(limits: &DraftingCacheLimits) -> DraftResult<()> {
    if limits.max_tessellation_bytes == 0
        || limits.max_tessellation_bytes > 512 * 1024 * 1024
        || limits.max_tessellation_entries == 0
        || limits.max_tessellation_entries > 80_000
        || limits.max_retained_frame_bases == 0
        || limits.max_retained_frame_bases > 4
        || limits.max_index_nodes == 0
        || limits.max_index_nodes > 4 * MAX_DOCUMENT_ENTITIES
        || limits.max_segments_per_curve == 0
        || limits.max_segments_per_curve > 65_536
        || limits.max_generated_segments == 0
        || limits.max_generated_segments > 1_000_000
        || limits.max_timing_samples == 0
        || limits.max_timing_samples > 4096
    {
        return Err(DraftError::new(
            "cache_limits",
            "cache retention, curve, segment or timing limits are outside supported bounds",
        ));
    }
    Ok(())
}

fn index_node_count(
    global: &MaintainedIndex,
    by_layer: &BTreeMap<LayerId, MaintainedIndex>,
) -> usize {
    by_layer.values().fold(global.allocated_nodes(), |total, index| {
        total.saturating_add(index.allocated_nodes())
    })
}

fn reserved_index_node_count(
    global: &MaintainedIndex,
    by_layer: &BTreeMap<LayerId, MaintainedIndex>,
) -> usize {
    by_layer.values().fold(global.reserved_nodes(), |total, index| {
        total.saturating_add(index.reserved_nodes())
    })
}

fn projected_index_node_count(
    global_index: &MaintainedIndex,
    current_entity_count: usize,
    current_layer_counts: &BTreeMap<LayerId, usize>,
    layer_indexes: &BTreeMap<LayerId, MaintainedIndex>,
    deltas: &[EntityDelta],
) -> DraftResult<usize> {
    let mut final_entity_count = current_entity_count;
    let mut affected_layers = BTreeMap::<LayerId, usize>::new();
    for delta in deltas {
        let old_layer = delta.old.as_ref().map(|record| record.layer_id);
        let new_layer = delta.new.as_ref().map(|record| record.layer_id);
        match (delta.old.is_some(), delta.new.is_some()) {
            (true, false) => {
                final_entity_count = final_entity_count.checked_sub(1).ok_or_else(|| {
                    DraftError::new("cache_index_corrupt", "entity count underflow")
                })?;
            }
            (false, true) => {
                final_entity_count = final_entity_count.checked_add(1).ok_or_else(|| {
                    DraftError::new("cache_index_corrupt", "entity count overflow")
                })?;
            }
            _ => {}
        }
        if old_layer != new_layer {
            if let Some(layer_id) = old_layer {
                let count = affected_layers.entry(layer_id).or_insert_with(|| {
                    current_layer_counts.get(&layer_id).copied().unwrap_or(0)
                });
                *count = count.checked_sub(1).ok_or_else(|| {
                    DraftError::new("cache_index_corrupt", "layer entity count underflow")
                })?;
            }
            if let Some(layer_id) = new_layer {
                let count = affected_layers.entry(layer_id).or_insert_with(|| {
                    current_layer_counts.get(&layer_id).copied().unwrap_or(0)
                });
                *count = count.checked_add(1).ok_or_else(|| {
                    DraftError::new("cache_index_corrupt", "layer entity count overflow")
                })?;
            }
        }
    }

    let mut projected = index_node_count(global_index, layer_indexes)
        .checked_sub(global_index.allocated_nodes())
        .ok_or_else(|| DraftError::new("cache_index_corrupt", "global index count underflow"))?
        .checked_add(nodes_for_entity_count(final_entity_count))
        .ok_or_else(|| DraftError::new("cache_index_budget", "global index count overflow"))?;
    for (layer_id, final_count) in affected_layers {
        let old_nodes = layer_indexes
            .get(&layer_id)
            .map(MaintainedIndex::allocated_nodes)
            .unwrap_or(0);
        projected = projected
            .checked_sub(old_nodes)
            .ok_or_else(|| DraftError::new("cache_index_corrupt", "layer index count underflow"))?
            .checked_add(nodes_for_entity_count(final_count))
            .ok_or_else(|| DraftError::new("cache_index_budget", "layer index count overflow"))?;
    }
    Ok(projected)
}

fn nodes_for_entity_count(count: usize) -> usize {
    if count == 0 {
        0
    } else {
        count.saturating_mul(2).saturating_sub(1)
    }
}

fn push_timing(samples: &mut VecDeque<f64>, value: f64, max: usize) {
    if max == 0 {
        return;
    }
    while samples.len() >= max {
        samples.pop_front();
    }
    samples.push_back(value);
}

fn timing_summary(samples: &VecDeque<f64>) -> CpuTimingSummary {
    if samples.is_empty() {
        return CpuTimingSummary::default();
    }
    let mut sorted: Vec<_> = samples.iter().copied().collect();
    sorted.sort_by(f64::total_cmp);
    let median_ms = if sorted.len() % 2 == 0 {
        let upper = sorted.len() / 2;
        (sorted[upper - 1] + sorted[upper]) * 0.5
    } else {
        sorted[sorted.len() / 2]
    };
    let p95_index = ((sorted.len() * 95).div_ceil(100)).saturating_sub(1);
    CpuTimingSummary {
        samples: sorted.len(),
        last_ms: *samples.back().unwrap_or(&0.0),
        median_ms,
        p95_ms: sorted[p95_index.min(sorted.len() - 1)],
    }
}

fn kind_key(kind: EntityKind) -> String {
    match kind {
        EntityKind::Line => "line",
        EntityKind::Circle => "circle",
        EntityKind::Arc => "arc",
        EntityKind::Polyline => "polyline",
    }
    .to_owned()
}

fn geometry_hash(geometry: &EntityGeometry) -> DraftResult<[u8; 32]> {
    let bytes = serde_json::to_vec(geometry)
        .map_err(|error| DraftError::new("serialization", error.to_string()))?;
    Ok(hash_bytes("drawing.geometry.v1", &bytes))
}

fn entity_leaf_hash(entity: &DraftEntity, geometry_digest: [u8; 32]) -> DraftResult<[u8; 32]> {
    let bytes = serde_json::to_vec(&(
        entity.id,
        entity.layer_id,
        geometry_digest,
        &entity.style,
        entity.revision,
        &entity.source,
    ))
    .map_err(|error| DraftError::new("serialization", error.to_string()))?;
    Ok(hash_bytes("drawing.entity-leaf.v1", &bytes))
}

fn layer_leaf_hash(layer: &DraftLayer) -> DraftResult<[u8; 32]> {
    let bytes = serde_json::to_vec(layer)
        .map_err(|error| DraftError::new("serialization", error.to_string()))?;
    Ok(hash_bytes("drawing.layer-leaf.v1", &bytes))
}

fn asset_leaf_hash(asset: &ImportAsset) -> DraftResult<[u8; 32]> {
    let bytes = serde_json::to_vec(asset)
        .map_err(|error| DraftError::new("serialization", error.to_string()))?;
    Ok(hash_bytes("drawing.import-asset-leaf.v1", &bytes))
}

fn source_root_hash(
    source: &CacheSourceKey,
    drawing: &DraftDrawing,
    tree: &ContentDigestTree,
) -> DraftResult<String> {
    let bytes = serde_json::to_vec(&(
        &source.document_id,
        source.document_revision,
        drawing.id,
        &drawing.name,
        drawing.revision,
        &drawing.plane,
        tree.root_hash(),
    ))
    .map_err(|error| DraftError::new("serialization", error.to_string()))?;
    Ok(hex_digest(&hash_bytes("drawing.source-root.v1", &bytes)))
}

fn style_digest(style: &DraftStyle) -> DraftResult<[u8; 32]> {
    let bytes = serde_json::to_vec(style)
        .map_err(|error| DraftError::new("serialization", error.to_string()))?;
    Ok(hash_bytes("drawing.style.v1", &bytes))
}

impl DraftingCache {
    /// Synchronize the one adjacent project revision directly from its authority.
    ///
    /// No external receipt or delta is accepted. BIM-only and drawing-metadata
    /// revisions rebind the source while preserving analytic geometry, maintained
    /// indexes, tessellations and prepared frame bases. A gap or changed runtime
    /// identity requires an explicit reset; it never triggers an implicit scan.
    pub fn sync_from_source(
        &mut self,
        doc: &(impl DrawingSource + ?Sized),
        epoch: u64,
    ) -> DraftResult<CacheUpdateSummary> {
        let started = Instant::now();
        let result = self.sync_from_source_inner(doc, epoch);
        push_timing(
            &mut self.sync_samples,
            started.elapsed().as_secs_f64() * 1000.0,
            self.limits.max_timing_samples,
        );
        self.metrics.sync_cpu = timing_summary(&self.sync_samples);
        if result.is_ok() {
            self.refresh_metrics();
        }
        result
    }

    fn sync_from_source_inner(
        &mut self,
        doc: &(impl DrawingSource + ?Sized),
        epoch: u64,
    ) -> DraftResult<CacheUpdateSummary> {
        let lookup_started = Instant::now();
        let previous = self.source.clone()
            .ok_or_else(|| stale_cache_error("cache has not been reset"))?;
        if self.metadata.as_ref().is_none_or(|metadata| metadata.source != previous) {
            return Err(stale_cache_error("cache metadata is unavailable or stale"));
        }
        let current = doc.cache_source_key(previous.drawing_id, epoch)
            .map_err(|_| stale_cache_error("active drawing is no longer present"))?;
        if !source_identity_matches(&previous, &current)
            || current.document_epoch != epoch
        {
            return Err(stale_cache_error(
                "cache belongs to a different document instance or epoch",
            ));
        }
        if current == previous {
            self.record_source_lookup(lookup_started.elapsed());
            self.metrics.replay_noops = self.metrics.replay_noops.saturating_add(1);
            return Ok(CacheUpdateSummary { replayed: true, ..CacheUpdateSummary::default() });
        }
        if previous.document_revision.checked_add(1) != Some(current.document_revision) {
            return Err(DraftError::new(
                "cache_revision_gap",
                "confirmed source is not adjacent to the cached project revision",
            ));
        }
        let drawing = doc.drawing(previous.drawing_id)
            .ok_or_else(|| stale_cache_error("active drawing was deleted"))?;
        validate_source_binding(&current, drawing, previous.drawing_id, epoch)?;
        if self.metadata.as_ref().is_some_and(|metadata| metadata.plane != drawing.plane) {
            return Err(stale_cache_error("drawing plane changed; explicitly reset the cache"));
        }
        let changes = doc.confirmed_drafting_changes(previous.document_revision)?;
        if changes.len() > MAX_COMMAND_CHANGES {
            return Err(DraftError::new("cache_change_budget", "confirmed journal exceeds its change budget"));
        }
        if doc.cache_source_key(previous.drawing_id, epoch)? != current {
            return Err(stale_cache_error("source changed while confirming the journal"));
        }
        self.record_source_lookup(lookup_started.elapsed());
        self.apply_confirmed_changes(
            ConfirmedSourceChanges { previous, current, changes: &changes },
            drawing,
        )
    }

    pub fn sync_after_commit(
        &mut self,
        doc: &LabDocument,
        receipt: &CommandReceipt,
        epoch: u64,
    ) -> DraftResult<CacheUpdateSummary> {
        let started = Instant::now();
        let result = self.sync_inner(doc, receipt, epoch);
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        push_timing(&mut self.sync_samples, elapsed, self.limits.max_timing_samples);
        self.metrics.sync_cpu = timing_summary(&self.sync_samples);
        if result.is_ok() {
            self.refresh_metrics();
        }
        result
    }

    fn sync_inner(
        &mut self,
        doc: &LabDocument,
        receipt: &CommandReceipt,
        epoch: u64,
    ) -> DraftResult<CacheUpdateSummary> {
        let lookup_started = Instant::now();
        let cached_source = self
            .source
            .as_ref()
            .cloned()
            .ok_or_else(|| stale_cache_error("cache has not been reset"))?;
        let replay_record = doc
            .receipts
            .get(&receipt.command_id)
            .ok_or_else(|| {
                DraftError::new(
                    "cache_receipt_mismatch",
                    "commit receipt is not present in the document authority",
                )
            })?;
        if !receipt_matches_authority(receipt, &replay_record.receipt) {
            return Err(DraftError::new(
                "cache_receipt_mismatch",
                "commit receipt differs from the document's authoritative receipt",
            ));
        }
        if cached_source.document_id != doc.document_id()
            || cached_source.runtime_instance_id != doc.runtime_instance_id()
            || cached_source.document_epoch != epoch
        {
            return Err(stale_cache_error(
                "cache belongs to a different document instance or epoch",
            ));
        }

        if receipt.replayed {
            let current = doc
                .cache_source_key(cached_source.drawing_id, epoch)
                .map_err(|_| stale_cache_error("active drawing is no longer present"))?;
            if self.metadata.is_none()
                || self.source.as_ref() != Some(&current)
                || self
                    .metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.source != current)
            {
                return Err(stale_cache_error(
                    "replayed receipt does not match the current cache source",
                ));
            }
            self.record_source_lookup(lookup_started.elapsed());
            self.metrics.replay_noops = self.metrics.replay_noops.saturating_add(1);
            return Ok(CacheUpdateSummary {
                replayed: true,
                ..CacheUpdateSummary::default()
            });
        }

        if self.metadata.is_none()
            || self
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata.source != cached_source)
            || cached_source.document_revision != receipt.revision_before
            || doc.revision() != receipt.revision_after
            || receipt.revision_before.checked_add(1) != Some(receipt.revision_after)
        {
            return Err(DraftError::new(
                "cache_revision_gap",
                "commit receipt is not contiguous with the cache and current document",
            ));
        }

        let current_source = doc
            .cache_source_key(cached_source.drawing_id, epoch)
            .map_err(|_| stale_cache_error("active drawing is no longer present"))?;
        let drawing = doc
            .drawings()
            .get(&cached_source.drawing_id)
            .ok_or_else(|| stale_cache_error("active drawing was deleted"))?;
        self.record_source_lookup(lookup_started.elapsed());

        let history_index = match &replay_record.action {
            ReplayAction::Apply { .. } => doc.history_cursor.checked_sub(1).ok_or_else(|| {
                DraftError::new("cache_history_mismatch", "apply receipt has no current history event")
            })?,
            ReplayAction::Undo { .. } => doc.history_cursor,
            ReplayAction::Redo { .. } => doc.history_cursor.checked_sub(1).ok_or_else(|| {
                DraftError::new("cache_history_mismatch", "redo receipt has no current history event")
            })?,
            ReplayAction::Pruned { .. } => {
                return Err(DraftError::new(
                    "cache_history_mismatch",
                    "pruned receipt cannot authorize an incremental cache update",
                ));
            }
        };
        let history = doc.history.get(history_index).ok_or_else(|| {
            DraftError::new(
                "cache_history_mismatch",
                "receipt does not identify a current history event",
            )
        })?;
        let history_command_matches = match &replay_record.action {
            ReplayAction::Apply { expected_revision, .. } => {
                *expected_revision == receipt.revision_before
                    && history.command_id == receipt.command_id
                    && history.revision_before == receipt.revision_before
                    && history.revision_after == receipt.revision_after
            }
            ReplayAction::Undo { expected_revision } | ReplayAction::Redo { expected_revision } => {
                *expected_revision == receipt.revision_before
                    && replay_record
                .target_command_id
                .as_deref()
                .is_some_and(|target| target == history.command_id)
            }
            ReplayAction::Pruned { .. } => false,
        };
        if !history_command_matches
            || matches!(&replay_record.action, ReplayAction::Undo { .. })
                && doc.history_cursor != history_index
            || matches!(&replay_record.action, ReplayAction::Redo { .. })
                && doc.history_cursor != history_index + 1
            || matches!(&replay_record.action, ReplayAction::Apply { .. })
                && doc.history_cursor != history_index + 1
        {
            return Err(DraftError::new(
                "cache_history_mismatch",
                "receipt action, target and history cursor disagree",
            ));
        }

        self.apply_confirmed_changes(
            ConfirmedSourceChanges {
                previous: cached_source,
                current: current_source,
                changes: &history.changes,
            },
            drawing,
        )
    }

    // This private binding is constructed only after obtaining a confirmed
    // journal directly from a source, or after the legacy receipt checks above.
    fn apply_confirmed_changes(
        &mut self,
        feed: ConfirmedSourceChanges<'_>,
        drawing: &DraftDrawing,
    ) -> DraftResult<CacheUpdateSummary> {
        if self.source.as_ref() != Some(&feed.previous)
            || self.metadata.as_ref().is_none_or(|metadata| metadata.source != feed.previous)
        {
            return Err(stale_cache_error("confirmed feed no longer matches the cache"));
        }
        let cached_source = feed.previous;
        let current_source = feed.current;
        let mut entity_ids = BTreeSet::new();
        let mut layer_ids = BTreeSet::new();
        let mut asset_ids = BTreeSet::new();
        for change in feed.changes {
            match change {
                DraftChange::Entity { drawing_id, id, .. }
                    if *drawing_id == cached_source.drawing_id =>
                {
                    entity_ids.insert(*id);
                }
                DraftChange::Layer { drawing_id, id, .. }
                    if *drawing_id == cached_source.drawing_id =>
                {
                    layer_ids.insert(*id);
                }
                DraftChange::ImportAsset { drawing_id, id, .. }
                    if *drawing_id == cached_source.drawing_id =>
                {
                    asset_ids.insert(*id);
                }
                DraftChange::Drawing {
                    drawing_id,
                    before,
                    after,
                } if *drawing_id == cached_source.drawing_id => {
                    if before.is_none() || after.is_none() {
                        return Err(stale_cache_error(
                            "history creates or removes the cached active drawing",
                        ));
                    }
                    return Err(DraftError::new(
                        "cache_history_mismatch",
                        "whole-drawing replacement is outside the incremental cache contract",
                    ));
                }
                _ => {}
            }
        }

        let mut content_hash_elapsed = Duration::ZERO;
        let mut entity_inputs = Vec::with_capacity(entity_ids.len());
        let mut geometry_invalidations = 0usize;
        let mut entity_style_changed = false;
        let mut entity_visibility_changed = false;
        let mut visual_changed = false;

        for id in entity_ids.iter().copied() {
            let old = self.entities.get(&id).cloned();
            let current = drawing.entities.get(&id);
            if old.is_none() && current.is_none() {
                continue;
            }
            let geometry_hash_started = Instant::now();
            let (next_geometry_hash, geometry_changed) = match (old.as_ref(), current) {
                (Some(previous), Some(next))
                    if geometry_bits_equal(previous.geometry.as_ref(), &next.geometry) =>
                {
                    match self.geometry_hashes.get(&id).copied() {
                        Some(digest) => (Some(digest), false),
                        None => (Some(geometry_hash(&next.geometry)?), true),
                    }
                }
                (Some(_), Some(next)) => (Some(geometry_hash(&next.geometry)?), true),
                (None, Some(next)) => (Some(geometry_hash(&next.geometry)?), true),
                (Some(_), None) => (None, true),
                (None, None) => (None, false),
            };
            content_hash_elapsed += geometry_hash_started.elapsed();
            let bounds = current.map(|entity| entity.geometry.bounds()).transpose()?;
            let old_visible = old
                .as_ref()
                .and_then(|record| self.layer_states.get(&record.layer_id))
                .is_some_and(|layer| layer.visible);
            let new_visible = current
                .and_then(|entity| drawing.layers.get(&entity.layer_id))
                .is_some_and(|layer| layer.visible);
            let visibility_changed = old.is_some() && current.is_some() && old_visible != new_visible;
            let style_hash_started = Instant::now();
            let style_changed = match (old.as_ref(), current) {
                (Some(previous), Some(next)) => {
                    style_digest(&previous.style)? != style_digest(&next.style)?
                }
                _ => false,
            };
            content_hash_elapsed += style_hash_started.elapsed();
            let entity_hash_started = Instant::now();
            let entity_leaf = match (current, next_geometry_hash) {
                (Some(entity), Some(digest)) => Some(entity_leaf_hash(entity, digest)?),
                _ => None,
            };
            content_hash_elapsed += entity_hash_started.elapsed();
            let entity_visual_change = geometry_changed
                || style_changed
                || visibility_changed
                || old.is_none()
                || current.is_none();
            if geometry_changed {
                geometry_invalidations += 1;
            }
            entity_style_changed |= style_changed;
            entity_visibility_changed |= visibility_changed;
            visual_changed |= entity_visual_change;
            entity_inputs.push((
                id,
                old,
                current,
                next_geometry_hash,
                bounds,
                entity_leaf,
                geometry_changed,
            ));
        }

        let mut layer_deltas = Vec::with_capacity(layer_ids.len());
        let mut layer_visibility_changed = false;
        for id in layer_ids.iter().copied() {
            let old = self.layers.get(&id).cloned();
            let new = drawing.layers.get(&id).cloned();
            let invalidates_visibility = old.as_ref().map(|layer| layer.visible)
                != new.as_ref().map(|layer| layer.visible);
            let hash_started = Instant::now();
            let leaf = new.as_ref().map(layer_leaf_hash).transpose()?;
            content_hash_elapsed += hash_started.elapsed();
            layer_visibility_changed |= invalidates_visibility;
            layer_deltas.push(LayerDelta {
                id,
                old,
                new,
                leaf,
            });
        }
        visual_changed |= layer_visibility_changed;

        let mut asset_deltas = Vec::with_capacity(asset_ids.len());
        for id in asset_ids.iter().copied() {
            let new = drawing.import_sources.get(&id);
            let hash_started = Instant::now();
            let leaf = new.map(asset_leaf_hash).transpose()?;
            content_hash_elapsed += hash_started.elapsed();
            asset_deltas.push(AssetDelta {
                id,
                leaf,
            });
        }

        let next_geometry_generation = if geometry_invalidations > 0 {
            self.generations.geometry.checked_add(1).ok_or_else(|| {
                DraftError::new("cache_generation_exhausted", "geometry generation exhausted")
            })?
        } else {
            self.generations.geometry
        };
        let next_style_visibility_generation = if entity_style_changed
            || entity_visibility_changed
            || layer_visibility_changed
        {
            self.generations
                .style_visibility
                .checked_add(1)
                .ok_or_else(|| {
                    DraftError::new(
                        "cache_generation_exhausted",
                        "style/visibility generation exhausted",
                    )
                })?
        } else {
            self.generations.style_visibility
        };
        let next_visual_generation = if visual_changed {
            self.visual_generation.checked_add(1).ok_or_else(|| {
                DraftError::new("cache_generation_exhausted", "visual generation exhausted")
            })?
        } else {
            self.visual_generation
        };

        let mut entity_deltas = Vec::with_capacity(entity_inputs.len());
        let mut geometry_clones = 0u64;
        for (
            id,
            old,
            current,
            geometry_hash,
            bounds,
            entity_leaf,
            geometry_changed,
        ) in entity_inputs
        {
            let new = match current {
                Some(entity) => {
                    let (geometry, geometry_generation) = if geometry_changed {
                        geometry_clones = geometry_clones.saturating_add(1);
                        (Arc::new(entity.geometry.clone()), next_geometry_generation)
                    } else if let Some(previous) = old.as_ref() {
                        (
                            Arc::clone(&previous.geometry),
                            previous.geometry_generation,
                        )
                    } else {
                        return Err(DraftError::new(
                            "cache_delta_corrupt",
                            format!("entity {id} reused geometry without a previous record"),
                        ));
                    };
                    Some(Arc::new(CachedEntityRecord {
                        id,
                        layer_id: entity.layer_id,
                        geometry,
                        style: entity.style.clone(),
                        revision: entity.revision,
                        source: entity.source.clone(),
                        geometry_generation,
                    }))
                }
                None => None,
            };
            entity_deltas.push(EntityDelta {
                id,
                old,
                new,
                geometry_hash,
                bounds,
                entity_leaf,
            });
        }

        let mut next_digest_tree = self.digest_tree.clone();
        let mut leaf_updates: Vec<((u8, u64), Option<[u8; 32]>)> = Vec::new();
        let mut digest_leaf_updates = 0u64;
        let mut digest_node_hashes = 0u64;
        let merkle_started = Instant::now();
        for delta in &entity_deltas {
            digest_leaf_hash_change(
                &mut next_digest_tree,
                &self.leaf_hashes,
                DigestDomain::Entity,
                delta.id.get(),
                delta.entity_leaf,
                &mut digest_leaf_updates,
                &mut digest_node_hashes,
                &mut leaf_updates,
            );
        }
        for delta in &layer_deltas {
            digest_leaf_hash_change(
                &mut next_digest_tree,
                &self.leaf_hashes,
                DigestDomain::Layer,
                delta.id.get(),
                delta.leaf,
                &mut digest_leaf_updates,
                &mut digest_node_hashes,
                &mut leaf_updates,
            );
        }
        for delta in &asset_deltas {
            digest_leaf_hash_change(
                &mut next_digest_tree,
                &self.leaf_hashes,
                DigestDomain::ImportAsset,
                delta.id.get(),
                delta.leaf,
                &mut digest_leaf_updates,
                &mut digest_node_hashes,
                &mut leaf_updates,
            );
        }
        content_hash_elapsed += merkle_started.elapsed();
        let root_hash_started = Instant::now();
        let source_sha256 = source_root_hash(&current_source, drawing, &next_digest_tree)?;
        content_hash_elapsed += root_hash_started.elapsed();
        self.record_content_hash(content_hash_elapsed);

        let next_generations = CacheGenerations {
            geometry: next_geometry_generation,
            style_visibility: next_style_visibility_generation,
            ..self.generations
        };
        let current_metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| stale_cache_error("cache metadata is unavailable"))?;
        let next_metadata_layers: Arc<[DraftLayer]> = if layer_deltas.is_empty() {
            Arc::clone(&current_metadata.layers)
        } else {
            Arc::from(drawing.layers.values().cloned().collect::<Vec<_>>())
        };
        let next_metadata = CachedDrawingMetadata {
            source: current_source.clone(),
            name: drawing.name.clone(),
            plane: drawing.plane.clone(),
            entity_count: if entity_deltas.is_empty() {
                self.entities.len()
            } else {
                self.entities.len()
                    + entity_deltas
                        .iter()
                        .filter(|delta| delta.old.is_none() && delta.new.is_some())
                        .count()
                    - entity_deltas
                        .iter()
                        .filter(|delta| delta.old.is_some() && delta.new.is_none())
                        .count()
            },
            counts_by_type: staged_counts_by_type(&self.counts_by_type, &entity_deltas),
            source_fingerprint: format!("sha256:{source_sha256}"),
            source_sha256,
            source_hash_scope: SOURCE_HASH_SCOPE.to_owned(),
            layers: next_metadata_layers,
        };

        // All serialization, geometry bounds and generation checks have
        // succeeded. Estimate index growth before mutating any maintained
        // hierarchy so a budget failure can be repaired by an explicit reset.
        let projected_index_nodes = match projected_index_node_count(
            &self.global_index,
            self.entities.len(),
            &self.layer_entity_counts,
            &self.layer_indexes,
            &entity_deltas,
        ) {
            Ok(nodes) => nodes,
            Err(error) => {
                if error.code == "cache_index_corrupt" {
                    self.invalidate();
                }
                return Err(error);
            }
        };
        if projected_index_nodes > self.limits.max_index_nodes {
            self.invalidate();
            return Err(DraftError::new(
                "cache_index_budget",
                format!(
                    "incremental spatial indexes may need {projected_index_nodes} nodes; limit is {}",
                    self.limits.max_index_nodes
                ),
            ));
        }

        // The following in-place delta operations are bounded by the unique
        // IDs in the authoritative history event. If an index rejects a
        // prevalidated bound, it may already be partially changed, so make
        // the cache inaccessible and require an explicit rebuild.
        let mut index_updates = 0u64;
        for delta in &entity_deltas {
            if let Err(error) = apply_entity_index_delta(
                &mut self.global_index,
                &mut self.layer_indexes,
                &self.entity_bounds,
                delta,
                &mut index_updates,
            ) {
                self.invalidate();
                return Err(error);
            }
        }
        for delta in &entity_deltas {
            match &delta.new {
                Some(record) => {
                    self.entities.insert(delta.id, Arc::clone(record));
                    if let Some(hash) = delta.geometry_hash {
                        self.geometry_hashes.insert(delta.id, hash);
                    }
                    if let Some(bounds) = delta.bounds {
                        self.entity_bounds.insert(delta.id, bounds);
                    }
                }
                None => {
                    self.entities.remove(&delta.id);
                    self.geometry_hashes.remove(&delta.id);
                    self.entity_bounds.remove(&delta.id);
                }
            }
        }
        for delta in &layer_deltas {
            match &delta.new {
                Some(layer) => {
                    self.layers.insert(delta.id, layer.clone());
                    self.layer_states.insert(
                        delta.id,
                        LayerState {
                            visible: layer.visible,
                            locked: layer.locked,
                        },
                    );
                    self.layer_entity_counts.entry(delta.id).or_insert(0);
                }
                None => {
                    self.layers.remove(&delta.id);
                    self.layer_states.remove(&delta.id);
                    self.layer_entity_counts.remove(&delta.id);
                    self.layer_indexes.remove(&delta.id);
                }
            }
        }
        for delta in &asset_deltas {
            match drawing.import_sources.get(&delta.id) {
                Some(asset) => {
                    self.assets.insert(delta.id, asset.clone());
                }
                None => {
                    self.assets.remove(&delta.id);
                }
            }
        }
        for delta in &entity_deltas {
            match (&delta.old, &delta.new) {
                (Some(old), Some(new)) if old.layer_id == new.layer_id => {}
                (Some(old), Some(new)) => {
                    if let Some(count) = self.layer_entity_counts.get_mut(&old.layer_id) {
                        *count = count.saturating_sub(1);
                    }
                    *self.layer_entity_counts.entry(new.layer_id).or_insert(0) += 1;
                }
                (Some(old), None) => {
                    if let Some(count) = self.layer_entity_counts.get_mut(&old.layer_id) {
                        *count = count.saturating_sub(1);
                    }
                }
                (None, Some(new)) => {
                    *self.layer_entity_counts.entry(new.layer_id).or_insert(0) += 1;
                }
                (None, None) => {}
            }
            update_type_counts(&mut self.counts_by_type, delta.old.as_deref(), delta.new.as_deref());
        }
        self.layer_indexes
            .retain(|_, index| index.bounds().is_some());
        for (key, value) in leaf_updates {
            match value {
                Some(hash) => {
                    self.leaf_hashes.insert(key, hash);
                }
                None => {
                    self.leaf_hashes.remove(&key);
                }
            }
        }
        self.digest_tree = next_digest_tree;
        self.visible_entity_count = self
            .layer_entity_counts
            .iter()
            .filter(|(id, _)| self.layer_states.get(id).is_some_and(|layer| layer.visible))
            .map(|(_, count)| *count)
            .sum();
        self.source = Some(current_source);
        self.metadata = Some(next_metadata);
        self.generations = next_generations;
        self.visual_generation = next_visual_generation;
        self.last_frame_key = None;
        self.metrics.commits = self.metrics.commits.saturating_add(1);
        self.metrics.source_entity_visits = self
            .metrics
            .source_entity_visits
            .saturating_add(entity_ids.len() as u64);
        self.metrics.geometry_clones = self.metrics.geometry_clones.saturating_add(geometry_clones);
        self.metrics.geometry_invalidations = self
            .metrics
            .geometry_invalidations
            .saturating_add(geometry_invalidations as u64);
        self.metrics.digest_leaf_updates = self
            .metrics
            .digest_leaf_updates
            .saturating_add(digest_leaf_updates);
        self.metrics.digest_node_hashes = self
            .metrics
            .digest_node_hashes
            .saturating_add(digest_node_hashes);
        self.metrics.index_updates = self.metrics.index_updates.saturating_add(index_updates);
        self.metrics.generations = self.generations;
        Ok(CacheUpdateSummary {
            changed_entities: entity_deltas.len(),
            geometry_invalidations,
            changed_layers: layer_deltas.len(),
            changed_import_assets: asset_deltas.len(),
            replayed: false,
        })
    }
}

fn stale_cache_error(message: &str) -> DraftError {
    DraftError::new("stale_cache_source", message)
}

fn receipt_matches_authority(receipt: &CommandReceipt, authoritative: &CommandReceipt) -> bool {
    receipt.command_id == authoritative.command_id
        && receipt.revision_before == authoritative.revision_before
        && receipt.revision_after == authoritative.revision_after
        && receipt.created_ids == authoritative.created_ids
        && receipt.updated_ids == authoritative.updated_ids
        && receipt.deleted_ids == authoritative.deleted_ids
        && receipt.created_drawing_ids == authoritative.created_drawing_ids
        && receipt.changed_layer_ids == authoritative.changed_layer_ids
        && receipt.import_asset_ids == authoritative.import_asset_ids
}

fn digest_leaf_hash_change(
    tree: &mut ContentDigestTree,
    existing: &BTreeMap<(u8, u64), [u8; 32]>,
    domain: DigestDomain,
    id: u64,
    next: Option<[u8; 32]>,
    leaf_updates: &mut u64,
    node_hashes: &mut u64,
    staged: &mut Vec<((u8, u64), Option<[u8; 32]>)>,
) {
    let key = (domain as u8, id);
    let previous = existing.get(&key).copied();
    match next {
        Some(hash) if previous != Some(hash) => {
            *leaf_updates = leaf_updates.saturating_add(1);
            *node_hashes = node_hashes.saturating_add(tree.upsert(domain, id, hash));
            staged.push((key, Some(hash)));
        }
        None if previous.is_some() => {
            *leaf_updates = leaf_updates.saturating_add(1);
            *node_hashes = node_hashes.saturating_add(tree.remove(domain, id));
            staged.push((key, None));
        }
        _ => {}
    }
}

fn update_type_counts(
    counts: &mut BTreeMap<String, usize>,
    old: Option<&CachedEntityRecord>,
    new: Option<&CachedEntityRecord>,
) {
    if let Some(old) = old {
        let key = kind_key(old.kind());
        if let Some(count) = counts.get_mut(&key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(&key);
            }
        }
    }
    if let Some(new) = new {
        *counts.entry(kind_key(new.kind())).or_insert(0) += 1;
    }
}

fn staged_counts_by_type(
    current: &BTreeMap<String, usize>,
    deltas: &[EntityDelta],
) -> BTreeMap<String, usize> {
    let mut counts = current.clone();
    for delta in deltas {
        update_type_counts(&mut counts, delta.old.as_deref(), delta.new.as_deref());
    }
    counts
}

fn apply_entity_index_delta(
    global: &mut MaintainedIndex,
    by_layer: &mut BTreeMap<LayerId, MaintainedIndex>,
    old_bounds: &BTreeMap<DraftEntityId, Bounds2>,
    delta: &EntityDelta,
    updates: &mut u64,
) -> DraftResult<()> {
    let old_layer = delta.old.as_ref().map(|record| record.layer_id);
    let new_layer = delta.new.as_ref().map(|record| record.layer_id);
    let previous_bounds = old_bounds.get(&delta.id).copied();
    match (previous_bounds, delta.bounds) {
        (Some(_), None) => {
            global.remove(delta.id);
            *updates = updates.saturating_add(1);
        }
        (None, Some(bounds)) => {
            global.upsert(delta.id, bounds)?;
            *updates = updates.saturating_add(1);
        }
        (Some(old), Some(new)) if old != new => {
            global.upsert(delta.id, new)?;
            *updates = updates.saturating_add(1);
        }
        _ => {}
    }
    match (old_layer, new_layer, previous_bounds, delta.bounds) {
        (Some(old_layer), Some(new_layer), Some(old_bounds), Some(new_bounds))
            if old_layer == new_layer =>
        {
            if old_bounds != new_bounds {
                by_layer.entry(old_layer).or_default().upsert(delta.id, new_bounds)?;
                *updates = updates.saturating_add(1);
            }
        }
        (Some(old_layer), Some(new_layer), Some(_), Some(new_bounds)) => {
            if let Some(index) = by_layer.get_mut(&old_layer) {
                index.remove(delta.id);
                *updates = updates.saturating_add(1);
            }
            by_layer.entry(new_layer).or_default().upsert(delta.id, new_bounds)?;
            *updates = updates.saturating_add(1);
        }
        (Some(old_layer), None, Some(_), None) => {
            if let Some(index) = by_layer.get_mut(&old_layer) {
                index.remove(delta.id);
                *updates = updates.saturating_add(1);
            }
        }
        (None, Some(new_layer), None, Some(new_bounds)) => {
            by_layer.entry(new_layer).or_default().upsert(delta.id, new_bounds)?;
            *updates = updates.saturating_add(1);
        }
        _ => {}
    }
    Ok(())
}

fn point_bits_equal(a: Point2, b: Point2) -> bool {
    a.x.to_bits() == b.x.to_bits() && a.y.to_bits() == b.y.to_bits()
}

fn geometry_bits_equal(a: &EntityGeometry, b: &EntityGeometry) -> bool {
    match (a, b) {
        (EntityGeometry::Line { start: a0, end: a1 }, EntityGeometry::Line { start: b0, end: b1 }) => {
            point_bits_equal(*a0, *b0) && point_bits_equal(*a1, *b1)
        }
        (
            EntityGeometry::Circle { center: a0, radius: ar },
            EntityGeometry::Circle { center: b0, radius: br },
        ) => point_bits_equal(*a0, *b0) && ar.to_bits() == br.to_bits(),
        (
            EntityGeometry::Arc { center: ac, radius: ar, start_angle: as_, sweep_angle: aw },
            EntityGeometry::Arc { center: bc, radius: br, start_angle: bs, sweep_angle: bw },
        ) => {
            point_bits_equal(*ac, *bc)
                && ar.to_bits() == br.to_bits()
                && as_.to_bits() == bs.to_bits()
                && aw.to_bits() == bw.to_bits()
        }
        (
            EntityGeometry::Polyline { vertices: av, closed: ac },
            EntityGeometry::Polyline { vertices: bv, closed: bc },
        ) => {
            ac == bc
                && av.len() == bv.len()
                && av.iter().zip(bv).all(|(a, b)| {
                    point_bits_equal(a.point, b.point) && a.bulge.to_bits() == b.bulge.to_bits()
                })
        }
        _ => false,
    }
}

impl DraftingCache {
    fn require_current(&self, doc: &(impl DrawingSource + ?Sized), epoch: u64) -> DraftResult<CacheSourceKey> {
        let cached = self
            .source
            .as_ref()
            .ok_or_else(|| stale_cache_error("cache has not been reset"))?;
        let current = doc
            .cache_source_key(cached.drawing_id, epoch)
            .map_err(|_| stale_cache_error("cached active drawing is no longer present"))?;
        if !source_identity_matches(cached, &current)
            || current.document_epoch != epoch
            || current != *cached
            || self
                .metadata
                .as_ref()
                .is_none_or(|metadata| metadata.source != current)
        {
            return Err(stale_cache_error(
                "document source changed; synchronize the confirmed receipt or reset the cache",
            ));
        }
        Ok(current)
    }

    /// Current, identity-checked source binding without materializing a snapshot.
    pub fn source_key(
        &self,
        doc: &(impl DrawingSource + ?Sized),
        epoch: u64,
    ) -> DraftResult<CacheSourceKey> {
        self.require_current(doc, epoch)
    }

    pub fn metadata(
        &self,
        doc: &(impl DrawingSource + ?Sized),
        epoch: u64,
    ) -> DraftResult<&CachedDrawingMetadata> {
        self.require_current(doc, epoch)?;
        self.metadata
            .as_ref()
            .ok_or_else(|| stale_cache_error("cache metadata is unavailable"))
    }

    pub fn entity(
        &self,
        doc: &(impl DrawingSource + ?Sized),
        epoch: u64,
        id: DraftEntityId,
    ) -> DraftResult<Arc<CachedEntityRecord>> {
        self.require_current(doc, epoch)?;
        self.entities
            .get(&id)
            .cloned()
            .ok_or_else(|| DraftError::new("entity_not_found", format!("entity {id} is absent")))
    }

    pub fn bounds(
        &self,
        doc: &(impl DrawingSource + ?Sized),
        epoch: u64,
        visible_only: bool,
    ) -> DraftResult<Option<Bounds2>> {
        self.require_current(doc, epoch)?;
        if !visible_only {
            return Ok(self.global_index.bounds());
        }
        let mut result: Option<Bounds2> = None;
        for (layer_id, index) in &self.layer_indexes {
            if !self
                .layer_states
                .get(layer_id)
                .is_some_and(|layer| layer.visible)
            {
                continue;
            }
            if let Some(bounds) = index.bounds() {
                result = Some(result.map_or(bounds, |current| current.union(bounds)));
            }
        }
        Ok(result)
    }

    pub fn prepare(
        &mut self,
        doc: &(impl DrawingSource + ?Sized),
        epoch: u64,
        camera: &Camera2D,
        options: &DisplayOptions,
    ) -> DraftResult<Arc<CachedFrame>> {
        let started = Instant::now();
        let result = self.prepare_inner(doc, epoch, camera, options);
        push_timing(
            &mut self.prepare_samples,
            started.elapsed().as_secs_f64() * 1000.0,
            self.limits.max_timing_samples,
        );
        self.metrics.prepare_cpu = timing_summary(&self.prepare_samples);
        if result.is_ok() {
            self.refresh_metrics();
        }
        result
    }

    fn prepare_inner(
        &mut self,
        doc: &(impl DrawingSource + ?Sized),
        epoch: u64,
        camera: &Camera2D,
        options: &DisplayOptions,
    ) -> DraftResult<Arc<CachedFrame>> {
        let source_lookup_started = Instant::now();
        let source = self.require_current(doc, epoch)?;
        self.record_source_lookup(source_lookup_started.elapsed());
        camera.validate()?;
        options.validate()?;

        let selected: BTreeSet<_> = options.selected_ids.iter().copied().collect();
        if selected.iter().any(|id| !self.entities.contains_key(id))
            || options
                .hover_id
                .is_some_and(|id| !self.entities.contains_key(&id))
        {
            return Err(DraftError::new(
                "entity_not_found",
                "selection or hover refers to absent entity",
            ));
        }
        let selected_ids: Vec<_> = selected.into_iter().collect();
        let camera_key = CameraKey::new(camera);
        let camera_changed = self.camera_key != Some(camera_key);
        let previous_overlay = self
            .last_frame
            .as_ref()
            .map(|frame| Arc::clone(&frame.overlay));
        let overlay_changed = previous_overlay.as_ref().map_or(
            !selected_ids.is_empty() || options.hover_id.is_some(),
            |overlay| {
                overlay.selected_ids.as_ref() != selected_ids.as_slice()
                    || overlay.hover_id != options.hover_id
            },
        );

        let mut next_generations = self.generations;
        if camera_changed {
            next_generations.camera = next_generations
                .camera
                .checked_add(1)
                .ok_or_else(|| {
                    DraftError::new("cache_generation_exhausted", "camera generation exhausted")
                })?;
        }
        if overlay_changed {
            next_generations.overlay = next_generations
                .overlay
                .checked_add(1)
                .ok_or_else(|| {
                    DraftError::new("cache_generation_exhausted", "overlay generation exhausted")
                })?;
        }

        let work_budget = options
            .max_primitives
            .min(self.limits.max_generated_segments);
        let max_segments_per_curve = options
            .max_segments_per_curve
            .min(self.limits.max_segments_per_curve);
        let base_key = BaseKey {
            visual_generation: self.visual_generation,
            camera: camera_key,
            dpi_factor_bits: options.dpi_factor.to_bits(),
            pixel_error_bits: options.pixel_error.to_bits(),
            max_segments_per_curve,
            max_generated_segments: work_budget,
        };
        let reusable_base = self
            .last_frame
            .as_ref()
            .filter(|_| self.last_base_key.as_ref() == Some(&base_key))
            .map(|frame| Arc::clone(&frame.base));
        let needs_new_base = reusable_base.is_none();
        if needs_new_base {
            self.live_frame_bases
                .retain(|base| base.strong_count() > 0);
        }
        if needs_new_base && !self.can_publish_frame_base() {
            return Err(DraftError::new(
                "cache_frame_base_budget",
                "externally retained frame bases occupy the configured retention limit",
            ));
        }
        if needs_new_base {
            next_generations.resources = next_generations
                .resources
                .checked_add(1)
                .ok_or_else(|| {
                    DraftError::new(
                        "cache_generation_exhausted",
                        "CPU resource generation exhausted",
                    )
                })?;
        }

        let frame_key = FrameKey {
            source: source.clone(),
            camera: camera_key,
            camera_revision: camera.revision,
            options_generation: options.generation,
            pixel_error_bits: options.pixel_error.to_bits(),
            dpi_factor_bits: options.dpi_factor.to_bits(),
            max_segments_per_curve: options.max_segments_per_curve,
            max_primitives: options.max_primitives,
            selected_ids: selected_ids.clone(),
            hover_id: options.hover_id,
            generations: next_generations,
        };
        if self.last_frame_key.as_ref() == Some(&frame_key) {
            if let Some(frame) = self.last_frame.as_ref().map(Arc::clone) {
                self.metrics.frame_hits = self.metrics.frame_hits.saturating_add(1);
                self.metrics.frames_prepared = self.metrics.frames_prepared.saturating_add(1);
                return Ok(frame);
            }
        }

        let base = if let Some(base) = reusable_base {
            base
        } else {
            let base = Arc::new(self.build_frame_base(
                camera,
                camera_key,
                options,
                max_segments_per_curve,
                work_budget,
            )?);
            self.live_frame_bases.push(Arc::downgrade(&base));
            self.last_base_key = Some(base_key);
            base
        };

        let overlay = match previous_overlay {
            Some(previous)
                if previous.selected_ids.as_ref() == selected_ids.as_slice()
                    && previous.hover_id == options.hover_id => previous,
            _ => Arc::new(CachedOverlay {
                selected_ids: Arc::from(selected_ids),
                hover_id: options.hover_id,
            }),
        };
        let metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| stale_cache_error("cache metadata is unavailable"))?;
        let token = frame_token(&frame_key)?;
        let frame = Arc::new(CachedFrame {
            token,
            source,
            source_fingerprint: metadata.source_fingerprint.clone(),
            source_sha256: metadata.source_sha256.clone(),
            source_hash_scope: metadata.source_hash_scope.clone(),
            generations: next_generations,
            camera: camera.clone(),
            pixel_error: options.pixel_error,
            dpi_factor: options.dpi_factor,
            base,
            overlay,
        });

        if overlay_changed && self.last_frame.is_some() {
            self.metrics.overlay_updates = self.metrics.overlay_updates.saturating_add(1);
        }
        self.generations = next_generations;
        self.camera_key = Some(camera_key);
        self.last_frame_key = Some(frame_key);
        self.last_frame = Some(Arc::clone(&frame));
        self.last_frame_options = Some(options.clone());
        self.metrics.frames_prepared = self.metrics.frames_prepared.saturating_add(1);
        self.metrics.generations = self.generations;
        Ok(frame)
    }

    fn can_publish_frame_base(&self) -> bool {
        let live = self.live_base_count();
        if live < self.limits.max_retained_frame_bases {
            return true;
        }
        let releasable = self.last_frame.as_ref().is_some_and(|frame| {
            Arc::strong_count(frame) == 1 && Arc::strong_count(&frame.base) == 1
        });
        releasable && live.saturating_sub(1) < self.limits.max_retained_frame_bases
    }

    fn build_frame_base(
        &mut self,
        camera: &Camera2D,
        camera_key: CameraKey,
        options: &DisplayOptions,
        max_segments_per_curve: usize,
        work_budget: usize,
    ) -> DraftResult<CachedFrameBase> {
        let query_started = Instant::now();
        let half_width = camera.viewport_width * 0.5 / camera.pixels_per_metre
            + 64.0 / camera.pixels_per_metre;
        let half_height = camera.viewport_height * 0.5 / camera.pixels_per_metre
            + 64.0 / camera.pixels_per_metre;
        let query_bounds = Bounds2::from_points(
            Point2::new(-half_width, -half_height),
            Point2::new(half_width, half_height),
        );
        if !query_bounds.min.is_finite() || !query_bounds.max.is_finite() {
            return Err(DraftError::new(
                "render_overflow",
                "camera-relative visibility query is not finite",
            ));
        }
        let query = self.global_index.query_relative(camera.center, query_bounds);
        self.metrics.index_queries = self.metrics.index_queries.saturating_add(1);
        self.metrics.index_nodes_visited = self
            .metrics
            .index_nodes_visited
            .saturating_add(query.nodes_visited);
        self.metrics.index_leaves_examined = self
            .metrics
            .index_leaves_examined
            .saturating_add(query.leaves_examined);
        let mut candidates = Vec::new();
        for id in query.ids {
            let record = self.entities.get(&id).cloned().ok_or_else(|| {
                DraftError::new(
                    "cache_index_corrupt",
                    format!("spatial index refers to absent entity {id}"),
                )
            })?;
            let layer = self
                .layer_states
                .get(&record.layer_id)
                .ok_or_else(|| {
                    DraftError::new(
                        "cache_layer_corrupt",
                        format!("entity {} refers to absent cached layer {}", record.id, record.layer_id),
                    )
                })?;
            if layer.visible {
                candidates.push(record);
            }
        }
        self.record_visibility_query(query_started.elapsed());

        let actual_scale = camera.pixels_per_metre;
        let bucket_scale = upper_tessellation_scale(actual_scale);
        let tessellation_started = Instant::now();
        let mesh_result = (|| {
            match self.collect_tessellations(
                &candidates,
                bucket_scale,
                options,
                max_segments_per_curve,
                work_budget,
            ) {
                Ok(meshes) => Ok(meshes),
                Err(error)
                    if bucket_scale.to_bits() != actual_scale.to_bits()
                        && is_tessellation_budget_error(&error) =>
                {
                    self.collect_tessellations(
                        &candidates,
                        actual_scale,
                        options,
                        max_segments_per_curve,
                        work_budget,
                    )
                }
                Err(error) => Err(error),
            }
        })();
        self.record_tessellation(tessellation_started.elapsed());
        let meshes = mesh_result?;
        let generated_segments = generated_total(&meshes)?;
        if generated_segments > work_budget {
            return Err(tessellation_work_budget_error());
        }

        let projection_started = Instant::now();
        self.projection_cache
            .retain(|_, segments| segments.strong_count() > 0);
        let mut batches = Vec::with_capacity(candidates.len());
        let mut primitives = Vec::new();
        for (record, (mesh, mesh_key)) in candidates.into_iter().zip(meshes) {
            let segments = match self.projected_segments(&record, &mesh, &mesh_key, camera, camera_key) {
                Ok(segments) => segments,
                Err(error) => {
                    self.record_projection(projection_started.elapsed());
                    return Err(error);
                }
            };
            if primitives
                .len()
                .checked_add(segments.len())
                .is_none_or(|count| count > work_budget)
            {
                self.record_projection(projection_started.elapsed());
                return Err(DraftError::new(
                    "display_budget",
                    "projected primitive count exceeds the bounded display budget",
                ));
            }
            for segment in segments.iter() {
                primitives.push(CachedPrimitive {
                    primitive_id: RenderPrimitiveId::new(primitives.len() as u64 + 1),
                    owner_entity_id: record.id,
                    start: segment.start,
                    end: segment.end,
                    appearance: record.style.clone(),
                });
            }
            batches.push(CachedGeometryBatch {
                owner_entity_id: record.id,
                geometry_generation: record.geometry_generation,
                geometry: mesh,
                segments,
            });
        }
        self.record_projection(projection_started.elapsed());
        self.metrics.generated_segments = generated_segments;
        self.metrics.primitive_count = primitives.len();
        let visible_entity_count = batches.len();
        Ok(CachedFrameBase {
            batches: Arc::from(batches),
            primitives: Arc::from(primitives),
            visible_entity_count,
            generated_segments,
        })
    }

    fn collect_tessellations(
        &mut self,
        candidates: &[Arc<CachedEntityRecord>],
        pixels_per_metre: f64,
        options: &DisplayOptions,
        max_segments_per_curve: usize,
        work_budget: usize,
    ) -> DraftResult<Vec<(Arc<TessellatedGeometry>, TessellationKey)>> {
        let mut remaining = work_budget;
        let mut meshes = Vec::with_capacity(candidates.len());
        for record in candidates {
            let mesh = self.get_tessellation(
                record,
                pixels_per_metre,
                options,
                max_segments_per_curve,
                remaining,
            )?;
            let generated = mesh.0.generated_segments();
            if generated > remaining {
                return Err(tessellation_work_budget_error());
            }
            remaining -= generated;
            meshes.push(mesh);
        }
        Ok(meshes)
    }

    fn get_tessellation(
        &mut self,
        record: &CachedEntityRecord,
        pixels_per_metre: f64,
        options: &DisplayOptions,
        max_segments_per_curve: usize,
        max_generated_segments: usize,
    ) -> DraftResult<(Arc<TessellatedGeometry>, TessellationKey)> {
        if max_generated_segments == 0 {
            return Err(tessellation_work_budget_error());
        }
        let key = TessellationKey {
            entity_id: record.id,
            geometry_generation: record.geometry_generation,
            pixels_per_metre_bits: pixels_per_metre.to_bits(),
            dpi_factor_bits: options.dpi_factor.to_bits(),
            pixel_error_bits: options.pixel_error.to_bits(),
            max_segments_per_curve,
        };
        if let Some(entry) = self.tessellations.get(&key) {
            if entry.mesh.generated_segments() > max_generated_segments {
                return Err(tessellation_work_budget_error());
            }
            let tick = self.next_lru_tick()?;
            if let Some(entry) = self.tessellations.get_mut(&key) {
                let last_used = entry.last_used;
                entry.last_used = tick;
                self.tessellation_lru.remove(&(last_used, key.clone()));
                let mesh = Arc::clone(&entry.mesh);
                self.tessellation_lru.insert((tick, key.clone()));
                self.metrics.tessellation_hits = self.metrics.tessellation_hits.saturating_add(1);
                return Ok((mesh, key));
            }
        }
        self.metrics.tessellation_misses = self.metrics.tessellation_misses.saturating_add(1);
        let mesh = Arc::new(build_tessellation(
            record.geometry.as_ref(),
            pixels_per_metre,
            options.dpi_factor,
            options.pixel_error,
            max_segments_per_curve,
            max_generated_segments,
        )?);
        let bytes = mesh.estimated_bytes();
        self.retain_tessellation(key.clone(), Arc::clone(&mesh), bytes)?;
        Ok((mesh, key))
    }

    fn retain_tessellation(
        &mut self,
        key: TessellationKey,
        mesh: Arc<TessellatedGeometry>,
        bytes: usize,
    ) -> DraftResult<()> {
        if bytes > self.limits.max_tessellation_bytes {
            return Ok(());
        }
        while self.tessellations.len() >= self.limits.max_tessellation_entries
            || self.tessellation_bytes.saturating_add(bytes)
                > self.limits.max_tessellation_bytes
        {
            let Some((last_used, oldest_key)) = self.tessellation_lru.iter().next().cloned() else {
                break;
            };
            self.tessellation_lru.remove(&(last_used, oldest_key.clone()));
            if let Some(entry) = self.tessellations.remove(&oldest_key) {
                self.tessellation_bytes = self.tessellation_bytes.saturating_sub(entry.bytes);
                self.metrics.tessellation_evictions =
                    self.metrics.tessellation_evictions.saturating_add(1);
            }
        }
        if self.tessellations.len() >= self.limits.max_tessellation_entries
            || self.tessellation_bytes.saturating_add(bytes)
                > self.limits.max_tessellation_bytes
        {
            return Ok(());
        }
        let last_used = self.next_lru_tick()?;
        self.tessellation_bytes = self.tessellation_bytes.saturating_add(bytes);
        self.tessellations.insert(
            key.clone(),
            TessellationEntry {
                mesh,
                bytes,
                last_used,
            },
        );
        self.tessellation_lru.insert((last_used, key));
        Ok(())
    }

    fn next_lru_tick(&mut self) -> DraftResult<u64> {
        if self.lru_clock == u64::MAX {
            let oldest_first: Vec<_> = self.tessellation_lru.iter().cloned().collect();
            self.tessellation_lru.clear();
            for (index, (_, key)) in oldest_first.into_iter().enumerate() {
                let tick = index as u64 + 1;
                if let Some(entry) = self.tessellations.get_mut(&key) {
                    entry.last_used = tick;
                    self.tessellation_lru.insert((tick, key));
                }
            }
            self.lru_clock = self.tessellations.len() as u64;
        }
        self.lru_clock = self.lru_clock.checked_add(1).ok_or_else(|| {
            DraftError::new("cache_clock_exhausted", "tessellation LRU clock exhausted")
        })?;
        Ok(self.lru_clock)
    }

    fn projected_segments(
        &mut self,
        record: &CachedEntityRecord,
        mesh: &Arc<TessellatedGeometry>,
        mesh_key: &TessellationKey,
        camera: &Camera2D,
        camera_key: CameraKey,
    ) -> DraftResult<Arc<[ScreenSegment]>> {
        let key = ProjectionKey {
            entity_id: record.id,
            geometry_generation: record.geometry_generation,
            camera: camera_key,
            mesh_pixels_per_metre_bits: mesh_key.pixels_per_metre_bits,
            mesh_dpi_factor_bits: mesh_key.dpi_factor_bits,
            mesh_pixel_error_bits: mesh_key.pixel_error_bits,
            mesh_max_segments_per_curve: mesh_key.max_segments_per_curve,
        };
        if let Some(segments) = self.projection_cache.get(&key).and_then(Weak::upgrade) {
            self.metrics.projection_hits = self.metrics.projection_hits.saturating_add(1);
            return Ok(segments);
        }
        let fallback_segments = mesh.fallback_segments(camera)?;
        let segments: Arc<[ScreenSegment]> = Arc::from(mesh.project(camera, 64.0)?);
        self.projection_cache.insert(key, Arc::downgrade(&segments));
        self.metrics.projected_entities = self.metrics.projected_entities.saturating_add(1);
        self.metrics.analytic_fallback_segments = self
            .metrics
            .analytic_fallback_segments
            .saturating_add(fallback_segments as u64);
        Ok(segments)
    }

    pub fn pick(
        &mut self,
        doc: &(impl DrawingSource + ?Sized),
        epoch: u64,
        camera: &Camera2D,
        screen: Point2,
        tolerance_px: f64,
    ) -> DraftResult<Vec<PickHit>> {
        let started = Instant::now();
        let result = self.pick_inner(doc, epoch, camera, screen, tolerance_px);
        push_timing(
            &mut self.pick_samples,
            started.elapsed().as_secs_f64() * 1000.0,
            self.limits.max_timing_samples,
        );
        self.metrics.pick_cpu = timing_summary(&self.pick_samples);
        result
    }

    fn pick_inner(
        &mut self,
        doc: &(impl DrawingSource + ?Sized),
        epoch: u64,
        camera: &Camera2D,
        screen: Point2,
        tolerance_px: f64,
    ) -> DraftResult<Vec<PickHit>> {
        let source_lookup_started = Instant::now();
        self.require_current(doc, epoch)?;
        self.record_source_lookup(source_lookup_started.elapsed());
        camera.validate()?;
        if !screen.is_finite() || !tolerance_px.is_finite() || !(0.0..=128.0).contains(&tolerance_px)
        {
            return Err(DraftError::new(
                "invalid_pick",
                "pick point or aperture invalid",
            ));
        }
        if screen.x < 0.0
            || screen.y < 0.0
            || screen.x > camera.viewport_width
            || screen.y > camera.viewport_height
        {
            return Ok(Vec::new());
        }
        let camera_relative = Point2::new(
            (screen.x - camera.viewport_width * 0.5) / camera.pixels_per_metre,
            (camera.viewport_height * 0.5 - screen.y) / camera.pixels_per_metre,
        );
        let aperture = tolerance_px / camera.pixels_per_metre;
        let query_bounds = Bounds2::from_points(camera_relative, camera_relative).expanded(aperture);
        let query = self.global_index.query_relative(camera.center, query_bounds);
        self.metrics.index_queries = self.metrics.index_queries.saturating_add(1);
        self.metrics.index_nodes_visited = self
            .metrics
            .index_nodes_visited
            .saturating_add(query.nodes_visited);
        self.metrics.index_leaves_examined = self
            .metrics
            .index_leaves_examined
            .saturating_add(query.leaves_examined);
        let mut hits = Vec::new();
        for id in query.ids {
            let Some(record) = self.entities.get(&id) else {
                return Err(DraftError::new(
                    "cache_index_corrupt",
                    format!("spatial index refers to absent entity {id}"),
                ));
            };
            let Some(layer) = self.layer_states.get(&record.layer_id) else {
                return Err(DraftError::new(
                    "cache_layer_corrupt",
                    format!("entity {id} refers to absent cached layer {}", record.layer_id),
                ));
            };
            if !layer.visible {
                continue;
            }
            self.metrics.distance_checks = self.metrics.distance_checks.saturating_add(1);
            let distance_px = record
                .geometry
                .distance_relative(camera.center, camera_relative)?
                * camera.pixels_per_metre;
            if distance_px.is_finite() && distance_px <= tolerance_px + 1e-8 {
                hits.push(PickHit {
                    entity_id: id,
                    distance_px,
                    locked: layer.locked,
                });
            }
        }
        hits.sort_by(|a, b| {
            a.distance_px
                .total_cmp(&b.distance_px)
                .then(a.entity_id.cmp(&b.entity_id))
        });
        Ok(hits)
    }

    pub fn materialize_export(
        &mut self,
        doc: &(impl DrawingSource + ?Sized),
        epoch: u64,
        frame: &Arc<CachedFrame>,
    ) -> DraftResult<CapturedExport> {
        let source = self.require_current(doc, epoch)?;
        let Some(current_frame) = self.last_frame.as_ref() else {
            return Err(stale_cache_error("no current frame has been issued by this cache"));
        };
        if !Arc::ptr_eq(current_frame, frame)
            || self.last_frame_key.is_none()
            || frame.source != source
            || frame.generations != self.generations
        {
            return Err(stale_cache_error(
                "export frame was fabricated, superseded, or belongs to another source",
            ));
        }
        let options = self
            .last_frame_options
            .as_ref()
            .ok_or_else(|| stale_cache_error("issued frame options are unavailable"))?
            .clone();
        let frame_key = self
            .last_frame_key
            .as_ref()
            .ok_or_else(|| stale_cache_error("issued frame key is unavailable"))?;
        let normalized_selection: Vec<_> = options
            .selected_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if frame_key.source != frame.source
            || frame_key.camera != CameraKey::new(&frame.camera)
            || frame_key.camera_revision != frame.camera.revision
            || frame_key.options_generation != options.generation
            || frame_key.pixel_error_bits != options.pixel_error.to_bits()
            || frame_key.dpi_factor_bits != options.dpi_factor.to_bits()
            || frame_key.max_segments_per_curve != options.max_segments_per_curve
            || frame_key.max_primitives != options.max_primitives
            || frame_key.selected_ids != normalized_selection
            || frame_key.hover_id != options.hover_id
        {
            return Err(stale_cache_error(
                "issued frame key and options do not agree",
            ));
        }
        let metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| stale_cache_error("cache metadata is unavailable"))?;
        if frame.source_sha256 != metadata.source_sha256
            || frame.source_fingerprint != metadata.source_fingerprint
            || frame.source_hash_scope != metadata.source_hash_scope
        {
            return Err(stale_cache_error(
                "export frame content digest does not match current metadata",
            ));
        }

        // This is the sole compatibility boundary that materializes the
        // legacy full snapshot and display DTOs. All guards above are O(1).
        let snapshot = doc.snapshot(source.drawing_id)?;
        if snapshot.document_revision != source.document_revision
            || snapshot.revision != source.drawing_revision
        {
            return Err(stale_cache_error(
                "document changed while materializing the export snapshot",
            ));
        }
        let selected: BTreeSet<_> = frame.overlay.selected_ids.iter().copied().collect();
        let visible_layer_ids: Vec<_> = snapshot
            .layers
            .values()
            .filter(|layer| layer.visible)
            .map(|layer| layer.id)
            .collect();
        let analytic_entities: Vec<_> = snapshot
            .entities
            .values()
            .filter(|entity| {
                snapshot
                    .layers
                    .get(&entity.layer_id)
                    .is_some_and(|layer| layer.visible)
            })
            .cloned()
            .collect();
        let primitives: Vec<_> = frame
            .base
            .primitives
            .iter()
            .map(|primitive| {
                let is_selected = selected.contains(&primitive.owner_entity_id);
                let hovered = frame.overlay.hover_id == Some(primitive.owner_entity_id);
                let mut appearance = primitive.appearance.clone();
                if is_selected {
                    appearance.color = [1.0, 0.74, 0.22, 1.0];
                    appearance.width_px += 1.0;
                } else if hovered {
                    appearance.color = [0.35, 1.0, 0.9, 1.0];
                    appearance.width_px += 0.5;
                }
                RenderPrimitive {
                    primitive_id: primitive.primitive_id,
                    owner_entity_id: primitive.owner_entity_id,
                    drawing_id: frame.source.drawing_id,
                    source_revision: frame.source.drawing_revision,
                    start: primitive.start,
                    end: primitive.end,
                    appearance,
                    selected: is_selected,
                    hovered,
                }
            })
            .collect();
        let display = DisplaySnapshot {
            token: frame.token.clone(),
            document_id: frame.source.document_id.clone(),
            document_revision: frame.source.document_revision,
            drawing_id: frame.source.drawing_id,
            source_revision: frame.source.drawing_revision,
            source_fingerprint: snapshot.source_fingerprint.clone(),
            camera: frame.camera.clone(),
            viewport: frame.camera.viewport(),
            selected_ids: frame.overlay.selected_ids.to_vec(),
            hover_id: frame.overlay.hover_id,
            visible_layer_ids,
            analytic_entities,
            primitives,
            pixel_error: frame.pixel_error,
            dpi_factor: frame.dpi_factor,
            representation_quality: "analytic_2d;display_tessellated".to_owned(),
        };
        self.metrics.export_snapshot_geometry_clones = self
            .metrics
            .export_snapshot_geometry_clones
            .saturating_add(snapshot.entities.len() as u64);
        self.metrics.export_analytic_geometry_clones = self
            .metrics
            .export_analytic_geometry_clones
            .saturating_add(display.analytic_entities.len() as u64);
        self.metrics.export_materializations =
            self.metrics.export_materializations.saturating_add(1);
        Ok(CapturedExport { snapshot, display })
    }

    fn record_visibility_query(&mut self, elapsed: std::time::Duration) {
        push_timing(
            &mut self.visibility_query_samples,
            elapsed.as_secs_f64() * 1000.0,
            self.limits.max_timing_samples,
        );
        self.metrics.visibility_query_cpu = timing_summary(&self.visibility_query_samples);
    }

    fn record_tessellation(&mut self, elapsed: std::time::Duration) {
        push_timing(
            &mut self.tessellation_samples,
            elapsed.as_secs_f64() * 1000.0,
            self.limits.max_timing_samples,
        );
        self.metrics.tessellation_cpu = timing_summary(&self.tessellation_samples);
    }

    fn record_projection(&mut self, elapsed: std::time::Duration) {
        push_timing(
            &mut self.projection_samples,
            elapsed.as_secs_f64() * 1000.0,
            self.limits.max_timing_samples,
        );
        self.metrics.projection_cpu = timing_summary(&self.projection_samples);
    }
}

fn tessellation_work_budget_error() -> DraftError {
    DraftError::new(
        "display_budget",
        "visible geometry exceeds the bounded generated-segment budget",
    )
}

fn is_tessellation_budget_error(error: &DraftError) -> bool {
    matches!(error.code.as_str(), "tessellation_budget" | "display_budget")
}

fn generated_total(meshes: &[(Arc<TessellatedGeometry>, TessellationKey)]) -> DraftResult<usize> {
    meshes.iter().try_fold(0usize, |total, (mesh, _)| {
        total
            .checked_add(mesh.generated_segments())
            .ok_or_else(|| DraftError::new("display_budget", "generated segment count overflow"))
    })
}

fn upper_tessellation_scale(actual: f64) -> f64 {
    if actual >= 1.0e12 {
        return 1.0e12;
    }
    let octave = 2.0_f64.powf(actual.log2().ceil());
    if octave.is_finite() && octave <= 1.0e12 {
        octave.max(actual)
    } else {
        1.0e12
    }
}

fn frame_token(key: &FrameKey) -> DraftResult<String> {
    let bytes = serde_json::to_vec(key)
        .map_err(|error| DraftError::new("serialization", error.to_string()))?;
    Ok(format!(
        "cache-frame-sha256:{}",
        hex_digest(&hash_bytes("drawing.frame-token.v1", &bytes))
    ))
}
