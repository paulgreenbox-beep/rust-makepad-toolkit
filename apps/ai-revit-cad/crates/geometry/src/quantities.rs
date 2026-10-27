//! Cached quantities derived from geometry, with indexed grouped aggregates.
//!
//! Quantities are approximate mesh observations. Unknown measurements remain
//! `None`; invalid numeric observations are rejected from the cache.

use crate::RepresentationQuality;
use ai_revit_core::{ClassId, ElementId, LevelId, TypeId};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Physical measurements for one element or aggregate.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GeometryQuantities {
    pub quality: RepresentationQuality,
    pub gross_volume: Option<f64>,
    pub net_volume: Option<f64>,
    pub gross_surface_area: Option<f64>,
    pub net_surface_area: Option<f64>,
    pub opening_area: Option<f64>,
    pub opening_volume: Option<f64>,
    pub perimeter: Option<f64>,
    pub length: Option<f64>,
}

impl GeometryQuantities {
    fn is_valid(&self) -> bool {
        [
            self.gross_volume,
            self.net_volume,
            self.gross_surface_area,
            self.net_surface_area,
            self.opening_area,
            self.opening_volume,
            self.perimeter,
            self.length,
        ]
        .into_iter()
        .flatten()
        .all(|value| value.is_finite() && value >= 0.0)
    }
}

/// Reuse the existing compensated/optional aggregate semantics after GPU fields
/// replace equivalent mesh observations in the returned batch.
pub(crate) fn aggregate_quantities<'a>(
    records: impl Iterator<Item = &'a GeometryQuantities>,
) -> GeometryQuantities {
    let mut accumulator = QuantityAccumulator::default();
    for record in records {
        accumulator.add(record);
    }
    accumulator.finish()
}
/// Quantities and the geometry revision/fingerprint that produced them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ElementQuantities {
    pub element_id: ElementId,
    pub geometry_revision: u64,
    pub geometry_fingerprint: String,
    #[serde(flatten)]
    pub quantities: GeometryQuantities,
}

/// Semantic grouping metadata supplied by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantityIdentity {
    pub class_id: ClassId,
    pub type_id: Option<TypeId>,
    pub level_id: Option<LevelId>,
}

/// Dimension used to index grouped totals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuantityGrouping {
    Class,
    Type,
    Level,
}

/// An indexed group and its incrementally maintained aggregate.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QuantityGroup {
    pub class_id: Option<ClassId>,
    pub type_id: Option<TypeId>,
    pub level_id: Option<LevelId>,
    pub count: usize,
    pub element_ids: Vec<ElementId>,
    pub quantities: GeometryQuantities,
}

/// Result for a deduplicated, sorted set of requested element IDs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QuantityBatch {
    /// All unique requested IDs, including IDs that are missing from the cache.
    pub element_ids: Vec<ElementId>,
    /// Cached records, sorted by element ID.
    pub elements: Vec<ElementQuantities>,
    /// Aggregate of the cached records in `elements`.
    pub aggregate: GeometryQuantities,
    /// Requested IDs with no valid cached record, sorted and deduplicated.
    pub missing_element_ids: Vec<ElementId>,
}

/// Entry count and cumulative quantity-cache activity since construction/clear.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct QuantityCacheMetrics {
    pub entry_count: usize,
    pub quantity_rebuild_count: usize,
    pub quantity_cache_hit_count: usize,
}

/// Fingerprint-aware quantities with indexed class, type, and level totals.
#[derive(Default)]
pub struct QuantityCache {
    entries: BTreeMap<ElementId, CacheEntry>,
    class_groups: BTreeMap<ClassId, GroupBucket>,
    type_groups: BTreeMap<Option<TypeId>, GroupBucket>,
    level_groups: BTreeMap<Option<LevelId>, GroupBucket>,
    metrics: QuantityCacheMetrics,
}

