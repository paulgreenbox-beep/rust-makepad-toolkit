use ai_revit_core::{
    BUILTIN_WALL_CLASS_ID, BUILTIN_WINDOW_CLASS_ID, Command, DeleteStrategy, Document, ElementId,
    ElementKind, OpeningSpec, Point2, PropertyDef, PropertyType, PropertyValue, WallSpec,
};
use ai_revit_geometry::{
    GeometryBackend, GeometryCache, GeometryInput, GeometryOptions, MeshGeometryBackend,
    RepresentationQuality, SectionPlane, object_id_for, project_dirty_document,
};
use std::collections::BTreeMap;
use viewer_core::{Aabb, Ray, Vec3};

const EPSILON: f64 = 1.0e-8;

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

fn wall_spec(thickness_m: f64) -> WallSpec {
    WallSpec {
        start: Point2::new(0.0, 0.0),
        end: Point2::new(6.0, 0.0),
        base_elevation_m: 0.0,
        height_m: 3.0,
        thickness_m,
    }
}

fn create_wall(document: &mut Document, command_id: &str) -> ElementId {
    let expected_id = document.allocation_preview().element_id;
    let created = execute(
        document,
        command_id,
        vec![Command::CreateElement {
            name: "Test wall".into(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            kind: ElementKind::Wall(wall_spec(0.2)),
            properties: BTreeMap::new(),
        }],
    )
    .created_elements;
    assert_eq!(
        created,
        vec![expected_id],
        "fixture must use the allocation preview"
    );
    expected_id
}

fn opening_spec(center_x: f64, elevation_m: f64, width_m: f64, height_m: f64) -> OpeningSpec {
    OpeningSpec {
        center: Point2::new(center_x, 0.0),
        elevation_m,
        width_m,
        height_m,
        // The void must span the complete 0.2 m wall thickness.
        depth_m: 0.2,
        rotation_rad: 0.0,
    }
}

fn create_opening(
    document: &mut Document,
    command_id: &str,
    host_id: ElementId,
    spec: OpeningSpec,
) -> ElementId {
    let expected_id = document.allocation_preview().element_id;
    let created = execute(
        document,
        command_id,
        vec![Command::CreateOpening {
            name: "Hosted opening".into(),
            spec,
            properties: BTreeMap::new(),
            host_id,
            type_id: None,
        }],
    )
    .created_elements;
    assert_eq!(
        created,
        vec![expected_id],
        "fixture must use the allocation preview"
    );
    expected_id
}

fn create_window(
    document: &mut Document,
    command_id: &str,
    opening_id: ElementId,
    spec: OpeningSpec,
) -> ElementId {
    let expected_id = document.allocation_preview().element_id;
    let created = execute(
        document,
        command_id,
        vec![
            Command::CreateElement {
                name: "Hosted window".into(),
                class_id: BUILTIN_WINDOW_CLASS_ID.into(),
                kind: ElementKind::Window(spec),
                properties: BTreeMap::new(),
            },
            Command::HostElement {
                host_id: opening_id,
                element_id: expected_id,
            },
        ],
    )
    .created_elements;
    assert_eq!(
        created,
        vec![expected_id],
        "fixture must use the allocation preview"
    );
    expected_id
}

fn single_opening_fixture() -> (Document, ElementId, ElementId) {
    let mut document = Document::new("Hosted opening regression").unwrap();
    let wall_id = create_wall(&mut document, "create-wall");
    let opening_id = create_opening(
        &mut document,
        "create-opening",
        wall_id,
        opening_spec(2.0, 0.8, 1.2, 1.5),
    );
    (document, wall_id, opening_id)
}

fn volume(cache: &GeometryCache, element_id: ElementId) -> f64 {
    cache
        .inspect(element_id)
        .expect("derived geometry record should exist")
        .volume
        .expect("valid geometry should have a volume")
}

fn assert_near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < EPSILON,
        "expected {expected}, got {actual}"
    );
}

