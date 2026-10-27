use super::*;

#[test]
fn allocation_preview_is_read_only_and_does_not_reuse_undone_ids() {
    let mut d = Document::new("Allocation quote").unwrap();
    let quote = d.allocation_preview();
    assert_eq!(quote, d.allocation_preview());
    assert_eq!(quote.revision, 0);
    assert_eq!(quote.element_id, ElementId::new(1));
    let first = execute(&mut d, "allocated", vec![wall("First")]).created_elements[0];
    assert_eq!(first, quote.element_id);
    d.undo().unwrap();
    let next = d.allocation_preview();
    assert_eq!(next.element_id, ElementId::new(2));
    let stale = d
        .begin_transaction("stale-quote", quote.revision)
        .with(wall("Stale"));
    assert!(matches!(
        d.commit(stale),
        Err(CoreError::RevisionConflict { .. })
    ));
    assert_eq!(d.allocation_preview(), next);
    let second = execute(&mut d, "allocated-after-undo", vec![wall("Second")]).created_elements[0];
    assert_eq!(second, next.element_id);
}

#[test]
fn relationship_json_uses_requested_id_name_and_accepts_legacy_name() {
    let relationship = Relationship {
        id: RelationshipId::new(42),
        relationship_type: RelationshipType::ConnectsTo,
        source_id: EntityRef::Element(ElementId::new(1)),
        target_id: EntityRef::Element(ElementId::new(2)),
        metadata: BTreeMap::new(),
        revision_created: 7,
        revision_deleted: None,
    };
    let mut encoded = serde_json::to_value(&relationship).unwrap();
    assert_eq!(encoded["relationship_id"], 42);
    assert!(encoded.get("id").is_none());
    let object = encoded.as_object_mut().unwrap();
    let id = object.remove("relationship_id").unwrap();
    object.insert("id".into(), id);
    assert_eq!(
        serde_json::from_value::<Relationship>(encoded).unwrap(),
        relationship
    );
}

fn execute(document: &mut Document, id: &str, commands: Vec<Command>) -> CommitReceipt {
    let mut tx = document.begin_transaction(id, document.revision());
    for command in commands {
        tx.push(command);
    }
    document.commit(tx).unwrap()
}
fn number(value: f64) -> PropertyValue {
    PropertyValue::Number {
        value,
        unit: Unit::Meter,
    }
}
fn props(key: &str, value: f64) -> BTreeMap<PropertyId, PropertyValue> {
    [(key.into(), number(value))].into_iter().collect()
}
fn wall(name: &str) -> Command {
    Command::CreateElement {
        name: name.to_owned(),
        class_id: BUILTIN_WALL_CLASS_ID.into(),
        kind: ElementKind::Wall(WallSpec {
            start: Point2::new(0.0, 0.0),
            end: Point2::new(6.0, 0.0),
            base_elevation_m: 0.0,
            height_m: 3.0,
            thickness_m: 0.2,
        }),
        properties: BTreeMap::new(),
    }
}
fn rectangular() -> RectangularSpec {
    RectangularSpec {
        center: Point2::new(2.0, 0.0),
        elevation_m: 0.8,
        width_m: 1.2,
        height_m: 1.2,
        depth_m: 0.15,
        rotation_rad: 0.0,
    }
}
fn hosted_document() -> (Document, ElementId, ElementId, ElementId) {
    let mut document = Document::new("BIM graph").unwrap();
    let wall_id = execute(&mut document, "wall", vec![wall("Wall")]).created_elements[0];
    let opening_id = ElementId::new(2);
    let window_id = ElementId::new(3);
    execute(
        &mut document,
        "hosting",
        vec![
            Command::CreateOpening {
                name: "Opening".into(),
                spec: rectangular(),
                properties: BTreeMap::new(),
                host_id: wall_id,
                type_id: None,
            },
            Command::CreateElement {
                name: "Window".into(),
                class_id: BUILTIN_WINDOW_CLASS_ID.into(),
                kind: ElementKind::Window(rectangular()),
                properties: BTreeMap::new(),
            },
            Command::HostElement {
                host_id: opening_id,
                element_id: window_id,
            },
        ],
    );
    (document, wall_id, opening_id, window_id)
}

