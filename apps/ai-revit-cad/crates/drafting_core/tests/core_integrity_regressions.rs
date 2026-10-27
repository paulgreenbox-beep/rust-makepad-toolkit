//! Targeted persistence and command regressions. Cargo execution is coordinated
//! by the parent; these source tests are not an execution receipt.
use drafting_core::*;

fn create_line(document: &mut LabDocument, command_id: &str) -> DraftEntityId {
    document.apply(CommandEnvelope {
        command_id: command_id.into(),
        expected_revision: document.revision(),
        operations: vec![DraftOperation::Create {
            drawing_id: DrawingId::new(1),
            layer_id: LayerId::new(1),
            geometry: EntityGeometry::Line {
                start: Point2::new(0., 0.), end: Point2::new(1., 0.),
            },
            style: DraftStyle::default(),
        }],
    }).unwrap().created_ids[0]
}

fn lock_default_layer(document: &mut LabDocument) {
    document.apply(CommandEnvelope {
        command_id: "lock".into(),
        expected_revision: document.revision(),
        operations: vec![DraftOperation::SetLayer {
            drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
            visible: None, locked: Some(true), style: None,
        }],
    }).unwrap();
}

fn import_batch(list_layer: bool) -> ImportBatch {
    ImportBatch {
        drawing_id: DrawingId::new(1),
        source: ImportSourceSpec {
            name: "locked-layer.dxf".into(), sha256: "a".repeat(64),
            unit_resolution: UnitResolution {
                declared_unit: Some("metre".into()), resolved_unit: "metre".into(),
                scale_to_m: 1., decision_source: "declared".into(),
            },
            report: serde_json::json!({}),
        },
        layers: if list_layer {
            vec![ImportLayer {
                name: "Layer0".into(), visible: true, locked: true,
                style: DraftStyle::default(),
            }]
        } else { vec![] },
        entities: vec![ImportEntity {
            geometry: EntityGeometry::Circle { center: Point2::new(2., 2.), radius: 1. },
            layer_name: "Layer0".into(), style: DraftStyle::default(),
            source_handle: Some("AB".into()),
        }],
    }
}

#[test]
fn import_without_layer_definition_cannot_write_into_existing_locked_layer() {
    let mut document = LabDocument::new();
    create_line(&mut document, "seed");
    lock_default_layer(&mut document);
    let before = document.clone();
    let error = document.apply(CommandEnvelope {
        command_id: "import".into(), expected_revision: document.revision(),
        operations: vec![DraftOperation::Import { batch: import_batch(false) }],
    }).unwrap_err();
    assert_eq!(error.code, "locked_layer");
    assert_eq!(document, before, "refusal must roll back asset and all allocators");
}

#[test]
fn import_can_initialize_a_separate_locked_layer_without_touching_existing_lock() {
    let mut document = LabDocument::new();
    let old_id = create_line(&mut document, "seed");
    lock_default_layer(&mut document);
    let before = document.snapshot(DrawingId::new(1)).unwrap();
    let receipt = document.apply(CommandEnvelope {
        command_id: "import".into(), expected_revision: document.revision(),
        operations: vec![DraftOperation::Import { batch: import_batch(true) }],
    }).unwrap();
    let after = document.snapshot(DrawingId::new(1)).unwrap();
    assert_eq!(after.entity(old_id).unwrap(), before.entity(old_id).unwrap());
    assert_eq!(after.layers[&LayerId::new(1)], before.layers[&LayerId::new(1)]);
    let imported = after.entity(receipt.created_ids[0]).unwrap();
    assert_ne!(imported.layer_id, LayerId::new(1));
    assert!(after.layers[&imported.layer_id].locked);
    assert_eq!(after.layers[&imported.layer_id].name, "Layer0 (1)");
    let saved = document.to_json().unwrap();
    let mut reopened = LabDocument::from_json(&saved).unwrap();
    reopened.undo("undo-import", reopened.revision()).unwrap();
    assert_eq!(reopened.snapshot(DrawingId::new(1)).unwrap().entities, before.entities);
    reopened.redo("redo-import", reopened.revision()).unwrap();
    assert_eq!(reopened.snapshot(DrawingId::new(1)).unwrap().entity(imported.id).unwrap().layer_id,
               imported.layer_id);
}