fn assert_wall_bounds(bounds: Option<Aabb>) {
    let bounds = bounds.expect("valid wall geometry should have bounds");
    assert_near(bounds.min.x, 0.0);
    assert_near(bounds.min.y, -0.1);
    assert_near(bounds.min.z, 0.0);
    assert_near(bounds.max.x, 6.0);
    assert_near(bounds.max.y, 0.1);
    assert_near(bounds.max.z, 3.0);
}

fn through_wall_ray(x: f64, z: f64) -> Ray {
    Ray::new(Vec3::new(x, -1.0, z), Vec3::new(0.0, 1.0, 0.0))
}

#[test]
fn one_hosted_opening_cuts_the_derived_wall_and_preserves_its_bounds() {
    let (document, wall_id, opening_id) = single_opening_fixture();
    let source_wall = document.element(wall_id).unwrap().kind.clone();
    let mut cache = GeometryCache::new();

    let report = project_dirty_document(&document, &mut cache);
    assert!(report.rebuilt_element_ids.contains(&wall_id));
    let wall = cache.inspect(wall_id).expect("wall record");
    assert!(wall.valid);
    assert_eq!(
        wall.representation_quality,
        RepresentationQuality::ApproximateMesh
    );
    assert_eq!(wall.openings, vec![opening_id]);
    assert!(wall.derived_from.contains(&wall_id));
    assert!(wall.derived_from.contains(&opening_id));
    assert!(wall.triangle_count > 0);
    assert_wall_bounds(wall.bounds);
    // 6 x 3 x 0.2 minus 1.2 x 1.5 x 0.2.
    assert_near(wall.volume.unwrap(), 3.24);

    let host_mesh = &cache.scene().get(object_id_for(wall_id)).unwrap().mesh;
    assert!(
        host_mesh
            .pick(object_id_for(wall_id), through_wall_ray(2.0, 1.5), 2.0)
            .is_none(),
        "a ray through the void must not hit the wall"
    );
    assert!(
        host_mesh
            .pick(object_id_for(wall_id), through_wall_ray(4.0, 1.5), 2.0)
            .is_some(),
        "the adjacent solid wall must remain pickable"
    );
    assert_eq!(document.element(wall_id).unwrap().kind, source_wall);
}

#[test]
fn separated_openings_are_both_cut_and_reported_in_stable_id_order() {
    let mut document = Document::new("Multiple hosted openings").unwrap();
    let wall_id = create_wall(&mut document, "create-wall");
    let first = create_opening(
        &mut document,
        "create-opening-one",
        wall_id,
        opening_spec(1.5, 0.8, 1.2, 1.5),
    );
    let second = create_opening(
        &mut document,
        "create-opening-two",
        wall_id,
        opening_spec(4.5, 0.8, 1.2, 1.5),
    );
    let mut cache = GeometryCache::new();

    project_dirty_document(&document, &mut cache);
    let wall = cache.inspect(wall_id).unwrap();
    assert!(wall.valid);
    assert_eq!(wall.openings, vec![first, second]);
    assert_near(wall.volume.unwrap(), 2.88);
    let mesh = &cache.scene().get(object_id_for(wall_id)).unwrap().mesh;
    for x in [1.5, 4.5] {
        assert!(
            mesh.pick(object_id_for(wall_id), through_wall_ray(x, 1.5), 2.0)
                .is_none(),
            "opening centered at x={x} must leave a through-hole"
        );
    }
    assert!(
        mesh.pick(object_id_for(wall_id), through_wall_ray(3.0, 1.5), 2.0)
            .is_some(),
        "the wall between separated openings remains solid"
    );
}

fn overlapping_fixture(creation_order: [f64; 2]) -> (Document, ElementId, Vec<ElementId>) {
    let mut document = Document::new("Overlapping hosted openings").unwrap();
    let wall_id = create_wall(&mut document, "create-wall");
    let mut openings = Vec::new();
    for (index, center_x) in creation_order.into_iter().enumerate() {
        openings.push(create_opening(
            &mut document,
            &format!("create-overlap-{index}"),
            wall_id,
            opening_spec(center_x, 0.8, 1.2, 1.5),
        ));
    }
    (document, wall_id, openings)
}