impl QuantityCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Removes every cached record and resets the cache metrics.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.class_groups.clear();
        self.type_groups.clear();
        self.level_groups.clear();
        self.metrics = QuantityCacheMetrics::default();
    }

    /// Inserts a computed record. Matching fingerprints reuse cached quantities,
    /// while revision and grouping metadata are refreshed. A changed fingerprint
    /// counts as a rebuild even if its submitted measurements are rejected.
    ///
    /// Any non-finite or negative measurement invalidates the submitted record.
    /// If its fingerprint changed, an older record for the same element is
    /// removed so stale quantities cannot be returned.
    pub fn upsert(&mut self, record: ElementQuantities, identity: QuantityIdentity) {
        let id = record.element_id;
        if let Some(existing) = self.entries.get(&id) {
            if existing.record.geometry_fingerprint == record.geometry_fingerprint {
                let old_identity = existing.identity.clone();
                let cached_quantities = existing.record.quantities.clone();
                self.move_membership(id, &old_identity, &identity, &cached_quantities);
                self.entries.insert(
                    id,
                    CacheEntry {
                        record: ElementQuantities {
                            element_id: id,
                            geometry_revision: record.geometry_revision,
                            geometry_fingerprint: record.geometry_fingerprint,
                            quantities: cached_quantities,
                        },
                        identity,
                    },
                );
                increment(&mut self.metrics.quantity_cache_hit_count);
                return;
            }
        }

        increment(&mut self.metrics.quantity_rebuild_count);
        if !record.quantities.is_valid() {
            self.remove(id);
            return;
        }

        self.remove_entry(id);
        self.add_membership(id, &identity, &record.quantities);
        self.entries.insert(id, CacheEntry { record, identity });
        self.metrics.entry_count = self.entries.len();
    }

    /// Removes an element and subtracts its values from all indexed groups.
    pub fn remove(&mut self, id: ElementId) -> bool {
        let removed = self.remove_entry(id);
        self.metrics.entry_count = self.entries.len();
        removed
    }

    /// Returns a cached valid record for `id`.
    pub fn get(&self, id: ElementId) -> Option<&ElementQuantities> {
        self.entries.get(&id).map(|entry| &entry.record)
    }

    /// Returns cached values for unique IDs in ascending order.
    pub fn batch(&self, ids: &[ElementId]) -> QuantityBatch {
        let unique_ids: BTreeSet<_> = ids.iter().copied().collect();
        let mut elements = Vec::new();
        let mut missing_element_ids = Vec::new();
        let mut quantities = QuantityAccumulator::default();

        for id in unique_ids.iter().copied() {
            if let Some(entry) = self.entries.get(&id) {
                elements.push(entry.record.clone());
                quantities.add(&entry.record.quantities);
            } else {
                missing_element_ids.push(id);
            }
        }

        QuantityBatch {
            element_ids: unique_ids.into_iter().collect(),
            elements,
            aggregate: quantities.finish(),
            missing_element_ids,
        }
    }

    /// Returns indexed groups in key order, including the `None` type/level
    /// group whenever cached elements have no type or level assignment.
    pub fn grouped(&self, grouping: QuantityGrouping) -> Vec<QuantityGroup> {
        match grouping {
            QuantityGrouping::Class => self
                .class_groups
                .iter()
                .map(|(class_id, bucket)| QuantityGroup {
                    class_id: Some(class_id.clone()),
                    type_id: None,
                    level_id: None,
                    count: bucket.count,
                    element_ids: bucket.element_ids.iter().copied().collect(),
                    quantities: bucket.quantities.finish(),
                })
                .collect(),
            QuantityGrouping::Type => self
                .type_groups
                .iter()
                .map(|(type_id, bucket)| QuantityGroup {
                    class_id: None,
                    type_id: *type_id,
                    level_id: None,
                    count: bucket.count,
                    element_ids: bucket.element_ids.iter().copied().collect(),
                    quantities: bucket.quantities.finish(),
                })
                .collect(),
            QuantityGrouping::Level => self
                .level_groups
                .iter()
                .map(|(level_id, bucket)| QuantityGroup {
                    class_id: None,
                    type_id: None,
                    level_id: *level_id,
                    count: bucket.count,
                    element_ids: bucket.element_ids.iter().copied().collect(),
                    quantities: bucket.quantities.finish(),
                })
                .collect(),
        }
    }

    pub fn metrics(&self) -> QuantityCacheMetrics {
        self.metrics
    }

    fn remove_entry(&mut self, id: ElementId) -> bool {
        let Some(entry) = self.entries.remove(&id) else {
            return false;
        };
        self.remove_membership(id, &entry.identity, &entry.record.quantities);
        self.metrics.entry_count = self.entries.len();
        true
    }

    fn add_membership(
        &mut self,
        id: ElementId,
        identity: &QuantityIdentity,
        quantities: &GeometryQuantities,
    ) {
        add_to_group(
            &mut self.class_groups,
            identity.class_id.clone(),
            id,
            quantities,
        );
        add_to_group(&mut self.type_groups, identity.type_id, id, quantities);
        add_to_group(&mut self.level_groups, identity.level_id, id, quantities);
    }

    fn remove_membership(
        &mut self,
        id: ElementId,
        identity: &QuantityIdentity,
        quantities: &GeometryQuantities,
    ) {
        remove_from_group(&mut self.class_groups, &identity.class_id, id, quantities);
        remove_from_group(&mut self.type_groups, &identity.type_id, id, quantities);
        remove_from_group(&mut self.level_groups, &identity.level_id, id, quantities);
    }

    fn move_membership(
        &mut self,
        id: ElementId,
        old_identity: &QuantityIdentity,
        new_identity: &QuantityIdentity,
        quantities: &GeometryQuantities,
    ) {
        if old_identity.class_id != new_identity.class_id {
            move_group(
                &mut self.class_groups,
                old_identity.class_id.clone(),
                new_identity.class_id.clone(),
                id,
                quantities,
            );
        }
        if old_identity.type_id != new_identity.type_id {
            move_group(
                &mut self.type_groups,
                old_identity.type_id,
                new_identity.type_id,
                id,
                quantities,
            );
        }
        if old_identity.level_id != new_identity.level_id {
            move_group(
                &mut self.level_groups,
                old_identity.level_id,
                new_identity.level_id,
                id,
                quantities,
            );
        }
    }
}

