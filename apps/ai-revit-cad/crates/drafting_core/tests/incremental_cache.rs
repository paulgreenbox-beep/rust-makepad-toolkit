//! External regressions for the maintained drafting cache contract.
//! These tests exercise only public API behavior; they do not depend on cache internals.
use drafting_core::*;
use std::sync::Arc;

const DRAWING: DrawingId = DrawingId::new(1);
const EPOCH: u64 = 7;

fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> EntityGeometry {
    EntityGeometry::Line {
        start: Point2::new(x0, y0),
        end: Point2::new(x1, y1),
    }
}

fn apply(
    document: &mut LabDocument,
    command_id: &str,
    operations: Vec<DraftOperation>,
) -> CommandReceipt {
    document
        .apply(CommandEnvelope {
            command_id: command_id.into(),
            expected_revision: document.revision(),
            operations,
        })
        .unwrap()
}

fn create_lines(document: &mut LabDocument, count: usize) -> Vec<DraftEntityId> {
    let operations = (0..count)
        .map(|index| {
            let x = index as f64 * 2.0;
            DraftOperation::Create {
                drawing_id: DRAWING,
                layer_id: LayerId::new(1),
                geometry: line(x, 0.0, x + 1.0, 0.0),
                style: DraftStyle::default(),
            }
        })
        .collect();
    apply(document, "seed-lines", operations).created_ids
}

fn create_layer(document: &mut LabDocument, name: &str, visible: bool) -> LayerId {
    let receipt = apply(
        document,
        &format!("create-layer-{name}"),
        vec![DraftOperation::CreateLayer {
            drawing_id: DRAWING,
            name: name.into(),
            visible,
            locked: false,
            style: DraftStyle::default(),
        }],
    );
    receipt.changed_layer_ids[0]
}

fn new_cache(document: &LabDocument) -> IncrementalDraftingCache {
    let mut cache = IncrementalDraftingCache::new(DraftingCacheLimits::default()).unwrap();
    cache.reset(document, DRAWING, EPOCH).unwrap();
    cache
}

fn camera() -> Camera2D {
    Camera2D::new(1000.0, 700.0)
}

fn batch_geometry(frame: &CachedFrame, id: DraftEntityId) -> Arc<TessellatedGeometry> {
    frame
        .base
        .batches
        .iter()
        .find(|batch| batch.owner_entity_id == id)
        .unwrap_or_else(|| panic!("frame has no visible geometry batch for entity {id}"))
        .geometry
        .clone()
}

fn assert_no_source_geometry_work(before: &CacheMetrics, after: &CacheMetrics) {
    assert_eq!(after.source_entity_visits, before.source_entity_visits);
    assert_eq!(after.geometry_clones, before.geometry_clones);
    assert_eq!(after.geometry_invalidations, before.geometry_invalidations);
    assert_eq!(after.digest_leaf_updates, before.digest_leaf_updates);
    assert_eq!(after.digest_node_hashes, before.digest_node_hashes);
    assert_eq!(after.index_updates, before.index_updates);
    assert_eq!(after.tessellation_misses, before.tessellation_misses);
    assert_eq!(after.projected_entities, before.projected_entities);
}

