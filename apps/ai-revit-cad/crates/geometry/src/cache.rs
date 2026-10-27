use crate::*;
use ai_revit_core::{ClassId, LevelId};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DerivedGeometryRecord {
    pub element_id: ElementId,
    pub geometry_revision: u64,
    pub mesh_revision: u64,
    pub source_document_revision: u64,
    pub geometry_fingerprint: String,
    /// v0.4 alias for `geometry_fingerprint`.
    pub fingerprint: String,
    pub representation_quality: RepresentationQuality,
    pub valid: bool,
    pub bounds: Option<Aabb>,
    pub volume: Option<f64>,
    pub surface_area: Option<f64>,
    pub quantities: Option<GeometryQuantities>,
    pub triangle_count: usize,
    pub derived_from: Vec<ElementId>,
    pub openings: Vec<ElementId>,
    pub failure: Option<GeometryError>,
    #[serde(skip)]
    opening_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GeometryDependencies {
    pub element_id: ElementId,
    pub host_id: Option<ElementId>,
    pub derived_from: Vec<ElementId>,
    pub openings: Vec<ElementId>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GeometryComparison {
    pub element_id: ElementId,
    pub geometry_revision_before: u64,
    pub geometry_revision_after: u64,
    pub representation_quality: RepresentationQuality,
    pub bounds_changed: bool,
    pub volume_delta: Option<f64>,
    pub surface_area_delta: Option<f64>,
    pub triangle_count_delta: i64,
    pub openings_changed: bool,
    pub openings_before: Vec<ElementId>,
    pub openings_after: Vec<ElementId>,
    pub gross_quantity_delta: Option<VolumeSurfaceDelta>,
    pub net_quantity_delta: Option<VolumeSurfaceDelta>,
    pub opening_delta: Option<OpeningQuantityDelta>,
    pub perimeter_delta: Option<f64>,
    pub length_delta: Option<f64>,
    pub source_dependency_changes: SourceDependencyChanges,
    pub valid_before: bool,
    pub valid_after: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VolumeSurfaceDelta {
    pub volume: Option<f64>,
    pub surface_area: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OpeningQuantityDelta {
    pub area: Option<f64>,
    pub volume: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SourceDependencyChanges {
    pub added: Vec<ElementId>,
    pub removed: Vec<ElementId>,
}

/// Current cache sizes and retained lightweight history, without scanning records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GeometryObservationMetrics {
    pub record_count: usize,
    pub mesh_count: usize,
    pub projected_count: usize,
    pub geometry_error_count: usize,
    pub resolution_error_count: usize,
    pub error_count: usize,
    pub history_element_count: usize,
    pub history_record_count: usize,
    pub scanned_history_records: usize,
    pub duration_us: u64,
    pub reason: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GeometryObservationError {
    pub error_code: String,
    pub element_id: Option<ElementId>,
    pub message: String,
}
impl GeometryObservationError {
    pub fn new(code: &str, element_id: Option<ElementId>, message: impl Into<String>) -> Self {
        Self {
            error_code: code.to_owned(),
            element_id,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for GeometryObservationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error_code, self.message)
    }
}
impl std::error::Error for GeometryObservationError {}

/// Current meshes plus lightweight historical observations. Historical entries
/// contain quantities/provenance only; no old vertex buffers are retained.
pub struct GeometryCache {
    document_uuid: Option<String>,
    document_revision: Option<u64>,
    fingerprints: BTreeMap<ElementId, String>,
    scene: Scene,
    projected: BTreeMap<ElementId, ProjectedElement>,
    failures: BTreeMap<ElementId, GeometryError>,
    resolution_failures: BTreeMap<ElementId, String>,
    statuses: BTreeMap<ElementId, ElementGeometryStatus>,
    records: BTreeMap<ElementId, DerivedGeometryRecord>,
    history: BTreeMap<ElementId, BTreeMap<u64, DerivedGeometryRecord>>,
    history_record_count: usize,
    geometries: BTreeMap<ElementId, DerivedGeometry>,
    elements: BTreeMap<ElementId, ElementRecord>,
    host_ids: BTreeMap<ElementId, ElementId>,
    quantity_cache: QuantityCache,
    spatial_index: SpatialIndex,
    spatial_memberships: BTreeMap<ElementId, SpatialMembership>,
    next_geometry_revision: u64,
    options: GeometryOptions,
    backend: Box<dyn GeometryBackend>,
    show_opening_volumes: bool,
    pending_display_ids: BTreeSet<ElementId>,
}
#[derive(Clone, Debug, PartialEq)]
struct SpatialMembership {
    bounds: Aabb,
    class_id: ClassId,
    level_id: Option<LevelId>,
}

impl Default for GeometryCache {
    fn default() -> Self {
        Self::new()
    }
}

impl GeometryCache {
    pub fn new() -> Self {
        Self::with_options(GeometryOptions::default())
    }
    pub fn with_options(options: GeometryOptions) -> Self {
        Self::with_backend(Box::new(MeshGeometryBackend), options)
    }
    pub fn with_backend(backend: Box<dyn GeometryBackend>, options: GeometryOptions) -> Self {
        Self {
            document_uuid: None,
            document_revision: None,
            fingerprints: BTreeMap::new(),
            scene: Scene::new(),
            projected: BTreeMap::new(),
            failures: BTreeMap::new(),
            resolution_failures: BTreeMap::new(),
            statuses: BTreeMap::new(),
            records: BTreeMap::new(),
            history: BTreeMap::new(),
            history_record_count: 0,
            geometries: BTreeMap::new(),
            elements: BTreeMap::new(),
            host_ids: BTreeMap::new(),
            quantity_cache: QuantityCache::new(),
            spatial_index: SpatialIndex::new(),
            spatial_memberships: BTreeMap::new(),
            next_geometry_revision: 1,
            options,
            backend,
            show_opening_volumes: false,
            pending_display_ids: BTreeSet::new(),
        }
    }
    /// Required on an application open/replace operation, including same-UUID reload.
    pub fn invalidate_all(&mut self) {
        self.document_uuid = None;
        self.document_revision = None;
        self.clear_document_state();
    }
    fn clear_document_state(&mut self) {
        self.fingerprints.clear();
        self.scene = Scene::new();
        self.projected.clear();
        self.failures.clear();
        self.resolution_failures.clear();
        self.statuses.clear();
        self.records.clear();
        self.history.clear();
        self.history_record_count = 0;
        self.geometries.clear();
        self.elements.clear();
        self.host_ids.clear();
        self.quantity_cache.clear();
        self.spatial_index.clear();
        self.spatial_memberships.clear();
        self.pending_display_ids.clear();
    }
    pub fn show_opening_volumes(&self) -> bool {
        self.show_opening_volumes
    }
    /// Changes derived display only; never changes document revision or history.
    pub fn set_show_opening_volumes(&mut self, show: bool) {
        if self.show_opening_volumes == show {
            return;
        }
        self.show_opening_volumes = show;
        let ids: Vec<_> = self
            .elements
            .iter()
            .filter_map(|(id, e)| {
                matches!(
                    e.kind,
                    ElementKind::Opening(_) | ElementKind::PolygonalOpening(_)
                )
                .then_some(*id)
            })
            .collect();
        for id in ids {
            self.sync_object(id);
            self.pending_display_ids.insert(id);
        }
    }
    pub fn rebuild(&mut self, document: &Document) -> RebuildReport {
        let started = Instant::now();
        let uuid = document.document_uuid.to_string();
        let invalidated = self.document_uuid.as_deref() != Some(uuid.as_str());
        if invalidated {
            self.clear_document_state();
            self.document_uuid = Some(uuid);
            self.document_revision = None;
        }
        let quantity_rebuilds_before = self.quantity_cache.metrics().quantity_rebuild_count;
        let spatial_updates_before = self.spatial_index.metrics().update_count;
        let mut report = RebuildReport {
            document_invalidated: invalidated,
            ..RebuildReport::default()
        };
        // Dirty IDs describe the latest committed transaction. If revisions were
        // skipped, a measured full pass is necessary to recover all changes.
        let full_scan = self.document_revision.is_none()
            || self.document_revision.is_some_and(|revision| {
                revision != document.revision()
                    && revision.checked_add(1) != Some(document.revision())
            });
        let mut candidates: BTreeSet<ElementId> = if full_scan {
            report.total_document_scan_count = 1;
            report.scanned_element_count = document.elements().len();
            document.elements().map(|e| e.id).collect()
        } else if self.document_revision != Some(document.revision()) {
            document.dirty_ids().into_iter().collect()
        } else {
            BTreeSet::new()
        };
        if full_scan {
            candidates.extend(self.fingerprints.keys().copied());
        }
        for id in candidates {
            if document.element(id).is_none() {
                if self.fingerprints.contains_key(&id) {
                    self.remove_element(id);
                    report.removed_element_ids.push(id);
                }
                continue;
            }
            report.hosted_lookup_count += 1;
            let opening_ids = document.hosted_opening_ids(id);
            let host_id = document.host_of(id);
            let level_id = document.element_level(id);
            if let Some(host) = host_id {
                self.host_ids.insert(id, host);
            } else {
                self.host_ids.remove(&id);
            }
            let input = (|| -> Result<(GeometryInput, Option<ElementRecord>), String> {
                let element = document.effective_element(id).map_err(|e| e.to_string())?;
                let contextual_host = if matches!(element.kind, ElementKind::PolygonalOpening(_)) {
                    let host_id = host_id
                        .ok_or_else(|| "polygonal opening has no host relationship".to_owned())?;
                    Some(
                        document
                            .effective_element(host_id)
                            .map_err(|e| e.to_string())?,
                    )
                } else {
                    None
                };
                let openings = opening_ids
                    .iter()
                    .map(|id| document.effective_element(*id).map_err(|e| e.to_string()))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut derived_from = vec![id];
                derived_from.extend(opening_ids.iter().copied());
                if let Some(host) = host_id {
                    derived_from.push(host);
                }
                derived_from.sort();
                derived_from.dedup();
                Ok((
                    GeometryInput {
                        element,
                        openings,
                        derived_from,
                    },
                    contextual_host,
                ))
            })();
            let fingerprint = match &input {
                Ok((input, host)) => {
                    geometry_fingerprint(input, host.as_ref(), self.options, self.backend.name())
                }
                Err(error) => format!("resolution_failed:{error}"),
            };
            report.fingerprint_comparison_count += 1;
            // Invalid geometry carries semantic IDs in its error. A duplicate
            // void may leave the geometric input unchanged when deleted, but
            // the error must be reevaluated against the remaining contributors.
            let invalid_provenance_changed = self.records.get(&id).is_some_and(|record| {
                !record.valid
                    && input.as_ref().is_ok_and(|(input, _)| {
                        record.openings != opening_ids || record.derived_from != input.derived_from
                    })
            });
            if self.fingerprints.get(&id) == Some(&fingerprint) && !invalid_provenance_changed {
                if let Ok((input, _)) = input {
                    let display_changed = self
                        .elements
                        .get(&id)
                        .is_none_or(|old| old.name != input.element.name);
                    self.elements.insert(id, input.element);
                    self.refresh_provenance(
                        id,
                        input.derived_from,
                        opening_ids,
                        document.revision(),
                    );
                    self.refresh_indexes(id, level_id);
                    if display_changed {
                        self.sync_object(id);
                        self.pending_display_ids.insert(id);
                    }
                }
                continue;
            }
            self.fingerprints.insert(id, fingerprint.clone());
            match input {
                Ok((input, contextual_host)) => self.rebuild_element(
                    input,
                    contextual_host.as_ref(),
                    fingerprint,
                    document.revision(),
                    level_id,
                ),
                Err(error) => {
                    self.rebuild_resolution_failure(id, fingerprint, document.revision(), error)
                }
            }
            report.rebuilt_element_ids.push(id);
        }
        self.document_revision = Some(document.revision());
        report.display_changed_element_ids = std::mem::take(&mut self.pending_display_ids)
            .into_iter()
            .collect();
        report.rebuilt_element_count = report.rebuilt_element_ids.len();
        report.removed_element_count = report.removed_element_ids.len();
        let elapsed = started.elapsed();
        report.rebuild_duration_ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
        report.rebuild_duration_us = elapsed.as_micros().min(u64::MAX as u128) as u64;
        report.quantity_rebuilt_element_count = self
            .quantity_cache
            .metrics()
            .quantity_rebuild_count
            .saturating_sub(quantity_rebuilds_before);
        report.spatial_updated_element_count = self
            .spatial_index
            .metrics()
            .update_count
            .saturating_sub(spatial_updates_before);
        report
    }
    fn next_revision(&mut self) -> u64 {
        let revision = self.next_geometry_revision;
        self.next_geometry_revision = self.next_geometry_revision.saturating_add(1);
        revision
    }
    fn retain_history_record(&mut self, record: &DerivedGeometryRecord) {
        let previous = self
            .history
            .entry(record.element_id)
            .or_default()
            .insert(record.geometry_revision, record.clone());
        if previous.is_none() {
            self.history_record_count = self.history_record_count.saturating_add(1);
        }
    }
    /// Membership changes are observations even when the physical mesh is identical.
    fn refresh_provenance(
        &mut self,
        id: ElementId,
        derived_from: Vec<ElementId>,
        openings: Vec<ElementId>,
        doc_revision: u64,
    ) {
        let changed = self.records.get(&id).is_some_and(|record| {
            record.derived_from != derived_from || record.openings != openings
        });
        let revision = changed.then(|| self.next_revision());
        let Some(record) = self.records.get_mut(&id) else {
            return;
        };
        record.source_document_revision = doc_revision;
        record.derived_from = derived_from;
        record.openings = openings;
        if let Some(revision) = revision {
            record.geometry_revision = revision;
            let observation = record.clone();
            self.retain_history_record(&observation);
        }
        // mesh_revision, backend buffers and status.mesh_revision are unchanged.
        // refresh_indexes runs next and refreshes the quantity observation revision
        // through its fingerprint hit, without recomputing quantity values.
    }
    fn rebuild_element(
        &mut self,
        input: GeometryInput,
        contextual_host: Option<&ElementRecord>,
        fingerprint: String,
        doc_revision: u64,
        level_id: Option<LevelId>,
    ) {
        let id = input.element.id;
        let revision = self.next_revision();
        let openings = input
            .openings
            .iter()
            .filter_map(|o| {
                matches!(
                    o.kind,
                    ElementKind::Opening(_) | ElementKind::PolygonalOpening(_)
                )
                .then_some(o.id)
            })
            .collect();
        let mut record = DerivedGeometryRecord {
            element_id: id,
            geometry_revision: revision,
            mesh_revision: revision,
            source_document_revision: doc_revision,
            geometry_fingerprint: fingerprint.clone(),
            fingerprint,
            representation_quality: RepresentationQuality::ApproximateMesh,
            valid: false,
            bounds: None,
            volume: None,
            surface_area: None,
            quantities: None,
            triangle_count: 0,
            derived_from: input.derived_from.clone(),
            openings,
            failure: None,
            opening_fingerprint: opening_fingerprint(&input),
        };
        self.elements.insert(id, input.element.clone());
        self.resolution_failures.remove(&id);
        match self
            .backend
            .derive_contextual(&input, contextual_host, self.options)
        {
            Ok(geometry) => {
                record.valid = true;
                record.representation_quality = geometry.representation_quality;
                record.bounds = self.backend.bounds(&geometry);
                record.volume = self.backend.volume(&geometry);
                record.surface_area = self.backend.surface_area(&geometry);
                record.quantities = Some(geometry.quantities.clone());
                record.triangle_count = geometry.mesh.triangles().len();
                self.failures.remove(&id);
                self.projected.insert(
                    id,
                    ProjectedElement {
                        element_id: id,
                        object_id: object_id_for(id),
                        quality: GeometryQuality::ApproximateTriangleMesh,
                    },
                );
                self.statuses.insert(
                    id,
                    ElementGeometryStatus {
                        element_id: id,
                        status: GeometryStatusKind::Valid,
                        bounds: record.bounds,
                        mesh_revision: revision,
                        failure: None,
                        resolution_error: None,
                    },
                );
                self.geometries.insert(id, geometry);
            }
            Err(failure) => {
                record.failure = Some(failure.clone());
                self.geometries.remove(&id);
                self.projected.remove(&id);
                self.failures.insert(id, failure.clone());
                self.statuses.insert(
                    id,
                    ElementGeometryStatus {
                        element_id: id,
                        status: GeometryStatusKind::Invalid,
                        bounds: None,
                        mesh_revision: revision,
                        failure: Some(failure),
                        resolution_error: None,
                    },
                );
            }
        }
        self.retain_history_record(&record);
        self.records.insert(id, record);
        self.refresh_indexes(id, level_id);
        self.sync_object(id);
    }
    fn rebuild_resolution_failure(
        &mut self,
        id: ElementId,
        fingerprint: String,
        doc_revision: u64,
        message: String,
    ) {
        let revision = self.next_revision();
        self.scene.remove(object_id_for(id));
        self.geometries.remove(&id);
        self.projected.remove(&id);
        self.failures.remove(&id);
        self.resolution_failures.insert(id, message.clone());
        self.statuses.insert(
            id,
            ElementGeometryStatus {
                element_id: id,
                status: GeometryStatusKind::ResolutionFailed,
                bounds: None,
                mesh_revision: revision,
                failure: None,
                resolution_error: Some(message),
            },
        );
        let record = DerivedGeometryRecord {
            element_id: id,
            geometry_revision: revision,
            mesh_revision: revision,
            source_document_revision: doc_revision,
            geometry_fingerprint: fingerprint.clone(),
            fingerprint,
            representation_quality: RepresentationQuality::ApproximateMesh,
            valid: false,
            bounds: None,
            volume: None,
            surface_area: None,
            quantities: None,
            triangle_count: 0,
            derived_from: vec![id],
            openings: vec![],
            failure: None,
            opening_fingerprint: String::new(),
        };
        self.retain_history_record(&record);
        self.records.insert(id, record);
        self.clear_observation_indexes(id);
    }
    fn remove_element(&mut self, id: ElementId) {
        self.scene.remove(object_id_for(id));
        self.fingerprints.remove(&id);
        self.projected.remove(&id);
        self.failures.remove(&id);
        self.resolution_failures.remove(&id);
        self.statuses.remove(&id);
        self.records.remove(&id);
        self.geometries.remove(&id);
        self.elements.remove(&id);
        self.host_ids.remove(&id);
        self.clear_observation_indexes(id);
        // Keep lightweight history so old geometry revisions remain comparable.
    }
    fn clear_observation_indexes(&mut self, id: ElementId) {
        self.quantity_cache.remove(id);
        self.spatial_index.remove(id);
        self.spatial_memberships.remove(&id);
    }

    fn refresh_indexes(&mut self, id: ElementId, level_id: Option<LevelId>) {
        let Some(element) = self.elements.get(&id) else {
            self.clear_observation_indexes(id);
            return;
        };
        let Some((valid, bounds, geometry_revision, fingerprint, quantities)) =
            self.records.get(&id).map(|record| {
                (
                    record.valid,
                    record.bounds,
                    record.geometry_revision,
                    record.geometry_fingerprint.clone(),
                    record.quantities.clone(),
                )
            })
        else {
            self.clear_observation_indexes(id);
            return;
        };
        let identity = QuantityIdentity {
            class_id: element.class_id.clone(),
            type_id: element.type_id,
            level_id,
        };
        if valid {
            if let Some(quantities) = quantities {
                self.quantity_cache.upsert(
                    ElementQuantities {
                        element_id: id,
                        geometry_revision,
                        geometry_fingerprint: fingerprint,
                        quantities,
                    },
                    identity,
                );
            } else {
                self.quantity_cache.remove(id);
            }
        } else {
            self.quantity_cache.remove(id);
        }

        let desired_membership =
            valid
                .then_some(bounds)
                .flatten()
                .map(|bounds| SpatialMembership {
                    bounds,
                    class_id: element.class_id.clone(),
                    level_id,
                });
        if self.spatial_memberships.get(&id) == desired_membership.as_ref() {
            return;
        }
        if let Some(membership) = desired_membership {
            match self.spatial_index.upsert(
                id,
                membership.bounds,
                membership.class_id.clone(),
                membership.level_id,
            ) {
                Ok(()) => {
                    self.spatial_memberships.insert(id, membership);
                }
                Err(_) => {
                    self.spatial_index.remove(id);
                    self.spatial_memberships.remove(&id);
                }
            }
        } else {
            self.spatial_index.remove(id);
            self.spatial_memberships.remove(&id);
        }
    }
    fn sync_object(&mut self, id: ElementId) {
        let object_id = object_id_for(id);
        let Some(element) = self.elements.get(&id) else {
            self.scene.remove(object_id);
            return;
        };
        let Some(geometry) = self.geometries.get(&id) else {
            self.scene.remove(object_id);
            return;
        };
        if geometry.mesh.triangles().is_empty()
            || (matches!(
                element.kind,
                ElementKind::Opening(_) | ElementKind::PolygonalOpening(_)
            ) && !self.show_opening_volumes)
        {
            self.scene.remove(object_id);
            return;
        }
        let (name, mut color) = display_style(element);
        if matches!(
            element.kind,
            ElementKind::Opening(_) | ElementKind::PolygonalOpening(_)
        ) {
            color[3] = 0.25;
        }
        let mesh = Mesh::new(geometry.mesh.triangles().to_vec());
        if let Some(object) = self.scene.get_mut(object_id) {
            object.mesh = mesh;
            object.name = name;
            object.color = color;
        } else {
            let mut object = SceneObject::new(object_id, name, mesh);
            object.color = color;
            self.scene.insert(object);
        }
    }
    pub fn backend_capabilities(&self) -> BackendCapabilities {
        self.backend.capabilities()
    }

    pub fn quantities_get(&self, id: ElementId) -> Option<ElementQuantities> {
        self.quantity_cache.get(id).cloned()
    }

    pub fn quantities_batch(&self, ids: &[ElementId]) -> QuantityBatch {
        let mut batch = self.quantity_cache.batch(ids);
        let candidates: Vec<_> = batch
            .elements
            .iter()
            .enumerate()
            .filter_map(|(index, record)| {
                let geometry = self.geometries.get(&record.element_id)?;
                (record.quantities.net_volume.is_some()
                    && record.quantities.net_surface_area.is_some())
                .then_some((index, &geometry.mesh))
            })
            .collect();
        let meshes: Vec<_> = candidates.iter().map(|(_, mesh)| *mesh).collect();
        if let Some(values) = crate::gpu::mesh_quantities_batch(&meshes) {
            let mut accelerated = 0;
            for ((index, _), (volume, area)) in candidates.iter().zip(values) {
                let record = &mut batch.elements[*index];
                let q = &mut record.quantities;
                let (Some(old_volume), Some(old_area)) = (q.net_volume, q.net_surface_area) else {
                    continue;
                };
                if !crate::gpu::close_enough(volume, old_volume)
                    || !crate::gpu::close_enough(area, old_area)
                {
                    continue;
                }
                // Keep analytic opening/gross fields unless the unchanged gross
                // measurement already equals this same uncut mesh observation.
                if q.gross_volume == Some(old_volume) {
                    q.gross_volume = Some(volume);
                }
                if q.gross_surface_area == Some(old_area) {
                    q.gross_surface_area = Some(area);
                }
                q.net_volume = Some(volume);
                q.net_surface_area = Some(area);
                accelerated += 1;
            }
            batch.aggregate = crate::quantities::aggregate_quantities(
                batch.elements.iter().map(|record| &record.quantities),
            );
            crate::gpu::record_qto_batch_usage(
                accelerated,
                batch.elements.len().saturating_sub(accelerated),
            );
        } else {
            crate::gpu::record_qto_cpu_fallback(batch.elements.len());
        }
        batch
    }

    pub fn configure_gpu_compute(&self, luid: [u8; 8], local_headroom_bytes: u64) {
        crate::gpu::configure_gpu_compute(luid, local_headroom_bytes);
    }
    pub fn refresh_gpu_compute_budget(&self, sample: Option<([u8; 8], u64)>) {
        crate::gpu::refresh_gpu_compute_budget(sample);
    }
    pub fn gpu_observation(&self) -> serde_json::Value {
        crate::gpu::gpu_compute_observe()
    }

    pub fn quantities_grouped(&self, grouping: QuantityGrouping) -> Vec<QuantityGroup> {
        self.quantity_cache.grouped(grouping)
    }

    pub fn quantity_cache_metrics(&self) -> QuantityCacheMetrics {
        self.quantity_cache.metrics()
    }

    pub fn spatial_bounds_query(
        &self,
        query: &SpatialQuery,
    ) -> Result<SpatialQueryResult, GeometryObservationError> {
        self.spatial_index.bounds_query(query)
    }

    pub fn spatial_intersects_query(
        &self,
        query: &SpatialQuery,
    ) -> Result<SpatialQueryResult, GeometryObservationError> {
        self.spatial_index.intersects_query(query)
    }

    pub fn spatial_index_metrics(&self) -> SpatialIndexMetrics {
        self.spatial_index.metrics()
    }
    pub fn scene(&self) -> &Scene {
        &self.scene
    }
    pub fn projected_elements(&self) -> impl Iterator<Item = &ProjectedElement> {
        self.projected.values()
    }
    pub fn geometry_errors(&self) -> impl Iterator<Item = &GeometryError> {
        self.failures.values()
    }
    pub fn resolution_errors(&self) -> impl Iterator<Item = (ElementId, &str)> {
        self.resolution_failures
            .iter()
            .map(|(id, e)| (*id, e.as_str()))
    }
    pub fn element_status(&self, id: ElementId) -> Option<&ElementGeometryStatus> {
        self.statuses.get(&id)
    }
    pub fn record(&self, id: ElementId) -> Option<&DerivedGeometryRecord> {
        self.records.get(&id)
    }
    pub fn inspect(&self, id: ElementId) -> Option<&DerivedGeometryRecord> {
        self.record(id)
    }
    pub fn geometry_history_entry_count(&self) -> usize {
        self.history_record_count
    }
    /// Constant-time counts from the cache maps and maintained history total.
    /// mesh_count includes valid empty derived geometry, independently of visibility.
    pub fn observation_metrics(&self) -> GeometryObservationMetrics {
        let started = Instant::now();
        let mut metrics = GeometryObservationMetrics {
            record_count: self.records.len(),
            mesh_count: self.geometries.len(),
            projected_count: self.projected.len(),
            geometry_error_count: self.failures.len(),
            resolution_error_count: self.resolution_failures.len(),
            error_count: self
                .failures
                .len()
                .saturating_add(self.resolution_failures.len()),
            history_element_count: self.history.len(),
            history_record_count: self.history_record_count,
            scanned_history_records: 0,
            duration_us: 0,
            reason: "maintained_counts",
        };
        metrics.duration_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        metrics
    }
    pub fn dependencies(&self, id: ElementId) -> Option<GeometryDependencies> {
        let record = self.record(id)?;
        Some(GeometryDependencies {
            element_id: id,
            host_id: self.host_ids.get(&id).copied(),
            derived_from: record.derived_from.clone(),
            openings: record.openings.clone(),
        })
    }
    pub fn compare(
        &self,
        id: ElementId,
        before: u64,
        after: u64,
    ) -> Result<GeometryComparison, GeometryObservationError> {
        let history = self.history.get(&id).ok_or_else(|| {
            GeometryObservationError::new(
                "element_not_found",
                Some(id),
                "no geometry history for this element",
            )
        })?;
        let lookup = |revision| {
            history.get(&revision).ok_or_else(|| {
                GeometryObservationError::new(
                    "geometry_revision_not_found",
                    Some(id),
                    format!("geometry revision {revision} is unavailable"),
                )
            })
        };
        let a = lookup(before)?;
        let b = lookup(after)?;
        Ok(GeometryComparison {
            element_id: id,
            geometry_revision_before: before,
            geometry_revision_after: after,
            representation_quality: b.representation_quality,
            bounds_changed: a.bounds != b.bounds,
            volume_delta: a.volume.zip(b.volume).map(|(a, b)| b - a),
            surface_area_delta: a.surface_area.zip(b.surface_area).map(|(a, b)| b - a),
            triangle_count_delta: b.triangle_count as i64 - a.triangle_count as i64,
            openings_changed: a.openings != b.openings
                || a.opening_fingerprint != b.opening_fingerprint,
            openings_before: a.openings.clone(),
            openings_after: b.openings.clone(),
            gross_quantity_delta: volume_surface_delta(
                a.quantities.as_ref(),
                b.quantities.as_ref(),
                true,
            ),
            net_quantity_delta: volume_surface_delta(
                a.quantities.as_ref(),
                b.quantities.as_ref(),
                false,
            ),
            opening_delta: opening_quantity_delta(a.quantities.as_ref(), b.quantities.as_ref()),
            perimeter_delta: quantity_field_delta(
                a.quantities.as_ref(),
                b.quantities.as_ref(),
                |quantities| quantities.perimeter,
            ),
            length_delta: quantity_field_delta(
                a.quantities.as_ref(),
                b.quantities.as_ref(),
                |quantities| quantities.length,
            ),
            source_dependency_changes: source_dependency_changes(&a.derived_from, &b.derived_from),
            valid_before: a.valid,
            valid_after: b.valid,
        })
    }
    pub fn section(
        &self,
        ids: &[ElementId],
        plane: SectionPlane,
    ) -> Result<GeometrySection, GeometryObservationError> {
        let plane = crate::section::validate_plane(plane)?;
        let ids: Vec<_> = ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut loops = Vec::new();
        for id in &ids {
            let geometry = self.geometries.get(id).ok_or_else(|| {
                GeometryObservationError::new(
                    "geometry_unavailable",
                    Some(*id),
                    "element has no valid current geometry",
                )
            })?;
            loops.extend(
                self.backend
                    .section(geometry, plane, self.options.linear_tolerance_m)?,
            );
        }
        Ok(GeometrySection {
            plane,
            element_ids: ids,
            loops,
            representation_quality: RepresentationQuality::ApproximateMesh,
        })
    }
    pub fn rebuild_into(&mut self, document: &Document, target: &mut Scene) -> RebuildReport {
        let report = self.rebuild(document);
        if report.document_invalidated {
            for id in target.object_ids().collect::<Vec<_>>() {
                target.remove(id);
            }
        } else {
            for id in &report.removed_element_ids {
                target.remove(object_id_for(*id));
            }
        }
        let mut ids: BTreeSet<_> = report
            .rebuilt_element_ids
            .iter()
            .chain(&report.display_changed_element_ids)
            .map(|id| object_id_for(*id))
            .collect();
        ids.extend(
            self.scene
                .object_ids()
                .filter(|id| target.get(*id).is_none()),
        );
        for id in ids {
            let Some(source) = self.scene.get(id) else {
                target.remove(id);
                continue;
            };
            let mesh = Mesh::new(source.mesh.triangles().to_vec());
            if let Some(existing) = target.get_mut(id) {
                existing.mesh = mesh;
                existing.name = source.name.clone();
                existing.color = source.color;
            } else {
                let mut object = SceneObject::new(id, source.name.clone(), mesh);
                object.color = source.color;
                object.exact_geometry = source.exact_geometry.clone();
                object.transform = source.transform;
                object.visible = source.visible;
                object.selected = source.selected;
                object.highlighted = source.highlighted;
                object.render_texture = source.render_texture.clone();
                target.insert(object);
            }
        }
        report
    }
    pub fn into_projection(self) -> SceneProjection {
        SceneProjection {
            scene: self.scene,
            projected: self.projected.into_values().collect(),
            failures: self.failures.into_values().collect(),
        }
    }
}

fn quantity_field_delta(
    before: Option<&GeometryQuantities>,
    after: Option<&GeometryQuantities>,
    field: impl Fn(&GeometryQuantities) -> Option<f64>,
) -> Option<f64> {
    field(before?)
        .zip(field(after?))
        .map(|(before, after)| after - before)
}

fn volume_surface_delta(
    before: Option<&GeometryQuantities>,
    after: Option<&GeometryQuantities>,
    gross: bool,
) -> Option<VolumeSurfaceDelta> {
    let before = before?;
    let after = after?;
    let (before_volume, before_area, after_volume, after_area) = if gross {
        (
            before.gross_volume,
            before.gross_surface_area,
            after.gross_volume,
            after.gross_surface_area,
        )
    } else {
        (
            before.net_volume,
            before.net_surface_area,
            after.net_volume,
            after.net_surface_area,
        )
    };
    Some(VolumeSurfaceDelta {
        volume: before_volume
            .zip(after_volume)
            .map(|(before, after)| after - before),
        surface_area: before_area
            .zip(after_area)
            .map(|(before, after)| after - before),
    })
}

fn opening_quantity_delta(
    before: Option<&GeometryQuantities>,
    after: Option<&GeometryQuantities>,
) -> Option<OpeningQuantityDelta> {
    let before = before?;
    let after = after?;
    Some(OpeningQuantityDelta {
        area: before
            .opening_area
            .zip(after.opening_area)
            .map(|(before, after)| after - before),
        volume: before
            .opening_volume
            .zip(after.opening_volume)
            .map(|(before, after)| after - before),
    })
}

fn source_dependency_changes(before: &[ElementId], after: &[ElementId]) -> SourceDependencyChanges {
    let before: BTreeSet<_> = before.iter().copied().collect();
    let after: BTreeSet<_> = after.iter().copied().collect();
    SourceDependencyChanges {
        added: after.difference(&before).copied().collect(),
        removed: before.difference(&after).copied().collect(),
    }
}
/// Host-plane inputs that can change a polygonal opening debug proxy.
#[derive(Serialize)]
#[serde(tag = "host_plane", rename_all = "snake_case")]
enum PolygonalHostPlaneFingerprint {
    Missing,
    Wall {
        direction_bits: Option<(u64, u64)>,
    },
    Floor,
    Roof {
        slope_x_bits: u64,
        slope_y_bits: u64,
    },
    Other,
}

fn polygonal_host_plane_fingerprint(host: Option<&ElementRecord>) -> PolygonalHostPlaneFingerprint {
    match host.map(|host| &host.kind) {
        Some(ElementKind::Wall(wall)) => {
            let dx = wall.end.x - wall.start.x;
            let dy = wall.end.y - wall.start.y;
            let length = dx.hypot(dy);
            let direction_bits = if length.is_finite() && length > 0.0 {
                Some(((dx / length).to_bits(), (dy / length).to_bits()))
            } else {
                None
            };
            PolygonalHostPlaneFingerprint::Wall { direction_bits }
        }
        Some(ElementKind::Floor(_)) => PolygonalHostPlaneFingerprint::Floor,
        Some(ElementKind::Roof(roof)) => PolygonalHostPlaneFingerprint::Roof {
            slope_x_bits: roof.slope_x.to_bits(),
            slope_y_bits: roof.slope_y.to_bits(),
        },
        Some(_) => PolygonalHostPlaneFingerprint::Other,
        None => PolygonalHostPlaneFingerprint::Missing,
    }
}

/// Deterministic noncryptographic input fingerprint. Names, IDs, arbitrary
/// properties and type identities do not participate once geometry is resolved.
/// Polygonal debug proxies additionally depend on their host plane; rectangular
/// opening fingerprints retain the legacy tuple for cache compatibility.
/// Space boundary is a geometry input; semantic metadata is not.
fn geometry_fingerprint(
    input: &GeometryInput,
    contextual_host: Option<&ElementRecord>,
    options: GeometryOptions,
    backend: &str,
) -> String {
    let opening_fingerprint = opening_fingerprint(input);
    let bytes = if matches!(&input.element.kind, ElementKind::PolygonalOpening(_)) {
        serde_json::to_vec(&(
            backend,
            &input.element.kind,
            opening_fingerprint,
            polygonal_host_plane_fingerprint(contextual_host),
            options.linear_tolerance_m,
            options.max_vertices_per_element,
        ))
    } else if let ElementKind::Space(space) = &input.element.kind {
        serde_json::to_vec(&(
            backend,
            ("space", &space.boundary),
            opening_fingerprint,
            options.linear_tolerance_m,
            options.max_vertices_per_element,
        ))
    } else {
        serde_json::to_vec(&(
            backend,
            &input.element.kind,
            opening_fingerprint,
            options.linear_tolerance_m,
            options.max_vertices_per_element,
        ))
    }
    .expect("geometry fingerprint serialization");
    fingerprint_bytes(&bytes)
}
fn fingerprint_bytes(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}
fn opening_fingerprint(input: &GeometryInput) -> String {
    let mut opening_kinds: Vec<_> = input
        .openings
        .iter()
        .filter(|o| {
            matches!(
                o.kind,
                ElementKind::Opening(_) | ElementKind::PolygonalOpening(_)
            )
        })
        .map(|o| serde_json::to_string(&o.kind).expect("finite validated element kind"))
        .collect();
    opening_kinds.sort();
    opening_kinds.dedup();
    fingerprint_bytes(
        &serde_json::to_vec(&opening_kinds).expect("opening fingerprint serialization"),
    )
}
