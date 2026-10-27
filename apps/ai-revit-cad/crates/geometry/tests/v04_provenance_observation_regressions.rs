use ai_revit_core::{
    BUILTIN_FLOOR_CLASS_ID, BUILTIN_SPACE_CLASS_ID, BUILTIN_WALL_CLASS_ID, Command, DeleteStrategy,
    Document, ElementId, ElementKind, EntityRef, FloorSpec, OpeningSpec, Point2, PropertyDef,
    PropertySource, PropertyType, PropertyValue, SpaceSpec, WallSpec,
};
use ai_revit_geometry::{GeometryCache, object_id_for, project_dirty_document};
use std::collections::BTreeMap;

fn execute(
    document: &mut Document,
    command_id: &str,
    commands: Vec<Command>,
) -> ai_revit_core::CommitReceipt {
    let mut transaction = document.begin_transaction(command_id, document.revision());
    for command in commands {
        transaction.push(command);
    }
    document
        .commit(transaction)
        .expect("test transaction should commit")
}

fn wall_spec() -> WallSpec {
    WallSpec {
        start: Point2::new(0.0, 0.0),
        end: Point2::new(6.0, 0.0),
        base_elevation_m: 0.0,
        height_m: 3.0,
        thickness_m: 0.2,
    }
}

fn create_wall(document: &mut Document, command_id: &str, name: &str) -> ElementId {
    execute(
        document,
        command_id,
        vec![Command::CreateElement {
            name: name.to_owned(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            kind: ElementKind::Wall(wall_spec()),
            properties: BTreeMap::new(),
        }],
    )
    .created_elements[0]
}

fn opening_spec(depth_m: f64) -> OpeningSpec {
    OpeningSpec {
        center: Point2::new(2.0, 0.0),
        elevation_m: 0.8,
        width_m: 1.2,
        height_m: 1.5,
        depth_m,
        rotation_rad: 0.0,
    }
}

fn create_opening(
    document: &mut Document,
    command_id: &str,
    host_id: ElementId,
    depth_m: f64,
) -> ElementId {
    execute(
        document,
        command_id,
        vec![Command::CreateOpening {
            name: "Hosted opening".into(),
            spec: opening_spec(depth_m),
            properties: BTreeMap::new(),
            host_id,
            type_id: None,
        }],
    )
    .created_elements[0]
}

fn create_floor(document: &mut Document, command_id: &str) -> ElementId {
    execute(
        document,
        command_id,
        vec![Command::CreateElement {
            name: "Floor".into(),
            class_id: BUILTIN_FLOOR_CLASS_ID.into(),
            kind: ElementKind::Floor(FloorSpec {
                boundary: vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(4.0, 0.0),
                    Point2::new(4.0, 3.0),
                    Point2::new(0.0, 3.0),
                ],
                holes: Vec::new(),
                elevation_m: 0.2,
                thickness_m: 0.2,
            }),
            properties: BTreeMap::new(),
        }],
    )
    .created_elements[0]
}

fn create_space(document: &mut Document, command_id: &str) -> ElementId {
    execute(
        document,
        command_id,
        vec![Command::CreateElement {
            name: "Space".into(),
            class_id: BUILTIN_SPACE_CLASS_ID.into(),
            kind: ElementKind::Space(SpaceSpec {
                boundary: Some(vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(4.0, 0.0),
                    Point2::new(4.0, 3.0),
                    Point2::new(0.0, 3.0),
                ]),
                metadata: BTreeMap::new(),
            }),
            properties: BTreeMap::new(),
        }],
    )
    .created_elements[0]
}

