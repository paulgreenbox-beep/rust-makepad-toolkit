//! Disposable stable-ID indexes. The document relationships remain authoritative.
use crate::*;

type Links = BTreeMap<ElementId, BTreeMap<ElementId, BTreeSet<RelationshipId>>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RelationshipIndexes {
    pub active_ids: BTreeSet<RelationshipId>,
    pub by_entity: BTreeMap<EntityRef, BTreeSet<RelationshipId>>,
    pub by_source: BTreeMap<EntityRef, BTreeSet<RelationshipId>>,
    pub by_target: BTreeMap<EntityRef, BTreeSet<RelationshipId>>,
    pub elements_by_class: BTreeMap<ClassId, BTreeSet<ElementId>>,
    pub operations_by_element: BTreeMap<ElementId, BTreeSet<OperationId>>,
    pub operations_by_class: BTreeMap<ClassId, BTreeSet<OperationId>>,
    pub operations_by_parent: BTreeMap<OperationId, BTreeSet<OperationId>>,
    pub operations_by_analysis: BTreeMap<AnalysisCaseId, BTreeSet<OperationId>>,
    pub operations_by_ground_section: BTreeMap<GroundSectionId, BTreeSet<OperationId>>,
    pub circular_cases_by_ground_section: BTreeMap<GroundSectionId, BTreeSet<AnalysisCaseId>>,
    pub types_by_class: BTreeMap<ClassId, BTreeSet<TypeId>>,
    pub class_children: BTreeMap<ClassId, BTreeSet<ClassId>>,
    pub hosts: Links,
    pub host_of: Links,
    pub dependents: Links,
    pub dependencies: Links,
    pub type_instances: BTreeMap<TypeId, BTreeMap<ElementId, BTreeSet<RelationshipId>>>,
    pub level_elements: BTreeMap<LevelId, BTreeMap<ElementId, BTreeSet<RelationshipId>>>,
    pub element_levels: BTreeMap<ElementId, BTreeMap<LevelId, BTreeSet<RelationshipId>>>,
}

pub(crate) fn hosting_pair(r: &Relationship) -> Option<(ElementId, ElementId)> {
    match (r.relationship_type, r.source_id, r.target_id) {
        (RelationshipType::Hosts, EntityRef::Element(host), EntityRef::Element(child))
        | (RelationshipType::HostedBy, EntityRef::Element(child), EntityRef::Element(host)) => {
            Some((host, child))
        }
        _ => None,
    }
}

fn dependency_pair(r: &Relationship) -> Option<(ElementId, ElementId)> {
    match (r.relationship_type, r.source_id, r.target_id) {
        (
            RelationshipType::Hosts | RelationshipType::Contains | RelationshipType::Supports,
            EntityRef::Element(a),
            EntityRef::Element(b),
        ) => Some((a, b)),
        (
            RelationshipType::HostedBy
            | RelationshipType::ContainedIn
            | RelationshipType::SupportedBy,
            EntityRef::Element(a),
            EntityRef::Element(b),
        ) => Some((b, a)),
        _ => None,
    }
}

fn link<A: Ord, B: Ord>(
    index: &mut BTreeMap<A, BTreeMap<B, BTreeSet<RelationshipId>>>,
    a: A,
    b: B,
    id: RelationshipId,
    insert: bool,
) {
    if insert {
        index.entry(a).or_default().entry(b).or_default().insert(id);
    } else if let Some(targets) = index.get_mut(&a) {
        if let Some(ids) = targets.get_mut(&b) {
            ids.remove(&id);
            if ids.is_empty() {
                targets.remove(&b);
            }
        }
        if targets.is_empty() {
            index.remove(&a);
        }
    }
}