fn derive_host_directly(
    document: &Document,
    wall_id: ElementId,
    opening_ids: &[ElementId],
) -> ai_revit_geometry::DerivedGeometry {
    let mut derived_from = vec![wall_id];
    derived_from.extend(opening_ids.iter().copied());
    derived_from.sort_unstable();
    let input = GeometryInput {
        element: document.effective_element(wall_id).unwrap(),
        openings: opening_ids
            .iter()
            .map(|id| document.effective_element(*id).unwrap())
            .collect(),
        derived_from,
    };
    MeshGeometryBackend
        .derive_element(&input, GeometryOptions::default())
        .expect("valid hosted openings should derive geometry")
}

#[test]
fn overlapping_openings_use_a_union_independent_of_input_and_command_order() {
    let (document_a, wall_a, opening_ids_a) = overlapping_fixture([2.0, 2.8]);
    let (document_b, wall_b, opening_ids_b) = overlapping_fixture([2.8, 2.0]);

    let forward = derive_host_directly(&document_a, wall_a, &opening_ids_a);
    let mut reversed_ids = opening_ids_a.clone();
    reversed_ids.reverse();
    let reverse = derive_host_directly(&document_a, wall_a, &reversed_ids);
    let command_reversed = derive_host_directly(&document_b, wall_b, &opening_ids_b);

    // The union spans 2.0 m horizontally, 1.5 m vertically, through 0.2 m depth.
    for result in [&forward, &reverse, &command_reversed] {
        assert_near(result.volume.unwrap(), 3.0);
        assert!(result.mesh.triangles().len() > 0);
        assert_eq!(
            result.representation_quality,
            RepresentationQuality::ApproximateMesh
        );
    }
    assert_eq!(forward.mesh.triangles(), reverse.mesh.triangles());
    assert_eq!(forward.mesh.triangles(), command_reversed.mesh.triangles());
    assert_eq!(forward.bounds, reverse.bounds);
    assert_eq!(forward.surface_area, reverse.surface_area);
}

#[test]
fn moving_an_opening_moves_the_void_and_rebuilds_the_host() {
    let (mut document, wall_id, opening_id) = single_opening_fixture();
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let old_wall_revision = cache.inspect(wall_id).unwrap().geometry_revision;
    let old_opening_fingerprint = cache
        .inspect(opening_id)
        .unwrap()
        .geometry_fingerprint
        .clone();
    let mut moved = match document.element(opening_id).unwrap().kind.clone() {
        ElementKind::Opening(spec) => spec,
        other => panic!("expected opening, got {other:?}"),
    };
    moved.center.x = 3.2;

    execute(
        &mut document,
        "move-opening",
        vec![Command::UpdateElement {
            element_id: opening_id,
            kind: ElementKind::Opening(moved),
        }],
    );
    let report = project_dirty_document(&document, &mut cache);

    assert!(report.rebuilt_element_ids.contains(&opening_id));
    assert!(report.rebuilt_element_ids.contains(&wall_id));
    assert!(cache.inspect(wall_id).unwrap().geometry_revision > old_wall_revision);
    assert_ne!(
        cache.inspect(opening_id).unwrap().geometry_fingerprint,
        old_opening_fingerprint
    );
    assert_near(volume(&cache, wall_id), 3.24);
    let wall_mesh = &cache.scene().get(object_id_for(wall_id)).unwrap().mesh;
    assert!(
        wall_mesh
            .pick(object_id_for(wall_id), through_wall_ray(3.2, 1.5), 2.0)
            .is_none()
    );
    assert!(
        wall_mesh
            .pick(object_id_for(wall_id), through_wall_ray(2.0, 1.5), 2.0)
            .is_some()
    );
}

