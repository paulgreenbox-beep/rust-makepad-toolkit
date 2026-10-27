use ai_revit_core::{
    BUILTIN_DOOR_CLASS_ID, BUILTIN_WALL_CLASS_ID, BUILTIN_WINDOW_CLASS_ID, Command, Document,
    ElementId, ElementKind, Point2, PropertyId, PropertyValue, RectangularSpec, Unit, WallSpec,
};
use ai_revit_geometry::{
    GeometryCache, GeometryErrorCode, GeometryOptions, GeometryStatusKind, object_id_for,
    project_dirty_document, project_dirty_document_into_scene,
};
use std::collections::BTreeMap;
use viewer_core::{Ray, Scene, Vec3};

fn execute(
    document: &mut Document,
    id: &str,
    commands: Vec<Command>,
) -> ai_revit_core::CommitReceipt {
    let mut transaction = document.begin_transaction(id, document.revision());
    for command in commands {
        transaction.push(command);
    }
    document.commit(transaction).unwrap()
}

fn wall(name: &str, thickness_m: f64) -> Command {
    wall_with_properties(name, thickness_m, BTreeMap::new())
}

fn wall_with_properties(
    name: &str,
    thickness_m: f64,
    properties: BTreeMap<PropertyId, PropertyValue>,
) -> Command {
    Command::CreateElement {
        name: name.to_owned(),
        class_id: BUILTIN_WALL_CLASS_ID.into(),
        kind: ElementKind::Wall(WallSpec {
            start: Point2::new(0.0, 0.0),
            end: Point2::new(6.0, 0.0),
            base_elevation_m: 0.0,
            height_m: 3.0,
            thickness_m,
        }),
        properties,
    }
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

fn rectangular(elevation_m: f64, width_m: f64, height_m: f64) -> RectangularSpec {
    RectangularSpec {
        center: Point2::new(2.0, 0.0),
        elevation_m,
        width_m,
        height_m,
        depth_m: 0.3,
        rotation_rad: 0.0,
    }
}

fn assert_bounds(bounds: viewer_core::Aabb, min: [f64; 3], max: [f64; 3]) {
    let actual_min = [bounds.min.x, bounds.min.y, bounds.min.z];
    let actual_max = [bounds.max.x, bounds.max.y, bounds.max.z];
    for index in 0..3 {
        assert!((actual_min[index] - min[index]).abs() < 1.0e-9);
        assert!((actual_max[index] - max[index]).abs() < 1.0e-9);
    }
}

#[test]
fn no_change_rebuild_is_zero_and_status_reads_are_cached() {
    let mut document = Document::new("Incremental cache").unwrap();
    let ids = execute(
        &mut document,
        "walls",
        vec![wall("One", 0.2), wall("Two", 0.2)],
    )
    .created_elements;

    let mut cache = GeometryCache::new();
    let first = project_dirty_document(&document, &mut cache);
    assert_eq!(first.rebuilt_element_count, 2);
    assert!(first.document_invalidated);
    for id in &ids {
        let status = cache.element_status(*id).unwrap();
        assert_eq!(status.status, GeometryStatusKind::Valid);
        assert!(status.bounds.is_some());
        assert!(status.mesh_revision > 0);
        assert!(cache.scene().get(object_id_for(*id)).is_some());
    }

    let repeated = project_dirty_document(&document, &mut cache);
    assert_eq!(repeated.rebuilt_element_count, 0);
}

#[test]
fn type_update_rebuilds_only_instances_with_changed_effective_geometry() {
    let mut document = Document::new("Type geometry").unwrap();
    let type_id = execute(
        &mut document,
        "wall-type",
        vec![Command::CreateType {
            name: "Wall 200".into(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            properties: props("thickness", 0.2),
        }],
    )
    .created_types[0];
    let ids = execute(
        &mut document,
        "walls",
        vec![
            wall_with_properties("Override one", 0.2, props("thickness", 0.2)),
            wall("Typed two", 0.2),
            wall("Untyped", 0.2),
        ],
    )
    .created_elements;
    execute(
        &mut document,
        "assign-type",
        ids[..2]
            .iter()
            .map(|element_id| Command::AssignType {
                element_id: *element_id,
                type_id,
            })
            .collect(),
    );

    let mut cache = GeometryCache::new();
    assert_eq!(
        project_dirty_document(&document, &mut cache).rebuilt_element_count,
        3
    );
    let old_revisions: Vec<u64> = ids
        .iter()
        .map(|id| cache.element_status(*id).unwrap().mesh_revision)
        .collect();

    execute(
        &mut document,
        "update-wall-type",
        vec![Command::UpdateType {
            type_id,
            name: None,
            properties: props("thickness", 0.3),
        }],
    );
    let report = project_dirty_document(&document, &mut cache);
    assert_eq!(report.rebuilt_element_ids, vec![ids[1]]);
    assert_eq!(report.rebuilt_element_count, 1);
    assert_eq!(
        cache.element_status(ids[0]).unwrap().mesh_revision,
        old_revisions[0],
        "a type value hidden by an instance override must not rebuild that mesh"
    );
    assert_ne!(
        cache.element_status(ids[1]).unwrap().mesh_revision,
        old_revisions[1]
    );
    assert_eq!(
        cache.element_status(ids[2]).unwrap().mesh_revision,
        old_revisions[2]
    );
}

#[test]
fn host_geometry_change_rebuilds_only_changed_host_mesh() {
    let mut document = Document::new("Hosted geometry").unwrap();
    let wall_id = execute(&mut document, "wall", vec![wall("Host", 0.2)]).created_elements[0];
    let hosted = execute(
        &mut document,
        "opening-and-window",
        vec![
            Command::CreateOpening {
                name: "Opening".into(),
                spec: rectangular(0.8, 1.2, 1.2),
                properties: BTreeMap::new(),
                host_id: wall_id,
                type_id: None,
            },
            Command::CreateElement {
                name: "Window".into(),
                class_id: BUILTIN_WINDOW_CLASS_ID.into(),
                kind: ElementKind::Window(rectangular(0.8, 1.2, 1.2)),
                properties: BTreeMap::new(),
            },
            Command::HostElement {
                host_id: ElementId::new(2),
                element_id: ElementId::new(3),
            },
        ],
    )
    .created_elements;
    let opening_id = hosted[0];
    let window_id = hosted[1];

    let mut cache = GeometryCache::new();
    assert_eq!(
        project_dirty_document(&document, &mut cache).rebuilt_element_count,
        3
    );
    assert_eq!(
        cache.scene().object_ids().collect::<Vec<_>>(),
        vec![
            object_id_for(wall_id),
            object_id_for(window_id)
        ]
    );
    for element_id in [wall_id, opening_id, window_id] {
        assert!(cache.element_status(element_id).unwrap().bounds.is_some());
    }

    execute(
        &mut document,
        "change-host",
        vec![Command::UpdateElement {
            element_id: wall_id,
            kind: ElementKind::Wall(WallSpec {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(6.0, 0.0),
                base_elevation_m: 0.0,
                height_m: 3.0,
                thickness_m: 0.25,
            }),
        }],
    );
    let report = project_dirty_document(&document, &mut cache);
    assert_eq!(
        report.rebuilt_element_ids,
        vec![wall_id]
    );
    assert_eq!(report.rebuilt_element_count, 1);
}

#[test]
fn deleted_elements_are_removed_and_explicit_open_invalidation_rebuilds() {
    let mut document = Document::new("Cache lifecycle").unwrap();
    let ids = execute(
        &mut document,
        "walls",
        vec![wall("Keep", 0.2), wall("Delete", 0.2)],
    )
    .created_elements;
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);

    execute(
        &mut document,
        "delete",
        vec![Command::DeleteElement { element_id: ids[1] }],
    );
    let report = project_dirty_document(&document, &mut cache);
    assert_eq!(report.rebuilt_element_count, 0);
    assert_eq!(report.removed_element_ids, vec![ids[1]]);
    assert!(cache.element_status(ids[1]).is_none());
    assert!(cache.scene().get(object_id_for(ids[1])).is_none());

    let reopened = Document::from_json_str(&document.to_json_string().unwrap()).unwrap();
    cache.invalidate_all();
    let reopened_report = project_dirty_document(&reopened, &mut cache);
    assert!(reopened_report.document_invalidated);
    assert_eq!(reopened_report.rebuilt_element_count, 1);
    assert_eq!(reopened_report.rebuilt_element_ids, vec![ids[0]]);
}

#[test]
fn opening_debug_volume_and_window_have_distinct_semantic_scene_objects() {
    let mut document = Document::new("Opening display").unwrap();
    let wall_id = execute(&mut document, "wall", vec![wall("Host", 0.2)]).created_elements[0];
    let hosted = execute(
        &mut document,
        "opening-and-window",
        vec![
            Command::CreateOpening {
                name: "Opening".into(),
                spec: rectangular(0.8, 1.2, 1.2),
                properties: BTreeMap::new(),
                host_id: wall_id,
                type_id: None,
            },
            Command::CreateElement {
                name: "Window".into(),
                class_id: BUILTIN_WINDOW_CLASS_ID.into(),
                kind: ElementKind::Window(rectangular(0.8, 1.2, 1.2)),
                properties: BTreeMap::new(),
            },
            Command::HostElement {
                host_id: ElementId::new(2),
                element_id: ElementId::new(3),
            },
        ],
    )
    .created_elements;
    let mut cache = GeometryCache::new();
    cache.rebuild(&document);
    assert!(cache.scene().get(object_id_for(hosted[0])).is_none());
    assert!(cache.record(hosted[0]).unwrap().valid);
    cache.set_show_opening_volumes(true);
    let opening = cache.scene().get(object_id_for(hosted[0])).unwrap();
    let window = cache.scene().get(object_id_for(hosted[1])).unwrap();
    assert_eq!(opening.name, "Opening");
    assert_eq!(window.name, "Window");
    assert_ne!(opening.color, window.color);
    assert_ne!(hosted[0], hosted[1]);
}

#[test]
fn deleting_undoing_and_redoing_preserves_the_stable_projection_id() {
    let mut document = Document::new("Delete undo redo").unwrap();
    let id = execute(&mut document, "wall", vec![wall("Restored", 0.2)]).created_elements[0];
    let stable_object_id = object_id_for(id);
    assert_eq!(stable_object_id, id.get());

    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let first_mesh_revision = cache.element_status(id).unwrap().mesh_revision;

    execute(
        &mut document,
        "delete-wall",
        vec![Command::DeleteElement { element_id: id }],
    );
    let deleted = project_dirty_document(&document, &mut cache);
    assert_eq!(deleted.rebuilt_element_count, 0);
    assert_eq!(deleted.removed_element_ids, vec![id]);
    assert!(cache.element_status(id).is_none());
    assert!(cache.scene().get(stable_object_id).is_none());

    document.undo().unwrap().unwrap();
    let restored = project_dirty_document(&document, &mut cache);
    assert_eq!(restored.rebuilt_element_ids, vec![id]);
    assert_eq!(restored.rebuilt_element_count, 1);
    assert_eq!(object_id_for(id), stable_object_id);
    let restored_status = cache.element_status(id).unwrap();
    assert_eq!(restored_status.status, GeometryStatusKind::Valid);
    assert!(restored_status.mesh_revision > first_mesh_revision);
    assert!(cache.scene().get(stable_object_id).is_some());

    document.redo().unwrap().unwrap();
    let redone = project_dirty_document(&document, &mut cache);
    assert_eq!(redone.removed_element_ids, vec![id]);
    assert!(cache.element_status(id).is_none());
    assert!(cache.scene().get(stable_object_id).is_none());
}

#[test]
fn replacing_the_document_uuid_invalidates_the_previous_projection() {
    let mut original = Document::new("Original document").unwrap();
    execute(
        &mut original,
        "original-walls",
        vec![wall("Shared ID", 0.2), wall("Old only", 0.2)],
    );
    let mut cache = GeometryCache::new();
    assert_eq!(
        project_dirty_document(&original, &mut cache).rebuilt_element_count,
        2
    );

    let mut replacement = Document::new("Replacement document").unwrap();
    let replacement_id = execute(
        &mut replacement,
        "replacement-wall",
        vec![wall("Shared ID", 0.2)],
    )
    .created_elements[0];
    assert_ne!(original.document_uuid, replacement.document_uuid);

    let report = project_dirty_document(&replacement, &mut cache);
    assert!(report.document_invalidated);
    assert_eq!(report.rebuilt_element_ids, vec![replacement_id]);
    assert_eq!(report.rebuilt_element_count, 1);
    assert_eq!(
        cache.scene().object_ids().collect::<Vec<_>>(),
        vec![object_id_for(replacement_id)]
    );
}

#[test]
fn invalid_geometry_status_and_failure_are_cached_until_the_input_changes() {
    let mut document = Document::new("Invalid projection status").unwrap();
    let id = execute(&mut document, "wall", vec![wall("Wall", 0.2)]).created_elements[0];
    let mut cache = GeometryCache::with_options(GeometryOptions {
        max_vertices_per_element: 3,
        ..GeometryOptions::default()
    });

    let first = project_dirty_document(&document, &mut cache);
    assert_eq!(first.rebuilt_element_ids, vec![id]);
    let status = cache.element_status(id).unwrap();
    assert_eq!(status.status, GeometryStatusKind::Invalid);
    assert!(status.bounds.is_none());
    assert_eq!(
        status.failure.as_ref().map(|failure| failure.code),
        Some(GeometryErrorCode::VertexLimitExceeded)
    );
    assert!(cache.scene().get(object_id_for(id)).is_none());

    let repeated = project_dirty_document(&document, &mut cache);
    assert_eq!(repeated.rebuilt_element_count, 0);
    assert_eq!(
        cache.element_status(id).unwrap().status,
        GeometryStatusKind::Invalid
    );
}

#[test]
fn openings_doors_and_windows_keep_separate_objects_at_their_placements() {
    let mut document = Document::new("Hosted object placement").unwrap();
    let wall_id = execute(&mut document, "wall", vec![wall("Host", 0.2)]).created_elements[0];
    let created_ids = execute(
        &mut document,
        "opening-door-window",
        vec![
            Command::CreateOpening {
                name: "Opening".into(),
                spec: RectangularSpec {
                    center: Point2::new(3.0, 0.0),
                    elevation_m: 0.0,
                    width_m: 1.4,
                    height_m: 2.1,
                    depth_m: 0.2,
                    rotation_rad: 0.0,
                },
                properties: BTreeMap::new(),
                host_id: wall_id,
                type_id: None,
            },
            Command::CreateElement {
                name: "Door".into(),
                class_id: BUILTIN_DOOR_CLASS_ID.into(),
                kind: ElementKind::Door(RectangularSpec {
                    center: Point2::new(3.0, 0.0),
                    elevation_m: 0.0,
                    width_m: 0.9,
                    height_m: 2.0,
                    depth_m: 0.06,
                    rotation_rad: 0.0,
                }),
                properties: BTreeMap::new(),
            },
            Command::CreateElement {
                name: "Window".into(),
                class_id: BUILTIN_WINDOW_CLASS_ID.into(),
                kind: ElementKind::Window(RectangularSpec {
                    center: Point2::new(3.0, 0.0),
                    elevation_m: 0.8,
                    width_m: 1.2,
                    height_m: 1.0,
                    depth_m: 0.08,
                    rotation_rad: 0.0,
                }),
                properties: BTreeMap::new(),
            },
            Command::HostElement {
                host_id: ElementId::new(2),
                element_id: ElementId::new(3),
            },
            Command::HostElement {
                host_id: ElementId::new(2),
                element_id: ElementId::new(4),
            },
        ],
    )
    .created_elements;
    let opening_id = created_ids[0];
    let door_id = created_ids[1];
    let window_id = created_ids[2];

    let mut cache = GeometryCache::new();
    let report = project_dirty_document(&document, &mut cache);
    assert_eq!(report.rebuilt_element_count, 4);
    assert_eq!(
        cache.scene().object_ids().collect::<Vec<_>>(),
        vec![
            object_id_for(wall_id),
            object_id_for(door_id),
            object_id_for(window_id),
        ]
    );

    assert_bounds(
        cache.element_status(opening_id).unwrap().bounds.unwrap(),
        [2.3, -0.1, 0.0],
        [3.7, 0.1, 2.1],
    );
    assert_bounds(
        cache.element_status(door_id).unwrap().bounds.unwrap(),
        [2.55, -0.03, 0.0],
        [3.45, 0.03, 2.0],
    );
    assert_bounds(
        cache.element_status(window_id).unwrap().bounds.unwrap(),
        [2.4, -0.04, 0.8],
        [3.6, 0.04, 1.8],
    );
    let host_mesh = cache.scene().get(object_id_for(wall_id)).unwrap();
    assert!(
        host_mesh
            .mesh
            .pick(
                object_id_for(wall_id),
                Ray::new(Vec3::new(3.0, -1.0, 1.0), Vec3::new(0.0, 1.0, 0.0)),
                10.0,
            )
            .is_none(),
        "the semantic Opening cuts a real gap through the host mesh"
    );
    for id in [opening_id, door_id, window_id] {
        assert_eq!(
            cache
                .projected_elements()
                .find(|projected| projected.element_id == id)
                .unwrap()
                .quality,
            ai_revit_geometry::GeometryQuality::ApproximateTriangleMesh
        );
    }
}

#[test]
fn rebuilding_into_a_viewer_scene_preserves_object_state_and_removes_deletions() {
    let mut document = Document::new("Owned viewer scene").unwrap();
    let id = execute(&mut document, "wall", vec![wall("Wall", 0.2)]).created_elements[0];
    let mut cache = GeometryCache::new();
    let mut viewer_scene = Scene::new();

    let initial = project_dirty_document_into_scene(&document, &mut cache, &mut viewer_scene);
    assert_eq!(initial.rebuilt_element_ids, vec![id]);
    assert!(viewer_scene.get(object_id_for(id)).is_some());
    {
        let object = viewer_scene.get_mut(object_id_for(id)).unwrap();
        object.selected = true;
        object.visible = false;
    }

    execute(
        &mut document,
        "widen-wall",
        vec![Command::UpdateElement {
            element_id: id,
            kind: ElementKind::Wall(WallSpec {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(6.0, 0.0),
                base_elevation_m: 0.0,
                height_m: 3.0,
                thickness_m: 0.3,
            }),
        }],
    );
    let updated = project_dirty_document_into_scene(&document, &mut cache, &mut viewer_scene);
    assert_eq!(updated.rebuilt_element_ids, vec![id]);
    let object = viewer_scene.get(object_id_for(id)).unwrap();
    assert!(object.selected);
    assert!(!object.visible);

    let repeated = project_dirty_document_into_scene(&document, &mut cache, &mut viewer_scene);
    assert_eq!(repeated.rebuilt_element_count, 0);

    execute(
        &mut document,
        "delete-wall",
        vec![Command::DeleteElement { element_id: id }],
    );
    let deleted = project_dirty_document_into_scene(&document, &mut cache, &mut viewer_scene);
    assert_eq!(deleted.removed_element_ids, vec![id]);
    assert!(viewer_scene.get(object_id_for(id)).is_none());
}
