use crate::*;

#[test]
fn v04_receipt_payload_corruption_is_rejected_on_open_save_replay_and_retry() {
    let mut original = Document::new("Receipt payload corruption").unwrap();
    apply(&mut original, "original-command", vec![wall("Original")]);
    for payload in [
        "invalid JSON".to_owned(),
        "null".to_owned(),
        "[]".to_owned(),
        serde_json::to_string(&vec![wall("Forged")]).unwrap(),
        serde_json::to_string(&vec![wall("Original"), wall("Extra")]).unwrap(),
        format!(
            " {}",
            serde_json::to_string(&vec![wall("Original")]).unwrap()
        ),
    ] {
        let mut corrupt = original.clone();
        corrupt
            .command_receipts
            .get_mut(&"original-command".into())
            .unwrap()
            .payload_json = payload;
        let json = serde_json::to_string(&corrupt).unwrap();
        assert!(
            Document::from_json_str(&json).is_err(),
            "corrupt receipt accepted on open"
        );
        assert!(
            corrupt.to_json_string().is_err(),
            "corrupt receipt accepted on save"
        );
        assert!(
            corrupt.replay_revision(1).is_err(),
            "corrupt receipt accepted on replay"
        );
        assert!(
            corrupt
                .commit(Transaction::new("original-command", 0).with(wall("Forged")))
                .is_err(),
            "forged retry returned the old receipt"
        );
    }
}

#[test]
fn v04_typed_hosted_opening_receipts_use_complete_composite_journal_and_survive_history() {
    for polygonal in [false, true] {
        let mut d = Document::new("Typed hosted opening receipt").unwrap();
        let seed = apply(
            &mut d,
            "seed",
            vec![
                wall("Host"),
                Command::CreateType {
                    name: "Opening type".into(),
                    class_id: BUILTIN_OPENING_CLASS_ID.into(),
                    properties: BTreeMap::new(),
                },
            ],
        );
        let host_id = seed.created_elements[0];
        let type_id = Some(seed.created_types[0]);
        let command = if polygonal {
            Command::CreatePolygonalOpening {
                name: "Polygonal opening".into(),
                spec: PolygonalOpeningSpec {
                    center: Point2::new(2., 0.),
                    elevation_m: 0.5,
                    boundary: vec![
                        Point2::new(-0.5, 0.),
                        Point2::new(0.5, 0.),
                        Point2::new(0., 1.),
                    ],
                    depth_m: 0.3,
                    rotation_rad: 0.,
                },
                properties: BTreeMap::new(),
                host_id,
                type_id,
            }
        } else {
            Command::CreateOpening {
                name: "Rectangular opening".into(),
                spec: OpeningSpec {
                    center: Point2::new(2., 0.),
                    elevation_m: 0.5,
                    width_m: 1.,
                    height_m: 1.,
                    depth_m: 0.3,
                    rotation_rad: 0.,
                },
                properties: BTreeMap::new(),
                host_id,
                type_id,
            }
        };
        let receipt = apply(&mut d, "opening", vec![command.clone()]);
        assert_eq!(
            d.element(receipt.created_elements[0]).unwrap().type_id,
            type_id
        );
        let mut without_type = command.clone();
        match &mut without_type {
            Command::CreateOpening { type_id, .. }
            | Command::CreatePolygonalOpening { type_id, .. } => *type_id = None,
            _ => unreachable!(),
        }
        let mut corrupt = d.clone();
        let stored = corrupt.command_receipts.get_mut(&"opening".into()).unwrap();
        stored.payload_json = serde_json::to_string(&vec![without_type.clone()]).unwrap();
        stored.recorded_commands = Some(vec![without_type]);
        assert!(
            corrupt.to_json_string().is_err(),
            "omitting an originally requested Type must disagree with the journal"
        );
        let mut loaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
        assert!(
            loaded
                .commit(Transaction::new("opening", 0).with(command.clone()))
                .unwrap()
                .replayed
        );
        loaded.undo().unwrap();
        let loaded = Document::from_json_str(&loaded.to_json_string().unwrap()).unwrap();
        let mut replay = loaded.replay_revision(receipt.revision).unwrap();
        replay.undo().unwrap();
        replay.redo().unwrap();
        let mut loaded = Document::from_json_str(&replay.to_json_string().unwrap()).unwrap();
        assert!(
            loaded
                .commit(Transaction::new("opening", 0).with(command))
                .unwrap()
                .replayed
        );
    }
}

#[test]
fn v04_receipt_origin_effects_reject_omitted_type_property_space_level_and_element_override() {
    let mut d = Document::new("Omitted receipt effects").unwrap();
    let seed = apply(
        &mut d,
        "seed",
        vec![
            Command::CreateType {
                name: "Original type".into(),
                class_id: BUILTIN_WALL_CLASS_ID.into(),
                properties: [("height".into(), number(3.))].into_iter().collect(),
            },
            Command::CreateLevel {
                name: "Level".into(),
                elevation_m: 0.,
                height_m: None,
            },
        ],
    );
    let type_id = seed.created_types[0];
    let level_id = seed.created_levels[0];
    apply(
        &mut d,
        "type-edit",
        vec![Command::UpdateType {
            type_id,
            name: Some("Renamed".into()),
            properties: [("height".into(), number(4.))].into_iter().collect(),
        }],
    );
    apply(
        &mut d,
        "space",
        vec![Command::CreateSpace {
            name: "Space".into(),
            level_id: Some(level_id),
            boundary: None,
            metadata: BTreeMap::new(),
        }],
    );
    let mut element = wall("Override");
    if let Command::CreateElement { properties, .. } = &mut element {
        properties.insert("height".into(), number(3.2));
    }
    apply(&mut d, "element", vec![element]);
    for (id, command) in [
        (
            "type-edit",
            Command::UpdateType {
                type_id,
                name: None,
                properties: BTreeMap::new(),
            },
        ),
        (
            "space",
            Command::CreateSpace {
                name: "Space".into(),
                level_id: None,
                boundary: None,
                metadata: BTreeMap::new(),
            },
        ),
        ("element", wall("Override")),
    ] {
        let mut corrupt = d.clone();
        let stored = corrupt.command_receipts.get_mut(&id.into()).unwrap();
        stored.payload_json = serde_json::to_string(&vec![command.clone()]).unwrap();
        stored.recorded_commands = Some(vec![command]);
        assert!(
            corrupt
                .to_json_string()
                .unwrap_err()
                .to_string()
                .contains("recorded operation or effects"),
            "{id}"
        );
    }
    apply(
        &mut d,
        "legitimate-noop",
        vec![Command::UpdateType {
            type_id,
            name: None,
            properties: BTreeMap::new(),
        }],
    );
    Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
}