#[test]
fn resizing_an_opening_changes_host_volume_and_geometry_revision() {
    let (mut document, wall_id, opening_id) = single_opening_fixture();
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let before = cache.inspect(wall_id).unwrap().geometry_revision;
    let mut resized = match document.element(opening_id).unwrap().kind.clone() {
        ElementKind::Opening(spec) => spec,
        other => panic!("expected opening, got {other:?}"),
    };
    resized.width_m = 1.6;

    execute(
        &mut document,
        "resize-opening",
        vec![Command::UpdateElement {
            element_id: opening_id,
            kind: ElementKind::Opening(resized),
        }],
    );
    let report = project_dirty_document(&document, &mut cache);

    assert!(report.rebuilt_element_ids.contains(&wall_id));
    assert!(cache.inspect(wall_id).unwrap().geometry_revision > before);
    // The opening gains 0.4 x 1.5 x 0.2 m of void volume.
    assert_near(volume(&cache, wall_id), 3.12);
}

#[test]
fn deleting_an_opening_restores_the_full_wall_and_compare_reports_removed_void() {
    let (mut document, wall_id, opening_id) = single_opening_fixture();
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let before = cache.inspect(wall_id).unwrap().geometry_revision;

    execute(
        &mut document,
        "delete-opening",
        vec![Command::DeleteElementWithStrategy {
            element_id: opening_id,
            strategy: DeleteStrategy::Detach,
        }],
    );
    let report = project_dirty_document(&document, &mut cache);

    assert!(report.removed_element_ids.contains(&opening_id));
    assert!(report.rebuilt_element_ids.contains(&wall_id));
    assert!(cache.inspect(opening_id).is_none());
    let wall = cache.inspect(wall_id).unwrap();
    assert!(wall.openings.is_empty());
    assert_near(wall.volume.unwrap(), 3.6);
    let comparison = cache
        .compare(wall_id, before, wall.geometry_revision)
        .expect("both geometry revisions should be comparable");
    assert!(comparison.openings_changed);
    assert_near(comparison.volume_delta.unwrap(), 0.36);
}

#[test]
fn undo_and_redo_restore_the_opening_cut_with_history() {
    let (mut document, wall_id, opening_id) = single_opening_fixture();
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    assert_near(volume(&cache, wall_id), 3.24);

    let mut widened = match document.element(opening_id).unwrap().kind.clone() {
        ElementKind::Opening(spec) => spec,
        other => panic!("expected opening, got {other:?}"),
    };
    widened.width_m = 1.8;
    execute(
        &mut document,
        "widen-opening",
        vec![Command::UpdateElement {
            element_id: opening_id,
            kind: ElementKind::Opening(widened),
        }],
    );
    project_dirty_document(&document, &mut cache);
    assert_near(volume(&cache, wall_id), 3.06);

    assert!(document.undo().unwrap().is_some());
    project_dirty_document(&document, &mut cache);
    assert_near(volume(&cache, wall_id), 3.24);

    assert!(document.redo().unwrap().is_some());
    project_dirty_document(&document, &mut cache);
    assert_near(volume(&cache, wall_id), 3.06);
}

#[test]
fn reopening_a_saved_document_rederives_its_opening_cut() {
    let (document, wall_id, opening_id) = single_opening_fixture();
    let serialized = document.to_json_string().unwrap();
    let reopened = Document::from_json_str(&serialized).unwrap();
    let mut cache = GeometryCache::new();

    let report = project_dirty_document(&reopened, &mut cache);
    assert!(report.document_invalidated);
    let wall = cache.inspect(wall_id).expect("reopened wall record");
    assert!(wall.valid);
    assert_eq!(wall.openings, vec![opening_id]);
    assert_near(wall.volume.unwrap(), 3.24);
}