fn assert_source_hash_pair(source_fingerprint: &str, source_sha256: &str) {
    let fingerprint_digest = source_fingerprint
        .strip_prefix("sha256:")
        .expect("source fingerprint must identify its SHA-256 format");
    assert_eq!(fingerprint_digest, source_sha256);
    assert_eq!(source_sha256.len(), 64);
    assert!(source_sha256
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
}

#[test]
fn repeated_prepare_reuses_frame_and_overlay_edits_retain_its_exact_base() {
    let mut document = LabDocument::new();
    let id = create_lines(&mut document, 1)[0];
    let mut cache = new_cache(&document);
    let camera = camera();
    let mut options = DisplayOptions::default();

    let first = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    assert_source_hash_pair(&first.source_fingerprint, &first.source_sha256);
    let warm = cache.performance();
    let repeated = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    assert!(Arc::ptr_eq(&first, &repeated));
    assert!(Arc::ptr_eq(&first.base, &repeated.base));
    assert_eq!(first.token, repeated.token);

    let before_overlay = cache.performance();
    options.selected_ids.push(id);
    let selected = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    let selected_metrics = cache.performance();
    assert_ne!(first.token, selected.token);
    assert!(Arc::ptr_eq(&first.base, &selected.base));
    assert_eq!(selected.overlay.selected_ids.as_ref(), &[id]);
    assert!(selected.generations.overlay > first.generations.overlay);
    assert_no_source_geometry_work(&before_overlay, &selected_metrics);
    assert_eq!(selected_metrics.index_queries, before_overlay.index_queries);
    assert_eq!(selected.source_fingerprint, first.source_fingerprint);
    assert_eq!(selected.source_sha256, first.source_sha256);

    options.hover_id = Some(id);
    let hovered = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    let hovered_metrics = cache.performance();
    assert_ne!(selected.token, hovered.token);
    assert!(Arc::ptr_eq(&first.base, &hovered.base));
    assert_eq!(hovered.overlay.hover_id, Some(id));
    assert!(hovered.generations.overlay > selected.generations.overlay);
    assert_no_source_geometry_work(&selected_metrics, &hovered_metrics);
    assert_eq!(hovered_metrics.index_queries, selected_metrics.index_queries);
    assert_eq!(warm.tessellation_misses, before_overlay.tessellation_misses);
}

#[test]
fn same_scale_pan_reuses_each_entity_mesh_without_geometry_or_index_updates() {
    let mut document = LabDocument::new();
    let id = create_lines(&mut document, 1)[0];
    let mut cache = new_cache(&document);
    let mut camera = camera();
    let options = DisplayOptions::default();
    let before = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    let before_mesh = batch_geometry(&before, id);
    let before_metrics = cache.performance();

    camera.pan_pixels(12.0, -5.0).unwrap();
    let after = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    let after_metrics = cache.performance();

    assert!(Arc::ptr_eq(&before_mesh, &batch_geometry(&after, id)));
    assert_ne!(before.token, after.token);
    assert_eq!(before.source_fingerprint, after.source_fingerprint);
    assert_eq!(before.source_sha256, after.source_sha256);
    assert!(after.generations.camera > before.generations.camera);
    assert!(after.generations.resources > before.generations.resources);
    assert_eq!(after_metrics.tessellation_misses, before_metrics.tessellation_misses);
    assert_eq!(after_metrics.geometry_clones, before_metrics.geometry_clones);
    assert_eq!(after_metrics.digest_leaf_updates, before_metrics.digest_leaf_updates);
    assert_eq!(after_metrics.digest_node_hashes, before_metrics.digest_node_hashes);
    assert_eq!(after_metrics.index_updates, before_metrics.index_updates);
}

#[test]
fn one_entity_edit_invalidates_only_its_geometry_and_keeps_other_arcs() {
    let mut document = LabDocument::new();
    let ids = create_lines(&mut document, 3);
    let mut cache = new_cache(&document);
    let camera = camera();
    let options = DisplayOptions::default();
    let before = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    let old_records: Vec<_> = ids
        .iter()
        .map(|id| cache.entity(&document, EPOCH, *id).unwrap())
        .collect();
    let old_meshes: Vec<_> = ids
        .iter()
        .map(|id| batch_geometry(&before, *id))
        .collect();
    let before_metrics = cache.performance();

    let replacement = line(2.25, 0.0, 3.25, 0.0);
    let receipt = apply(
        &mut document,
        "edit-one-line",
        vec![DraftOperation::Edit {
            drawing_id: DRAWING,
            id: ids[1],
            geometry: replacement.clone(),
        }],
    );
    let summary = cache
        .sync_after_commit(&document, &receipt, EPOCH)
        .unwrap();
    assert_eq!(summary.changed_entities, 1);
    assert_eq!(summary.geometry_invalidations, 1);
    let after_sync = cache.performance();
    assert_eq!(after_sync.geometry_clones, before_metrics.geometry_clones + 1);
    assert_eq!(
        after_sync.geometry_invalidations,
        before_metrics.geometry_invalidations + 1
    );

    let after = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    let after_metrics = cache.performance();
    assert_eq!(after_metrics.tessellation_misses, after_sync.tessellation_misses + 1);
    for index in [0, 2] {
        let new_record = cache.entity(&document, EPOCH, ids[index]).unwrap();
        assert!(Arc::ptr_eq(&old_records[index].geometry, &new_record.geometry));
        assert!(Arc::ptr_eq(&old_meshes[index], &batch_geometry(&after, ids[index])));
    }
    let edited = cache.entity(&document, EPOCH, ids[1]).unwrap();
    assert_eq!(*edited.geometry, replacement);
    assert!(!Arc::ptr_eq(&old_records[1].geometry, &edited.geometry));
    assert!(!Arc::ptr_eq(&old_meshes[1], &batch_geometry(&after, ids[1])));
}

#[test]
fn locking_a_layer_updates_metadata_pick_and_hash_but_retains_frame_base() {
    let mut document = LabDocument::new();
    let id = create_lines(&mut document, 1)[0];
    let mut cache = new_cache(&document);
    let camera = camera();
    let options = DisplayOptions::default();
    let before_frame = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    let before_record = cache.entity(&document, EPOCH, id).unwrap();
    let before_hash = cache
        .metadata(&document, EPOCH)
        .unwrap()
        .source_sha256
        .clone();
    let point = camera.world_to_screen(Point2::new(0.5, 0.0));
    assert!(!cache
        .pick(&document, EPOCH, &camera, point, 4.0)
        .unwrap()[0]
        .locked);

    let receipt = apply(
        &mut document,
        "lock-default-layer",
        vec![DraftOperation::SetLayer {
            drawing_id: DRAWING,
            layer_id: LayerId::new(1),
            visible: None,
            locked: Some(true),
            style: None,
        }],
    );
    cache
        .sync_after_commit(&document, &receipt, EPOCH)
        .unwrap();
    let after_frame = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    let after_record = cache.entity(&document, EPOCH, id).unwrap();
    let metadata = cache.metadata(&document, EPOCH).unwrap();

    assert!(Arc::ptr_eq(&before_frame.base, &after_frame.base));
    assert!(Arc::ptr_eq(&before_record.geometry, &after_record.geometry));
    assert_ne!(before_hash, metadata.source_sha256);
    assert!(metadata
        .layers
        .iter()
        .find(|layer| layer.id == LayerId::new(1))
        .unwrap()
        .locked);
    let hits = cache.pick(&document, EPOCH, &camera, point, 4.0).unwrap();
    assert_eq!(hits[0].entity_id, id);
    assert!(hits[0].locked);

    let export = cache
        .materialize_export(&document, EPOCH, &after_frame)
        .unwrap();
    assert_eq!(export.display.source_revision, export.snapshot.revision);
    assert_eq!(
        export.display.source_revision,
        document.snapshot(DRAWING).unwrap().revision
    );
}

#[test]
fn visibility_changes_culling_and_fit_bounds_without_cloning_geometry() {
    let mut document = LabDocument::new();
    let far_layer = create_layer(&mut document, "far", true);
    apply(
        &mut document,
        "seed-visible-layers",
        vec![
            DraftOperation::Create {
                drawing_id: DRAWING,
                layer_id: LayerId::new(1),
                geometry: line(0.0, 0.0, 1.0, 0.0),
                style: DraftStyle::default(),
            },
            DraftOperation::Create {
                drawing_id: DRAWING,
                layer_id: far_layer,
                geometry: line(5.0, 0.0, 6.0, 0.0),
                style: DraftStyle::default(),
            },
        ],
    );
    let mut cache = new_cache(&document);
    let mut camera = camera();
    let options = DisplayOptions::default();
    let before_visible = cache.bounds(&document, EPOCH, true).unwrap().unwrap();
    let before_all = cache.bounds(&document, EPOCH, false).unwrap().unwrap();
    camera.fit_bounds(before_visible, 24.0).unwrap();
    let before_frame = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    let before_metrics = cache.performance();
    assert_eq!(before_frame.base.visible_entity_count, 2);

    let receipt = apply(
        &mut document,
        "hide-far-layer",
        vec![DraftOperation::SetLayer {
            drawing_id: DRAWING,
            layer_id: far_layer,
            visible: Some(false),
            locked: None,
            style: None,
        }],
    );
    cache
        .sync_after_commit(&document, &receipt, EPOCH)
        .unwrap();
    let after_visible = cache.bounds(&document, EPOCH, true).unwrap().unwrap();
    let after_all = cache.bounds(&document, EPOCH, false).unwrap().unwrap();
    camera.fit_bounds(after_visible, 24.0).unwrap();
    let after_frame = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    let after_metrics = cache.performance();

    assert!(after_visible.max.x < 2.0, "hidden layer leaked into fit bounds");
    assert_eq!(before_all, after_all, "all-entity bounds must retain hidden geometry");
    assert_eq!(after_frame.base.visible_entity_count, 1);
    assert_eq!(after_metrics.geometry_clones, before_metrics.geometry_clones);
    assert_eq!(after_metrics.geometry_invalidations, before_metrics.geometry_invalidations);
    assert_eq!(after_metrics.index_updates, before_metrics.index_updates);
}

#[test]
fn runtime_reload_rejects_all_stale_cache_reads_until_explicit_reset() {
    let mut original = LabDocument::new();
    let id = create_lines(&mut original, 1)[0];
    let mut cache = new_cache(&original);
    let camera = camera();
    let options = DisplayOptions::default();
    let old_frame = cache
        .prepare(&original, EPOCH, &camera, &options)
        .unwrap();
    let reloaded = original.clone();
    assert_eq!(original.document_id(), reloaded.document_id());
    assert_eq!(original.revision(), reloaded.revision());
    assert_ne!(original.runtime_instance_id(), reloaded.runtime_instance_id());

    assert!(cache.metadata(&reloaded, EPOCH).is_err());
    assert!(cache.entity(&reloaded, EPOCH, id).is_err());
    assert!(cache.prepare(&reloaded, EPOCH, &camera, &options).is_err());
    assert!(cache
        .pick(&reloaded, EPOCH, &camera, Point2::new(500.0, 350.0), 5.0)
        .is_err());
    assert!(cache.bounds(&reloaded, EPOCH, true).is_err());
    assert!(cache
        .materialize_export(&reloaded, EPOCH, &old_frame)
        .is_err());

    cache.reset(&reloaded, DRAWING, EPOCH).unwrap();
    let current = cache
        .prepare(&reloaded, EPOCH, &camera, &options)
        .unwrap();
    assert_eq!(cache.metadata(&reloaded, EPOCH).unwrap().entity_count, 1);
    assert_eq!(*cache.entity(&reloaded, EPOCH, id).unwrap().geometry, line(0.0, 0.0, 1.0, 0.0));
    assert!(!cache
        .pick(&reloaded, EPOCH, &camera, Point2::new(524.0, 350.0), 5.0)
        .unwrap()
        .is_empty());
    assert!(cache.bounds(&reloaded, EPOCH, true).unwrap().is_some());
    assert!(cache
        .materialize_export(&reloaded, EPOCH, &current)
        .is_ok());
}

#[test]
fn skipped_commit_and_forged_receipt_cannot_publish_cache_state() {
    let mut document = LabDocument::new();
    let id = create_lines(&mut document, 1)[0];
    let mut cache = new_cache(&document);
    let camera = camera();
    let options = DisplayOptions::default();
    let old_frame = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();

    let receipt = apply(
        &mut document,
        "unconfirmed-edit",
        vec![DraftOperation::Edit {
            drawing_id: DRAWING,
            id,
            geometry: line(-1.0, 0.0, 1.0, 0.0),
        }],
    );
    assert!(cache.metadata(&document, EPOCH).is_err());
    assert!(cache.prepare(&document, EPOCH, &camera, &options).is_err());

    let commits_before_forgery = cache.performance().commits;
    let mut forged = receipt.clone();
    forged.updated_ids.clear();
    assert!(cache
        .sync_after_commit(&document, &forged, EPOCH)
        .is_err());
    assert_eq!(cache.performance().commits, commits_before_forgery);
    assert!(cache
        .materialize_export(&document, EPOCH, &old_frame)
        .is_err());

    cache
        .sync_after_commit(&document, &receipt, EPOCH)
        .unwrap();
    assert!(cache.metadata(&document, EPOCH).is_ok());
}

#[test]
fn transient_batch_undo_redo_and_full_reset_converge_on_the_same_digest() {
    let mut document = LabDocument::new();
    let stable_id = create_lines(&mut document, 1)[0];
    let original_geometry = line(0.0, 0.0, 1.0, 0.0);
    let mut cache = new_cache(&document);
    let camera = camera();
    let options = DisplayOptions::default();
    cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();

    let transient_id = DraftEntityId::new(2);
    let edited_geometry = line(-1.0, 0.0, 2.0, 0.0);
    let receipt = apply(
        &mut document,
        "mixed-net-transaction",
        vec![
            DraftOperation::Edit {
                drawing_id: DRAWING,
                id: stable_id,
                geometry: edited_geometry.clone(),
            },
            DraftOperation::Create {
                drawing_id: DRAWING,
                layer_id: LayerId::new(1),
                geometry: line(3.0, 0.0, 4.0, 0.0),
                style: DraftStyle::default(),
            },
            DraftOperation::Move {
                drawing_id: DRAWING,
                ids: vec![transient_id],
                delta: Point2::new(0.5, 0.0),
            },
            DraftOperation::Delete {
                drawing_id: DRAWING,
                ids: vec![transient_id],
            },
        ],
    );
    assert_eq!(receipt.updated_ids, vec![stable_id]);
    assert!(receipt.created_ids.is_empty());
    assert!(receipt.deleted_ids.is_empty());
    let summary = cache
        .sync_after_commit(&document, &receipt, EPOCH)
        .unwrap();
    assert_eq!(summary.changed_entities, 1);
    assert_eq!(*cache.entity(&document, EPOCH, stable_id).unwrap().geometry, edited_geometry);
    assert!(cache.entity(&document, EPOCH, transient_id).is_err());
    cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();

    let undo = document.undo("undo-mixed", document.revision()).unwrap();
    assert_eq!(undo.updated_ids, vec![stable_id]);
    cache.sync_after_commit(&document, &undo, EPOCH).unwrap();
    assert_eq!(*cache.entity(&document, EPOCH, stable_id).unwrap().geometry, original_geometry);
    cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();

    let redo = document.redo("redo-mixed", document.revision()).unwrap();
    assert_eq!(redo.updated_ids, vec![stable_id]);
    cache.sync_after_commit(&document, &redo, EPOCH).unwrap();
    assert_eq!(*cache.entity(&document, EPOCH, stable_id).unwrap().geometry, edited_geometry);
    let current = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    let incremental = cache.metadata(&document, EPOCH).unwrap().clone();

    let mut rebuilt = new_cache(&document);
    let rebuilt_metadata = rebuilt.metadata(&document, EPOCH).unwrap();
    assert_eq!(incremental.source_sha256, rebuilt_metadata.source_sha256);
    assert_eq!(incremental.source_fingerprint, rebuilt_metadata.source_fingerprint);
    assert_eq!(incremental.source_hash_scope, rebuilt_metadata.source_hash_scope);
    assert_eq!(current.source_sha256, incremental.source_sha256);
}

#[test]
fn export_keeps_visible_offscreen_analytic_entities_and_current_source_revision() {
    let mut document = LabDocument::new();
    let hidden_layer = create_layer(&mut document, "hidden", false);
    let receipt = apply(
        &mut document,
        "seed-export-entities",
        vec![
            DraftOperation::Create {
                drawing_id: DRAWING,
                layer_id: LayerId::new(1),
                geometry: line(0.0, 0.0, 1.0, 0.0),
                style: DraftStyle::default(),
            },
            DraftOperation::Create {
                drawing_id: DRAWING,
                layer_id: LayerId::new(1),
                geometry: line(1e6, 0.0, 1e6 + 1.0, 0.0),
                style: DraftStyle::default(),
            },
            DraftOperation::Create {
                drawing_id: DRAWING,
                layer_id: hidden_layer,
                geometry: line(2.0, 0.0, 3.0, 0.0),
                style: DraftStyle::default(),
            },
        ],
    );
    let visible_near = receipt.created_ids[0];
    let visible_offscreen = receipt.created_ids[1];
    let hidden = receipt.created_ids[2];

    let mut cache = new_cache(&document);
    let camera = camera();
    let options = DisplayOptions::default();
    let frame = cache
        .prepare(&document, EPOCH, &camera, &options)
        .unwrap();
    let captured = cache
        .materialize_export(&document, EPOCH, &frame)
        .unwrap();
    assert_eq!(frame.base.visible_entity_count, 1);
    let analytic_ids: Vec<_> = captured
        .display
        .analytic_entities
        .iter()
        .map(|entity| entity.id)
        .collect();

    assert!(analytic_ids.contains(&visible_near));
    assert!(analytic_ids.contains(&visible_offscreen));
    assert!(!analytic_ids.contains(&hidden));
    assert_eq!(captured.snapshot.entities.len(), 3);
    assert_eq!(captured.display.source_revision, captured.snapshot.revision);
    assert_eq!(
        captured.display.source_revision,
        document.snapshot(DRAWING).unwrap().revision
    );
    let metadata = cache.metadata(&document, EPOCH).unwrap();
    assert!(!metadata.source_hash_scope.is_empty());
    assert_source_hash_pair(&metadata.source_fingerprint, &metadata.source_sha256);
}