struct CacheEntry {
    record: ElementQuantities,
    identity: QuantityIdentity,
}

#[derive(Default)]
struct GroupBucket {
    count: usize,
    element_ids: BTreeSet<ElementId>,
    quantities: QuantityAccumulator,
}

impl GroupBucket {
    fn add(&mut self, id: ElementId, quantities: &GeometryQuantities) {
        debug_assert!(!self.element_ids.contains(&id));
        self.element_ids.insert(id);
        self.count = self
            .count
            .checked_add(1)
            .expect("quantity group count overflow");
        self.quantities.add(quantities);
    }

    fn remove(&mut self, id: ElementId, quantities: &GeometryQuantities) {
        debug_assert!(self.element_ids.contains(&id));
        self.element_ids.remove(&id);
        self.count = self
            .count
            .checked_sub(1)
            .expect("quantity group count underflow");
        self.quantities.remove(quantities);
    }
}

fn add_to_group<K: Ord>(
    groups: &mut BTreeMap<K, GroupBucket>,
    key: K,
    id: ElementId,
    quantities: &GeometryQuantities,
) {
    groups.entry(key).or_default().add(id, quantities);
}

fn remove_from_group<K: Ord>(
    groups: &mut BTreeMap<K, GroupBucket>,
    key: &K,
    id: ElementId,
    quantities: &GeometryQuantities,
) {
    let empty = if let Some(bucket) = groups.get_mut(key) {
        bucket.remove(id, quantities);
        bucket.count == 0
    } else {
        debug_assert!(false, "quantity group index is inconsistent");
        return;
    };
    if empty {
        groups.remove(key);
    }
}

fn move_group<K: Ord>(
    groups: &mut BTreeMap<K, GroupBucket>,
    old_key: K,
    new_key: K,
    id: ElementId,
    quantities: &GeometryQuantities,
) {
    remove_from_group(groups, &old_key, id, quantities);
    add_to_group(groups, new_key, id, quantities);
}