#[test]
fn v04_receipt_payload_uses_independent_origin_effects_even_without_exact_legacy_request() {
    let mut original = Document::new("Independent receipt effects").unwrap();
    let id = apply(&mut original, "seed", vec![wall("Wall")]).created_elements[0];
    let edit = Command::SetProperty {
        element_id: id,
        property_id: "height".into(),
        value: number(3.119),
    };
    apply(&mut original, "edit", vec![edit.clone()]);
    for original_requests in [
        None,
        Some(vec![Command::SetProperty {
            element_id: id,
            property_id: "height".into(),
            value: number(3.5),
        }]),
    ] {
        let mut corrupt = original.clone();
        let stored = corrupt.command_receipts.get_mut(&"edit".into()).unwrap();
        stored.payload_json = serde_json::to_string(&vec![Command::SetProperty {
            element_id: id,
            property_id: "height".into(),
            value: number(3.5),
        }])
        .unwrap();
        stored.recorded_commands = original_requests;
        assert!(
            corrupt
                .to_json_string()
                .unwrap_err()
                .to_string()
                .contains("recorded operation or effects")
        );
    }
    original.undo().unwrap();
    let mut corrupt = original.clone();
    let stored = corrupt.command_receipts.get_mut(&"edit".into()).unwrap();
    stored.recorded_commands = None;
    stored.payload_json = serde_json::to_string(&vec![wall("Wrong command variant")]).unwrap();
    assert!(
        corrupt.to_json_string().is_err(),
        "undone operation must still verify against its originating journal"
    );
    let mut replay = original.replay_revision(0).unwrap();
    let before = replay.revision();
    let receipt = replay
        .commit(Transaction::new("edit", 99).with(edit))
        .unwrap();
    assert!(receipt.replayed);
    assert_eq!(replay.revision(), before);
    assert!(replay.receipt_payload_diagnostics().unwrap().is_empty());
    Document::from_json_str(&replay.to_json_string().unwrap()).unwrap();
}

#[test]
fn v04_legacy_original_receipts_retry_unchanged_and_report_missing_exact_request_evidence() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native-baseline");
    for relative in [
        "example.ai-revit.json",
        "v02/acceptance.ai-revit.json",
        "v03/acceptance.ai-revit.json",
    ] {
        let path = root.join(relative);
        let bytes = std::fs::read(&path).unwrap();
        let mut d = Document::from_json_str(std::str::from_utf8(&bytes).unwrap()).unwrap();
        let original_receipts = d.command_receipts.clone();
        let diagnostics = d.receipt_payload_diagnostics().unwrap();
        assert_eq!(diagnostics.len(), original_receipts.len());
        if relative != "example.ai-revit.json" {
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.verified_operation_count == 0),
                "orphan receipt diagnostic absent: {relative}"
            );
        }
        for (id, stored) in &original_receipts {
            let commands: Vec<Command> = serde_json::from_str(&stored.payload_json).unwrap();
            let mut retry = Transaction::new(id.clone(), 0);
            for command in commands {
                retry.push(command);
            }
            let mut expected = stored.receipt.clone();
            expected.replayed = true;
            assert_eq!(
                d.commit(retry).unwrap(),
                expected,
                "legacy receipt changed: {relative} {id}"
            );
        }
        let reopened = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
        assert_eq!(reopened.command_receipts, original_receipts);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn v04_property_definition_visits_only_indexed_descendants_and_reports_actual_classes() {
    let mut d = Document::new("Indexed descendant property validation").unwrap();
    let mut classes = (0..1000)
        .map(|i| {
            Command::DefineClass(ClassDef::new(
                format!("unrelated.{i}"),
                format!("Unrelated {i}"),
                ElementCategory::Generic,
            ))
        })
        .collect::<Vec<_>>();
    classes.extend([
        Command::DefineClass(ClassDef::new(
            "ancestor",
            "Ancestor",
            ElementCategory::Generic,
        )),
        Command::DefineClass(ClassDef::child("child", "Child", "ancestor")),
        Command::DefineClass(ClassDef::child("grandchild", "Grandchild", "child")),
    ]);
    apply(&mut d, "classes", classes);
    let receipt = apply(
        &mut d,
        "ancestor-property",
        vec![Command::DefineProperty {
            class_id: "ancestor".into(),
            definition: PropertyDef {
                id: "custom".into(),
                name: "Custom".into(),
                value_type: PropertyType::Text,
                required: false,
                default_value: None,
            },
        }],
    );
    let metric = receipt
        .performance
        .scans
        .iter()
        .find(|metric| metric.scan_name == "class_descendant_property_lookup")
        .expect("class descendant traversal must be instrumented");
    assert!(
        metric.reason.contains("2 descendant class records"),
        "{}",
        metric.reason
    );
    assert!(!metric.full_document);
    assert_eq!(receipt.performance.core_scan_count, 0);

    let definition = PropertyDef {
        id: "conflict".into(),
        name: "Conflict".into(),
        value_type: PropertyType::Text,
        required: false,
        default_value: None,
    };
    let leaf_receipt = apply(
        &mut d,
        "leaf-property",
        vec![Command::DefineProperty {
            class_id: "grandchild".into(),
            definition: definition.clone(),
        }],
    );
    assert!(
        leaf_receipt
            .performance
            .scans
            .iter()
            .any(
                |metric| metric.scan_name == "class_descendant_property_lookup"
                    && metric.reason.contains("0 descendant class records")
            )
    );
    assert_eq!(leaf_receipt.performance.core_scan_count, 0);
    let before = d.to_json_string().unwrap();
    let transaction =
        d.begin_transaction("ancestor-conflict", d.revision())
            .with(Command::DefineProperty {
                class_id: "ancestor".into(),
                definition,
            });
    assert!(matches!(
        d.commit(transaction),
        Err(CoreError::PropertyAlreadyDefined(_))
    ));
    assert_eq!(d.to_json_string().unwrap(), before);
    d.undo().unwrap();
    assert!(!d.property_definition_in_descendants(&"ancestor".into(), &"conflict".into()));
    d.redo().unwrap();
    assert!(d.property_definition_in_descendants(&"ancestor".into(), &"conflict".into()));
    d.verify_index_consistency().unwrap();
}