impl RelationshipIndexes {
    pub fn update_circular_case(&mut self, case: &CircularAnalysisCase, insert: bool) {
        update_set(
            &mut self.circular_cases_by_ground_section,
            case.definition.ground_section_id,
            case.analysis_id,
            insert,
        );
    }
    pub fn update_type(&mut self, value: &ElementType, insert: bool) {
        update_set(
            &mut self.types_by_class,
            value.class_id.clone(),
            value.id,
            insert,
        );
    }
    pub fn update_class(&mut self, value: &ClassDef, insert: bool) {
        if let Some(parent) = &value.parent_id {
            update_set(
                &mut self.class_children,
                parent.clone(),
                value.id.clone(),
                insert,
            );
        }
    }
    pub fn update_element(&mut self, element: &ElementRecord, insert: bool) {
        update_set(
            &mut self.elements_by_class,
            element.class_id.clone(),
            element.id,
            insert,
        );
    }
    pub fn update_operation(&mut self, operation: &OperationRecord, insert: bool) {
        if let OperationKind::EngineeringGroundChanged { ground_section_id, .. } = operation.kind {
            update_set(
                &mut self.operations_by_ground_section,
                ground_section_id,
                operation.id,
                insert,
            );
        }
        if let OperationKind::EngineeringChanged { analysis_id, .. } = operation.kind {
            update_set(
                &mut self.operations_by_analysis,
                analysis_id,
                operation.id,
                insert,
            );
        }
        if let Some(parent) = operation.parent {
            update_set(&mut self.operations_by_parent, parent, operation.id, insert);
        }
        for id in operation.inputs.iter().chain(&operation.outputs) {
            update_set(&mut self.operations_by_element, *id, operation.id, insert);
        }
        let mut classes = Vec::new();
        match &operation.kind {
            OperationKind::ClassDefined {
                class_id,
                parent_id,
                ..
            } => {
                classes.push(class_id.clone());
                classes.extend(parent_id.iter().cloned());
            }
            OperationKind::PropertyDefined { class_id, .. }
            | OperationKind::PropertySet { class_id, .. }
            | OperationKind::ElementCreated { class_id, .. }
            | OperationKind::ElementDeleted { class_id, .. } => classes.push(class_id.clone()),
            OperationKind::StructuralConfiguratorChanged { .. }
            | OperationKind::GeoFoundationChanged { .. }
            | OperationKind::FoundationStructuralChanged { .. }
            | OperationKind::BimChanged { .. }
            | OperationKind::DraftingChanged { .. }
            | OperationKind::EngineeringGroundChanged { .. }
            | OperationKind::EngineeringChanged { .. } => {}
        }
        for id in classes {
            update_set(&mut self.operations_by_class, id, operation.id, insert);
        }
    }
    pub fn update(&mut self, r: &Relationship, insert: bool) {
        if !r.is_active() {
            return;
        }
        if insert {
            self.active_ids.insert(r.id);
        } else {
            self.active_ids.remove(&r.id);
        }
        for entity in [r.source_id, r.target_id] {
            if insert {
                self.by_entity.entry(entity).or_default().insert(r.id);
            } else if let Some(ids) = self.by_entity.get_mut(&entity) {
                ids.remove(&r.id);
                if ids.is_empty() {
                    self.by_entity.remove(&entity);
                }
            }
        }
        update_set(&mut self.by_source, r.source_id, r.id, insert);
        update_set(&mut self.by_target, r.target_id, r.id, insert);
        if let Some((host, child)) = hosting_pair(r) {
            link(&mut self.hosts, host, child, r.id, insert);
            link(&mut self.host_of, child, host, r.id, insert);
        }
        if let Some((source, dependent)) = dependency_pair(r) {
            link(&mut self.dependents, source, dependent, r.id, insert);
            link(&mut self.dependencies, dependent, source, r.id, insert);
        }
        match (r.relationship_type, r.source_id, r.target_id) {
            (RelationshipType::InstanceOf, EntityRef::Element(id), EntityRef::Type(type_id)) => {
                link(&mut self.type_instances, type_id, id, r.id, insert);
            }
            (
                RelationshipType::BelongsToLevel,
                EntityRef::Element(id),
                EntityRef::Level(level_id),
            ) => {
                link(&mut self.level_elements, level_id, id, r.id, insert);
                link(&mut self.element_levels, id, level_id, r.id, insert);
            }
            _ => {}
        }
    }
}

fn update_set<K: Ord, V: Ord>(map: &mut BTreeMap<K, BTreeSet<V>>, key: K, value: V, insert: bool) {
    if insert {
        map.entry(key).or_default().insert(value);
    } else if let Some(values) = map.get_mut(&key) {
        values.remove(&value);
        if values.is_empty() {
            map.remove(&key);
        }
    }
}

