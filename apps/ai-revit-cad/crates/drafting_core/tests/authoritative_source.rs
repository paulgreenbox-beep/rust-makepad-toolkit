//! The cache adapter has no transaction owner and never snapshots while navigating.
use drafting_core::*;
use std::{cell::Cell, sync::Arc};

const DRAWING: DrawingId = DrawingId::new(1);
const EPOCH: u64 = 9;

/// Minimal test authority, deliberately retaining no LabDocument or cache log.
struct ProjectSource {
    document_id: String,
    runtime_id: String,
    revision: u64,
    drawing: DraftDrawing,
    committed_from: u64,
    committed_changes: Vec<DraftChange>,
    drawing_reads: Cell<usize>,
    snapshot_reads: Cell<usize>,
    journal_reads: Cell<usize>,
    reject_journal: bool,
}

impl ProjectSource {
    fn seeded() -> Self {
        let mut lab = LabDocument::new();
        lab.apply(CommandEnvelope {
            command_id: "seed".into(),
            expected_revision: 0,
            operations: (0..2).map(|i| DraftOperation::Create {
                drawing_id: DRAWING,
                layer_id: LayerId::new(1),
                geometry: line(i as f64 * 2.0),
                style: DraftStyle::default(),
            }).collect(),
        }).unwrap();
        Self {
            document_id: lab.document_id().to_owned(),
            runtime_id: "authoritative-test-runtime".into(),
            revision: lab.revision(),
            drawing: lab.drawings()[&DRAWING].clone(),
            committed_from: 0,
            committed_changes: Vec::new(),
            drawing_reads: Cell::new(0),
            snapshot_reads: Cell::new(0),
            journal_reads: Cell::new(0),
            reject_journal: false,
        }
    }

    fn commit(&mut self, changes: Vec<DraftChange>) {
        self.committed_from = self.revision;
        self.revision += 1;
        self.committed_changes = changes;
    }

    fn replace_entity(&mut self, mut after: DraftEntity) {
        let before = self.drawing.entities[&after.id].clone();
        after.revision = self.revision + 1;
        self.drawing.entities.insert(after.id, after.clone());
        self.drawing.revision = self.revision + 1;
        self.commit(vec![DraftChange::Entity {
            drawing_id: DRAWING,
            id: after.id,
            before: Some(before),
            after: Some(after),
        }]);
    }
}

impl DrawingSource for ProjectSource {
    fn drawing(&self, id: DrawingId) -> Option<&DraftDrawing> {
        self.drawing_reads.set(self.drawing_reads.get() + 1);
        (id == self.drawing.id).then_some(&self.drawing)
    }

    fn cache_source_key(&self, id: DrawingId, epoch: u64) -> DraftResult<CacheSourceKey> {
        if id != self.drawing.id {
            return Err(DraftError::new("drawing_not_found", "absent drawing"));
        }
        Ok(CacheSourceKey {
            document_id: self.document_id.clone(),
            runtime_instance_id: self.runtime_id.clone(),
            document_epoch: epoch,
            document_revision: self.revision,
            drawing_id: id,
            drawing_revision: self.drawing.revision,
        })
    }

    fn snapshot(&self, id: DrawingId) -> DraftResult<DraftSnapshot> {
        self.snapshot_reads.set(self.snapshot_reads.get() + 1);
        if id != self.drawing.id {
            return Err(DraftError::new("drawing_not_found", "absent drawing"));
        }
        Ok(DraftSnapshot {
            document_id: self.document_id.clone(),
            document_revision: self.revision,
            drawing_id: id,
            name: self.drawing.name.clone(),
            revision: self.drawing.revision,
            plane: self.drawing.plane.clone(),
            entities: self.drawing.entities.clone(),
            layers: self.drawing.layers.clone(),
            import_sources: self.drawing.import_sources.clone(),
            source_fingerprint: "test-export-snapshot".into(),
        })
    }