#[test]
fn type_propagation_three_instances_and_geometry_projection() {
    let mut d = Document::new("Types").unwrap();
    let type_id = execute(
        &mut d,
        "type",
        vec![Command::CreateType {
            name: "Exterior 150".into(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            properties: props("thickness", 0.15),
        }],
    )
    .created_types[0];
    let ids = execute(
        &mut d,
        "walls",
        vec![wall("One"), wall("Two"), wall("Three")],
    )
    .created_elements;
    execute(
        &mut d,
        "assign",
        ids.iter()
            .map(|id| Command::AssignType {
                element_id: *id,
                type_id,
            })
            .collect(),
    );
    let before = d.revision();
    let update = execute(
        &mut d,
        "type-edit",
        vec![Command::UpdateType {
            type_id,
            name: None,
            properties: props("thickness", 0.25),
        }],
    );
    assert_eq!(update.dirty_ids, ids);
    assert_eq!(d.history_diff(before, d.revision()).unwrap().modified, ids);
    for id in ids {
        let resolved = d.resolved_properties(id).unwrap();
        assert_eq!(resolved[&PropertyId::from("thickness")].value, number(0.25));
        assert_eq!(
            resolved[&PropertyId::from("thickness")].source,
            PropertySource::Type
        );
        assert!(d.element(id).unwrap().properties.is_empty());
        assert!(matches!(
            d.effective_element(id).unwrap().kind,
            ElementKind::Wall(WallSpec {
                thickness_m: 0.25,
                ..
            })
        ));
    }
}

#[test]
fn instance_override_survives_type_change_and_clear_restores_inheritance() {
    let mut d = Document::new("Overrides").unwrap();
    let type_id = execute(
        &mut d,
        "type",
        vec![Command::CreateType {
            name: "Wall 2800".into(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            properties: props("height", 2.8),
        }],
    )
    .created_types[0];
    let id = execute(&mut d, "wall", vec![wall("Wall")]).created_elements[0];
    execute(
        &mut d,
        "assign",
        vec![
            Command::AssignType {
                element_id: id,
                type_id,
            },
            Command::SetProperty {
                element_id: id,
                property_id: "height".into(),
                value: number(3.0),
            },
        ],
    );
    execute(
        &mut d,
        "edit",
        vec![Command::UpdateType {
            type_id,
            name: None,
            properties: props("height", 2.9),
        }],
    );
    assert_eq!(
        d.resolved_properties(id).unwrap()[&PropertyId::from("height")].value,
        number(3.0)
    );
    assert_eq!(
        d.resolved_properties(id).unwrap()[&PropertyId::from("height")].source,
        PropertySource::InstanceOverride
    );
    execute(
        &mut d,
        "clear",
        vec![Command::ClearPropertyOverride {
            element_id: id,
            property_id: "height".into(),
        }],
    );
    assert_eq!(
        d.resolved_properties(id).unwrap()[&PropertyId::from("height")].value,
        number(2.9)
    );
}

#[test]
fn hosting_graph_save_reload_and_levels_preserve_explicit_placement() {
    let (mut d, wall_id, opening_id, window_id) = hosted_document();
    let level_id = execute(
        &mut d,
        "level",
        vec![Command::CreateLevel {
            name: "Level 1".into(),
            elevation_m: 10.0,
            height_m: Some(3.0),
        }],
    )
    .created_levels[0];
    execute(
        &mut d,
        "assign-level",
        vec![Command::AssignLevel {
            element_id: wall_id,
            level_id,
        }],
    );
    let reloaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(reloaded.dependencies(wall_id), vec![opening_id, window_id]);
    assert_eq!(reloaded.element_level(wall_id), Some(level_id));
    assert_eq!(reloaded.level_elements(level_id), vec![wall_id]);
    assert!(matches!(
        reloaded.element(wall_id).unwrap().kind,
        ElementKind::Wall(WallSpec {
            base_elevation_m: 0.0,
            ..
        })
    ));
    assert_eq!(
        d.to_json_string().unwrap(),
        reloaded.to_json_string().unwrap()
    );
}

#[test]
fn undo_redo_relationships_restore_exact_graph() {
    let mut d = Document::new("Undo graph").unwrap();
    let ids = execute(&mut d, "walls", vec![wall("One"), wall("Two")]).created_elements;
    let receipt = execute(
        &mut d,
        "connect",
        vec![Command::CreateRelationship {
            relationship_type: RelationshipType::ConnectsTo,
            source_id: EntityRef::Element(ids[0]),
            target_id: EntityRef::Element(ids[1]),
            metadata: BTreeMap::new(),
        }],
    );
    let relationship_id = receipt.created_relationships[0];
    let saved = d.relationship(relationship_id).unwrap().clone();
    d.undo().unwrap();
    assert!(d.relationship(relationship_id).is_none());
    d.redo().unwrap();
    assert_eq!(d.relationship(relationship_id), Some(&saved));
    execute(
        &mut d,
        "remove",
        vec![Command::RemoveRelationship { relationship_id }],
    );
    assert_eq!(d.relationships().count(), 0);
    assert!(
        d.relationship(relationship_id)
            .unwrap()
            .revision_deleted
            .is_some()
    );
    d.undo().unwrap();
    assert_eq!(d.relationship(relationship_id), Some(&saved));
    d.redo().unwrap();
    assert_eq!(d.relationships().count(), 0);
}

#[test]
fn dependency_dirty_propagation_wall_opening_window() {
    let (mut d, wall_id, opening_id, window_id) = hosted_document();
    let receipt = execute(
        &mut d,
        "thicken",
        vec![Command::SetProperty {
            element_id: wall_id,
            property_id: "thickness".into(),
            value: number(0.3),
        }],
    );
    assert_eq!(receipt.affected_ids, vec![wall_id]);
    assert_eq!(receipt.dirty_ids, vec![wall_id, opening_id, window_id]);
    assert_eq!(
        d.dependency_info(window_id).unwrap().direct_dependencies,
        vec![opening_id]
    );
}

#[test]
fn missing_reference_and_batch_failure_are_atomic() {
    let mut d = Document::new("Atomic").unwrap();
    let before = d.to_json_string().unwrap();
    let tx = d
        .begin_transaction("invalid", 0)
        .with(wall("Transient"))
        .with(Command::HostElement {
            host_id: ElementId::new(1),
            element_id: ElementId::new(999),
        });
    assert!(d.commit(tx).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
    assert_eq!(
        execute(&mut d, "valid", vec![wall("Persistent")]).created_elements,
        vec![ElementId::new(1)]
    );
}

#[test]
fn hosting_cycle_self_host_and_multiple_hosts_rejected() {
    let mut d = Document::new("Cycles").unwrap();
    let ids = execute(
        &mut d,
        "walls",
        vec![wall("One"), wall("Two"), wall("Three")],
    )
    .created_elements;
    let before = d.to_json_string().unwrap();
    let cycle = d
        .begin_transaction("cycle", d.revision())
        .with(Command::HostElement {
            host_id: ids[0],
            element_id: ids[1],
        })
        .with(Command::HostElement {
            host_id: ids[1],
            element_id: ids[0],
        });
    assert!(d.commit(cycle).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
    let tx = d
        .begin_transaction("self", d.revision())
        .with(Command::HostElement {
            host_id: ids[0],
            element_id: ids[0],
        });
    assert!(d.commit(tx).is_err());
    let tx = d
        .begin_transaction("two-hosts", d.revision())
        .with(Command::HostElement {
            host_id: ids[0],
            element_id: ids[2],
        })
        .with(Command::HostElement {
            host_id: ids[1],
            element_id: ids[2],
        });
    assert!(d.commit(tx).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
}

#[test]
fn history_diff_matches_batch_and_undo_redo_survives_reload() {
    let (mut d, wall_id, opening_id, window_id) = hosted_document();
    let before = d.revision();
    let receipt = execute(
        &mut d,
        "edit",
        vec![Command::SetProperty {
            element_id: wall_id,
            property_id: "height".into(),
            value: number(3.5),
        }],
    );
    let diff = d.history_diff(before, receipt.revision_after).unwrap();
    assert_eq!(diff.modified, vec![wall_id]);
    assert!(diff.created.is_empty());
    assert!(diff.relationships_created.is_empty());
    let mut reloaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    let undo = reloaded.undo().unwrap().unwrap();
    assert_eq!(undo.dirty_ids, vec![wall_id, opening_id, window_id]);
    reloaded.redo().unwrap();
    assert_eq!(
        reloaded.resolved_properties(wall_id).unwrap()[&PropertyId::from("height")].value,
        number(3.5)
    );
}

#[test]
fn delete_default_reject_cascade_and_explicit_detach_rehost() {
    let (mut d, wall_id, opening_id, window_id) = hosted_document();
    let before = d.to_json_string().unwrap();
    let tx = d
        .begin_transaction("reject", d.revision())
        .with(Command::DeleteElement {
            element_id: wall_id,
        });
    assert!(d.commit(tx).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
    let second = execute(&mut d, "second-wall", vec![wall("Second")]).created_elements[0];
    execute(
        &mut d,
        "detach-rehost",
        vec![
            Command::DeleteElementWithStrategy {
                element_id: wall_id,
                strategy: DeleteStrategy::Detach,
            },
            Command::HostElement {
                host_id: second,
                element_id: opening_id,
            },
        ],
    );
    assert!(d.element(window_id).is_some());
    assert_eq!(d.dependencies(second), vec![opening_id, window_id]);
    d.undo().unwrap();
    assert!(d.element(wall_id).is_some());
    let before = d.revision();
    execute(
        &mut d,
        "cascade",
        vec![Command::DeleteElementWithStrategy {
            element_id: wall_id,
            strategy: DeleteStrategy::Cascade,
        }],
    );
    assert_eq!(
        d.history_diff(before, d.revision()).unwrap().deleted,
        vec![wall_id, opening_id, window_id]
    );
    assert!(
        d.relationships()
            .all(|r| r.source_id != EntityRef::Element(wall_id))
    );
    d.undo().unwrap();
    assert!(d.element(window_id).is_some());
}

#[test]
fn compatibility_level_reference_and_orphan_opening_validation() {
    let mut d = Document::new("Validation").unwrap();
    let type_id = execute(
        &mut d,
        "type",
        vec![Command::CreateType {
            name: "Slab".into(),
            class_id: BUILTIN_FLOOR_CLASS_ID.into(),
            properties: props("thickness", 0.2),
        }],
    )
    .created_types[0];
    let id = execute(&mut d, "wall", vec![wall("Wall")]).created_elements[0];
    let before = d.to_json_string().unwrap();
    let tx = d
        .begin_transaction("incompatible", d.revision())
        .with(Command::AssignType {
            element_id: id,
            type_id,
        });
    assert!(d.commit(tx).is_err());
    let tx = d
        .begin_transaction("level-missing", d.revision())
        .with(Command::AssignLevel {
            element_id: id,
            level_id: LevelId::new(999),
        });
    assert!(d.commit(tx).is_err());
    let tx = d
        .begin_transaction("orphan", d.revision())
        .with(Command::CreateElement {
            name: "Orphan".into(),
            class_id: BUILTIN_OPENING_CLASS_ID.into(),
            kind: ElementKind::Opening(rectangular()),
            properties: BTreeMap::new(),
        });
    assert!(d.commit(tx).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
}

#[test]
fn idempotency_new_bim_command_after_undo_does_not_reapply() {
    let mut d = Document::new("Idempotency").unwrap();
    let command = Command::CreateType {
        name: "Wall".into(),
        class_id: BUILTIN_WALL_CLASS_ID.into(),
        properties: props("thickness", 0.15),
    };
    let original = execute(&mut d, "create-type", vec![command.clone()]);
    d.undo().unwrap();
    let replay = d
        .commit(d.begin_transaction("create-type", 0).with(command))
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.transaction_id, original.transaction_id);
    assert_eq!(d.types().len(), 0);
    d.redo().unwrap();
    assert_eq!(d.types().len(), 1);
}

#[test]
fn old_fixture_extended_v1_migration_preserves_ids_dimensions_and_history() {
    let source = include_str!("../tests/fixtures/native-baseline/example.ai-revit.json");
    let old: serde_json::Value = serde_json::from_str(source).unwrap();
    let mut migrated = Document::from_json_str(source).unwrap();
    assert_eq!(migrated.revision(), old["revision"].as_u64().unwrap());
    assert_eq!(
        migrated.elements().len(),
        old["elements"].as_object().unwrap().len()
    );
    assert_eq!(migrated.types().len(), 0);
    assert_eq!(migrated.relationships().count(), 0);
    for element in migrated.elements() {
        assert_eq!(
            migrated.effective_element(element.id).unwrap().kind,
            element.kind
        );
        assert_eq!(
            serde_json::to_value(&element.kind).unwrap(),
            old["elements"][element.id.to_string()]["kind"]
        );
    }
    let extended = migrated.to_json_string().unwrap();
    let reopened = Document::from_json_str(&extended).unwrap();
    assert_eq!(extended, reopened.to_json_string().unwrap());
    let latest = migrated.revision();
    migrated.undo().unwrap();
    migrated.redo().unwrap();
    assert_eq!(
        migrated
            .history_diff(latest, migrated.revision())
            .unwrap()
            .modified,
        Vec::<ElementId>::new()
    );
}

#[test]
fn stable_entity_counters_are_not_reused_after_undo() {
    let mut d = Document::new("Stable IDs").unwrap();
    let first = execute(
        &mut d,
        "one",
        vec![Command::CreateLevel {
            name: "One".into(),
            elevation_m: 0.0,
            height_m: None,
        }],
    )
    .created_levels[0];
    d.undo().unwrap();
    let second = execute(
        &mut d,
        "two",
        vec![Command::CreateLevel {
            name: "Two".into(),
            elevation_m: 3.0,
            height_m: None,
        }],
    )
    .created_levels[0];
    assert!(second > first);
}

#[test]
fn old_fixture_can_receive_new_types_without_copying_properties() {
    let mut d =
        Document::from_json_str(include_str!("../tests/fixtures/native-baseline/example.ai-revit.json")).unwrap();
    let wall_id = d
        .elements()
        .find(|e| matches!(e.kind, ElementKind::Wall(_)))
        .unwrap()
        .id;
    let original_properties = d.element(wall_id).unwrap().properties.clone();
    let type_id = execute(
        &mut d,
        "new-type-after-migration",
        vec![Command::CreateType {
            name: "Migrated wall type".into(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            properties: props("thickness", 0.17),
        }],
    )
    .created_types[0];
    execute(
        &mut d,
        "assign-migrated-type",
        vec![Command::AssignType {
            element_id: wall_id,
            type_id,
        }],
    );
    assert_eq!(d.element(wall_id).unwrap().properties, original_properties);
    assert_eq!(
        d.resolved_properties(wall_id).unwrap()[&PropertyId::from("thickness")].value,
        number(0.17)
    );
}

#[test]
fn required_property_can_be_provided_by_type_in_same_atomic_batch() {
    let mut d = Document::new("Required resolution").unwrap();
    execute(
        &mut d,
        "required-definition",
        vec![Command::DefineProperty {
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            definition: PropertyDef::new("fire_rating", "Fire rating", PropertyType::Text)
                .required(),
        }],
    );
    let type_id = execute(
        &mut d,
        "required-type",
        vec![Command::CreateType {
            name: "Fire rated".into(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            properties: [("fire_rating".into(), PropertyValue::Text("R60".into()))]
                .into_iter()
                .collect(),
        }],
    )
    .created_types[0];
    let id = ElementId::new(1);
    execute(
        &mut d,
        "resolved-at-batch-end",
        vec![
            wall("Wall"),
            Command::AssignType {
                element_id: id,
                type_id,
            },
        ],
    );
    assert!(d.element(id).unwrap().properties.is_empty());
    assert_eq!(
        d.resolved_properties(id).unwrap()[&PropertyId::from("fire_rating")].source,
        PropertySource::Type
    );
    let before = d.to_json_string().unwrap();
    let invalid = d
        .begin_transaction("required-missing", d.revision())
        .with(wall("Missing rating"));
    assert!(d.commit(invalid).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
}

#[test]
fn all_requested_type_categories_and_material_identity_are_supported() {
    let mut d = Document::new("Categories").unwrap();
    let ids = execute(
        &mut d,
        "all-types",
        [
            BUILTIN_WALL_CLASS_ID,
            BUILTIN_FLOOR_CLASS_ID,
            BUILTIN_ROOF_CLASS_ID,
            BUILTIN_OPENING_CLASS_ID,
            BUILTIN_DOOR_CLASS_ID,
            BUILTIN_WINDOW_CLASS_ID,
            BUILTIN_MATERIAL_CLASS_ID,
        ]
        .iter()
        .map(|id| Command::CreateType {
            name: id.to_string(),
            class_id: (*id).into(),
            properties: BTreeMap::new(),
        })
        .collect(),
    )
    .created_types;
    assert_eq!(d.types().len(), 7);
    assert_eq!(d.materials().count(), 1);
    assert_eq!(d.materials().next().unwrap().id, ids[6]);
    let reloaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(reloaded.materials().next().unwrap().id, ids[6]);
}

#[test]
fn history_diff_observes_override_source_changes_at_equal_values() {
    let mut d = Document::new("Semantic diff").unwrap();
    let type_id = execute(
        &mut d,
        "type",
        vec![Command::CreateType {
            name: "Wall 200".into(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            properties: props("thickness", 0.2),
        }],
    )
    .created_types[0];
    let id = execute(&mut d, "wall", vec![wall("Wall")]).created_elements[0];
    execute(
        &mut d,
        "assign",
        vec![Command::AssignType {
            element_id: id,
            type_id,
        }],
    );
    let before = d.revision();
    execute(
        &mut d,
        "equal-override",
        vec![Command::SetProperty {
            element_id: id,
            property_id: "thickness".into(),
            value: number(0.2),
        }],
    );
    assert_eq!(
        d.history_diff(before, d.revision()).unwrap().modified,
        vec![id]
    );
    let override_revision = d.revision();
    execute(
        &mut d,
        "clear-equal-override",
        vec![Command::ClearPropertyOverride {
            element_id: id,
            property_id: "thickness".into(),
        }],
    );
    assert_eq!(
        d.history_diff(override_revision, d.revision())
            .unwrap()
            .modified,
        vec![id]
    );
    assert!(
        d.history_diff(before, d.revision())
            .unwrap()
            .modified
            .is_empty()
    );
    let mut reloaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    let undo = reloaded.undo().unwrap().unwrap();
    assert_eq!(
        reloaded
            .history_diff(undo.revision_before, undo.revision_after)
            .unwrap()
            .modified,
        vec![id]
    );
}

#[test]
fn legacy_child_geometric_definitions_load_and_survive_undo() {
    // Before v0.2 built-in wall had no geometric property definitions, so this
    // valid child was allowed to declare thickness itself.
    let mut old = Document::new("Old class").unwrap();
    old.classes
        .get_mut(&BUILTIN_WALL_CLASS_ID.into())
        .unwrap()
        .properties
        .clear();
    let definition = PropertyDef::new(
        "thickness",
        "Legacy thickness",
        PropertyType::Number { unit: Unit::Meter },
    )
    .with_default(number(0.35));
    let mut child = ClassDef::child("legacy.wall", "Legacy wall", BUILTIN_WALL_CLASS_ID);
    child.properties.insert(definition.id.clone(), definition);
    old.classes.insert(child.id.clone(), child);
    old.revision_states.clear();
    let mut json = serde_json::to_value(&old).unwrap();
    for key in [
        "types",
        "levels",
        "relationships",
        "revision_states",
        "semantic_schema",
        "next_type_id",
        "next_level_id",
        "next_relationship_id",
        "dirty_elements",
    ] {
        json.as_object_mut().unwrap().remove(key);
    }
    let mut migrated = Document::from_json_str(&serde_json::to_string(&json).unwrap()).unwrap();
    let mut command = wall("Legacy");
    if let Command::CreateElement { class_id, .. } = &mut command {
        *class_id = "legacy.wall".into();
    }
    let id = execute(&mut migrated, "legacy-wall", vec![command]).created_elements[0];
    let type_id = execute(
        &mut migrated,
        "legacy-type",
        vec![Command::CreateType {
            name: "Wall 250".into(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            properties: props("thickness", 0.25),
        }],
    )
    .created_types[0];
    execute(
        &mut migrated,
        "legacy-assign",
        vec![Command::AssignType {
            element_id: id,
            type_id,
        }],
    );
    assert_eq!(
        migrated.resolved_properties(id).unwrap()[&PropertyId::from("thickness")].value,
        number(0.25)
    );
    migrated.undo().unwrap();
    migrated.redo().unwrap();
    Document::from_json_str(&migrated.to_json_string().unwrap()).unwrap();
}

#[test]
fn persisted_counters_cannot_reuse_undone_entity_ids() {
    let mut d = Document::new("Counter integrity").unwrap();
    execute(
        &mut d,
        "type",
        vec![Command::CreateType {
            name: "Original".into(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            properties: BTreeMap::new(),
        }],
    );
    d.undo().unwrap();
    let mut json = serde_json::to_value(&d).unwrap();
    json["next_type_id"] = 1.into();
    assert!(
        Document::from_json_str(&serde_json::to_string(&json).unwrap()).is_err(),
        "must reject reuse of the historical TypeId after reload"
    );
}

#[test]
fn current_revision_snapshot_cannot_disagree_with_authoritative_document() {
    let mut d = Document::new("Snapshot integrity").unwrap();
    execute(&mut d, "wall", vec![wall("Wall")]);
    let mut json = serde_json::to_value(&d).unwrap();
    json["revision_states"][d.revision().to_string()]["elements"]["1"]["name"] =
        "Fabricated".into();
    assert!(
        Document::from_json_str(&serde_json::to_string(&json).unwrap()).is_err(),
        "history.diff must not expose a fabricated current revision snapshot"
    );
}

#[test]
fn untyped_parameter_resolution_matches_authoritative_geometry() {
    let mut d = Document::new("Parameter sources").unwrap();
    let mut cmd = wall("Nondefault dimensions");
    if let Command::CreateElement {
        kind: ElementKind::Wall(spec),
        ..
    } = &mut cmd
    {
        spec.height_m = 2.85;
        spec.thickness_m = 0.175;
    }
    let id = execute(&mut d, "wall", vec![cmd]).created_elements[0];
    let resolved = d.resolved_properties(id).unwrap();
    assert_eq!(resolved[&PropertyId::from("height")].value, number(2.85));
    assert_eq!(
        resolved[&PropertyId::from("height")].source,
        PropertySource::InstanceOverride
    );
    assert_eq!(
        resolved[&PropertyId::from("thickness")].value,
        number(0.175)
    );
    assert!(d.element(id).unwrap().properties.is_empty());
    let reloaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(reloaded.resolved_properties(id).unwrap(), resolved);
}

#[test]
fn misplaced_opening_and_wall_resize_are_atomic() {
    let (mut d, wall_id, opening_id, _) = hosted_document();
    let before = d.to_json_string().unwrap();
    let mut moved = rectangular();
    moved.center.x = 10.0;
    let tx = d
        .begin_transaction("move-opening-outside", d.revision())
        .with(Command::UpdateElement {
            element_id: opening_id,
            kind: ElementKind::Opening(moved),
        });
    assert!(d.commit(tx).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
    let tx = d
        .begin_transaction("shrink-wall", d.revision())
        .with(Command::SetProperty {
            element_id: wall_id,
            property_id: "height".into(),
            value: number(1.0),
        });
    assert!(
        matches!(d.commit(tx), Err(CoreError::BimValidation { entity: EntityRef::Element(id), .. }) if id == opening_id)
    );
    assert_eq!(d.to_json_string().unwrap(), before);
}

#[test]
fn hosted_window_type_cannot_exceed_opening() {
    let (mut d, _, _, window_id) = hosted_document();
    let type_id = execute(
        &mut d,
        "window-type",
        vec![Command::CreateType {
            name: "Window".into(),
            class_id: BUILTIN_WINDOW_CLASS_ID.into(),
            properties: props("width", 1.2),
        }],
    )
    .created_types[0];
    execute(
        &mut d,
        "assign-window-type",
        vec![Command::AssignType {
            element_id: window_id,
            type_id,
        }],
    );
    let before = d.to_json_string().unwrap();
    let tx = d
        .begin_transaction("oversized-window", d.revision())
        .with(Command::UpdateType {
            type_id,
            name: None,
            properties: props("width", 2.0),
        });
    assert!(
        matches!(d.commit(tx), Err(CoreError::BimValidation { entity: EntityRef::Element(id), .. }) if id == window_id)
    );
    assert_eq!(d.to_json_string().unwrap(), before);
}

#[test]
fn type_extension_override_is_validated_after_reload() {
    let mut d = Document::new("Custom type properties").unwrap();
    let mut cls = ClassDef::new("wall.fire", "Fire wall", ElementCategory::Wall);
    cls.properties.insert(
        "fire_rating".into(),
        PropertyDef::new("fire_rating", "Fire rating", PropertyType::Text),
    );
    execute(&mut d, "define-fire-class", vec![Command::DefineClass(cls)]);
    let type_id = execute(
        &mut d,
        "fire-type",
        vec![Command::CreateType {
            name: "R60".into(),
            class_id: "wall.fire".into(),
            properties: [("fire_rating".into(), PropertyValue::Text("R60".into()))]
                .into_iter()
                .collect(),
        }],
    )
    .created_types[0];
    let id = execute(&mut d, "wall", vec![wall("Wall")]).created_elements[0];
    execute(
        &mut d,
        "assign",
        vec![Command::AssignType {
            element_id: id,
            type_id,
        }],
    );
    execute(
        &mut d,
        "override",
        vec![Command::SetProperty {
            element_id: id,
            property_id: "fire_rating".into(),
            value: PropertyValue::Text("R90".into()),
        }],
    );
    let mut json = serde_json::to_value(&d).unwrap();
    json["elements"][id.to_string()]["properties"]["fire_rating"] =
        serde_json::to_value(PropertyValue::Integer(90)).unwrap();
    // Update current snapshot too to isolate typed override validation.
    json["revision_states"][d.revision().to_string()]["elements"][id.to_string()]["properties"]["fire_rating"] =
        serde_json::to_value(PropertyValue::Integer(90)).unwrap();
    if let Some(instance) =
        json["revision_states"][d.revision().to_string()]["instances"].get_mut(id.to_string())
    {
        instance["properties"]["fire_rating"] =
            serde_json::to_value(PropertyValue::Integer(90)).unwrap();
    }
    assert!(matches!(
        Document::from_json_str(&serde_json::to_string(&json).unwrap()),
        Err(CoreError::PropertyTypeMismatch { .. })
    ));
}

#[test]
fn type_class_extra_defaults_and_required_values_are_resolved() {
    let mut d = Document::new("Type class definitions").unwrap();
    let mut cls = ClassDef::new("wall.fire", "Fire wall", ElementCategory::Wall);
    cls.properties.insert(
        "fire_rating".into(),
        PropertyDef::new("fire_rating", "Fire rating", PropertyType::Text)
            .required()
            .with_default(PropertyValue::Text("R30".into())),
    );
    execute(&mut d, "define", vec![Command::DefineClass(cls)]);
    let type_id = execute(
        &mut d,
        "type",
        vec![Command::CreateType {
            name: "Default fire rating".into(),
            class_id: "wall.fire".into(),
            properties: BTreeMap::new(),
        }],
    )
    .created_types[0];
    let id = execute(&mut d, "wall", vec![wall("Wall")]).created_elements[0];
    execute(
        &mut d,
        "assign",
        vec![Command::AssignType {
            element_id: id,
            type_id,
        }],
    );
    assert_eq!(
        d.resolved_properties(id)
            .unwrap()
            .get(&PropertyId::from("fire_rating"))
            .unwrap()
            .value,
        PropertyValue::Text("R30".into())
    );
}

#[test]
fn assigning_types_preserves_rotated_hosted_instance_placement() {
    let mut d = Document::new("Rotated instances").unwrap();
    let mut command = wall("North wall");
    if let Command::CreateElement {
        kind: ElementKind::Wall(spec),
        ..
    } = &mut command
    {
        spec.end = Point2::new(0.0, 6.0);
    }
    let wall_id = execute(&mut d, "wall", vec![command]).created_elements[0];
    let mut spec = rectangular();
    spec.center = Point2::new(0.0, 2.0);
    spec.rotation_rad = std::f64::consts::FRAC_PI_2;
    let opening_id = execute(
        &mut d,
        "opening",
        vec![Command::CreateOpening {
            name: "Rotated opening".into(),
            spec: spec.clone(),
            properties: BTreeMap::new(),
            host_id: wall_id,
            type_id: None,
        }],
    )
    .created_elements[0];
    let window_id = ElementId::new(3);
    execute(
        &mut d,
        "window",
        vec![
            Command::CreateElement {
                name: "Rotated window".into(),
                class_id: BUILTIN_WINDOW_CLASS_ID.into(),
                kind: ElementKind::Window(spec.clone()),
                properties: BTreeMap::new(),
            },
            Command::HostElement {
                host_id: opening_id,
                element_id: window_id,
            },
        ],
    );
    let types = execute(
        &mut d,
        "types",
        vec![
            Command::CreateType {
                name: "Opening type".into(),
                class_id: BUILTIN_OPENING_CLASS_ID.into(),
                properties: BTreeMap::new(),
            },
            Command::CreateType {
                name: "Window type".into(),
                class_id: BUILTIN_WINDOW_CLASS_ID.into(),
                properties: BTreeMap::new(),
            },
        ],
    )
    .created_types;
    execute(
        &mut d,
        "assign",
        vec![
            Command::AssignType {
                element_id: opening_id,
                type_id: types[0],
            },
            Command::AssignType {
                element_id: window_id,
                type_id: types[1],
            },
        ],
    );
    for id in [opening_id, window_id] {
        assert_eq!(
            d.resolved_properties(id).unwrap()[&PropertyId::from("rotation")].value,
            PropertyValue::Number {
                value: spec.rotation_rad,
                unit: Unit::Radian
            }
        );
        let element = d.effective_element(id).unwrap();
        let actual = match element.kind {
            ElementKind::Opening(s) | ElementKind::Window(s) => s,
            _ => unreachable!(),
        };
        assert_eq!(actual.center, spec.center);
        assert_eq!(actual.rotation_rad, spec.rotation_rad);
    }
    let reloaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    assert_eq!(
        reloaded.effective_element(window_id).unwrap(),
        d.effective_element(window_id).unwrap()
    );
}

#[test]
fn typed_instance_rejects_missing_required_type_extension_atomically() {
    let mut d = Document::new("Required type extension").unwrap();
    let mut cls = ClassDef::new("wall.fire", "Fire wall", ElementCategory::Wall);
    cls.properties.insert(
        "fire_rating".into(),
        PropertyDef::new("fire_rating", "Fire rating", PropertyType::Text).required(),
    );
    execute(&mut d, "define", vec![Command::DefineClass(cls)]);
    let type_id = execute(
        &mut d,
        "type",
        vec![Command::CreateType {
            name: "Unrated".into(),
            class_id: "wall.fire".into(),
            properties: BTreeMap::new(),
        }],
    )
    .created_types[0];
    let id = execute(&mut d, "wall", vec![wall("Wall")]).created_elements[0];
    let before = d.to_json_string().unwrap();
    let tx = d
        .begin_transaction("assign-missing", d.revision())
        .with(Command::AssignType {
            element_id: id,
            type_id,
        });
    assert!(d.commit(tx).is_err());
    assert_eq!(d.to_json_string().unwrap(), before);
    execute(
        &mut d,
        "assign-filled",
        vec![
            Command::AssignType {
                element_id: id,
                type_id,
            },
            Command::SetProperty {
                element_id: id,
                property_id: "fire_rating".into(),
                value: PropertyValue::Text("R90".into()),
            },
        ],
    );
}

#[test]
fn relationship_endpoint_validation_and_inverse_dependencies_are_consistent() {
    for kind in [
        RelationshipType::Hosts,
        RelationshipType::HostedBy,
        RelationshipType::Contains,
        RelationshipType::ContainedIn,
        RelationshipType::ConnectsTo,
        RelationshipType::SupportedBy,
        RelationshipType::Supports,
        RelationshipType::Bounds,
    ] {
        let mut d = Document::new("Relations").unwrap();
        let ids = execute(&mut d, "walls", vec![wall("A"), wall("B")]).created_elements;
        let type_id = execute(
            &mut d,
            "type",
            vec![Command::CreateType {
                name: "Wall".into(),
                class_id: BUILTIN_WALL_CLASS_ID.into(),
                properties: BTreeMap::new(),
            }],
        )
        .created_types[0];
        let before = d.to_json_string().unwrap();
        let invalid = d.begin_transaction("invalid-endpoint", d.revision()).with(
            Command::CreateRelationship {
                relationship_type: kind,
                source_id: EntityRef::Element(ids[0]),
                target_id: EntityRef::Type(type_id),
                metadata: BTreeMap::new(),
            },
        );
        assert!(d.commit(invalid).is_err());
        assert_eq!(d.to_json_string().unwrap(), before);
        execute(
            &mut d,
            "valid-relation",
            vec![Command::CreateRelationship {
                relationship_type: kind,
                source_id: EntityRef::Element(ids[0]),
                target_id: EntityRef::Element(ids[1]),
                metadata: BTreeMap::new(),
            }],
        );
        let (root, leaf) = match kind {
            RelationshipType::HostedBy
            | RelationshipType::ContainedIn
            | RelationshipType::SupportedBy => (ids[1], ids[0]),
            _ => (ids[0], ids[1]),
        };
        let rebuild_relation = matches!(
            kind,
            RelationshipType::Hosts
                | RelationshipType::HostedBy
                | RelationshipType::Contains
                | RelationshipType::ContainedIn
                | RelationshipType::Supports
                | RelationshipType::SupportedBy
        );
        assert_eq!(
            d.dependencies(root),
            if rebuild_relation { vec![leaf] } else { vec![] }
        );
        let receipt = execute(
            &mut d,
            "edit-root",
            vec![Command::SetProperty {
                element_id: root,
                property_id: "height".into(),
                value: number(3.1),
            }],
        );
        assert_eq!(
            receipt.dirty_ids.len(),
            if rebuild_relation { 2 } else { 1 }
        );
        let reloaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
        assert_eq!(reloaded.dependencies(root), d.dependencies(root));
    }
}

#[test]
fn slab_and_roof_hosted_openings_must_intersect_host_region() {
    for roof in [false, true] {
        let mut d = Document::new("Planar hosts").unwrap();
        let ring = vec![
            Point2::new(0.0, 0.0),
            Point2::new(6.0, 0.0),
            Point2::new(6.0, 6.0),
            Point2::new(0.0, 6.0),
        ];
        let kind = if roof {
            ElementKind::Roof(RoofSpec {
                footprint: ring,
                elevation_m: 3.0,
                thickness_m: 0.2,
                slope_x: 0.1,
                slope_y: 0.0,
            })
        } else {
            ElementKind::Floor(FloorSpec {
                boundary: ring,
                holes: vec![],
                elevation_m: 3.0,
                thickness_m: 0.2,
            })
        };
        let host_id = execute(
            &mut d,
            "host",
            vec![Command::CreateElement {
                name: "Host".into(),
                class_id: if roof {
                    BUILTIN_ROOF_CLASS_ID.into()
                } else {
                    BUILTIN_FLOOR_CLASS_ID.into()
                },
                kind,
                properties: BTreeMap::new(),
            }],
        )
        .created_elements[0];
        let mut spec = rectangular();
        spec.center = Point2::new(3.0, 3.0);
        spec.elevation_m = 2.0;
        let opening_id = execute(
            &mut d,
            "opening",
            vec![Command::CreateOpening {
                name: "Valid cut".into(),
                spec: spec.clone(),
                properties: BTreeMap::new(),
                host_id,
                type_id: None,
            }],
        )
        .created_elements[0];
        let before = d.to_json_string().unwrap();
        spec.center.x = 7.0;
        let tx = d
            .begin_transaction("outside", d.revision())
            .with(Command::UpdateElement {
                element_id: opening_id,
                kind: ElementKind::Opening(spec.clone()),
            });
        assert!(d.commit(tx).is_err());
        assert_eq!(d.to_json_string().unwrap(), before);
        spec.center.x = 3.0;
        spec.elevation_m = 6.0;
        let tx = d
            .begin_transaction("floating", d.revision())
            .with(Command::UpdateElement {
                element_id: opening_id,
                kind: ElementKind::Opening(spec),
            });
        assert!(d.commit(tx).is_err());
        assert_eq!(d.to_json_string().unwrap(), before);
    }
}

#[test]
fn history_diff_observes_class_default_to_type_source_at_equal_value() {
    let mut d = Document::new("Default provenance").unwrap();
    let type_id = execute(
        &mut d,
        "type",
        vec![Command::CreateType {
            name: "Defaults".into(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            properties: BTreeMap::new(),
        }],
    )
    .created_types[0];
    let id = execute(&mut d, "wall", vec![wall("Wall")]).created_elements[0];
    execute(
        &mut d,
        "assign",
        vec![Command::AssignType {
            element_id: id,
            type_id,
        }],
    );
    assert_eq!(
        d.resolved_properties(id).unwrap()[&PropertyId::from("height")].source,
        PropertySource::ClassDefault
    );
    let before = d.revision();
    execute(
        &mut d,
        "explicit-type-default",
        vec![Command::UpdateType {
            type_id,
            name: None,
            properties: props("height", 3.0),
        }],
    );
    assert_eq!(
        d.resolved_properties(id).unwrap()[&PropertyId::from("height")].source,
        PropertySource::Type
    );
    assert_eq!(
        d.history_diff(before, d.revision()).unwrap().modified,
        vec![id]
    );
    let mut d = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    let undo = d.undo().unwrap().unwrap();
    assert_eq!(
        d.history_diff(undo.revision_before, undo.revision_after)
            .unwrap()
            .modified,
        vec![id]
    );
    assert_eq!(
        d.resolved_properties(id).unwrap()[&PropertyId::from("height")].source,
        PropertySource::ClassDefault
    );
    d.redo().unwrap();
    assert_eq!(
        d.resolved_properties(id).unwrap()[&PropertyId::from("height")].source,
        PropertySource::Type
    );
}

#[test]
fn level_graph_endpoints_singleton_reassignment_and_undo_are_validated() {
    let mut d = Document::new("Level graph").unwrap();
    let id = execute(&mut d, "wall", vec![wall("Wall")]).created_elements[0];
    let levels = execute(
        &mut d,
        "levels",
        vec![
            Command::CreateLevel {
                name: "Ground".into(),
                elevation_m: 0.0,
                height_m: None,
            },
            Command::CreateLevel {
                name: "First".into(),
                elevation_m: 3.0,
                height_m: None,
            },
        ],
    )
    .created_levels;
    execute(
        &mut d,
        "assign",
        vec![Command::AssignLevel {
            element_id: id,
            level_id: levels[0],
        }],
    );
    let before = d.to_json_string().unwrap();
    for (source, target) in [
        (EntityRef::Element(id), EntityRef::Level(levels[1])),
        (EntityRef::Level(levels[0]), EntityRef::Element(id)),
        (EntityRef::Element(id), EntityRef::Element(id)),
        (EntityRef::Element(id), EntityRef::Level(LevelId::new(999))),
    ] {
        let tx =
            d.begin_transaction("invalid-level", d.revision())
                .with(Command::CreateRelationship {
                    relationship_type: RelationshipType::BelongsToLevel,
                    source_id: source,
                    target_id: target,
                    metadata: BTreeMap::new(),
                });
        assert!(d.commit(tx).is_err());
        assert_eq!(d.to_json_string().unwrap(), before);
    }
    execute(
        &mut d,
        "reassign",
        vec![Command::AssignLevel {
            element_id: id,
            level_id: levels[1],
        }],
    );
    assert_eq!(d.element_level(id), Some(levels[1]));
    assert!(d.level_elements(levels[0]).is_empty());
    assert_eq!(d.level_elements(levels[1]), vec![id]);
    let mut reloaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    reloaded.undo().unwrap();
    assert_eq!(reloaded.element_level(id), Some(levels[0]));
    reloaded.redo().unwrap();
    assert_eq!(reloaded.element_level(id), Some(levels[1]));
}

#[test]
fn current_revision_property_source_snapshot_cannot_be_fabricated() {
    let mut d = Document::new("Source snapshot integrity").unwrap();
    execute(&mut d, "wall", vec![wall("Wall")]);
    let mut json = serde_json::to_value(&d).unwrap();
    json["revision_states"][d.revision().to_string()]["property_sources"]["1"]["height"] =
        "type".into();
    assert!(Document::from_json_str(&serde_json::to_string(&json).unwrap()).is_err());
}

#[test]
fn class_change_reaches_assigned_type_classes_and_dependents() {
    let (mut d, wall_id, opening_id, window_id) = hosted_document();
    let other_ids = execute(
        &mut d,
        "other-walls",
        vec![wall("Inherited type wall"), wall("Unrelated wall")],
    )
    .created_elements;
    let typed_wall_id = other_ids[0];
    let unrelated_id = other_ids[1];
    execute(
        &mut d,
        "custom-classes",
        vec![
            Command::DefineClass(ClassDef::child(
                "custom.wall",
                "Custom wall",
                BUILTIN_WALL_CLASS_ID,
            )),
            Command::DefineClass(ClassDef::child(
                "custom.wall.child",
                "Custom child wall",
                "custom.wall",
            )),
        ],
    );
    let types = execute(
        &mut d,
        "custom-types",
        vec![
            Command::CreateType {
                name: "Exact custom class".into(),
                class_id: "custom.wall".into(),
                properties: BTreeMap::new(),
            },
            Command::CreateType {
                name: "Inherited custom class".into(),
                class_id: "custom.wall.child".into(),
                properties: BTreeMap::new(),
            },
        ],
    )
    .created_types;
    execute(
        &mut d,
        "assign-custom-types",
        vec![
            Command::AssignType {
                element_id: wall_id,
                type_id: types[0],
            },
            Command::AssignType {
                element_id: typed_wall_id,
                type_id: types[1],
            },
        ],
    );
    let before = d.revision();
    let receipt = execute(
        &mut d,
        "define-type-class-default",
        vec![Command::DefineProperty {
            class_id: "custom.wall".into(),
            definition: PropertyDef::new("fire_rating", "Fire rating", PropertyType::Text)
                .with_default(PropertyValue::Text("R60".into())),
        }],
    );
    let affected = vec![wall_id, typed_wall_id];
    let dirty = vec![wall_id, opening_id, window_id, typed_wall_id];
    assert_eq!(receipt.affected_ids, affected);
    assert_eq!(receipt.dirty_ids, dirty);
    assert_eq!(d.dirty_ids(), dirty);
    assert_eq!(
        d.history_diff(before, d.revision()).unwrap().modified,
        affected
    );
    for id in &affected {
        let properties = d.resolved_properties(*id).unwrap();
        let rating = &properties[&PropertyId::from("fire_rating")];
        assert_eq!(rating.value, PropertyValue::Text("R60".into()));
        assert_eq!(rating.source, PropertySource::ClassDefault);
    }
    assert!(
        !d.resolved_properties(unrelated_id)
            .unwrap()
            .contains_key(&PropertyId::from("fire_rating"))
    );
    let mut reloaded = Document::from_json_str(&d.to_json_string().unwrap()).unwrap();
    let undo = reloaded.undo().unwrap().unwrap();
    assert_eq!(undo.affected_ids, affected);
    assert_eq!(undo.dirty_ids, dirty);
    for id in &affected {
        assert!(
            !reloaded
                .resolved_properties(*id)
                .unwrap()
                .contains_key(&PropertyId::from("fire_rating"))
        );
    }
    let redo = reloaded.redo().unwrap().unwrap();
    assert_eq!(redo.affected_ids, affected);
    assert_eq!(redo.dirty_ids, dirty);
    assert_eq!(
        reloaded
            .history_diff(redo.revision_before, redo.revision_after)
            .unwrap()
            .modified,
        affected
    );
}

#[test]
fn opening_resize_and_move_dirty_cut_host_without_semantic_cycle() {
    let (mut d, host, opening, window) = hosted_document();
    let unrelated = execute(&mut d, "unrelated", vec![wall("Unrelated")]).created_elements[0];
    let mut sibling = rectangular();
    sibling.center.x = 4.5;
    let sibling_id = execute(
        &mut d,
        "sibling-opening",
        vec![Command::CreateOpening {
            name: "Unaffected sibling".into(),
            spec: sibling,
            properties: BTreeMap::new(),
            host_id: host,
            type_id: None,
        }],
    )
    .created_elements[0];
    let resized = execute(
        &mut d,
        "resize-opening-v03",
        vec![Command::SetProperty {
            element_id: opening,
            property_id: "width".into(),
            value: number(1.6),
        }],
    );
    assert_eq!(resized.affected_ids, vec![opening]);
    assert_eq!(resized.dirty_ids, vec![host, opening, window]);
    assert!(!resized.dirty_ids.contains(&unrelated));
    assert!(!resized.dirty_ids.contains(&sibling_id));
    let mut moved = rectangular();
    moved.center.x += 0.1;
    let receipt = execute(
        &mut d,
        "move-opening-v03",
        vec![Command::UpdateElement {
            element_id: opening,
            kind: ElementKind::Opening(moved),
        }],
    );
    assert_eq!(receipt.dirty_ids, vec![host, opening, window]);
    assert_eq!(d.dependencies(opening), vec![window]);
    assert_eq!(
        d.dependency_info(host).unwrap().direct_dependencies,
        Vec::new()
    );
    assert_eq!(
        d.dependency_info(opening).unwrap().direct_dependencies,
        vec![host]
    );
    assert_eq!(
        d.undo().unwrap().unwrap().dirty_ids,
        vec![host, opening, window]
    );
    assert_eq!(
        d.redo().unwrap().unwrap().dirty_ids,
        vec![host, opening, window]
    );
}

#[test]
fn opening_delete_rehost_and_history_dirty_old_and_new_cut_hosts() {
    let (mut d, host, opening, window) = hosted_document();
    let new_host = execute(&mut d, "new-cut-host", vec![wall("New host")]).created_elements[0];
    let old_link = d
        .element_relationships(opening)
        .into_iter()
        .find(|r| r.source_id == EntityRef::Element(host))
        .unwrap()
        .id;
    let receipt = execute(
        &mut d,
        "rehost-opening-v03",
        vec![
            Command::RemoveRelationship {
                relationship_id: old_link,
            },
            Command::HostElement {
                host_id: new_host,
                element_id: opening,
            },
        ],
    );
    let all = vec![host, opening, window, new_host];
    assert_eq!(receipt.dirty_ids, all);
    assert_eq!(d.host_of(opening), Some(new_host));
    assert!(d.hosted_opening_ids(host).is_empty());
    assert_eq!(d.hosted_opening_ids(new_host), vec![opening]);
    assert_eq!(d.undo().unwrap().unwrap().dirty_ids, all);
    assert_eq!(d.host_of(opening), Some(host));
    assert_eq!(d.redo().unwrap().unwrap().dirty_ids, all);
    let deleted = execute(
        &mut d,
        "delete-opening-v03",
        vec![Command::DeleteElementWithStrategy {
            element_id: opening,
            strategy: DeleteStrategy::Cascade,
        }],
    );
    assert_eq!(deleted.dirty_ids, vec![opening, window, new_host]);
    assert_eq!(d.host_of(opening), None);
    assert!(d.hosted_opening_ids(new_host).is_empty());
    assert_eq!(
        d.undo().unwrap().unwrap().dirty_ids,
        vec![opening, window, new_host]
    );
    assert_eq!(d.hosted_opening_ids(new_host), vec![opening]);
    assert_eq!(d.hosted_element_ids(opening), vec![window]);
    assert_eq!(
        d.redo().unwrap().unwrap().dirty_ids,
        vec![opening, window, new_host]
    );
    assert!(d.hosted_element_ids(opening).is_empty());
}

#[test]
fn opening_metadata_and_equal_effective_input_do_not_dirty_wall() {
    let (mut d, host, opening, window) = hosted_document();
    let definition = execute(
        &mut d,
        "opening-description-definition",
        vec![Command::DefineProperty {
            class_id: BUILTIN_OPENING_CLASS_ID.into(),
            definition: PropertyDef::new("description", "Description", PropertyType::Text),
        }],
    );
    assert_eq!(definition.dirty_ids, vec![opening, window]);
    let metadata = execute(
        &mut d,
        "opening-description",
        vec![Command::SetProperty {
            element_id: opening,
            property_id: "description".into(),
            value: PropertyValue::Text("Semantic note".into()),
        }],
    );
    assert_eq!(metadata.dirty_ids, vec![opening, window]);
    let equal = execute(
        &mut d,
        "same-opening-width",
        vec![Command::SetProperty {
            element_id: opening,
            property_id: "width".into(),
            value: number(1.2),
        }],
    );
    assert_eq!(equal.dirty_ids, vec![opening, window]);
    assert!(!d.dirty_ids().contains(&host));
    assert_eq!(d.undo().unwrap().unwrap().dirty_ids, vec![opening, window]);
}

#[test]
fn window_material_and_geometry_changes_do_not_dirty_cut_host() {
    let (mut d, host, opening, window) = hosted_document();
    execute(
        &mut d,
        "window-material-definition",
        vec![Command::DefineProperty {
            class_id: BUILTIN_WINDOW_CLASS_ID.into(),
            definition: PropertyDef::new("material", "Material", PropertyType::Text),
        }],
    );
    let receipt = execute(
        &mut d,
        "window-material-v03",
        vec![Command::SetProperty {
            element_id: window,
            property_id: "material".into(),
            value: PropertyValue::Text("Timber".into()),
        }],
    );
    assert_eq!(receipt.dirty_ids, vec![window]);
    let geometry = execute(
        &mut d,
        "smaller-window-v03",
        vec![Command::SetProperty {
            element_id: window,
            property_id: "width".into(),
            value: number(1.0),
        }],
    );
    assert_eq!(geometry.dirty_ids, vec![window]);
    assert_eq!(d.host_of(window), Some(opening));
    assert!(!d.dirty_ids().contains(&host));
}

#[test]
fn inverse_hosting_indexes_reconstruct_for_direct_serde_reload_and_history() {
    let (mut d, host, opening, window) = hosted_document();
    let old_link = d
        .element_relationships(opening)
        .into_iter()
        .find(|r| r.source_id == EntityRef::Element(host))
        .unwrap()
        .id;
    execute(
        &mut d,
        "inverse-host-link",
        vec![
            Command::RemoveRelationship {
                relationship_id: old_link,
            },
            Command::CreateRelationship {
                relationship_type: RelationshipType::HostedBy,
                source_id: EntityRef::Element(opening),
                target_id: EntityRef::Element(host),
                metadata: BTreeMap::new(),
            },
        ],
    );
    let source = d.to_json_string().unwrap();
    let value: serde_json::Value = serde_json::from_str(&source).unwrap();
    assert!(value["document"].get("relationship_indexes").is_none());
    assert!(value["document"].get("scan_counters").is_none());
    // Native v2 files have a guarded envelope. Exercise the separate direct
    // serde path on the internal Document DTO, keeping both reload paths.
    let mut direct: Document = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
    let mut loaded = Document::from_json_str(&source).unwrap();
    for document in [&mut direct, &mut loaded] {
        assert_eq!(document.host_of(opening), Some(host));
        assert_eq!(document.host_of(window), Some(opening));
        assert_eq!(document.hosted_opening_ids(host), vec![opening]);
        assert_eq!(document.dependencies(host), vec![opening, window]);
        assert_eq!(document.element_relationships(opening).len(), 2);
        assert_eq!(document.relationships().count(), 2);
        document.undo().unwrap();
        assert_eq!(document.hosted_opening_ids(host), vec![opening]);
        document.redo().unwrap();
        assert_eq!(document.hosted_opening_ids(host), vec![opening]);
        let receipt = execute(
            document,
            "after-index-reload",
            vec![Command::SetProperty {
                element_id: opening,
                property_id: "width".into(),
                value: number(1.5),
            }],
        );
        assert_eq!(receipt.dirty_ids, vec![host, opening, window]);
        document.to_json_string().unwrap();
    }
}

#[test]
fn dependency_index_keeps_parallel_relationship_contributions() {
    let (mut d, host, opening, window) = hosted_document();
    let extra = execute(
        &mut d,
        "parallel-edge",
        vec![Command::CreateRelationship {
            relationship_type: RelationshipType::Contains,
            source_id: EntityRef::Element(host),
            target_id: EntityRef::Element(opening),
            metadata: BTreeMap::new(),
        }],
    )
    .created_relationships[0];
    assert_eq!(d.dependencies(host), vec![opening, window]);
    execute(
        &mut d,
        "remove-parallel-edge",
        vec![Command::RemoveRelationship {
            relationship_id: extra,
        }],
    );
    assert_eq!(d.dependencies(host), vec![opening, window]);
    d.undo().unwrap();
    assert_eq!(d.dependencies(host), vec![opening, window]);
    d.redo().unwrap();
    assert_eq!(d.dependencies(host), vec![opening, window]);
}

#[test]
fn invalid_opening_geometry_mutations_reject_before_commit_atomically() {
    let (mut d, _, opening, _) = hosted_document();
    for (label, width, center, rotation) in [
        ("oversized-v03", 8.0, 2.0, 0.0),
        ("outside-v03", 1.2, 9.0, 0.0),
        ("zero-size-v03", 0.0, 2.0, 0.0),
        ("invalid-rotation-v03", 1.2, 2.0, 0.5),
    ] {
        let before = d.to_json_string().unwrap();
        let quote = d.allocation_preview();
        let mut spec = rectangular();
        spec.width_m = width;
        spec.center.x = center;
        spec.rotation_rad = rotation;
        let tx = d
            .begin_transaction(label, d.revision())
            .with(Command::UpdateElement {
                element_id: opening,
                kind: ElementKind::Opening(spec),
            });
        assert!(matches!(d.commit(tx), Err(CoreError::BimValidation {
            entity: EntityRef::Element(id), ..
        }) if id == opening));
        assert_eq!(d.to_json_string().unwrap(), before);
        assert_eq!(d.allocation_preview(), quote);
    }
}

#[test]
fn transaction_and_history_measurements_are_actual_serialized_bytes() {
    let (mut d, _, opening, _) = hosted_document();
    let receipt = execute(
        &mut d,
        "measured-opening-v03",
        vec![Command::SetProperty {
            element_id: opening,
            property_id: "width".into(),
            value: number(1.5),
        }],
    );
    assert!(receipt.transaction_duration_us > 0);
    assert!(receipt.transaction_duration_us >= receipt.dirty_propagation_duration_us);
    assert!(receipt.transaction_duration_us >= receipt.snapshot_serialization_duration_us);
    assert!(receipt.document_scan_count > 0);
    assert!(receipt.scanned_relationship_count > 0);
    assert_eq!(
        receipt.snapshot_serialized_bytes,
        serde_json::to_vec(d.revision_states.get(&d.revision()).unwrap())
            .unwrap()
            .len() as u64
    );
    let metrics = d.history_metrics().unwrap();
    assert_eq!(
        metrics.history_entry_count,
        d.undo_stack.len() + d.redo_stack.len()
    );
    assert_eq!(
        metrics.history_serialized_bytes,
        serde_json::to_vec(&(
            &d.undo_stack,
            &d.redo_stack,
            &d.revision_states,
            &d.command_receipts,
        ))
        .unwrap()
        .len() as u64
    );
    assert_eq!(
        metrics.document_serialized_bytes,
        serde_json::to_vec(&d).unwrap().len() as u64
    );
    let source = d.to_json_string().unwrap();
    let restored = Document::from_json_str(&source).unwrap();
    assert_eq!(restored.history_metrics().unwrap(), metrics);
    let undo = d.undo().unwrap().unwrap();
    assert!(undo.snapshot_serialized_bytes > 0);
    assert_eq!(
        d.history_metrics().unwrap().history_entry_count,
        metrics.history_entry_count
    );
    assert!(d.redo().unwrap().unwrap().document_scan_count > 0);
}

#[test]
fn opening_type_geometry_updates_dirty_host_using_effective_inputs() {
    let (mut d, host, opening, window) = hosted_document();
    let opening_type = execute(
        &mut d,
        "opening-type-v03",
        vec![Command::CreateType {
            name: "Opening type".into(),
            class_id: BUILTIN_OPENING_CLASS_ID.into(),
            properties: [
                ("width".into(), number(1.2)),
                ("depth".into(), number(0.15)),
            ]
            .into_iter()
            .collect(),
        }],
    )
    .created_types[0];
    let assign = execute(
        &mut d,
        "assign-opening-type-v03",
        vec![Command::AssignType {
            element_id: opening,
            type_id: opening_type,
        }],
    );
    assert_eq!(assign.dirty_ids, vec![opening, window]);
    let change = execute(
        &mut d,
        "opening-type-width-v03",
        vec![Command::UpdateType {
            type_id: opening_type,
            name: None,
            properties: props("width", 1.7),
        }],
    );
    assert_eq!(change.affected_ids, vec![opening]);
    assert_eq!(change.dirty_ids, vec![host, opening, window]);
    assert_eq!(
        d.undo().unwrap().unwrap().dirty_ids,
        vec![host, opening, window]
    );
}

#[test]
fn legacy_receipts_without_instrumentation_load_with_zero_defaults() {
    let (d, host, opening, window) = hosted_document();
    let mut value = serde_json::to_value(&d).unwrap();
    let fields = [
        "transaction_duration_us",
        "dirty_propagation_duration_us",
        "scanned_relationship_count",
        "document_scan_count",
        "snapshot_serialization_duration_us",
        "snapshot_serialized_bytes",
    ];
    for record in value["command_receipts"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        let receipt = record["receipt"].as_object_mut().unwrap();
        for field in fields {
            receipt.remove(field);
        }
    }
    let mut loaded = Document::from_json_str(&serde_json::to_string(&value).unwrap()).unwrap();
    assert_eq!(loaded.host_of(opening), Some(host));
    assert_eq!(loaded.dependencies(host), vec![opening, window]);
    for stored in loaded.command_receipts.values() {
        assert_eq!(stored.receipt.transaction_duration_us, 0);
        assert_eq!(stored.receipt.snapshot_serialized_bytes, 0);
    }
    let receipt = loaded.undo().unwrap().unwrap();
    assert!(receipt.snapshot_serialized_bytes > 0);
    assert!(loaded.hosted_opening_ids(host).is_empty());
    loaded.redo().unwrap();
    assert_eq!(loaded.hosted_opening_ids(host), vec![opening]);
}
