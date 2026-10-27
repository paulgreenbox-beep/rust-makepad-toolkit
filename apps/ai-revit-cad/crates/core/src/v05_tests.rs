use crate::*;
use drafting_core::DrawingSource;

fn commit(d: &mut Document, id: &str, commands: Vec<Command>) -> CommitReceipt {
    let mut t = d.begin_transaction(id, d.revision());
    for c in commands {
        t.push(c);
    }
    d.commit(t).unwrap()
}
fn drawing(d: &mut Document) -> (DrawingId, DraftLayerId) {
    let r = commit(
        d,
        "drawing",
        vec![Command::Draft(DraftCommand::CreateDrawing {
            name: "Plan".into(),
            plane: DrawingPlane::default(),
        })],
    );
    let id = r.created_drawings[0];
    (id, *d.drawing(id).unwrap().layers.keys().next().unwrap())
}
fn line(id: DrawingId, layer: DraftLayerId) -> Command {
    Command::Draft(DraftCommand::CreateDraftEntity {
        drawing_id: id,
        layer_id: layer,
        geometry: DraftGeometry::Line {
            start: DraftPoint2::new(0., 0.),
            end: DraftPoint2::new(2., 0.),
        },
        style: DraftStyle::default(),
    })
}
fn wall() -> Command {
    Command::CreateElement {
        name: "Wall".into(),
        class_id: BUILTIN_WALL_CLASS_ID.into(),
        kind: ElementKind::Wall(WallSpec {
            start: Point2::new(0., 0.),
            end: Point2::new(5., 0.),
            base_elevation_m: 0.,
            height_m: 3.,
            thickness_m: 0.2,
        }),
        properties: BTreeMap::new(),
    }
}
fn wall_update(id: ElementId, length: f64) -> Command {
    Command::UpdateElement {
        element_id: id,
        kind: ElementKind::Wall(WallSpec {
            start: Point2::new(0., 0.),
            end: Point2::new(length, 0.),
            base_elevation_m: 0.,
            height_m: 3.,
            thickness_m: 0.2,
        }),
    }
}

#[test]
fn v05_cross_domain_atomic_undo_redo_and_rollback() {
    let mut d = Document::new("Unified").unwrap();
    let (drawing, layer) = drawing(&mut d);
    let wall_id = commit(&mut d, "wall", vec![wall()]).created_elements[0];
    let old = d.element(wall_id).unwrap().clone();
    let rev = d.revision();
    let r = commit(
        &mut d,
        "mixed",
        vec![wall_update(wall_id, 8.), line(drawing, layer)],
    );
    assert_eq!(r.revision_before, rev);
    assert_eq!(r.revision_after, rev + 1);
    assert_eq!(r.created_draft_entities.len(), 1);
    let eid = r.created_draft_entities[0];
    let edited = d.element(wall_id).unwrap().clone();
    assert_ne!(old, edited);
    assert!(d.draft_entity(drawing, eid).is_some());
    assert_eq!(r.delta.updated_elements, vec![wall_id]);
    assert_eq!(r.delta.drafting.created_entities, vec![eid]);
    let h = d.undo().unwrap().unwrap();
    assert_eq!(d.element(wall_id), Some(&old));
    assert!(d.draft_entity(drawing, eid).is_none());
    assert_eq!(h.delta.drafting.deleted_entities, vec![eid]);
    d.redo().unwrap().unwrap();
    assert_eq!(d.element(wall_id), Some(&edited));
    assert!(d.draft_entity(drawing, eid).is_some());
    let before = d.to_json_string().unwrap();
    let bad = Command::Draft(DraftCommand::CreateDraftEntity {
        drawing_id: drawing,
        layer_id: layer,
        geometry: DraftGeometry::Circle {
            center: DraftPoint2::new(0., 0.),
            radius: -1.,
        },
        style: DraftStyle::default(),
    });
    let mut t = d.begin_transaction("bad-mixed", d.revision());
    t.push(wall_update(wall_id, 12.)).push(bad);
    assert!(d.commit(t).is_err());
    assert_eq!(before, d.to_json_string().unwrap());
}