/// These counters are disposable and excluded from serialization. Independent
/// clones keep failed atomic candidates from changing the live operation's data.
#[derive(Debug, Default)]
pub(crate) struct ScanCounters {
    document: AtomicU64,
    relationships: AtomicU64,
    snapshot_duration: AtomicU64,
    snapshot_bytes: AtomicU64,
    snapshot_total: AtomicU64,
    elements: AtomicU64,
    full_scans: AtomicU64,
    scans: std::sync::Mutex<Vec<CoreScanMetric>>,
}
impl Clone for ScanCounters {
    fn clone(&self) -> Self {
        Self {
            document: AtomicU64::new(self.document_count()),
            relationships: AtomicU64::new(self.relationship_count()),
            snapshot_duration: AtomicU64::new(self.snapshot_duration()),
            snapshot_bytes: AtomicU64::new(self.snapshot_bytes()),
            snapshot_total: AtomicU64::new(self.snapshot_total_us()),
            elements: AtomicU64::new(self.element_count()),
            full_scans: AtomicU64::new(self.full_scan_count()),
            scans: std::sync::Mutex::new(self.metrics()),
        }
    }
}
impl ScanCounters {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn document_scan(&self) {
        self.document.fetch_add(1, Ordering::Relaxed);
    }
    pub fn relationship_scan(&self, count: usize) {
        self.relationships
            .fetch_add(count as u64, Ordering::Relaxed);
    }
    pub fn snapshot(&self, duration: u64, bytes: usize) {
        self.snapshot_duration.store(duration, Ordering::Relaxed);
        self.snapshot_bytes.store(bytes as u64, Ordering::Relaxed);
    }
    pub fn document_count(&self) -> u64 {
        self.document.load(Ordering::Relaxed)
    }
    pub fn relationship_count(&self) -> u64 {
        self.relationships.load(Ordering::Relaxed)
    }
    pub fn snapshot_duration(&self) -> u64 {
        self.snapshot_duration.load(Ordering::Relaxed)
    }
    pub fn snapshot_bytes(&self) -> u64 {
        self.snapshot_bytes.load(Ordering::Relaxed)
    }
    pub fn snapshot_total_us(&self) -> u64 {
        self.snapshot_total.load(Ordering::Relaxed)
    }
    pub fn snapshot_total(&self, duration: u64) {
        self.snapshot_total.store(duration, Ordering::Relaxed);
    }
    pub fn element_count(&self) -> u64 {
        self.elements.load(Ordering::Relaxed)
    }
    pub fn full_scan_count(&self) -> u64 {
        self.full_scans.load(Ordering::Relaxed)
    }
    pub fn metrics(&self) -> Vec<CoreScanMetric> {
        self.scans.lock().expect("scan metrics").clone()
    }
    pub fn track(
        &self,
        name: &str,
        elements: usize,
        relationships: usize,
        reason: &str,
        full_document: bool,
    ) -> ScanGuard<'_> {
        self.elements.fetch_add(elements as u64, Ordering::Relaxed);
        if full_document {
            self.full_scans.fetch_add(1, Ordering::Relaxed);
        }
        ScanGuard {
            counters: self,
            start: Instant::now(),
            metric: CoreScanMetric {
                scan_name: name.into(),
                elements_scanned: elements as u64,
                relationships_scanned: relationships as u64,
                duration_us: 0,
                reason: reason.into(),
                full_document,
            },
        }
    }
}

pub(crate) struct ScanGuard<'a> {
    counters: &'a ScanCounters,
    start: Instant,
    metric: CoreScanMetric,
}
impl ScanGuard<'_> {
    pub fn set_reason(&mut self, reason: String) {
        self.metric.reason = reason;
    }
}
impl Drop for ScanGuard<'_> {
    fn drop(&mut self) {
        self.metric.duration_us = elapsed_us(self.start);
        self.counters
            .scans
            .lock()
            .expect("scan metrics")
            .push(self.metric.clone());
    }
}