fn wall(name: &str) -> Command {
    Command::CreateElement {
        name: name.into(),
        class_id: BUILTIN_WALL_CLASS_ID.into(),
        kind: ElementKind::Wall(WallSpec {
            start: Point2::new(0., 0.),
            end: Point2::new(8., 0.),
            base_elevation_m: 0.,
            height_m: 3.,
            thickness_m: 0.2,
        }),
        properties: BTreeMap::new(),
    }
}
fn number(value: f64) -> PropertyValue {
    PropertyValue::Number {
        value,
        unit: Unit::Meter,
    }
}
fn apply(d: &mut Document, name: &str, commands: Vec<Command>) -> CommitReceipt {
    let mut t = d.begin_transaction(name, d.revision());
    for c in commands {
        t.push(c);
    }
    d.commit(t).unwrap()
}

#[test]
fn v04_local_update_on_10k_document_has_bounded_journal_and_zero_full_scans() {
    let mut d = Document::new("10k bounded Core").unwrap();
    let seed = apply(
        &mut d,
        "seed",
        (0..10_000).map(|i| wall(&format!("Wall {i}"))).collect(),
    );
    let id = seed.created_elements[5_000];
    let receipt = apply(
        &mut d,
        "local",
        vec![Command::SetProperty {
            element_id: id,
            property_id: "height".into(),
            value: number(3.25),
        }],
    );
    assert_eq!(receipt.performance.core_scan_count, 0);
    assert!(
        receipt.performance.elements_scanned < 32,
        "{:?}",
        receipt.performance
    );
    assert_eq!(receipt.performance.journal_entry_count, 2);
    assert!(receipt.snapshot_serialized_bytes < 8_000);
    assert_eq!(receipt.delta.updated_elements, vec![id]);
    assert_eq!(d.history_diff(1, 2).unwrap().modified, vec![id]);
    d.verify_index_consistency().unwrap();
}

#[test]
fn v04_checkpoint_reconstruction_retains_sources_types_levels_graph_and_operations() {
    let mut d = Document::new("Checkpoint sources").unwrap();
    d.set_history_policy(HistoryPolicy::compact(4)).unwrap();
    let seed = apply(
        &mut d,
        "seed",
        vec![
            wall("Wall"),
            Command::CreateType {
                name: "Type".into(),
                class_id: BUILTIN_WALL_CLASS_ID.into(),
                properties: [("height".into(), number(3.))].into_iter().collect(),
            },
            Command::CreateLevel {
                name: "L1".into(),
                elevation_m: 0.,
                height_m: None,
            },
        ],
    );
    let id = seed.created_elements[0];
    let type_id = seed.created_types[0];
    let level = seed.created_levels[0];
    apply(
        &mut d,
        "assign",
        vec![
            Command::AssignType {
                element_id: id,
                type_id,
            },
            Command::AssignLevel {
                element_id: id,
                level_id: level,
            },
        ],
    );
    apply(
        &mut d,
        "override",
        vec![Command::SetProperty {
            element_id: id,
            property_id: "height".into(),
            value: number(3.),
        }],
    );
    apply(
        &mut d,
        "checkpoint",
        vec![Command::ClearPropertyOverride {
            element_id: id,
            property_id: "height".into(),
        }],
    );
    apply(
        &mut d,
        "change-type",
        vec![Command::UpdateType {
            type_id,
            name: None,
            properties: [("height".into(), number(3.5))].into_iter().collect(),
        }],
    );
    let old = d.replay_revision(3).unwrap();
    assert_eq!(
        old.resolved_properties(id).unwrap()[&"height".into()].source,
        PropertySource::InstanceOverride
    );
    assert_eq!(old.element_level(id), Some(level));
    assert_eq!(old.type_instances(type_id), vec![id]);
    assert_eq!(old.operation_tree().len(), seed.operation_ids.len() + 3);
    let checkpoint = d.replay_revision(4).unwrap();
    assert_eq!(
        checkpoint.resolved_properties(id).unwrap()[&"height".into()].source,
        PropertySource::Type
    );
    assert_eq!(d.history_diff(3, 4).unwrap().modified, vec![id]);
    let loaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(
        loaded.history_diff(2, 5).unwrap(),
        d.history_diff(2, 5).unwrap()
    );
    loaded.verify_index_consistency().unwrap();
    assert_eq!(
        loaded.history_delta(3).unwrap().property_changes[0].source_after,
        Some(PropertySource::InstanceOverride)
    );
}

#[test]
fn v04_thousand_local_edits_undo_redo_reopen_and_receipts_survive() {
    let mut d = Document::new("1000 edits").unwrap();
    let seed = apply(&mut d, "seed", vec![wall("Wall")]);
    let id = seed.created_elements[0];
    for i in 0..1000 {
        apply(
            &mut d,
            &format!("edit-{i}"),
            vec![Command::SetProperty {
                element_id: id,
                property_id: "height".into(),
                value: number(3. + i as f64 / 1000.),
            }],
        );
    }
    let metrics = d.history_metrics().unwrap();
    assert_eq!(metrics.history_entry_count, 1001);
    assert!(metrics.checkpoint_count < 40);
    assert!(metrics.delta_count > 950);
    let mut d = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    for _ in 0..1000 {
        d.undo().unwrap().unwrap();
    }
    assert!(
        !d.element(id)
            .unwrap()
            .properties
            .contains_key(&"height".into())
    );
    let revision = d.revision();
    let retry = d
        .commit(Transaction::new("edit-500", 0).with(Command::SetProperty {
            element_id: id,
            property_id: "height".into(),
            value: number(3.5),
        }))
        .unwrap();
    assert!(retry.replayed);
    assert_eq!(d.revision(), revision);
    for _ in 0..1000 {
        d.redo().unwrap().unwrap();
    }
    assert_eq!(
        d.resolved_properties(id).unwrap()[&"height".into()].value,
        number(3.999)
    );
    assert!(d.allocation_preview().element_id.get() > id.get());
    d.verify_index_consistency().unwrap();
}