#[test]
fn v05_high_water_typed_ids_idempotency_and_save_reopen() {
    let mut d = Document::new("Unified").unwrap();
    let (drawing, layer) = drawing(&mut d);
    let r = commit(&mut d, "first", vec![line(drawing, layer)]);
    let eid = r.created_draft_entities[0];
    assert_eq!(eid.get(), 1);
    assert_eq!(serde_json::to_value(eid).unwrap(), "1");
    assert_ne!(
        serde_json::to_value(ProjectObjectRef::Bim {
            element_id: ElementId::new(1)
        })
        .unwrap(),
        serde_json::to_value(ProjectObjectRef::Drafting {
            drawing_id: drawing,
            entity_id: eid
        })
        .unwrap()
    );
    d.undo().unwrap();
    let revision = d.revision();
    let mut retry = d.begin_transaction("first", 0);
    retry.push(line(drawing, layer));
    assert!(d.commit(retry).unwrap().replayed);
    assert_eq!(d.revision(), revision);
    assert!(d.draft_entity(drawing, eid).is_none());
    let r = commit(&mut d, "second", vec![line(drawing, layer)]);
    let eid2 = r.created_draft_entities[0];
    assert!(eid2.get() > eid.get());
    commit(
        &mut d,
        "delete",
        vec![Command::Draft(DraftCommand::DeleteDraftEntity {
            drawing_id: drawing,
            entity_ids: vec![eid2],
        })],
    );
    let text = d.to_json_string().unwrap();
    assert!(text.contains("typed_drafting_v1"));
    let mut loaded = Document::from_json_str(&text).unwrap();
    assert_eq!(d.document_uuid, loaded.document_uuid);
    assert_eq!(d.revision(), loaded.revision());
    assert_eq!(d.drafting(), loaded.drafting());
    let r = commit(&mut loaded, "third", vec![line(drawing, layer)]);
    assert!(r.created_draft_entities[0].get() > eid2.get());
    let mut stale = loaded.begin_transaction("stale", 0);
    stale.push(line(drawing, layer));
    assert!(matches!(
        loaded.commit(stale),
        Err(CoreError::RevisionConflict { .. })
    ));
}

#[test]
fn v05_compact_history_replay_diff_and_tamper_rejected() {
    let mut d = Document::new("History").unwrap();
    let (drawing, layer) = drawing(&mut d);
    let from = d.revision();
    let r = commit(&mut d, "line", vec![line(drawing, layer)]);
    let eid = r.created_draft_entities[0];
    let created = d.revision();
    commit(
        &mut d,
        "move",
        vec![Command::Draft(DraftCommand::MoveDraftEntity {
            drawing_id: drawing,
            entity_ids: vec![eid],
            delta: DraftPoint2::new(3., 4.),
        })],
    );
    let moved = d.revision();
    assert_eq!(
        d.history_diff(from, created)
            .unwrap()
            .drafting
            .created_entities,
        vec![eid]
    );
    assert_eq!(
        d.history_diff(created, moved)
            .unwrap()
            .drafting
            .updated_entities,
        vec![eid]
    );
    assert_ne!(
        d.replay_revision(created)
            .unwrap()
            .draft_entity(drawing, eid),
        d.draft_entity(drawing, eid)
    );
    let source = d.to_json_string().unwrap();
    Document::from_json_str(&source).unwrap();
    let mut bad: serde_json::Value = serde_json::from_str(&source).unwrap();
    bad["document"]["draft_ids"]["entity"] = serde_json::json!("1");
    assert!(Document::from_json_str(&bad.to_string()).is_err());
    let mut bad: serde_json::Value = serde_json::from_str(&source).unwrap();
    bad["document"]["command_receipts"]["line"]["recorded_commands"][0]["draft"]["geometry"]["end"]
        ["x"] = serde_json::json!(99);
    assert!(Document::from_json_str(&bad.to_string()).is_err());
}

#[test]
fn v05_legacy_migration_and_native_envelope_gate() {
    let d = Document::new("Legacy").unwrap();
    let mut old = serde_json::to_value(&d).unwrap();
    old["format_id"] = serde_json::json!("ai-revit-provisional-json");
    old["format_version"] = serde_json::json!(1);
    let o = old.as_object_mut().unwrap();
    o.remove("drafting");
    o.remove("draft_ids");
    let migrated = Document::from_json_str(&old.to_string()).unwrap();
    assert!(migrated.drawings().is_empty());
    assert_eq!(migrated.document_uuid, d.document_uuid);
    let native: serde_json::Value =
        serde_json::from_str(&migrated.to_json_string().unwrap()).unwrap();
    assert!(native.get("document_uuid").is_none());
    assert_eq!(native["format_version"], 2);
    assert_eq!(
        native["document"]["document_uuid"],
        serde_json::to_value(d.document_uuid).unwrap()
    );
    let mut bad = native.clone();
    bad["required_features"] = serde_json::json!(["future_unknown"]);
    assert!(Document::from_json_str(&bad.to_string()).is_err());
}