#[derive(Clone, Copy)]
pub(crate) enum CollectionKind {
    Elements,
    Relationships,
    Other,
}
/// Counts records actually consumed, including partial and reverse iteration.
/// Full enumeration is reported only after every available record was visited.
pub(crate) struct MeasuredIterator<'a, I> {
    inner: I,
    counters: &'a ScanCounters,
    start: Instant,
    name: &'static str,
    reason: &'static str,
    kind: CollectionKind,
    total: usize,
    visited: usize,
}
impl ScanCounters {
    pub fn enumerate<I: Iterator>(
        &self,
        inner: I,
        total: usize,
        name: &'static str,
        reason: &'static str,
        kind: CollectionKind,
    ) -> MeasuredIterator<'_, I> {
        MeasuredIterator {
            inner,
            counters: self,
            start: Instant::now(),
            name,
            reason,
            kind,
            total,
            visited: 0,
        }
    }
}
impl<I: Iterator> Iterator for MeasuredIterator<'_, I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<Self::Item> {
        let value = self.inner.next();
        if value.is_some() {
            self.visited += 1;
        }
        value
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}
impl<I: DoubleEndedIterator> DoubleEndedIterator for MeasuredIterator<'_, I> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let value = self.inner.next_back();
        if value.is_some() {
            self.visited += 1;
        }
        value
    }
}
impl<I: ExactSizeIterator> ExactSizeIterator for MeasuredIterator<'_, I> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}
impl<I> Drop for MeasuredIterator<'_, I> {
    fn drop(&mut self) {
        let elements = if matches!(self.kind, CollectionKind::Elements) {
            self.visited as u64
        } else {
            0
        };
        let relationships = if matches!(self.kind, CollectionKind::Relationships) {
            self.visited as u64
        } else {
            0
        };
        let full = self.total > 0 && self.visited == self.total;
        self.counters
            .elements
            .fetch_add(elements, Ordering::Relaxed);
        if full {
            self.counters.full_scans.fetch_add(1, Ordering::Relaxed);
        }
        self.counters
            .scans
            .lock()
            .expect("scan metrics")
            .push(CoreScanMetric {
                scan_name: self.name.into(),
                elements_scanned: elements,
                relationships_scanned: relationships,
                duration_us: elapsed_us(self.start),
                reason: self.reason.into(),
                full_document: full,
            });
    }
}

impl Document {
    pub(crate) fn indexes(&self) -> &RelationshipIndexes {
        self.relationship_indexes.get_or_init(|| {
            let _scan = self.scan_counters.track(
                "runtime_index_build",
                self.elements.len(),
                self.relationships.len(),
                "rebuild disposable indexes on open or direct deserialization",
                true,
            );
            let mut indexes = RelationshipIndexes::default();
            self.scan_counters
                .relationship_scan(self.relationships.len());
            for relationship in self.relationships.values() {
                indexes.update(relationship, true);
            }
            for element in self.elements.values() {
                indexes.update_element(element, true);
            }
            for operation in self.operations.values() {
                indexes.update_operation(operation, true);
            }
            for case in self.engineering.circular_cases.values() {
                indexes.update_circular_case(case, true);
            }
            for element_type in self.types.values() {
                indexes.update_type(element_type, true);
            }
            for class in self.classes.values() {
                indexes.update_class(class, true);
            }
            indexes
        })
    }

    pub(crate) fn index_relationship_change(
        &mut self,
        before: Option<&Relationship>,
        after: Option<&Relationship>,
    ) {
        // Initialize from the map before its corresponding authoritative mutation.
        self.indexes();
        let indexes = self
            .relationship_indexes
            .get_mut()
            .expect("initialized index");
        if let Some(before) = before {
            indexes.update(before, false);
        }
        if let Some(after) = after {
            indexes.update(after, true);
        }
    }

    pub(crate) fn index_nonrelationship_change(&mut self, change: &Change, forward: bool) {
        self.indexes();
        let indexes = self
            .relationship_indexes
            .get_mut()
            .expect("initialized index");
        match change {
            Change::CircularEngineeringCase { before, after, .. } => {
                let (old, new) = if forward { (before, after) } else { (after, before) };
                if let Some(old) = old {
                    indexes.update_circular_case(old, false);
                }
                if let Some(new) = new {
                    indexes.update_circular_case(new, true);
                }
            }
            Change::Element { before, after, .. } => {
                let (old, new) = if forward {
                    (before, after)
                } else {
                    (after, before)
                };
                if let Some(old) = old {
                    indexes.update_element(old, false);
                }
                if let Some(new) = new {
                    indexes.update_element(new, true);
                }
            }
            Change::Operation { before, after, .. } => {
                let (old, new) = if forward {
                    (before, after)
                } else {
                    (after, before)
                };
                if let Some(old) = old {
                    indexes.update_operation(old, false);
                }
                if let Some(new) = new {
                    indexes.update_operation(new, true);
                }
            }
            Change::Type { before, after, .. } => {
                let (old, new) = if forward {
                    (before, after)
                } else {
                    (after, before)
                };
                if let Some(old) = old {
                    indexes.update_type(old, false);
                }
                if let Some(new) = new {
                    indexes.update_type(new, true);
                }
            }
            Change::Class { before, after, .. } => {
                let (old, new) = if forward {
                    (before, after)
                } else {
                    (after, before)
                };
                if let Some(old) = old {
                    indexes.update_class(old, false);
                }
                if let Some(new) = new {
                    indexes.update_class(new, true);
                }
            }
            _ => {}
        }
    }
}