fn move_line(document: &mut LabDocument, id: DraftEntityId, command_id: &str) {
    document.apply(CommandEnvelope {
        command_id: command_id.into(), expected_revision: document.revision(),
        operations: vec![DraftOperation::Move {
            drawing_id: DrawingId::new(1), ids: vec![id], delta: Point2::new(1., 0.),
        }],
    }).unwrap();
}

#[test]
fn saved_undo_cannot_target_a_different_move_with_the_same_entity_summary() {
    let mut document = LabDocument::new();
    let id = create_line(&mut document, "seed");
    move_line(&mut document, id, "move-first");
    move_line(&mut document, id, "move-last");
    document.undo("undo-last", document.revision()).unwrap();
    let mut saved: serde_json::Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
    saved["receipts"]["undo-last"]["target_command_id"] = serde_json::json!("move-first");
    assert!(LabDocument::from_json(&saved.to_string()).is_err(),
            "equal updated_ids cannot authorize undoing an earlier history record");
}

#[test]
fn load_rejects_missing_or_duplicate_revision_events_and_an_invented_apply() {
    let mut document = LabDocument::new();
    create_line(&mut document, "seed");
    document.undo("undo", document.revision()).unwrap();
    let original: serde_json::Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
    let mut missing = original.clone();
    missing["receipts"].as_object_mut().unwrap().remove("undo");
    assert!(LabDocument::from_json(&missing.to_string()).is_err());

    let mut duplicate = original.clone();
    duplicate["receipts"]["ghost"] = duplicate["receipts"]["seed"].clone();
    duplicate["receipts"]["ghost"]["receipt"]["command_id"] = serde_json::json!("ghost");
    duplicate["state"]["revision"] = serde_json::json!(3);
    assert!(LabDocument::from_json(&duplicate.to_string()).is_err());

    let mut orphan = original;
    orphan["receipts"]["ghost"] = orphan["receipts"]["seed"].clone();
    orphan["receipts"]["ghost"]["receipt"]["command_id"] = serde_json::json!("ghost");
    orphan["receipts"]["ghost"]["receipt"]["revision_before"] = serde_json::json!(2);
    orphan["receipts"]["ghost"]["receipt"]["revision_after"] = serde_json::json!(3);
    orphan["receipts"]["ghost"]["action"]["expected_revision"] = serde_json::json!(2);
    orphan["state"]["revision"] = serde_json::json!(3);
    assert!(LabDocument::from_json(&orphan.to_string()).is_err());
}

#[test]
fn discarded_branches_and_retained_undo_redo_keep_one_valid_event_chain() {
    let mut document = LabDocument::new();
    let id = create_line(&mut document, "seed");
    move_line(&mut document, id, "discard-first");
    document.undo("undo-discard-first", document.revision()).unwrap();
    document.redo("redo-discard-first", document.revision()).unwrap();
    document.undo("undo-discard-first-again", document.revision()).unwrap();
    move_line(&mut document, id, "keep-second");
    document.undo("undo-keep-second", document.revision()).unwrap();
    document.undo("undo-seed", document.revision()).unwrap();
    document.redo("redo-seed", document.revision()).unwrap();
    document.redo("redo-keep-second", document.revision()).unwrap();
    let serialized = document.to_json().unwrap();
    let reopened = LabDocument::from_json(&serialized).unwrap();
    assert_eq!(reopened, document);
    assert_eq!(reopened.history_cursor(), 2);
    assert_eq!(reopened.revision(), 10);
}

#[test]
fn saved_geometry_cursor_cannot_override_the_recorded_undo_event() {
    let mut document = LabDocument::new();
    create_line(&mut document, "seed");
    document.undo("undo", document.revision()).unwrap();
    let mut saved: serde_json::Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
    saved["state"]["drawings"]["1"]["entities"]["1"] = saved["history"][0]["changes"][0]["after"].clone();
    saved["history_cursor"] = serde_json::json!(1);
    assert!(LabDocument::from_json(&saved.to_string()).is_err());
}