    fn confirmed_drafting_changes(&self, from: u64) -> DraftResult<Vec<DraftChange>> {
        self.journal_reads.set(self.journal_reads.get() + 1);
        if self.reject_journal || from != self.committed_from || from.checked_add(1) != Some(self.revision) {
            return Err(DraftError::new("cache_history_mismatch", "authority refused the journal"));
        }
        Ok(self.committed_changes.clone())
    }
}

fn line(x: f64) -> EntityGeometry {
    EntityGeometry::Line { start: Point2::new(x, 0.0), end: Point2::new(x + 1.0, 0.0) }
}

fn new_cache(source: &(impl DrawingSource + ?Sized)) -> IncrementalDraftingCache {
    let mut cache = IncrementalDraftingCache::new(DraftingCacheLimits::default()).unwrap();
    cache.reset(source, DRAWING, EPOCH).unwrap();
    cache
}

fn no_manual_geometry_work(before: &CacheMetrics, after: &CacheMetrics) {
    assert_eq!(before.source_entity_visits, after.source_entity_visits);
    assert_eq!(before.geometry_clones, after.geometry_clones);
    assert_eq!(before.geometry_invalidations, after.geometry_invalidations);
    assert_eq!(before.index_updates, after.index_updates);
    assert_eq!(before.digest_leaf_updates, after.digest_leaf_updates);
    assert_eq!(before.digest_node_hashes, after.digest_node_hashes);
    assert_eq!(before.tessellation_misses, after.tessellation_misses);
    assert_eq!(before.projected_entities, after.projected_entities);
}

#[test]
fn bim_only_revision_rebinds_source_without_manual_geometry_work() {
    let mut source = ProjectSource::seeded();
    let mut cache = new_cache(&source);
    let camera = Camera2D::new(800.0, 600.0);
    let options = DisplayOptions::default();
    let first = cache.prepare(&source, EPOCH, &camera, &options).unwrap();
    let entity = cache.entity(&source, EPOCH, DraftEntityId::new(1)).unwrap();
    let before = cache.performance();
    source.commit(Vec::new());
    let update = cache.sync_from_source(&source, EPOCH).unwrap();
    assert_eq!(update.changed_entities, 0);
    assert_eq!(update.geometry_invalidations, 0);
    let rebound = cache.prepare(&source, EPOCH, &camera, &options).unwrap();
    assert!(Arc::ptr_eq(&first.base, &rebound.base));
    assert!(Arc::ptr_eq(&entity, &cache.entity(&source, EPOCH, entity.id).unwrap()));
    assert_eq!(first.generations, rebound.generations);
    assert_eq!(rebound.source.document_revision, first.source.document_revision + 1);
    assert_eq!(rebound.source.drawing_revision, first.source.drawing_revision);
    assert_ne!(rebound.source_sha256, first.source_sha256);
    no_manual_geometry_work(&before, &cache.performance());
    assert_eq!(source.snapshot_reads.get(), 0);
    assert_eq!(source.journal_reads.get(), 1);
    assert!(cache.sync_from_source(&source, EPOCH).unwrap().replayed);
    assert_eq!(source.journal_reads.get(), 1);
}

#[test]
fn generic_navigation_never_snapshots_or_scans_the_authoritative_drawing() {
    let authority = ProjectSource::seeded();
    let source: &dyn DrawingSource = &authority;
    let mut cache = new_cache(source);
    let mut camera = Camera2D::new(800.0, 600.0);
    let options = DisplayOptions::default();
    let _ = cache.prepare(source, EPOCH, &camera, &options).unwrap();
    let reads_after_reset = authority.drawing_reads.get();
    camera.pan_delta(Point2::new(15.0, -5.0)).unwrap();
    let _ = cache.prepare(source, EPOCH, &camera, &options).unwrap();
    let _ = cache.bounds(source, EPOCH, true).unwrap();
    let _ = cache.metadata(source, EPOCH).unwrap();
    assert_eq!(cache.source_key(source, EPOCH).unwrap().document_revision, authority.revision);
    let _ = cache.entity(source, EPOCH, DraftEntityId::new(1)).unwrap();
    let _ = cache.pick(source, EPOCH, &camera, Point2::new(400.0, 300.0), 4.0).unwrap();
    // The trait-object path also proves all interactive entry points accept
    // borrowed immutable sources without depending on LabDocument's storage.
    assert_eq!(authority.drawing_reads.get(), reads_after_reset);
    assert_eq!(authority.snapshot_reads.get(), 0);
    assert_eq!(authority.journal_reads.get(), 0);
    assert_eq!(authority.revision, 1);
}

