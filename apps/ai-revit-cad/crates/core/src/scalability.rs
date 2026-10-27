//! Compact revision storage and bounded transactional observation.
use crate::*;

/// Interval one is the explicit full-snapshot comparison policy. Checkpoints
/// never evict command receipts or deltas and changing the interval is safe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryPolicy {
    pub checkpoint_interval: u64,
}
impl Default for HistoryPolicy {
    fn default() -> Self {
        Self {
            checkpoint_interval: 32,
        }
    }
}
impl HistoryPolicy {
    pub const fn full_snapshot() -> Self {
        Self {
            checkpoint_interval: 1,
        }
    }
    pub const fn compact(checkpoint_interval: u64) -> Self {
        Self {
            checkpoint_interval,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreScanMetric {
    pub scan_name: String,
    pub elements_scanned: u64,
    pub relationships_scanned: u64,
    pub duration_us: u64,
    pub reason: String,
    pub full_document: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorePerformance {
    pub core_scan_count: u64,
    pub elements_scanned: u64,
    pub relationships_scanned: u64,
    pub dirty_propagation_us: u64,
    pub snapshot_us: u64,
    pub journal_entry_count: usize,
    pub scans: Vec<CoreScanMetric>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexMetrics {
    pub element_by_id: usize,
    pub type_by_id: usize,
    pub level_by_id: usize,
    pub relationship_by_id: usize,
    pub relationships_by_source: usize,
    pub relationships_by_target: usize,
    pub hosted_by_host: usize,
    pub host_of_element: usize,
    pub dependencies_forward: usize,
    pub dependencies_reverse: usize,
    pub instances_by_type: usize,
    pub elements_by_level: usize,
    pub elements_by_class: usize,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropertyChange {
    pub entity: EntityRef,
    pub property: PropertyId,
    pub before: Option<PropertyValue>,
    pub after: Option<PropertyValue>,
    pub source_before: Option<PropertySource>,
    pub source_after: Option<PropertySource>,
    #[serde(default)]
    pub type_id_before: Option<TypeId>,
    #[serde(default)]
    pub type_id_after: Option<TypeId>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TransactionDelta {
    #[serde(default, skip_serializing_if = "GeoFoundationDelta::is_empty")]
    pub geo_foundation: GeoFoundationDelta,
    #[serde(default, skip_serializing_if = "FoundationStructuralDelta::is_empty")]
    pub foundation_structural: FoundationStructuralDelta,
    #[serde(default, skip_serializing_if = "StructuralConfiguratorDelta::is_empty")]
    pub structural_configurator: StructuralConfiguratorDelta,
    #[serde(default)]
    pub engineering: EngineeringDelta,
    #[serde(default)]
    pub drafting: DraftingDelta,
    pub revision_before: u64,
    pub revision_after: u64,
    pub created_elements: Vec<ElementId>,
    pub updated_elements: Vec<ElementId>,
    pub deleted_elements: Vec<ElementId>,
    pub created_relationships: Vec<RelationshipId>,
    pub updated_relationships: Vec<RelationshipId>,
    pub deleted_relationships: Vec<RelationshipId>,
    pub created_types: Vec<TypeId>,
    pub updated_types: Vec<TypeId>,
    pub deleted_types: Vec<TypeId>,
    pub created_levels: Vec<LevelId>,
    pub updated_levels: Vec<LevelId>,
    pub deleted_levels: Vec<LevelId>,
    pub property_changes: Vec<PropertyChange>,
    /// Includes elements whose resolved values change via Type/Class edits.
    pub affected_elements: Vec<ElementId>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct AllocationCounters {
    #[serde(default)]
    pub drafting: DraftAllocators,
    pub element: u64,
    pub operation: u64,
    pub element_type: u64,
    pub level: u64,
    pub relationship: u64,
}
impl AllocationCounters {
    pub(super) fn retains_high_water(&self, previous: &Self) -> bool {
        self.drafting.retains(&previous.drafting)
            && self.element >= previous.element
            && self.operation >= previous.operation
            && self.element_type >= previous.element_type
            && self.level >= previous.level
            && self.relationship >= previous.relationship
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct RevisionDelta {
    pub summary: TransactionDelta,
    pub changes: Vec<Change>,
    pub counters: AllocationCounters,
}
#[derive(Clone, Debug, Default)]
pub(super) struct ElementProjection {
    pub effective: Option<ElementRecord>,
    pub instance: Option<ElementRecord>,
    pub properties: BTreeMap<PropertyId, ResolvedProperty>,
    pub host: Option<ElementId>,
    pub dependents: Vec<ElementId>,
}

impl Document {
    pub fn history_policy(&self) -> HistoryPolicy {
        self.history_policy
    }
    pub fn set_history_policy(&mut self, policy: HistoryPolicy) -> Result<(), CoreError> {
        if policy.checkpoint_interval == 0 {
            return Err(CoreError::InvalidDocument(
                "checkpoint interval must be positive".into(),
            ));
        }
        self.history_policy = policy;
        Ok(())
    }
    pub fn index_metrics(&self) -> IndexMetrics {
        let i = self.indexes();
        IndexMetrics {
            element_by_id: self.elements.len(),
            type_by_id: self.types.len(),
            level_by_id: self.levels.len(),
            relationship_by_id: self.relationships.len(),
            relationships_by_source: i.by_source.len(),
            relationships_by_target: i.by_target.len(),
            hosted_by_host: i.hosts.len(),
            host_of_element: i.host_of.len(),
            dependencies_forward: i.dependencies.len(),
            dependencies_reverse: i.dependents.len(),
            instances_by_type: i.type_instances.len(),
            elements_by_level: i.level_elements.len(),
            elements_by_class: i.elements_by_class.len(),
        }
    }
    pub fn scan_metrics(&self) -> Vec<CoreScanMetric> {
        self.scan_counters.metrics()
    }
    pub fn class_elements(&self, class_id: &ClassId) -> Vec<ElementId> {
        self.indexes()
            .elements_by_class
            .get(class_id)
            .into_iter()
            .flatten()
            .copied()
            .collect()
    }
    pub fn relationships_from(&self, entity: EntityRef) -> Vec<&Relationship> {
        self.indexes()
            .by_source
            .get(&entity)
            .into_iter()
            .flatten()
            .filter_map(|id| self.relationships.get(id))
            .collect()
    }
    pub fn relationships_to(&self, entity: EntityRef) -> Vec<&Relationship> {
        self.indexes()
            .by_target
            .get(&entity)
            .into_iter()
            .flatten()
            .filter_map(|id| self.relationships.get(id))
            .collect()
    }
    pub fn verify_index_consistency(&self) -> Result<(), CoreError> {
        let _scan = self.scan_counters.track(
            "index_consistency",
            self.elements.len(),
            self.relationships.len(),
            "explicit disposable-index consistency audit",
            true,
        );
        let mut expected = RelationshipIndexes::default();
        for e in self.elements.values() {
            expected.update_element(e, true);
        }
        for r in self.relationships.values() {
            expected.update(r, true);
        }
        for o in self.operations.values() {
            expected.update_operation(o, true);
        }
        for case in self.engineering.circular_cases.values() {
            expected.update_circular_case(case, true);
        }
        for t in self.types.values() {
            expected.update_type(t, true);
        }
        for c in self.classes.values() {
            expected.update_class(c, true);
        }
        if &expected != self.indexes() {
            return Err(CoreError::InvalidDocument(
                "runtime indexes disagree with authoritative data".into(),
            ));
        }
        Ok(())
    }
    pub fn history_delta(&self, revision: u64) -> Result<TransactionDelta, CoreError> {
        let state = self
            .revision_states
            .get(&revision)
            .ok_or(CoreError::UnknownRevision(revision))?;
        if let Some(delta) = &state.delta {
            return Ok(delta.summary.clone());
        }
        if revision == 0 {
            return Ok(TransactionDelta::default());
        }
        let diff = self.history_diff(revision.saturating_sub(1), revision)?;
        Ok(TransactionDelta {
            revision_before: diff.revision_from,
            revision_after: diff.revision_to,
            drafting: diff.drafting,
            engineering: diff.engineering,
            foundation_structural: diff.foundation_structural,
            structural_configurator: diff.structural_configurator,
            geo_foundation: diff.geo_foundation,
            created_elements: diff.created,
            updated_elements: diff.modified,
            deleted_elements: diff.deleted,
            created_relationships: diff.relationships_created,
            deleted_relationships: diff.relationships_deleted,
            created_types: diff.types_created,
            updated_types: diff.types_modified,
            deleted_types: diff.types_deleted,
            created_levels: diff.levels_created,
            updated_levels: diff.levels_modified,
            deleted_levels: diff.levels_deleted,
            ..TransactionDelta::default()
        })
    }
    pub(super) fn allocation_counters(&self) -> AllocationCounters {
        AllocationCounters {
            drafting: self.draft_ids.clone(),
            element: self.next_element_id,
            operation: self.next_operation_id,
            element_type: self.next_type_id,
            level: self.next_level_id,
            relationship: self.next_relationship_id,
        }
    }
    pub(super) fn restore_counters(&mut self, c: &AllocationCounters) {
        self.draft_ids = c.drafting.clone();
        self.next_element_id = c.element;
        self.next_operation_id = c.operation;
        self.next_type_id = c.element_type;
        self.next_level_id = c.level;
        self.next_relationship_id = c.relationship;
    }
    pub(super) fn projection(&self, id: ElementId) -> Result<ElementProjection, CoreError> {
        if !self.elements.contains_key(&id) {
            return Ok(ElementProjection::default());
        }
        Ok(ElementProjection {
            effective: Some(self.effective_element(id)?),
            instance: self.elements.get(&id).cloned(),
            properties: self.resolved_properties(id)?,
            host: self.host_of(id),
            dependents: self.dependencies(id),
        })
    }
    pub(super) fn bounded_projections(
        &self,
        ids: &[ElementId],
    ) -> Result<BTreeMap<ElementId, ElementProjection>, CoreError> {
        let _scan = self.scan_counters.track(
            "bounded_element_projection",
            ids.len(),
            0,
            "affected elements and their resolved property sources",
            false,
        );
        ids.iter()
            .map(|id| Ok((*id, self.projection(*id)?)))
            .collect()
    }
    pub(super) fn dirty_from_projections(
        &self,
        affected: &[ElementId],
        before: &BTreeMap<ElementId, ElementProjection>,
    ) -> Vec<ElementId> {
        let _scan = self.scan_counters.track(
            "bounded_dirty_propagation",
            affected.len(),
            0,
            "forward dependencies and old/new hosted opening inputs",
            false,
        );
        let mut dirty: BTreeSet<_> = affected.iter().copied().collect();
        for id in affected {
            dirty.extend(self.dependencies(*id));
            if let Some(old) = before.get(id) {
                dirty.extend(&old.dependents);
            }
            let opening_input = |e: Option<&ElementRecord>, host| {
                e.filter(|e| {
                    matches!(
                        e.kind,
                        ElementKind::Opening(_) | ElementKind::PolygonalOpening(_)
                    )
                })
                .map(|e| (e.kind.clone(), host))
            };
            let old = before
                .get(id)
                .and_then(|p| opening_input(p.effective.as_ref(), p.host));
            let new = self
                .effective_element(*id)
                .ok()
                .and_then(|e| opening_input(Some(&e), self.host_of(*id)));
            if old != new {
                for (_, host) in old.iter().chain(new.iter()) {
                    dirty.extend(host);
                }
            }
        }
        dirty.into_iter().collect()
    }
    pub(super) fn make_delta(
        &self,
        revision_before: u64,
        changes: &[Change],
        affected: &[ElementId],
        before: &BTreeMap<ElementId, ElementProjection>,
    ) -> Result<TransactionDelta, CoreError> {
        let _scan = self.scan_counters.track(
            "bounded_transaction_delta",
            affected.len(),
            changes
                .iter()
                .filter(|c| matches!(c, Change::Relationship { .. }))
                .count(),
            "derive complete net entity changes and resolved property-source transitions",
            false,
        );
        let mut d = TransactionDelta {
            revision_before,
            revision_after: self.revision,
            affected_elements: affected.to_vec(),
            ..TransactionDelta::default()
        };
        for id in affected {
            let old = before.get(id).cloned().unwrap_or_default();
            let new = self.projection(*id)?;
            match (&old.instance, &new.instance) {
                (None, Some(_)) => d.created_elements.push(*id),
                (Some(_), None) => d.deleted_elements.push(*id),
                (Some(a), Some(b))
                    if a != b
                        || old.properties != new.properties
                        || old.effective != new.effective =>
                {
                    d.updated_elements.push(*id)
                }
                _ => {}
            }
            let keys: BTreeSet<_> = old
                .properties
                .keys()
                .chain(new.properties.keys())
                .cloned()
                .collect();
            for property in keys {
                let a = old.properties.get(&property);
                let b = new.properties.get(&property);
                if a != b {
                    d.property_changes.push(PropertyChange {
                        entity: EntityRef::Element(*id),
                        property,
                        before: a.map(|p| p.value.clone()),
                        after: b.map(|p| p.value.clone()),
                        source_before: a.map(|p| p.source),
                        source_after: b.map(|p| p.source),
                        type_id_before: a.and_then(|p| p.type_id),
                        type_id_after: b.and_then(|p| p.type_id),
                    });
                }
            }
        }
        // Summarize net transitions, so repeated commands in one batch collapse.
        let mut types = BTreeMap::new();
        let mut levels = BTreeMap::new();
        let mut relations = BTreeMap::new();
        for c in changes {
            match c {
                Change::Type { id, before, after } => {
                    let e = types.entry(*id).or_insert((before.clone(), after.clone()));
                    e.1 = after.clone();
                }
                Change::Level { id, before, after } => {
                    let e = levels.entry(*id).or_insert((before.clone(), after.clone()));
                    e.1 = after.clone();
                }
                Change::Relationship { id, before, after } => {
                    let e = relations
                        .entry(*id)
                        .or_insert((before.clone(), after.clone()));
                    e.1 = after.clone();
                }
                _ => {}
            }
        }
        for (id, (a, b)) in types {
            match (&a, &b) {
                (None, Some(_)) => d.created_types.push(id),
                (Some(_), None) => d.deleted_types.push(id),
                (Some(_), Some(_)) if a != b => d.updated_types.push(id),
                _ => {}
            }
            let keys: BTreeSet<_> = a
                .iter()
                .flat_map(|t| t.properties.keys())
                .chain(b.iter().flat_map(|t| t.properties.keys()))
                .cloned()
                .collect();
            for property in keys {
                let old = a.as_ref().and_then(|t| t.properties.get(&property));
                let new = b.as_ref().and_then(|t| t.properties.get(&property));
                if old != new {
                    d.property_changes.push(PropertyChange {
                        entity: EntityRef::Type(id),
                        property,
                        before: old.cloned(),
                        after: new.cloned(),
                        source_before: old.map(|_| PropertySource::Type),
                        source_after: new.map(|_| PropertySource::Type),
                        type_id_before: old.map(|_| id),
                        type_id_after: new.map(|_| id),
                    });
                }
            }
        }
        for (id, (a, b)) in levels {
            match (&a, &b) {
                (None, Some(_)) => d.created_levels.push(id),
                (Some(_), None) => d.deleted_levels.push(id),
                (Some(_), Some(_)) if a != b => d.updated_levels.push(id),
                _ => {}
            }
        }
        for (id, (a, b)) in relations {
            match (
                a.as_ref().filter(|r| r.is_active()),
                b.as_ref().filter(|r| r.is_active()),
            ) {
                (None, Some(_)) => d.created_relationships.push(id),
                (Some(_), None) => d.deleted_relationships.push(id),
                (Some(a), Some(b)) if a != b => d.updated_relationships.push(id),
                _ => {}
            }
        }
        d.structural_configurator = configured_building::summarize_configurator(changes);
        d.geo_foundation = geo_foundation::summarize_geo_foundation(changes);
        geo_foundation::add_geo_load_status_effects(&mut d.geo_foundation, &self.geo_foundation, changes);
        d.foundation_structural = foundation_structural::summarize_foundation_structural(changes);
        d.drafting = summarize_drafting(changes);
        d.engineering = engineering::summarize_engineering(
            changes, &self.engineering, &self.indexes().circular_cases_by_ground_section,
        );
        Ok(d)
    }
    pub(super) fn core_performance(&self, dirty_us: u64, journal: usize) -> CorePerformance {
        CorePerformance {
            core_scan_count: self.scan_counters.full_scan_count(),
            elements_scanned: self.scan_counters.element_count(),
            relationships_scanned: self.scan_counters.relationship_count(),
            dirty_propagation_us: dirty_us,
            snapshot_us: self.scan_counters.snapshot_total_us(),
            journal_entry_count: journal,
            scans: self.scan_metrics(),
        }
    }
}