#[derive(Default)]
struct QuantityAccumulator {
    gross_volume: ScalarAccumulator,
    net_volume: ScalarAccumulator,
    gross_surface_area: ScalarAccumulator,
    net_surface_area: ScalarAccumulator,
    opening_area: ScalarAccumulator,
    opening_volume: ScalarAccumulator,
    perimeter: ScalarAccumulator,
    length: ScalarAccumulator,
}

impl QuantityAccumulator {
    fn add(&mut self, quantities: &GeometryQuantities) {
        self.gross_volume.add(quantities.gross_volume);
        self.net_volume.add(quantities.net_volume);
        self.gross_surface_area.add(quantities.gross_surface_area);
        self.net_surface_area.add(quantities.net_surface_area);
        self.opening_area.add(quantities.opening_area);
        self.opening_volume.add(quantities.opening_volume);
        self.perimeter.add(quantities.perimeter);
        self.length.add(quantities.length);
    }

    fn remove(&mut self, quantities: &GeometryQuantities) {
        self.gross_volume.remove(quantities.gross_volume);
        self.net_volume.remove(quantities.net_volume);
        self.gross_surface_area
            .remove(quantities.gross_surface_area);
        self.net_surface_area.remove(quantities.net_surface_area);
        self.opening_area.remove(quantities.opening_area);
        self.opening_volume.remove(quantities.opening_volume);
        self.perimeter.remove(quantities.perimeter);
        self.length.remove(quantities.length);
    }

    fn finish(&self) -> GeometryQuantities {
        GeometryQuantities {
            // The current backend exposes only ApproximateMesh.
            quality: RepresentationQuality::ApproximateMesh,
            gross_volume: self.gross_volume.sum(),
            net_volume: self.net_volume.sum(),
            gross_surface_area: self.gross_surface_area.sum(),
            net_surface_area: self.net_surface_area.sum(),
            opening_area: self.opening_area.sum(),
            opening_volume: self.opening_volume.sum(),
            perimeter: self.perimeter.sum(),
            length: self.length.sum(),
        }
    }
}

/// Exact nonnegative f64 summation bins, kept incrementally so removing a
/// value can recover a finite total after a previous aggregate overflow.
#[derive(Default)]
struct ScalarAccumulator {
    observed_count: usize,
    /// Each finite f64 is an integer significand times `2^exponent`. There are
    /// only 2046 possible exponents and the sum per exponent fits in i128 for
    /// any usize-sized cache.
    bins: BTreeMap<i16, i128>,
}

impl ScalarAccumulator {
    fn add(&mut self, value: Option<f64>) {
        let Some(value) = value else { return };
        let (exponent, significand) = f64_parts(value);
        self.observed_count = self
            .observed_count
            .checked_add(1)
            .expect("quantity observation count overflow");
        if significand != 0 {
            *self.bins.entry(exponent).or_default() += i128::from(significand);
        }
    }

    fn remove(&mut self, value: Option<f64>) {
        let Some(value) = value else { return };
        let (exponent, significand) = f64_parts(value);
        self.observed_count = self
            .observed_count
            .checked_sub(1)
            .expect("quantity observation count underflow");
        if significand == 0 {
            return;
        }
        let remove_bin = if let Some(bin) = self.bins.get_mut(&exponent) {
            *bin -= i128::from(significand);
            debug_assert!(*bin >= 0);
            *bin == 0
        } else {
            debug_assert!(false, "quantity sum index is inconsistent");
            return;
        };
        if remove_bin {
            self.bins.remove(&exponent);
        }
    }

    fn sum(&self) -> Option<f64> {
        if self.observed_count == 0 {
            return None;
        }
        let mut sum = 0.0;
        for (exponent, significand) in &self.bins {
            let contribution = (*significand as f64) * 2.0_f64.powi(i32::from(*exponent));
            sum += contribution;
            if !sum.is_finite() {
                return None;
            }
        }
        Some(sum)
    }
}