#[test]
fn window_fills_the_opening_without_defining_the_wall_cut_and_is_pickable() {
    let (mut document, wall_id, opening_id) = single_opening_fixture();
    let source_wall = document.element(wall_id).unwrap().kind.clone();
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let before = cache.inspect(wall_id).unwrap().clone();

    let window_id = create_window(
        &mut document,
        "create-window",
        opening_id,
        opening_spec(2.0, 0.95, 0.8, 1.2),
    );
    project_dirty_document(&document, &mut cache);
    let after_window = cache.inspect(wall_id).unwrap().clone();
    assert_eq!(after_window.geometry_revision, before.geometry_revision);
    assert_eq!(
        after_window.geometry_fingerprint,
        before.geometry_fingerprint
    );
    assert_near(after_window.volume.unwrap(), 3.24);
    assert_eq!(document.element(wall_id).unwrap().kind, source_wall);

    let mut resized_window = match document.element(window_id).unwrap().kind.clone() {
        ElementKind::Window(spec) => spec,
        other => panic!("expected window, got {other:?}"),
    };
    resized_window.width_m = 1.0;
    execute(
        &mut document,
        "resize-window",
        vec![Command::UpdateElement {
            element_id: window_id,
            kind: ElementKind::Window(resized_window),
        }],
    );
    project_dirty_document(&document, &mut cache);
    assert_eq!(
        cache.inspect(wall_id).unwrap().geometry_revision,
        before.geometry_revision,
        "changing the fill must not rebuild the wall void"
    );
    let ray = through_wall_ray(2.0, 1.5);
    let wall_mesh = &cache.scene().get(object_id_for(wall_id)).unwrap().mesh;
    let window_mesh = &cache.scene().get(object_id_for(window_id)).unwrap().mesh;
    assert!(
        wall_mesh.pick(object_id_for(wall_id), ray, 2.0).is_none(),
        "the wall must not occupy the opening"
    );
    assert!(
        window_mesh
            .pick(object_id_for(window_id), ray, 2.0)
            .is_some(),
        "the hosted window should be pickable through the opening"
    );
}