#[test]
fn v05_cache_source_uses_unified_journal_without_lab_owner() {
    let mut d = Document::new("Cache").unwrap();
    let (drawing, layer) = drawing(&mut d);
    let mut cache = drafting_core::DraftingCache::new(Default::default()).unwrap();
    cache.reset(&d, drawing, 1).unwrap();
    let camera = drafting_core::Camera2D::default();
    let options = drafting_core::DisplayOptions::default();
    let first = cache.prepare(&d, 1, &camera, &options).unwrap();
    commit(&mut d, "bim-only", vec![wall()]);
    cache.sync_from_source(&d, 1).unwrap();
    let second = cache.prepare(&d, 1, &camera, &options).unwrap();
    assert!(std::sync::Arc::ptr_eq(&first.base, &second.base));
    assert_eq!(second.source.document_revision, d.revision());
    commit(&mut d, "new-line", vec![line(drawing, layer)]);
    cache.sync_from_source(&d, 1).unwrap();
    assert_eq!(cache.metadata(&d, 1).unwrap().entity_count, 1);
    d.undo().unwrap();
    cache.sync_from_source(&d, 1).unwrap();
    assert_eq!(cache.metadata(&d, 1).unwrap().entity_count, 0);
    let cloned = d.clone();
    assert_ne!(
        d.cache_source_key(drawing, 1).unwrap().runtime_instance_id,
        cloned
            .cache_source_key(drawing, 1)
            .unwrap()
            .runtime_instance_id
    );
    assert!(cache.prepare(&cloned, 1, &camera, &options).is_err());
}

#[test]
fn v05_lab_import_is_one_transaction_with_mapping_and_signed_bulge() {
    let mut lab = drafting_core::LabDocument::new();
    let old_drawing = lab.default_drawing_id();
    let layer = *lab.drawings()[&old_drawing].layers.keys().next().unwrap();
    let geometry = DraftGeometry::Polyline {
        vertices: vec![
            PolylineVertex::new(DraftPoint2::new(1e8, 1e8), -0.75),
            PolylineVertex::new(DraftPoint2::new(1e8 + 4., 1e8 + 2.), 0.),
        ],
        closed: false,
    };
    lab.apply(drafting_core::CommandEnvelope {
        command_id: "old-line".into(),
        expected_revision: 0,
        operations: vec![drafting_core::DraftOperation::Create {
            drawing_id: old_drawing,
            layer_id: layer,
            geometry: geometry.clone(),
            style: DraftStyle::default(),
        }],
    })
    .unwrap();
    let old_entity = *lab.drawings()[&old_drawing].entities.keys().next().unwrap();
    let json = lab.to_json().unwrap();
    let mut d = Document::new("Imported").unwrap();
    let preview = d.prepare_lab_import(&json, "source.lab.json").unwrap();
    assert!(!preview.history_imported);
    assert_eq!(preview.commands.len(), 1);
    let before = d.revision();
    let receipt = commit(&mut d, "import-lab", preview.commands.clone());
    assert_eq!(d.revision(), before + 1);
    assert_eq!(receipt.created_drawings.len(), 1);
    assert_eq!(receipt.created_draft_entities.len(), 1);
    let drawing = preview.mapping.drawings[&old_drawing];
    let entity = preview.mapping.entities[&old_entity];
    assert_eq!(d.draft_entity(drawing, entity).unwrap().geometry, geometry);
    let source = d
        .draft_entity(drawing, entity)
        .unwrap()
        .source
        .as_ref()
        .unwrap();
    assert_eq!(
        d.drawing(drawing).unwrap().import_sources[&source.import_asset_id]
            .source
            .sha256,
        format!(
            "{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(json.as_bytes())
        )
    );
    d.undo().unwrap();
    assert!(d.drawings().is_empty());
    d.redo().unwrap();
    assert_eq!(d.draft_entity(drawing, entity).unwrap().geometry, geometry);
    let loaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(loaded.drafting(), d.drafting());
}