#[test]
fn deleting_one_of_two_coincident_openings_changes_provenance_without_rebuilding_wall_mesh() {
    let mut document = Document::new("Coincident opening provenance").unwrap();
    let wall_id = create_wall(&mut document, "wall", "Wall");
    let opening_a = create_opening(&mut document, "opening-a", wall_id, 0.2);
    let opening_b = create_opening(&mut document, "opening-b", wall_id, 0.2);
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);

    let before = cache.inspect(wall_id).unwrap().clone();
    let mesh_before = cache
        .scene()
        .get(object_id_for(wall_id))
        .unwrap()
        .mesh
        .triangles()
        .to_vec();
    let quantities_before = cache.quantities_get(wall_id).unwrap();
    let quantity_rebuilds_before = cache.quantity_cache_metrics().quantity_rebuild_count;
    let history_before = cache.geometry_history_entry_count();
    assert_eq!(before.openings, vec![opening_a, opening_b]);

    execute(
        &mut document,
        "delete-duplicate-opening",
        vec![Command::DeleteElementWithStrategy {
            element_id: opening_b,
            strategy: DeleteStrategy::Detach,
        }],
    );
    assert!(document.element(opening_b).is_none());
    let report = project_dirty_document(&document, &mut cache);

    assert_eq!(report.rebuilt_element_count, 0);
    assert!(report.removed_element_ids.contains(&opening_b));
    assert!(cache.inspect(opening_b).is_none());
    let after = cache.inspect(wall_id).unwrap().clone();
    assert_eq!(after.openings, vec![opening_a]);
    assert!(after.geometry_revision > before.geometry_revision);
    assert_eq!(after.mesh_revision, before.mesh_revision);
    assert_eq!(after.geometry_fingerprint, before.geometry_fingerprint);
    assert_eq!(
        cache
            .scene()
            .get(object_id_for(wall_id))
            .unwrap()
            .mesh
            .triangles(),
        mesh_before.as_slice()
    );
    let deletion = cache
        .compare(wall_id, before.geometry_revision, after.geometry_revision)
        .unwrap();
    assert!(deletion.openings_changed);
    assert_eq!(deletion.openings_before, vec![opening_a, opening_b]);
    assert_eq!(deletion.openings_after, vec![opening_a]);
    assert_eq!(deletion.source_dependency_changes.removed, vec![opening_b]);
    assert!(deletion.source_dependency_changes.added.is_empty());
    assert_eq!(cache.geometry_history_entry_count(), history_before + 1);
    let quantities_after = cache.quantities_get(wall_id).unwrap();
    assert_eq!(quantities_after.quantities, quantities_before.quantities);
    assert_eq!(quantities_after.geometry_revision, after.geometry_revision);
    assert_eq!(
        cache.quantity_cache_metrics().quantity_rebuild_count,
        quantity_rebuilds_before
    );

    document
        .undo()
        .unwrap()
        .expect("opening deletion should undo");
    project_dirty_document(&document, &mut cache);
    assert!(document.element(opening_b).is_some());
    assert!(cache.inspect(opening_b).is_some());
    let after_undo = cache.inspect(wall_id).unwrap().clone();
    assert_eq!(after_undo.openings, vec![opening_a, opening_b]);
    assert!(after_undo.geometry_revision > after.geometry_revision);
    assert_eq!(after_undo.mesh_revision, before.mesh_revision);
    assert_eq!(after_undo.geometry_fingerprint, before.geometry_fingerprint);
    assert_eq!(
        cache
            .scene()
            .get(object_id_for(wall_id))
            .unwrap()
            .mesh
            .triangles(),
        mesh_before.as_slice()
    );
    let restoration = cache
        .compare(
            wall_id,
            after.geometry_revision,
            after_undo.geometry_revision,
        )
        .unwrap();
    assert!(restoration.openings_changed);
    assert_eq!(restoration.source_dependency_changes.added, vec![opening_b]);
    assert!(restoration.source_dependency_changes.removed.is_empty());
    let undo_quantities = cache.quantities_get(wall_id).unwrap();
    assert_eq!(undo_quantities.quantities, quantities_before.quantities);
    assert_eq!(
        undo_quantities.geometry_revision,
        after_undo.geometry_revision
    );

    document
        .redo()
        .unwrap()
        .expect("opening deletion should redo");
    project_dirty_document(&document, &mut cache);
    assert!(document.element(opening_b).is_none());
    assert!(cache.inspect(opening_b).is_none());
    let after_redo = cache.inspect(wall_id).unwrap().clone();
    assert_eq!(after_redo.openings, vec![opening_a]);
    assert!(after_redo.geometry_revision > after_undo.geometry_revision);
    assert_eq!(after_redo.mesh_revision, before.mesh_revision);
    assert_eq!(after_redo.geometry_fingerprint, before.geometry_fingerprint);
    assert_eq!(
        cache
            .scene()
            .get(object_id_for(wall_id))
            .unwrap()
            .mesh
            .triangles(),
        mesh_before.as_slice()
    );
    let redo = cache
        .compare(
            wall_id,
            after_undo.geometry_revision,
            after_redo.geometry_revision,
        )
        .unwrap();
    assert!(redo.openings_changed);
    assert_eq!(redo.source_dependency_changes.removed, vec![opening_b]);
    assert!(redo.source_dependency_changes.added.is_empty());
    let redo_quantities = cache.quantities_get(wall_id).unwrap();
    assert_eq!(redo_quantities.quantities, quantities_before.quantities);
    assert_eq!(
        redo_quantities.geometry_revision,
        after_redo.geometry_revision
    );
}