#[test]
fn metadata_type_assignment_and_type_name_changes_do_not_rebuild_identical_geometry() {
    let mut document = Document::new("Metadata and geometry fingerprint").unwrap();
    execute(
        &mut document,
        "define-description",
        vec![Command::DefineProperty {
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            definition: PropertyDef::new("description", "Description", PropertyType::Text),
        }],
    );
    let type_id = execute(
        &mut document,
        "create-wall-type",
        vec![Command::CreateType {
            name: "Geometry-neutral type".into(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            properties: BTreeMap::new(),
        }],
    )
    .created_types[0];
    let wall_id = create_wall(&mut document, "create-wall");
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let initial = cache.inspect(wall_id).unwrap().clone();

    execute(
        &mut document,
        "assign-wall-type",
        vec![Command::AssignType {
            element_id: wall_id,
            type_id,
        }],
    );
    let assigned = project_dirty_document(&document, &mut cache);
    assert_eq!(assigned.rebuilt_element_count, 0);
    assert_eq!(
        cache.inspect(wall_id).unwrap().geometry_revision,
        initial.geometry_revision
    );
    assert_eq!(
        cache.inspect(wall_id).unwrap().geometry_fingerprint,
        initial.geometry_fingerprint
    );

    execute(
        &mut document,
        "rename-wall-type",
        vec![Command::UpdateType {
            type_id,
            name: Some("Renamed geometry-neutral type".into()),
            properties: BTreeMap::new(),
        }],
    );
    assert_eq!(
        project_dirty_document(&document, &mut cache).rebuilt_element_count,
        0
    );

    execute(
        &mut document,
        "set-wall-description",
        vec![Command::SetProperty {
            element_id: wall_id,
            property_id: "description".into(),
            value: PropertyValue::Text("Facade facing the courtyard".into()),
        }],
    );
    assert_eq!(
        project_dirty_document(&document, &mut cache).rebuilt_element_count,
        0
    );
    let final_record = cache.inspect(wall_id).unwrap();
    assert_eq!(final_record.geometry_revision, initial.geometry_revision);
    assert_eq!(
        final_record.geometry_fingerprint,
        initial.geometry_fingerprint
    );
}

#[test]
fn editing_an_unrelated_wall_does_not_rebuild_the_host_with_an_opening() {
    let (mut document, wall_id, _) = single_opening_fixture();
    let other_id = create_wall(&mut document, "create-unrelated-wall");
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let host_revision = cache.inspect(wall_id).unwrap().geometry_revision;
    let mut other = match document.element(other_id).unwrap().kind.clone() {
        ElementKind::Wall(spec) => spec,
        other => panic!("expected wall, got {other:?}"),
    };
    other.thickness_m = 0.3;

    execute(
        &mut document,
        "resize-unrelated-wall",
        vec![Command::UpdateElement {
            element_id: other_id,
            kind: ElementKind::Wall(other),
        }],
    );
    let report = project_dirty_document(&document, &mut cache);

    assert_eq!(report.rebuilt_element_ids, vec![other_id]);
    assert_eq!(
        cache.inspect(wall_id).unwrap().geometry_revision,
        host_revision
    );
    assert_near(volume(&cache, wall_id), 3.24);
}

#[test]
fn oversized_opening_creation_is_rejected_without_advancing_document_state() {
    let mut document = Document::new("Reject oversized opening").unwrap();
    let wall_id = create_wall(&mut document, "create-wall");
    let before = document.to_json_string().unwrap();
    let expected_opening_id = document.allocation_preview().element_id;
    let transaction = document
        .begin_transaction("oversized-opening", document.revision())
        .with(Command::CreateOpening {
            name: "Too wide".into(),
            spec: opening_spec(3.0, 0.8, 7.0, 1.5),
            properties: BTreeMap::new(),
            host_id: wall_id,
            type_id: None,
        });

    assert!(
        document.commit(transaction).is_err(),
        "an opening extending past its host must be rejected"
    );
    assert_eq!(document.to_json_string().unwrap(), before);
    assert!(document.element(expected_opening_id).is_none());
    assert_eq!(
        document.allocation_preview().element_id,
        expected_opening_id,
        "a rejected transaction must not consume the opening ID"
    );
}

#[test]
fn backend_rejects_an_opening_that_does_not_span_its_host_thickness() {
    let mut document = Document::new("Opening depth validation").unwrap();
    let wall_id = create_wall(&mut document, "create-wall");
    let mut shallow = opening_spec(2.0, 0.8, 1.2, 1.5);
    shallow.depth_m = 0.1;
    let opening_id = create_opening(&mut document, "create-shallow-opening", wall_id, shallow);
    let before = document.to_json_string().unwrap();
    let input = GeometryInput {
        element: document.effective_element(wall_id).unwrap(),
        openings: vec![document.effective_element(opening_id).unwrap()],
        derived_from: vec![wall_id, opening_id],
    };

    let error = MeshGeometryBackend
        .derive_element(&input, GeometryOptions::default())
        .err()
        .expect("a shallow opening must be rejected by the geometry backend");
    assert_eq!(error.error_code, "opening_depth_insufficient");
    assert_eq!(error.host_id, Some(wall_id));
    assert_eq!(error.opening_id, Some(opening_id));
    assert_eq!(document.to_json_string().unwrap(), before);

    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let record = cache.inspect(wall_id).unwrap();
    assert!(!record.valid);
    let failure = record.failure.as_ref().unwrap();
    assert_eq!(failure.error_code, "opening_depth_insufficient");
    assert_eq!(failure.host_id, Some(wall_id));
    assert_eq!(failure.opening_id, Some(opening_id));
}

#[test]
fn opening_debug_volume_mode_is_off_by_default_and_can_be_toggled() {
    let (document, wall_id, opening_id) = single_opening_fixture();
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);

    assert!(!cache.show_opening_volumes());
    assert!(cache.inspect(opening_id).unwrap().valid);
    cache.set_show_opening_volumes(true);
    assert!(cache.show_opening_volumes());
    assert!(
        cache
            .scene()
            .get(object_id_for(opening_id))
            .is_some_and(|object| object.visible),
        "enabling debug mode should show the opening volume"
    );
    assert_eq!(cache.inspect(wall_id).unwrap().openings, vec![opening_id]);

    cache.set_show_opening_volumes(false);
    assert!(!cache.show_opening_volumes());
    assert!(
        !cache
            .scene()
            .get(object_id_for(opening_id))
            .is_some_and(|object| object.visible),
        "disabling debug mode should hide the opening volume"
    );
}