#[test]
fn v05_import_handle_namespaces_and_all_analytic_geometry() {
    let mut d = Document::new("Geometry").unwrap();
    let (drawing, layer) = drawing(&mut d);
    let geometries = vec![
        DraftGeometry::Line {
            start: DraftPoint2::new(0., 0.),
            end: DraftPoint2::new(3., 0.),
        },
        DraftGeometry::Circle {
            center: DraftPoint2::new(5., 5.),
            radius: 2.,
        },
        DraftGeometry::Arc {
            center: DraftPoint2::new(10., 10.),
            radius: 3.,
            start_angle: 0.,
            sweep_angle: -std::f64::consts::PI,
        },
        DraftGeometry::Polyline {
            vertices: vec![
                PolylineVertex::new(DraftPoint2::new(0., 0.), 0.5),
                PolylineVertex::new(DraftPoint2::new(3., 3.), -0.5),
            ],
            closed: true,
        },
    ];
    let commands = geometries
        .iter()
        .map(|g| {
            Command::Draft(DraftCommand::CreateDraftEntity {
                drawing_id: drawing,
                layer_id: layer,
                geometry: g.clone(),
                style: DraftStyle::default(),
            })
        })
        .collect();
    let r = commit(&mut d, "geometry", commands);
    assert_eq!(r.created_draft_entities.len(), 4);
    let make_batch = |name: &str| ImportBatch {
        drawing_id: drawing,
        source: ImportSourceSpec {
            name: name.into(),
            sha256: "a".repeat(64),
            unit_resolution: UnitResolution {
                declared_unit: Some("mm".into()),
                resolved_unit: "millimetre".into(),
                scale_to_m: 0.001,
                decision_source: "test_explicit".into(),
            },
            report: serde_json::json!({"bounded":true}),
        },
        layers: vec![ImportLayer {
            name: "0".into(),
            visible: true,
            locked: false,
            style: DraftStyle::default(),
        }],
        entities: vec![ImportEntity {
            geometry: geometries[0].clone(),
            layer_name: "0".into(),
            style: DraftStyle::default(),
            source_handle: Some("AB".into()),
        }],
    };
    let a = commit(
        &mut d,
        "file-a",
        vec![Command::Draft(DraftCommand::ImportDraftingEntities {
            batch: make_batch("a.dxf"),
        })],
    );
    let b = commit(
        &mut d,
        "file-b",
        vec![Command::Draft(DraftCommand::ImportDraftingEntities {
            batch: make_batch("b.dxf"),
        })],
    );
    let a = d
        .draft_entity(drawing, a.created_draft_entities[0])
        .unwrap()
        .source
        .as_ref()
        .unwrap();
    let b = d
        .draft_entity(drawing, b.created_draft_entities[0])
        .unwrap()
        .source
        .as_ref()
        .unwrap();
    assert_ne!(a.import_asset_id, b.import_asset_id);
    assert_eq!(a.source_handle, b.source_handle);
    Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    let before = d.to_json_string().unwrap();
    let mut bad = d.begin_transaction("nonfinite", d.revision());
    bad.push(Command::Draft(DraftCommand::UpdateDraftEntity {
        drawing_id: drawing,
        entity_id: r.created_draft_entities[0],
        geometry: DraftGeometry::Line {
            start: DraftPoint2::new(f64::NAN, 0.),
            end: DraftPoint2::new(1., 1.),
        },
    }));
    assert!(d.commit(bad).is_err());
    assert_eq!(before, d.to_json_string().unwrap());
}