#[test]
fn metadata_revision_updates_name_and_keeps_exact_frame_base() {
    let mut source = ProjectSource::seeded();
    let mut cache = new_cache(&source);
    let camera = Camera2D::new(800.0, 600.0);
    let options = DisplayOptions::default();
    let first = cache.prepare(&source, EPOCH, &camera, &options).unwrap();
    let before = cache.performance();
    source.drawing.name = "Renamed in project history".into();
    source.drawing.revision = source.revision + 1;
    source.commit(Vec::new());
    cache.sync_from_source(&source, EPOCH).unwrap();
    assert_eq!(cache.metadata(&source, EPOCH).unwrap().name, source.drawing.name);
    let renamed = cache.prepare(&source, EPOCH, &camera, &options).unwrap();
    assert!(Arc::ptr_eq(&first.base, &renamed.base));
    no_manual_geometry_work(&before, &cache.performance());
    assert_eq!(source.snapshot_reads.get(), 0);
}

#[test]
fn source_journal_edit_undo_redo_invalidate_only_the_affected_entity() {
    let mut source = ProjectSource::seeded();
    let mut cache = new_cache(&source);
    let original = source.drawing.entities[&DraftEntityId::new(1)].clone();
    let untouched = cache.entity(&source, EPOCH, DraftEntityId::new(2)).unwrap();
    let old_geometry = cache.entity(&source, EPOCH, original.id).unwrap().geometry.clone();
    let before = cache.performance();
    let mut edited = original.clone();
    edited.geometry = line(5.0);
    source.replace_entity(edited.clone());
    let summary = cache.sync_from_source(&source, EPOCH).unwrap();
    assert_eq!(summary.changed_entities, 1);
    assert_eq!(summary.geometry_invalidations, 1);
    assert_eq!(cache.performance().source_entity_visits, before.source_entity_visits + 1);
    assert_eq!(cache.performance().geometry_clones, before.geometry_clones + 1);
    assert!(Arc::ptr_eq(&untouched, &cache.entity(&source, EPOCH, untouched.id).unwrap()));
    assert!(!Arc::ptr_eq(&old_geometry, &cache.entity(&source, EPOCH, original.id).unwrap().geometry));
    source.replace_entity(original.clone());
    cache.sync_from_source(&source, EPOCH).unwrap();
    assert_eq!(*cache.entity(&source, EPOCH, original.id).unwrap().geometry, original.geometry);
    source.replace_entity(edited.clone());
    cache.sync_from_source(&source, EPOCH).unwrap();
    assert_eq!(*cache.entity(&source, EPOCH, original.id).unwrap().geometry, edited.geometry);
    let rebuilt = new_cache(&source);
    assert_eq!(cache.metadata(&source, EPOCH).unwrap().source_sha256,
        rebuilt.metadata(&source, EPOCH).unwrap().source_sha256);
    assert_eq!(source.snapshot_reads.get(), 0);
}