#[test]
fn geometry_compare_reports_deltas_between_cached_revisions() {
    let (mut document, wall_id, opening_id) = single_opening_fixture();
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let before = cache.inspect(wall_id).unwrap().clone();
    let mut resized = match document.element(opening_id).unwrap().kind.clone() {
        ElementKind::Opening(spec) => spec,
        other => panic!("expected opening, got {other:?}"),
    };
    resized.width_m = 1.6;
    execute(
        &mut document,
        "resize-for-compare",
        vec![Command::UpdateElement {
            element_id: opening_id,
            kind: ElementKind::Opening(resized),
        }],
    );
    project_dirty_document(&document, &mut cache);
    let after = cache.inspect(wall_id).unwrap().clone();

    let comparison = cache
        .compare(wall_id, before.geometry_revision, after.geometry_revision)
        .expect("both geometry revisions should be comparable");
    assert!(!comparison.bounds_changed);
    assert!(comparison.openings_changed);
    assert_near(
        comparison.volume_delta.unwrap(),
        after.volume.unwrap() - before.volume.unwrap(),
    );
    assert_near(comparison.volume_delta.unwrap(), -0.12);
    assert_near(
        comparison.surface_area_delta.unwrap(),
        after.surface_area.unwrap() - before.surface_area.unwrap(),
    );
    assert_eq!(
        comparison.triangle_count_delta,
        after.triangle_count as i64 - before.triangle_count as i64
    );
}

#[test]
fn section_returns_closed_polylines_for_the_wall_and_its_opening() {
    let (document, wall_id, _) = single_opening_fixture();
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);

    let section = cache
        .section(
            &[wall_id],
            SectionPlane {
                origin: Vec3::new(0.0, 0.0, 0.0),
                normal: Vec3::new(0.0, 1.0, 0.0),
            },
        )
        .expect("a valid section plane should produce section geometry");
    let wall_loops: Vec<_> = section
        .loops
        .iter()
        .filter(|line| line.element_id == wall_id)
        .collect();
    assert!(
        wall_loops.len() >= 2,
        "the section should include the outer wall contour and the opening contour"
    );
    assert!(wall_loops.iter().all(|line| line.closed));
    assert!(
        wall_loops
            .iter()
            .flat_map(|line| &line.points)
            .all(|point| point.x.is_finite() && point.y.is_finite() && point.z.is_finite())
    );
    assert!(
        wall_loops
            .iter()
            .flat_map(|line| &line.points)
            .any(|point| { (point.x - 1.4).abs() < 1.0e-6 && (point.z - 0.8).abs() < 1.0e-6 }),
        "section output should retain the lower-left corner of the opening"
    );
}

#[test]
fn opening_geometry_records_keep_approximate_mesh_quality() {
    let (document, wall_id, opening_id) = single_opening_fixture();
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);

    for id in [wall_id, opening_id] {
        let record = cache.inspect(id).unwrap();
        assert!(record.valid);
        assert_eq!(
            record.representation_quality,
            RepresentationQuality::ApproximateMesh
        );
        assert!(record.geometry_revision > 0);
        assert!(!record.geometry_fingerprint.is_empty());
        assert!(record.volume.unwrap() > 0.0);
        assert!(record.surface_area.unwrap() > 0.0);
    }
}