#[test]
fn v04_failed_composite_creation_restores_indexes_counters_history_and_dirty_state() {
    let mut d = Document::new("Composite failure").unwrap();
    apply(&mut d, "seed", vec![wall("Wall")]);
    let before = d.to_json_string().unwrap();
    let counters = d.allocation_preview();
    let tx = d
        .begin_transaction("partial", d.revision())
        .with(Command::CreateOpening {
            name: "Opening".into(),
            spec: OpeningSpec {
                center: Point2::new(2., 0.),
                elevation_m: 0.5,
                width_m: 1.,
                height_m: 1.,
                depth_m: 0.3,
                rotation_rad: 0.,
            },
            properties: BTreeMap::new(),
            host_id: ElementId::new(99),
            type_id: None,
        });
    assert!(d.commit(tx).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
    assert_eq!(d.allocation_preview(), counters);
    d.verify_index_consistency().unwrap();
}

#[test]
fn v04_index_changes_are_transactional_through_type_level_relationship_delete_and_history() {
    let mut d = Document::new("Index mutations").unwrap();
    let seed = apply(
        &mut d,
        "seed",
        vec![
            wall("A"),
            wall("B"),
            Command::CreateType {
                name: "Type".into(),
                class_id: BUILTIN_WALL_CLASS_ID.into(),
                properties: BTreeMap::new(),
            },
            Command::CreateLevel {
                name: "L".into(),
                elevation_m: 0.,
                height_m: None,
            },
        ],
    );
    let a = seed.created_elements[0];
    let b = seed.created_elements[1];
    let t = seed.created_types[0];
    let l = seed.created_levels[0];
    let receipt = apply(
        &mut d,
        "links",
        vec![
            Command::AssignType {
                element_id: a,
                type_id: t,
            },
            Command::AssignLevel {
                element_id: a,
                level_id: l,
            },
            Command::CreateRelationship {
                relationship_type: RelationshipType::Contains,
                source_id: EntityRef::Element(a),
                target_id: EntityRef::Element(b),
                metadata: BTreeMap::new(),
            },
        ],
    );
    assert_eq!(receipt.performance.core_scan_count, 0);
    d.verify_index_consistency().unwrap();
    assert_eq!(d.type_instances(t), vec![a]);
    assert_eq!(d.level_elements(l), vec![a]);
    assert_eq!(d.dependencies(a), vec![b]);
    d.undo().unwrap();
    d.verify_index_consistency().unwrap();
    assert!(d.type_instances(t).is_empty());
    assert!(d.dependencies(a).is_empty());
    d.redo().unwrap();
    d.verify_index_consistency().unwrap();
    apply(
        &mut d,
        "delete",
        vec![Command::DeleteElementWithStrategy {
            element_id: a,
            strategy: DeleteStrategy::Cascade,
        }],
    );
    assert!(d.class_elements(&BUILTIN_WALL_CLASS_ID.into()).is_empty());
    d.verify_index_consistency().unwrap();
    d.undo().unwrap();
    assert_eq!(d.class_elements(&BUILTIN_WALL_CLASS_ID.into()), vec![a, b]);
    d.verify_index_consistency().unwrap();
}

#[test]
fn v04_space_entity_optional_boundary_metadata_and_explicit_relationships_round_trip() {
    let mut d = Document::new("Spaces").unwrap();
    let seed = apply(
        &mut d,
        "seed",
        vec![
            wall("Wall"),
            Command::CreateLevel {
                name: "Ground".into(),
                elevation_m: 0.,
                height_m: None,
            },
        ],
    );
    let wall = seed.created_elements[0];
    let level = seed.created_levels[0];
    let s = apply(
        &mut d,
        "space",
        vec![Command::CreateSpace {
            name: "Room 101".into(),
            level_id: Some(level),
            boundary: Some(vec![
                Point2::new(0., 0.),
                Point2::new(4., 0.),
                Point2::new(4., 4.),
                Point2::new(0., 4.),
            ]),
            metadata: [("usage".into(), serde_json::json!("office"))]
                .into_iter()
                .collect(),
        }],
    )
    .created_elements[0];
    apply(
        &mut d,
        "relations",
        vec![
            Command::CreateRelationship {
                relationship_type: RelationshipType::BoundsSpace,
                source_id: EntityRef::Element(wall),
                target_id: EntityRef::Element(s),
                metadata: BTreeMap::new(),
            },
            Command::CreateRelationship {
                relationship_type: RelationshipType::ContainedInSpace,
                source_id: EntityRef::Element(wall),
                target_id: EntityRef::Element(s),
                metadata: BTreeMap::new(),
            },
        ],
    );
    let d = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(d.element_level(s), Some(level));
    assert_eq!(
        d.element(s).unwrap().kind.category(),
        ElementCategory::Space
    );
    assert_eq!(d.class_elements(&BUILTIN_SPACE_CLASS_ID.into()), vec![s]);
    if let ElementKind::Space(spec) = &d.element(s).unwrap().kind {
        assert_eq!(spec.metadata["usage"], "office");
    } else {
        panic!("expected Space");
    }
    assert_eq!(d.relationships_to(EntityRef::Element(s)).len(), 2);
}

#[test]
fn v04_invalid_polygon_and_space_boundaries_are_atomic() {
    let mut d = Document::new("Invalid contours").unwrap();
    let host = apply(&mut d, "seed", vec![wall("Wall")]).created_elements[0];
    let before = d.to_json_string().unwrap();
    let ring = vec![
        Point2::new(0., 0.),
        Point2::new(1., 1.),
        Point2::new(0., 1.),
        Point2::new(1., 0.),
    ];
    assert!(
        d.commit(d.begin_transaction("polygon", d.revision()).with(
            Command::CreatePolygonalOpening {
                name: "Void".into(),
                spec: PolygonalOpeningSpec {
                    center: Point2::new(2., 0.),
                    elevation_m: 1.,
                    boundary: ring.clone(),
                    depth_m: 0.3,
                    rotation_rad: 0.
                },
                properties: BTreeMap::new(),
                host_id: host,
                type_id: None
            }
        ))
        .is_err()
    );
    assert!(
        d.commit(
            d.begin_transaction("space", d.revision())
                .with(Command::CreateSpace {
                    name: "Invalid space".into(),
                    level_id: None,
                    boundary: Some(ring),
                    metadata: BTreeMap::new()
                })
        )
        .is_err()
    );
    assert_eq!(d.to_json_string().unwrap(), before);
}

#[test]
fn v04_full_checkpoint_policy_is_configurable_without_losing_previous_deltas() {
    let mut d = Document::new("Policy").unwrap();
    let id = apply(&mut d, "seed", vec![wall("Wall")]).created_elements[0];
    apply(
        &mut d,
        "a",
        vec![Command::SetProperty {
            element_id: id,
            property_id: "height".into(),
            value: number(3.1),
        }],
    );
    d.set_history_policy(HistoryPolicy::full_snapshot())
        .unwrap();
    apply(
        &mut d,
        "b",
        vec![Command::SetProperty {
            element_id: id,
            property_id: "height".into(),
            value: number(3.2),
        }],
    );
    d.set_history_policy(HistoryPolicy::compact(8)).unwrap();
    apply(
        &mut d,
        "c",
        vec![Command::SetProperty {
            element_id: id,
            property_id: "height".into(),
            value: number(3.3),
        }],
    );
    assert_eq!(d.history_metrics().unwrap().checkpoint_count, 2);
    assert_eq!(d.history_diff(1, 4).unwrap().modified, vec![id]);
    assert_eq!(
        d.replay_revision(2)
            .unwrap()
            .resolved_properties(id)
            .unwrap()[&"height".into()]
            .value,
        number(3.1)
    );
    assert!(d.set_history_policy(HistoryPolicy::compact(0)).is_err());
}

#[test]
fn v04_deleted_branch_ids_and_receipts_survive_checkpoint_and_save() {
    let mut d = Document::new("Retained branch IDs").unwrap();
    d.set_history_policy(HistoryPolicy::compact(2)).unwrap();
    let old = apply(&mut d, "old", vec![wall("Old")]);
    let old_id = old.created_elements[0];
    d.undo().unwrap();
    let new = apply(&mut d, "new", vec![wall("New")]);
    assert!(new.created_elements[0] > old_id);
    assert!(d.redo().unwrap().is_none());
    let mut d = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    let revision = d.revision();
    let receipt = d
        .commit(Transaction::new("old", 0).with(wall("Old")))
        .unwrap();
    assert!(receipt.replayed);
    assert_eq!(d.revision(), revision);
    assert_eq!(d.history_diff(1, 2).unwrap().deleted, vec![old_id]);
    let mut json = serde_json::to_value(&d).unwrap();
    json["next_element_id"] = old_id.get().into();
    assert!(Document::from_json_str(&serde_json::to_string(&json).unwrap()).is_err());
}

#[test]
fn v04_legacy_replay_does_not_import_future_classes_or_operations() {
    let mut d = Document::new("Legacy replay").unwrap();
    apply(&mut d, "wall", vec![wall("Wall")]);
    // Model legacy snapshots that predate retained raw class/operation maps.
    for state in d.revision_states.values_mut() {
        state.classes = None;
        state.operations = None;
    }
    apply(
        &mut d,
        "future-class",
        vec![Command::DefineClass(ClassDef::new(
            "future",
            "Future",
            ElementCategory::Generic,
        ))],
    );
    assert!(matches!(
        d.replay_revision(1),
        Err(CoreError::UnknownRevision(1))
    ));
    assert!(d.history_diff(0, 1).is_ok());
}

#[test]
fn v04_current_checkpoint_classes_and_operations_cannot_be_fabricated() {
    let mut d = Document::new("Checkpoint integrity").unwrap();
    d.set_history_policy(HistoryPolicy::full_snapshot())
        .unwrap();
    apply(&mut d, "wall", vec![wall("Wall")]);
    let mut value = serde_json::to_value(&d).unwrap();
    value["revision_states"]["1"]["classes"][BUILTIN_WALL_CLASS_ID]["name"] = "Fabricated".into();
    assert!(Document::from_json_str(&serde_json::to_string(&value).unwrap()).is_err());
    let mut value = serde_json::to_value(&d).unwrap();
    value["revision_states"]["1"]["operations"]["1"]["label"] = "Fabricated".into();
    assert!(Document::from_json_str(&serde_json::to_string(&value).unwrap()).is_err());
}

#[test]
fn v04_incomplete_delta_summaries_sources_and_undo_journals_are_rejected() {
    let mut d = Document::new("History corruption").unwrap();
    d.set_history_policy(HistoryPolicy::compact(3)).unwrap();
    let seed = apply(
        &mut d,
        "seed",
        vec![
            wall("Wall"),
            wall("Child"),
            Command::CreateType {
                name: "Type".into(),
                class_id: BUILTIN_WALL_CLASS_ID.into(),
                properties: [("height".into(), number(3.))].into_iter().collect(),
            },
        ],
    );
    let wall = seed.created_elements[0];
    let child = seed.created_elements[1];
    let type_id = seed.created_types[0];
    let relationship = apply(
        &mut d,
        "relations",
        vec![
            Command::AssignType {
                element_id: wall,
                type_id,
            },
            Command::CreateRelationship {
                relationship_type: RelationshipType::Contains,
                source_id: EntityRef::Element(wall),
                target_id: EntityRef::Element(child),
                metadata: BTreeMap::new(),
            },
        ],
    )
    .created_relationships[1];
    apply(
        &mut d,
        "type-change-at-checkpoint",
        vec![Command::UpdateType {
            type_id,
            name: None,
            properties: [("height".into(), number(3.5))].into_iter().collect(),
        }],
    );
    apply(
        &mut d,
        "remove",
        vec![Command::RemoveRelationship {
            relationship_id: relationship,
        }],
    );
    let source = serde_json::to_value(&d).unwrap();
    let mut value = source.clone();
    value["revision_states"]["4"]["delta"]["summary"]["deleted_relationships"] =
        serde_json::json!([]);
    assert!(Document::from_json_str(&serde_json::to_string(&value).unwrap()).is_err());
    let mut value = source.clone();
    value["revision_states"]["3"]["delta"]["summary"]["affected_elements"] = serde_json::json!([]);
    assert!(Document::from_json_str(&serde_json::to_string(&value).unwrap()).is_err());
    let mut value = source.clone();
    value["revision_states"]["3"]["delta"]["summary"]["property_changes"] = serde_json::json!([]);
    assert!(Document::from_json_str(&serde_json::to_string(&value).unwrap()).is_err());
    let mut value = source.clone();
    value["revision_states"]["2"]["property_sources"][wall.to_string()]["height"] =
        "instance_override".into();
    assert!(Document::from_json_str(&serde_json::to_string(&value).unwrap()).is_err());
    let mut value = source.clone();
    let changes = value["undo_stack"][2]["changes"].as_array_mut().unwrap();
    let change = changes
        .iter_mut()
        .find(|c| c.get("Type").is_some())
        .unwrap();
    change["Type"]["before"]["name"] = "Fabricated".into();
    assert!(Document::from_json_str(&serde_json::to_string(&value).unwrap()).is_err());
}

#[test]
fn v04_historical_allocator_high_water_cannot_move_backward() {
    let mut d = Document::new("Counter chronology").unwrap();
    let seed = apply(
        &mut d,
        "seed",
        vec![
            wall("A"),
            wall("B"),
            Command::CreateType {
                name: "Type".into(),
                class_id: BUILTIN_WALL_CLASS_ID.into(),
                properties: BTreeMap::new(),
            },
            Command::CreateLevel {
                name: "Level".into(),
                elevation_m: 0.,
                height_m: None,
            },
        ],
    );
    apply(
        &mut d,
        "link",
        vec![Command::CreateRelationship {
            relationship_type: RelationshipType::Contains,
            source_id: EntityRef::Element(seed.created_elements[0]),
            target_id: EntityRef::Element(seed.created_elements[1]),
            metadata: BTreeMap::new(),
        }],
    );
    apply(
        &mut d,
        "class-only",
        vec![Command::DefineClass(ClassDef::new(
            "custom",
            "Custom",
            ElementCategory::Generic,
        ))],
    );
    let source = serde_json::to_value(&d).unwrap();
    for family in ["element", "element_type", "level", "relationship"] {
        let mut value = source.clone();
        value["revision_states"]["3"]["delta"]["counters"][family] = 1.into();
        let error = Document::from_json_str(&serde_json::to_string(&value).unwrap()).unwrap_err();
        assert!(
            error.to_string().contains("high-water"),
            "{family}: {error}"
        );
    }
    let mut value = source;
    for revision in ["1", "2", "3"] {
        value["revision_states"][revision]["delta"]["counters"]["element"] = 100.into();
    }
    let error = Document::from_json_str(&serde_json::to_string(&value).unwrap()).unwrap_err();
    assert!(error.to_string().contains("high-water"));

    let mut forged = Document::new("Reused branch ID").unwrap();
    apply(&mut forged, "original", vec![wall("Original")]);
    forged.undo().unwrap();
    // Simulate a corrupted producer: its final counter is still two, but the
    // new branch reuses the original element's ID below the prior high-water.
    forged.next_element_id = 1;
    apply(&mut forged, "reused", vec![wall("Reused")]);
    let error = Document::from_json_str(&serde_json::to_string(&forged).unwrap()).unwrap_err();
    assert!(error.to_string().contains("high-water"));
}

#[test]
fn v04_persisted_undo_redo_stack_order_and_membership_match_journal_timeline() {
    let mut d = Document::new("Stack chronology").unwrap();
    apply(&mut d, "a", vec![wall("A")]);
    apply(&mut d, "b", vec![wall("B")]);
    apply(&mut d, "c", vec![wall("C")]);
    let source = serde_json::to_value(&d).unwrap();
    let mut value = source.clone();
    value["undo_stack"].as_array_mut().unwrap().swap(0, 1);
    assert!(Document::from_json_str(&serde_json::to_string(&value).unwrap()).is_err());
    let mut value = source;
    value["undo_stack"].as_array_mut().unwrap().remove(0);
    assert!(Document::from_json_str(&serde_json::to_string(&value).unwrap()).is_err());
    d.undo().unwrap();
    d.undo().unwrap();
    let mut value = serde_json::to_value(&d).unwrap();
    value["redo_stack"].as_array_mut().unwrap().swap(0, 1);
    assert!(Document::from_json_str(&serde_json::to_string(&value).unwrap()).is_err());
    let mut reopened = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(reopened.redo().unwrap().unwrap().command_id, "b".into());
    assert_eq!(reopened.redo().unwrap().unwrap().command_id, "c".into());
}

#[test]
fn v04_legacy_stack_anchor_preserves_original_undo_redo_and_replay() {
    let mut original = Document::new("Legacy stack anchor").unwrap();
    original
        .set_history_policy(HistoryPolicy::full_snapshot())
        .unwrap();
    apply(&mut original, "a", vec![wall("A")]);
    apply(&mut original, "b", vec![wall("B")]);
    original.undo().unwrap();
    // Legacy formats have complete snapshots but no raw revision journals.
    original.semantic_schema = 2;
    for state in original.revision_states.values_mut() {
        state.delta = None;
        state.classes = None;
        state.operations = None;
        state.relationship_records = None;
    }
    let mut imported = Document::from_json_str(&serde_json::to_string(&original).unwrap()).unwrap();
    let anchor_revision = imported.revision();
    assert!(
        imported.revision_states[&anchor_revision]
            .history_stack_anchor
            .is_some()
    );
    assert_eq!(imported.redo().unwrap().unwrap().command_id, "b".into());
    assert_eq!(imported.undo().unwrap().unwrap().command_id, "b".into());
    let mut replay = imported.replay_revision(anchor_revision).unwrap();
    assert_eq!(replay.redo().unwrap().unwrap().command_id, "b".into());
    let mut reopened = Document::from_json_str(&imported.to_json_string().unwrap()).unwrap();
    assert_eq!(reopened.undo().unwrap().unwrap().command_id, "a".into());
    assert_eq!(reopened.redo().unwrap().unwrap().command_id, "a".into());
    assert_eq!(reopened.redo().unwrap().unwrap().command_id, "b".into());
}

#[test]
fn v04_partial_but_semantically_consistent_undo_redo_journals_are_rejected() {
    for redo in [false, true] {
        let mut d = Document::new("Partial history journal").unwrap();
        let seed = apply(&mut d, "batch", vec![wall("A"), wall("B")]);
        if redo {
            d.undo().unwrap();
        }
        let stack = if redo {
            &mut d.redo_stack
        } else {
            &mut d.undo_stack
        };
        let original = stack.last().unwrap().clone();
        // A corrupt producer performs only one member of a two-wall batch,
        // but retains the correct full stack entry. Its raw snapshots, delta,
        // sources and current semantic state remain internally consistent.
        stack
            .last_mut()
            .unwrap()
            .changes
            .retain(|change| match change {
                Change::Element { id, .. } => *id == seed.created_elements[0],
                Change::Operation { id, .. } => *id == seed.operation_ids[0],
                _ => false,
            });
        if redo {
            d.redo().unwrap();
            *d.undo_stack.last_mut().unwrap() = original;
        } else {
            d.undo().unwrap();
            *d.redo_stack.last_mut().unwrap() = original;
        }
        assert_eq!(d.elements.len(), 1);
        let error = Document::from_json_str(&serde_json::to_string(&d).unwrap()).unwrap_err();
        assert!(error.to_string().contains("journal"), "{redo}: {error}");
    }
}

#[test]
fn v04_replayed_document_preserves_prefix_history_undo_redo_tombstones_and_high_water() {
    let mut d = Document::new("Replay lifecycle").unwrap();
    let seed = apply(&mut d, "seed", vec![wall("A"), wall("B")]);
    let a = seed.created_elements[0];
    let b = seed.created_elements[1];
    let relationship = apply(
        &mut d,
        "link",
        vec![Command::CreateRelationship {
            relationship_type: RelationshipType::Contains,
            source_id: EntityRef::Element(a),
            target_id: EntityRef::Element(b),
            metadata: BTreeMap::new(),
        }],
    )
    .created_relationships[0];
    apply(
        &mut d,
        "remove",
        vec![Command::RemoveRelationship {
            relationship_id: relationship,
        }],
    );
    let removed = d.revision();
    d.undo().unwrap();
    let undone = d.revision();
    d.redo().unwrap();
    let mut replay = d.replay_revision(removed).unwrap();
    assert!(!replay.relationship(relationship).unwrap().is_active());
    assert_eq!(
        replay.history_diff(2, 3).unwrap().relationships_deleted,
        vec![relationship]
    );
    replay.undo().unwrap();
    assert!(replay.relationship(relationship).unwrap().is_active());
    replay.redo().unwrap();
    let mut replay = d.replay_revision(undone).unwrap();
    assert!(replay.relationship(relationship).unwrap().is_active());
    replay.redo().unwrap();
    assert!(!replay.relationship(relationship).unwrap().is_active());
    let reloaded = Document::from_json_str(&replay.to_json_string().unwrap()).unwrap();
    assert_eq!(
        reloaded.allocation_preview().relationship_id,
        d.allocation_preview().relationship_id
    );
    reloaded.verify_index_consistency().unwrap();
}

#[test]
fn v04_actual_v01_v02_v03_native_fixtures_open_edit_undo_redo_and_reopen_without_file_changes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native-baseline");
    for relative in [
        "example.ai-revit.json",
        "v02/acceptance.ai-revit.json",
        "v03/acceptance.ai-revit.json",
    ] {
        let path = root.join(relative);
        let bytes = std::fs::read(&path).unwrap();
        let mut d = Document::from_json_str(std::str::from_utf8(&bytes).unwrap()).unwrap();
        d.verify_index_consistency().unwrap();
        let (id, height) = d
            .elements()
            .find_map(|e| {
                if let ElementKind::Wall(spec) = &e.kind {
                    Some((e.id, spec.height_m))
                } else {
                    None
                }
            })
            .unwrap();
        let revision = d.revision();
        apply(
            &mut d,
            "v04-legacy-local-edit",
            vec![Command::SetProperty {
                element_id: id,
                property_id: "height".into(),
                value: number(height + 0.1),
            }],
        );
        assert!(
            d.history_diff(revision, d.revision())
                .unwrap()
                .modified
                .contains(&id),
            "{relative}"
        );
        d.undo().unwrap();
        d.redo().unwrap();
        let d = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
        d.verify_index_consistency().unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "historical fixture changed: {relative}"
        );
    }
}