#[test]
fn source_gap_runtime_epoch_and_refused_journal_require_explicit_recovery() {
    let mut source = ProjectSource::seeded();
    let mut cache = new_cache(&source);
    let before = cache.performance();
    source.commit(Vec::new());
    source.reject_journal = true;
    assert_eq!(cache.sync_from_source(&source, EPOCH).unwrap_err().code, "cache_history_mismatch");
    no_manual_geometry_work(&before, &cache.performance());
    assert_eq!(cache.metadata(&source, EPOCH).unwrap_err().code, "stale_cache_source");
    source.reject_journal = false;
    cache.sync_from_source(&source, EPOCH).unwrap();
    source.commit(Vec::new());
    source.commit(Vec::new());
    assert_eq!(cache.sync_from_source(&source, EPOCH).unwrap_err().code, "cache_revision_gap");
    cache.reset(&source, DRAWING, EPOCH).unwrap();
    assert_eq!(cache.sync_from_source(&source, EPOCH + 1).unwrap_err().code, "stale_cache_source");
    source.runtime_id = "fresh-load".into();
    assert_eq!(cache.sync_from_source(&source, EPOCH).unwrap_err().code, "stale_cache_source");
    cache.reset(&source, DRAWING, EPOCH).unwrap();
    assert!(cache.metadata(&source, EPOCH).is_ok());
}

#[test]
fn changed_plane_requires_explicit_reset_without_hidden_geometry_work() {
    let mut source = ProjectSource::seeded();
    let mut cache = new_cache(&source);
    let before = cache.performance();
    source.drawing.plane.origin.z = 2.0;
    source.drawing.revision = source.revision + 1;
    source.commit(Vec::new());
    assert_eq!(cache.sync_from_source(&source, EPOCH).unwrap_err().code, "stale_cache_source");
    assert_eq!(source.journal_reads.get(), 0);
    no_manual_geometry_work(&before, &cache.performance());
    cache.reset(&source, DRAWING, EPOCH).unwrap();
    assert_eq!(cache.metadata(&source, EPOCH).unwrap().plane.origin.z, 2.0);
}

#[test]
fn generic_export_is_the_only_snapshot_materialization_boundary() {
    let source = ProjectSource::seeded();
    let mut cache = new_cache(&source);
    let camera = Camera2D::new(800.0, 600.0);
    let frame = cache.prepare(&source, EPOCH, &camera, &DisplayOptions::default()).unwrap();
    assert_eq!(source.snapshot_reads.get(), 0);
    let export = cache.materialize_export(&source, EPOCH, &frame).unwrap();
    assert_eq!(source.snapshot_reads.get(), 1);
    assert_eq!(export.snapshot.entities.len(), 2);
    assert_eq!(export.snapshot.document_revision, source.revision);
    assert_eq!(cache.performance().export_materializations, 1);
}

#[test]
fn lab_source_bridge_preserves_confirmed_undo_and_legacy_receipt_guards() {
    let mut lab = LabDocument::new();
    let mut cache = new_cache(&lab);
    let receipt = lab.apply(CommandEnvelope {
        command_id: "create".into(), expected_revision: 0,
        operations: vec![DraftOperation::Create {
            drawing_id: DRAWING, layer_id: LayerId::new(1), geometry: line(0.0),
            style: DraftStyle::default(),
        }],
    }).unwrap();
    let mut forged = receipt.clone();
    forged.created_ids.clear();
    assert_eq!(cache.sync_after_commit(&lab, &forged, EPOCH).unwrap_err().code, "cache_receipt_mismatch");
    cache.sync_from_source(&lab, EPOCH).unwrap();
    let _ = lab.undo("undo", 1).unwrap();
    let inverse = DrawingSource::confirmed_drafting_changes(&lab, 1).unwrap();
    assert!(matches!(&inverse[0], DraftChange::Entity { before: Some(_), after: None, .. }));
    cache.sync_from_source(&lab, EPOCH).unwrap();
    assert_eq!(cache.metadata(&lab, EPOCH).unwrap().entity_count, 0);
    let redo = lab.redo("redo", 2).unwrap();
    cache.sync_after_commit(&lab, &redo, EPOCH).unwrap();
    assert_eq!(cache.metadata(&lab, EPOCH).unwrap().entity_count, 1);
}