#[test]
fn rehosting_between_identical_walls_updates_child_provenance_without_rebuilding_its_prism() {
    let mut document = Document::new("Rehosted opening provenance").unwrap();
    let old_host = create_wall(&mut document, "old-host", "Old host");
    let new_host = create_wall(&mut document, "new-host", "New host");
    let opening_id = create_opening(&mut document, "opening", old_host, 0.2);
    let mut cache = GeometryCache::new();
    cache.set_show_opening_volumes(true);
    project_dirty_document(&document, &mut cache);

    let before = cache.inspect(opening_id).unwrap().clone();
    let mesh_before = cache
        .scene()
        .get(object_id_for(opening_id))
        .unwrap()
        .mesh
        .triangles()
        .to_vec();
    let quantities_before = cache.quantities_get(opening_id).unwrap();
    let quantity_rebuilds_before = cache.quantity_cache_metrics().quantity_rebuild_count;
    let old_host_link = document
        .element_relationships(opening_id)
        .into_iter()
        .find(|relationship| {
            relationship.source_id == EntityRef::Element(old_host)
                && relationship.target_id == EntityRef::Element(opening_id)
        })
        .expect("the opening should have a relationship from its old host")
        .id;

    let report = execute(
        &mut document,
        "rehost-opening",
        vec![
            Command::RemoveRelationship {
                relationship_id: old_host_link,
            },
            Command::HostElement {
                host_id: new_host,
                element_id: opening_id,
            },
        ],
    );
    assert_eq!(document.host_of(opening_id), Some(new_host));
    let rebuild = project_dirty_document(&document, &mut cache);

    assert_eq!(rebuild.rebuilt_element_count, 2);
    assert!(rebuild.rebuilt_element_ids.contains(&old_host));
    assert!(rebuild.rebuilt_element_ids.contains(&new_host));
    assert!(!rebuild.rebuilt_element_ids.contains(&opening_id));
    assert!(report.dirty_ids.contains(&old_host));
    assert!(report.dirty_ids.contains(&new_host));
    let after = cache.inspect(opening_id).unwrap().clone();
    assert!(after.geometry_revision > before.geometry_revision);
    assert_eq!(after.geometry_fingerprint, before.geometry_fingerprint);
    assert_eq!(after.mesh_revision, before.mesh_revision);
    assert_eq!(
        cache.element_status(opening_id).unwrap().mesh_revision,
        before.mesh_revision
    );
    assert_eq!(
        cache
            .scene()
            .get(object_id_for(opening_id))
            .unwrap()
            .mesh
            .triangles(),
        mesh_before.as_slice()
    );
    let comparison = cache
        .compare(
            opening_id,
            before.geometry_revision,
            after.geometry_revision,
        )
        .unwrap();
    assert!(!comparison.openings_changed);
    assert_eq!(comparison.source_dependency_changes.removed, vec![old_host]);
    assert_eq!(comparison.source_dependency_changes.added, vec![new_host]);
    let quantities_after = cache.quantities_get(opening_id).unwrap();
    assert_eq!(quantities_after.quantities, quantities_before.quantities);
    assert_eq!(quantities_after.geometry_revision, after.geometry_revision);
    assert_eq!(
        cache.quantity_cache_metrics().quantity_rebuild_count,
        quantity_rebuilds_before + 2,
        "only the old and new host quantity values should be recomputed"
    );
}