#[test]
fn v04_polygon_host_local_placement_and_through_depth_are_validated_atomically() {
    let hosts = vec![
        (
            BUILTIN_WALL_CLASS_ID,
            ElementKind::Wall(WallSpec {
                start: Point2::new(0., 0.),
                end: Point2::new(4., 4.),
                base_elevation_m: 0.,
                height_m: 3.,
                thickness_m: 0.2,
            }),
            PolygonalOpeningSpec {
                center: Point2::new(2., 2.),
                elevation_m: 0.5,
                boundary: vec![
                    Point2::new(-0.5, 0.),
                    Point2::new(0.5, 0.),
                    Point2::new(0.5, 1.),
                    Point2::new(-0.5, 1.),
                ],
                depth_m: 0.3,
                rotation_rad: std::f64::consts::FRAC_PI_4,
            },
        ),
        (
            BUILTIN_FLOOR_CLASS_ID,
            ElementKind::Floor(FloorSpec {
                boundary: vec![
                    Point2::new(-5., -5.),
                    Point2::new(5., -5.),
                    Point2::new(5., 5.),
                    Point2::new(-5., 5.),
                ],
                holes: vec![],
                elevation_m: 3.,
                thickness_m: 0.2,
            }),
            PolygonalOpeningSpec {
                center: Point2::new(0., 0.),
                elevation_m: 2.7,
                boundary: vec![
                    Point2::new(-0.5, -0.5),
                    Point2::new(0.5, -0.5),
                    Point2::new(0.5, 0.5),
                    Point2::new(-0.5, 0.5),
                ],
                depth_m: 0.3,
                rotation_rad: 0.42,
            },
        ),
        (
            BUILTIN_ROOF_CLASS_ID,
            ElementKind::Roof(RoofSpec {
                footprint: vec![
                    Point2::new(-5., -5.),
                    Point2::new(5., -5.),
                    Point2::new(5., 5.),
                    Point2::new(-5., 5.),
                ],
                elevation_m: 3.,
                thickness_m: 0.2,
                slope_x: 0.15,
                slope_y: 0.1,
            }),
            PolygonalOpeningSpec {
                center: Point2::new(1., 1.),
                elevation_m: 3.,
                boundary: vec![
                    Point2::new(-0.5, -0.5),
                    Point2::new(0.5, -0.5),
                    Point2::new(0.5, 0.5),
                    Point2::new(-0.5, 0.5),
                ],
                depth_m: 0.3,
                rotation_rad: 0.42,
            },
        ),
    ];
    for (class_id, kind, spec) in hosts {
        let mut d = Document::new("Polygon host placement").unwrap();
        let host = apply(
            &mut d,
            "host",
            vec![Command::CreateElement {
                name: "Host".into(),
                class_id: class_id.into(),
                kind,
                properties: BTreeMap::new(),
            }],
        )
        .created_elements[0];
        let opening = apply(
            &mut d,
            "opening",
            vec![Command::CreatePolygonalOpening {
                name: "Void".into(),
                spec: spec.clone(),
                properties: BTreeMap::new(),
                host_id: host,
                type_id: None,
            }],
        )
        .created_elements[0];
        assert_eq!(d.hosted_opening_ids(host), vec![opening]);
        let before = d.to_json_string().unwrap();
        let mut shallow = spec.clone();
        shallow.depth_m = 0.01;
        let error = d
            .commit(
                d.begin_transaction("shallow", d.revision())
                    .with(Command::UpdateElement {
                        element_id: opening,
                        kind: ElementKind::PolygonalOpening(shallow),
                    }),
            )
            .unwrap_err();
        assert!(error.to_string().contains("void_depth_insufficient"));
        assert_eq!(d.to_json_string().unwrap(), before);
        let mut outside = spec;
        outside.center.x = 50.;
        assert!(
            d.commit(
                d.begin_transaction("outside", d.revision())
                    .with(Command::UpdateElement {
                        element_id: opening,
                        kind: ElementKind::PolygonalOpening(outside)
                    })
            )
            .is_err()
        );
        assert_eq!(d.to_json_string().unwrap(), before);
        d.verify_index_consistency().unwrap();
    }
}