#[test]
fn v05_bim_view_reference_is_durable_and_manual_geometry_survives() {
    let mut d = Document::new("Mixed").unwrap();
    let (drawing, layer) = drawing(&mut d);
    let wall_id = commit(&mut d, "wall", vec![wall()]).created_elements[0];
    let entity = commit(&mut d, "manual", vec![line(drawing, layer)]).created_draft_entities[0];
    let manual = d.draft_entity(drawing, entity).unwrap().clone();
    let receipt = commit(
        &mut d,
        "view",
        vec![Command::Draft(DraftCommand::CreateViewDefinition {
            drawing_id: drawing,
            name: "XY reference".into(),
            source: DraftSourceReference {
                element_ids: vec![wall_id],
                elevation_m: Some(0.),
                coordinate_system: DraftCoordinateSystem::Xy,
            },
        })],
    );
    let view = receipt.delta.drafting.created_views[0];
    commit(&mut d, "edit-wall", vec![wall_update(wall_id, 9.)]);
    assert_eq!(d.draft_entity(drawing, entity), Some(&manual));
    assert_eq!(
        d.view_definition(view).unwrap().source.element_ids,
        vec![wall_id]
    );
    d.undo().unwrap();
    assert_eq!(d.draft_entity(drawing, entity), Some(&manual));
    let reopened = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(reopened.view_definition(view), d.view_definition(view));
}

#[test]
fn v05_lab_preview_mapping_refuses_prepended_allocation_atomically() {
    let lab = drafting_core::LabDocument::new();
    let json = lab.to_json().unwrap();
    let mut d = Document::new("Mapping").unwrap();
    let preview = d.prepare_lab_import(&json, "mapped.lab.json").unwrap();
    let before = d.to_json_string().unwrap();
    let mut transaction = d.begin_transaction("stale-allocation-plan", preview.expected_revision);
    transaction.push(Command::Draft(DraftCommand::CreateDrawing {
        name: "Prepended allocation".into(),
        plane: DrawingPlane::default(),
    }));
    for command in &preview.commands {
        transaction.push(command.clone());
    }
    assert!(d.commit(transaction).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
    assert!(d.drawings().is_empty());
    let receipt = commit(&mut d, "exact-allocation-plan", preview.commands);
    assert_eq!(
        receipt.created_drawings,
        preview
            .mapping
            .drawings
            .values()
            .copied()
            .collect::<Vec<_>>()
    );
    assert_eq!(d.revision(), 1);
}

#[test]
fn v05_qualified_receipts_cover_metadata_delete_net_zero_and_legacy() {
    let mut d = Document::new("Qualified").unwrap();
    let (drawing, layer) = drawing(&mut d);
    assert!(
        d.command_receipts[&CommandId::from("drawing")]
            .receipt
            .affected_objects
            .contains(&ProjectObjectRef::Drawing {
                drawing_id: drawing
            })
    );
    let transient = DraftEntityId::new(d.draft_ids.entity);
    let receipt = commit(
        &mut d,
        "net-zero",
        vec![
            line(drawing, layer),
            Command::Draft(DraftCommand::DeleteDraftEntity {
                drawing_id: drawing,
                entity_ids: vec![transient],
            }),
        ],
    );
    let reference = ProjectObjectRef::Drafting {
        drawing_id: drawing,
        entity_id: transient,
    };
    assert!(receipt.created_draft_entities.is_empty());
    assert!(receipt.affected_objects.contains(&reference));
    assert!(d.draft_entity(drawing, transient).is_none());
    assert!(
        d.undo()
            .unwrap()
            .unwrap()
            .affected_objects
            .contains(&reference)
    );
    d.redo().unwrap();
    let entity = commit(&mut d, "create", vec![line(drawing, layer)]).created_draft_entities[0];
    let receipt = commit(
        &mut d,
        "delete-qualified",
        vec![Command::Draft(DraftCommand::DeleteDraftEntity {
            drawing_id: drawing,
            entity_ids: vec![entity],
        })],
    );
    assert!(
        receipt
            .affected_objects
            .contains(&ProjectObjectRef::Drafting {
                drawing_id: drawing,
                entity_id: entity
            })
    );
    Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    let mut legacy = Document::new("Legacy qualified").unwrap();
    let receipt = commit(&mut legacy, "legacy-wall", vec![wall()]);
    let mut value = serde_json::to_value(&legacy).unwrap();
    value["format_id"] = serde_json::json!("ai-revit-provisional-json");
    value["format_version"] = serde_json::json!(1);
    value["command_receipts"]["legacy-wall"]["receipt"]
        .as_object_mut()
        .unwrap()
        .remove("affected_objects");
    let mut loaded = Document::from_json_str(&value.to_string()).unwrap();
    let mut retry = loaded.begin_transaction("legacy-wall", 0);
    retry.push(wall());
    let replayed = loaded.commit(retry).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.affected_objects, receipt.affected_objects);
    assert_eq!(loaded.revision(), legacy.revision());
}