#[test]
fn metadata_rename_and_property_source_change_preserve_geometry_and_cached_quantities() {
    let mut document = Document::new("Metadata provenance").unwrap();
    execute(
        &mut document,
        "define-wall-note",
        vec![Command::DefineProperty {
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            definition: PropertyDef::new("finish_note", "Finish note", PropertyType::Text),
        }],
    );
    let wall_id = create_wall(&mut document, "wall", "Wall");
    let mut type_properties = BTreeMap::new();
    type_properties.insert(
        "finish_note".into(),
        PropertyValue::Text("Type metadata".into()),
    );
    let type_id = execute(
        &mut document,
        "create-wall-type",
        vec![Command::CreateType {
            name: "Wall type".into(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            properties: type_properties,
        }],
    )
    .created_types[0];
    execute(
        &mut document,
        "assign-type-and-override-metadata",
        vec![
            Command::AssignType {
                element_id: wall_id,
                type_id,
            },
            Command::SetProperty {
                element_id: wall_id,
                property_id: "finish_note".into(),
                value: PropertyValue::Text("Instance metadata".into()),
            },
        ],
    );
    assert_eq!(
        document.resolved_properties(wall_id).unwrap()[&"finish_note".into()].source,
        PropertySource::InstanceOverride
    );

    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let before = cache.inspect(wall_id).unwrap().clone();
    let quantities_before = cache.quantities_get(wall_id).unwrap();
    let quantity_rebuilds_before = cache.quantity_cache_metrics().quantity_rebuild_count;

    let mut updated_type_properties = BTreeMap::new();
    updated_type_properties.insert(
        "finish_note".into(),
        PropertyValue::Text("Renamed type metadata".into()),
    );
    execute(
        &mut document,
        "rename-type-and-clear-instance-source",
        vec![
            Command::ClearPropertyOverride {
                element_id: wall_id,
                property_id: "finish_note".into(),
            },
            Command::UpdateType {
                type_id,
                name: Some("Renamed wall type".into()),
                properties: updated_type_properties,
            },
        ],
    );
    assert_eq!(
        document.element_type(type_id).unwrap().name,
        "Renamed wall type"
    );
    let resolved = document.resolved_properties(wall_id).unwrap();
    assert_eq!(resolved[&"finish_note".into()].source, PropertySource::Type);
    assert_eq!(
        resolved[&"finish_note".into()].value,
        PropertyValue::Text("Renamed type metadata".into())
    );
    let report = project_dirty_document(&document, &mut cache);

    assert_eq!(report.rebuilt_element_count, 0);
    let after = cache.inspect(wall_id).unwrap();
    assert_eq!(after.geometry_revision, before.geometry_revision);
    assert_eq!(after.mesh_revision, before.mesh_revision);
    assert_eq!(after.geometry_fingerprint, before.geometry_fingerprint);
    let quantities_after = cache.quantities_get(wall_id).unwrap();
    assert_eq!(quantities_after, quantities_before);
    assert_eq!(
        cache.quantity_cache_metrics().quantity_rebuild_count,
        quantity_rebuilds_before
    );
}

#[test]
fn observation_metrics_serialize_maintained_counts_and_clear_with_invalidation() {
    let mut document = Document::new("Observation counts").unwrap();
    let floor_id = create_floor(&mut document, "floor");
    let space_id = create_space(&mut document, "space");
    let wall_id = create_wall(&mut document, "wall", "Wall");
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);

    let initial = cache.observation_metrics();
    let initial_json = serde_json::to_value(&initial).unwrap();
    assert_eq!(initial.record_count, 3);
    assert_eq!(initial.mesh_count, 3);
    assert_eq!(initial.projected_count, 3);
    assert_eq!(initial.geometry_error_count, 0);
    assert_eq!(initial.resolution_error_count, 0);
    assert_eq!(initial.error_count, 0);
    assert_eq!(initial.history_element_count, 3);
    assert_eq!(initial.history_record_count, 3);
    assert_eq!(initial.scanned_history_records, 0);
    assert_eq!(initial.reason, "maintained_counts");
    assert!(initial_json["duration_us"].is_number());
    assert_eq!(initial_json["mesh_count"], 3);
    assert!(cache.scene().get(object_id_for(floor_id)).is_some());
    assert!(cache.scene().get(object_id_for(wall_id)).is_some());
    assert!(cache.scene().get(object_id_for(space_id)).is_none());

    execute(
        &mut document,
        "delete-floor",
        vec![Command::DeleteElement {
            element_id: floor_id,
        }],
    );
    project_dirty_document(&document, &mut cache);
    let after_delete = cache.observation_metrics();
    assert_eq!(after_delete.record_count, 2);
    assert_eq!(after_delete.mesh_count, 2);
    assert_eq!(after_delete.projected_count, 2);
    assert_eq!(after_delete.geometry_error_count, 0);
    assert_eq!(after_delete.resolution_error_count, 0);
    assert_eq!(after_delete.error_count, 0);
    assert_eq!(after_delete.history_element_count, 3);
    assert_eq!(after_delete.history_record_count, 3);
    assert_eq!(cache.geometry_history_entry_count(), 3);

    cache.invalidate_all();
    let cleared = serde_json::to_value(cache.observation_metrics()).unwrap();
    for field in [
        "record_count",
        "mesh_count",
        "projected_count",
        "geometry_error_count",
        "resolution_error_count",
        "error_count",
        "history_element_count",
        "history_record_count",
        "scanned_history_records",
    ] {
        assert_eq!(cleared[field].as_u64(), Some(0), "{field}");
    }
    assert_eq!(cleared["reason"], "maintained_counts");
}