#[test]
fn v04_direct_serde_lazy_index_initialization_is_visible_in_mutation_metrics() {
    let mut d = Document::new("Lazy serde indexes").unwrap();
    let id = apply(&mut d, "seed", vec![wall("Wall")]).created_elements[0];
    let mut d: Document = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
    let first = apply(
        &mut d,
        "first",
        vec![Command::SetProperty {
            element_id: id,
            property_id: "height".into(),
            value: number(3.1),
        }],
    );
    assert!(
        first
            .performance
            .scans
            .iter()
            .any(|s| s.scan_name == "runtime_index_build" && s.full_document)
    );
    assert!(first.performance.core_scan_count > 0);
    let second = apply(
        &mut d,
        "second",
        vec![Command::SetProperty {
            element_id: id,
            property_id: "height".into(),
            value: number(3.2),
        }],
    );
    assert_eq!(second.performance.core_scan_count, 0);
    d.verify_index_consistency().unwrap();
}

#[test]
fn v04_property_delta_retains_old_and_new_type_source_identity() {
    let mut d = Document::new("Type source identity").unwrap();
    let seed = apply(
        &mut d,
        "seed",
        vec![
            wall("Wall"),
            Command::CreateType {
                name: "A".into(),
                class_id: BUILTIN_WALL_CLASS_ID.into(),
                properties: [("height".into(), number(3.))].into_iter().collect(),
            },
            Command::CreateType {
                name: "B".into(),
                class_id: BUILTIN_WALL_CLASS_ID.into(),
                properties: [("height".into(), number(3.))].into_iter().collect(),
            },
        ],
    );
    let wall = seed.created_elements[0];
    let a = seed.created_types[0];
    let b = seed.created_types[1];
    apply(
        &mut d,
        "assign-a",
        vec![Command::AssignType {
            element_id: wall,
            type_id: a,
        }],
    );
    let change = apply(
        &mut d,
        "assign-b",
        vec![Command::AssignType {
            element_id: wall,
            type_id: b,
        }],
    );
    let property = change
        .delta
        .property_changes
        .iter()
        .find(|p| p.entity == EntityRef::Element(wall) && p.property == PropertyId::from("height"))
        .unwrap();
    assert_eq!(property.type_id_before, Some(a));
    assert_eq!(property.type_id_after, Some(b));
    assert_eq!(property.before, property.after);
    assert_eq!(property.source_before, Some(PropertySource::Type));
    assert_eq!(property.source_after, Some(PropertySource::Type));
    let loaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(loaded.history_delta(change.revision).unwrap(), change.delta);
}