/// Returns the exact integer significand and power-of-two exponent of a
/// validated, nonnegative finite f64. Negative zero contributes zero.
fn f64_parts(value: f64) -> (i16, u64) {
    debug_assert!(value.is_finite() && value >= 0.0);
    let bits = value.to_bits();
    let exponent_bits = ((bits >> 52) & 0x7ff) as i16;
    let fraction = bits & ((1_u64 << 52) - 1);
    if exponent_bits == 0 {
        (-1074, fraction)
    } else {
        let significand = (1_u64 << 52) | fraction;
        (exponent_bits - 1023 - 52, significand)
    }
}

fn increment(value: &mut usize) {
    *value = value
        .checked_add(1)
        .expect("quantity cache metric overflow");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantities(gross: Option<f64>, net: Option<f64>) -> GeometryQuantities {
        GeometryQuantities {
            quality: RepresentationQuality::ApproximateMesh,
            gross_volume: gross,
            net_volume: net,
            gross_surface_area: None,
            net_surface_area: None,
            opening_area: None,
            opening_volume: None,
            perimeter: None,
            length: None,
        }
    }

    fn identity(class: &str, type_id: Option<u64>, level_id: Option<u64>) -> QuantityIdentity {
        QuantityIdentity {
            class_id: ClassId::from(class),
            type_id: type_id.map(TypeId::new),
            level_id: level_id.map(LevelId::new),
        }
    }

    fn record(
        id: u64,
        revision: u64,
        fingerprint: &str,
        gross: Option<f64>,
        net: Option<f64>,
    ) -> ElementQuantities {
        ElementQuantities {
            element_id: ElementId::new(id),
            geometry_revision: revision,
            geometry_fingerprint: fingerprint.to_owned(),
            quantities: quantities(gross, net),
        }
    }

    #[test]
    fn aggregate_sums_observed_values_and_keeps_unknown_fields_unknown() {
        let mut cache = QuantityCache::new();
        cache.upsert(
            record(2, 1, "a", Some(3.0), None),
            identity("walls", Some(4), Some(9)),
        );
        cache.upsert(
            record(1, 1, "b", Some(2.0), Some(1.5)),
            identity("walls", Some(4), Some(9)),
        );

        let batch = cache.batch(&[ElementId::new(2), ElementId::new(1)]);
        assert_eq!(batch.aggregate.gross_volume, Some(5.0));
        assert_eq!(batch.aggregate.net_volume, Some(1.5));
        assert_eq!(batch.aggregate.gross_surface_area, None);
        assert_eq!(
            batch.element_ids,
            vec![ElementId::new(1), ElementId::new(2)]
        );
        assert_eq!(
            batch
                .elements
                .iter()
                .map(|item| item.element_id)
                .collect::<Vec<_>>(),
            batch.element_ids
        );
    }

    #[test]
    fn fingerprint_hit_updates_revision_and_groups_without_rebuilding_quantities() {
        let mut cache = QuantityCache::new();
        cache.upsert(
            record(1, 1, "same", Some(10.0), Some(8.0)),
            identity("old", Some(1), Some(1)),
        );
        cache.upsert(
            record(1, 2, "same", Some(999.0), Some(999.0)),
            identity("new", None, None),
        );

        let cached = cache.get(ElementId::new(1)).unwrap();
        assert_eq!(cached.geometry_revision, 2);
        assert_eq!(cached.quantities.gross_volume, Some(10.0));
        assert_eq!(cache.metrics().quantity_rebuild_count, 1);
        assert_eq!(cache.metrics().quantity_cache_hit_count, 1);

        let old_class = cache.grouped(QuantityGrouping::Class);
        assert_eq!(old_class.len(), 1);
        assert_eq!(old_class[0].class_id, Some(ClassId::from("new")));
        assert_eq!(old_class[0].quantities.gross_volume, Some(10.0));
        let untyped = cache.grouped(QuantityGrouping::Type);
        assert_eq!(untyped.len(), 1);
        assert_eq!(untyped[0].type_id, None);
        let unlevelled = cache.grouped(QuantityGrouping::Level);
        assert_eq!(unlevelled.len(), 1);
        assert_eq!(unlevelled[0].level_id, None);
    }

    #[test]
    fn changed_metadata_moves_incremental_totals_and_remove_subtracts() {
        let mut cache = QuantityCache::new();
        cache.upsert(
            record(1, 1, "first", Some(10.0), Some(6.0)),
            identity("A", Some(1), Some(8)),
        );
        cache.upsert(
            record(2, 1, "second", Some(4.0), Some(3.0)),
            identity("A", Some(1), Some(8)),
        );
        cache.upsert(
            record(1, 2, "first", None, None),
            identity("B", Some(2), Some(9)),
        );

        let groups = cache.grouped(QuantityGrouping::Class);
        assert_eq!(groups.len(), 2);
        let a = groups
            .iter()
            .find(|group| group.class_id == Some(ClassId::from("A")))
            .unwrap();
        let b = groups
            .iter()
            .find(|group| group.class_id == Some(ClassId::from("B")))
            .unwrap();
        assert_eq!(a.count, 1);
        assert_eq!(a.quantities.gross_volume, Some(4.0));
        assert_eq!(b.count, 1);
        assert_eq!(b.quantities.gross_volume, Some(10.0));

        assert!(cache.remove(ElementId::new(1)));
        assert_eq!(cache.metrics().entry_count, 1);
        let groups = cache.grouped(QuantityGrouping::Class);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].class_id, Some(ClassId::from("A")));
        assert_eq!(groups[0].quantities.net_volume, Some(3.0));
        assert!(!cache.remove(ElementId::new(99)));
    }

    #[test]
    fn batch_reports_sorted_missing_ids_and_deduplicates_requests() {
        let mut cache = QuantityCache::new();
        cache.upsert(
            record(3, 1, "three", Some(3.0), None),
            identity("A", None, None),
        );
        cache.upsert(
            record(1, 1, "one", Some(1.0), None),
            identity("A", None, None),
        );

        let batch = cache.batch(&[
            ElementId::new(3),
            ElementId::new(2),
            ElementId::new(3),
            ElementId::new(1),
            ElementId::new(2),
        ]);
        assert_eq!(
            batch.element_ids,
            vec![ElementId::new(1), ElementId::new(2), ElementId::new(3)]
        );
        assert_eq!(batch.missing_element_ids, vec![ElementId::new(2)]);
        assert_eq!(
            batch
                .elements
                .iter()
                .map(|item| item.element_id)
                .collect::<Vec<_>>(),
            vec![ElementId::new(1), ElementId::new(3)]
        );
        assert_eq!(batch.aggregate.gross_volume, Some(4.0));
    }

    #[test]
    fn overflow_is_unknown_and_reverses_after_removing_the_overflowing_entry() {
        let mut cache = QuantityCache::new();
        cache.upsert(
            record(1, 1, "large-a", Some(f64::MAX), None),
            identity("A", None, None),
        );
        cache.upsert(
            record(2, 1, "large-b", Some(f64::MAX), None),
            identity("A", None, None),
        );
        let groups = cache.grouped(QuantityGrouping::Class);
        assert_eq!(groups[0].quantities.gross_volume, None);
        cache.remove(ElementId::new(2));
        let groups = cache.grouped(QuantityGrouping::Class);
        assert_eq!(groups[0].quantities.gross_volume, Some(f64::MAX));
    }

    #[test]
    fn invalid_measurement_is_missing_and_replaces_stale_fingerprint_data() {
        let mut cache = QuantityCache::new();
        cache.upsert(
            record(1, 1, "good", Some(5.0), None),
            identity("A", None, None),
        );
        cache.upsert(
            record(1, 2, "bad", Some(f64::NAN), None),
            identity("A", None, None),
        );
        assert!(cache.get(ElementId::new(1)).is_none());
        let batch = cache.batch(&[ElementId::new(1)]);
        assert_eq!(batch.missing_element_ids, vec![ElementId::new(1)]);
        assert_eq!(batch.aggregate.gross_volume, None);
        assert_eq!(cache.metrics().entry_count, 0);
    }
}