#[test]
fn pruned_create_delete_keeps_issued_id_evidence_after_reopen() {
    let mut document = LabDocument::new();
    document.apply(CommandEnvelope {
        command_id: "transient".into(), expected_revision: 0,
        operations: vec![
            DraftOperation::Create {
                drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
                geometry: EntityGeometry::Line {
                    start: Point2::new(0., 0.), end: Point2::new(1., 0.),
                }, style: DraftStyle::default(),
            },
            DraftOperation::Delete { drawing_id: DrawingId::new(1), ids: vec![DraftEntityId::new(1)] },
        ],
    }).unwrap();
    document.undo("undo-transient", document.revision()).unwrap();
    document.apply(CommandEnvelope {
        command_id: "branch".into(), expected_revision: document.revision(),
        operations: vec![DraftOperation::SetLayer {
            drawing_id: DrawingId::new(1), layer_id: LayerId::new(1),
            visible: Some(false), locked: None, style: None,
        }],
    }).unwrap();
    let serialized = document.to_json().unwrap();
    let mut reopened = LabDocument::from_json(&serialized).unwrap();
    assert_eq!(create_line(&mut reopened, "after-reopen"), DraftEntityId::new(2));
    let mut forged: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    forged["state"]["next_ids"]["entity_high_water"] = serde_json::json!("0");
    forged["receipts"]["branch"]["allocators_before"]["entity_high_water"] = serde_json::json!("0");
    assert!(LabDocument::from_json(&forged.to_string()).is_err());
    let mut missing: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    missing["receipts"]["transient"].as_object_mut().unwrap().remove("allocators_after");
    assert_eq!(LabDocument::from_json(&missing.to_string()).unwrap_err().code, "allocator_evidence_missing");
}

#[test]
fn saved_command_identity_obeys_the_same_nul_rule_as_command_apply() {
    let mut document = LabDocument::new();
    create_line(&mut document, "seed");
    let mut saved: serde_json::Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
    let mut event = saved["receipts"].as_object_mut().unwrap().remove("seed").unwrap();
    event["receipt"]["command_id"] = serde_json::json!("seed\0");
    saved["receipts"]["seed\0"] = event;
    saved["history"][0]["command_id"] = serde_json::json!("seed\0");
    assert!(LabDocument::from_json(&saved.to_string()).is_err());
}

#[test]
fn earlier_straight_history_without_optional_allocator_after_can_be_loaded() {
    let mut document = LabDocument::new();
    let id = create_line(&mut document, "seed");
    move_line(&mut document, id, "move");
    let mut saved: serde_json::Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
    for event in saved["receipts"].as_object_mut().unwrap().values_mut() {
        event.as_object_mut().unwrap().remove("allocators_after");
    }
    let reopened = LabDocument::from_json(&saved.to_string()).unwrap();
    assert_eq!(reopened.snapshot(DrawingId::new(1)).unwrap().entities,
               document.snapshot(DrawingId::new(1)).unwrap().entities);
    assert_eq!(reopened.allocators(), document.allocators());
}

#[test]
fn import_conflict_names_skip_existing_suffixes_and_respect_utf8_byte_budget() {
    for requested in ["Layer0".to_string(), "я".repeat(256)] {
        let mut document = LabDocument::new();
        if requested != "Layer0" {
            document.apply(CommandEnvelope {
                command_id: "long-name".into(), expected_revision: document.revision(),
                operations: vec![DraftOperation::CreateLayer {
                    drawing_id: DrawingId::new(1), name: requested.clone(),
                    visible: true, locked: true, style: DraftStyle::default(),
                }],
            }).unwrap();
        } else {
            lock_default_layer(&mut document);
            document.apply(CommandEnvelope {
                command_id: "existing-suffix".into(), expected_revision: document.revision(),
                operations: vec![DraftOperation::CreateLayer {
                    drawing_id: DrawingId::new(1), name: "Layer0 (1)".into(),
                    visible: true, locked: false, style: DraftStyle::default(),
                }],
            }).unwrap();
        }
        let mut batch = import_batch(true);
        batch.layers[0].name = requested.clone();
        batch.entities[0].layer_name = requested.clone();
        let receipt = document.apply(CommandEnvelope {
            command_id: "import-named".into(), expected_revision: document.revision(),
            operations: vec![DraftOperation::Import { batch }],
        }).unwrap();
        let snapshot = document.snapshot(DrawingId::new(1)).unwrap();
        let imported = snapshot.entity(receipt.created_ids[0]).unwrap();
        let actual_name = &snapshot.layers[&imported.layer_id].name;
        assert!(actual_name.len() <= 512);
        if requested == "Layer0" { assert_eq!(actual_name, "Layer0 (1:2)"); }
        else { assert!(actual_name.ends_with(" (1)")); }
        LabDocument::from_json(&document.to_json().unwrap()).unwrap();
    }
}
