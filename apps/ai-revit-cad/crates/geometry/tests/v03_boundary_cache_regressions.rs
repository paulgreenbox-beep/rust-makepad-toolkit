use ai_revit_core::{
    BUILTIN_WALL_CLASS_ID, Command, DeleteStrategy, Document, ElementId, ElementKind, OpeningSpec,
    Point2, WallSpec,
};
use ai_revit_geometry::{
    GeometryBackend, GeometryCache, GeometryInput, GeometryOptions, MeshGeometryBackend,
    object_id_for, project_dirty_document, project_dirty_document_into_scene,
};
use std::collections::BTreeMap;
use viewer_core::{Mesh, Ray, Scene, Vec3};

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

fn wall_spec(thickness_m: f64, height_m: f64) -> WallSpec {
    WallSpec {
        start: Point2::new(0.0, 0.0),
        end: Point2::new(6.0, 0.0),
        base_elevation_m: 0.0,
        height_m,
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
            kind: ElementKind::Wall(wall_spec(0.2, 3.0)),
            properties: BTreeMap::new(),
        }],
    )
    .created_elements;
    assert_eq!(
        created,
        vec![expected_id],
        "fixture must use allocation preview"
    );
    expected_id
}

fn opening_spec(center_x: f64, elevation_m: f64, width_m: f64, height_m: f64) -> OpeningSpec {
    OpeningSpec {
        center: Point2::new(center_x, 0.0),
        elevation_m,
        width_m,
        height_m,
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
        "fixture must use allocation preview"
    );
    expected_id
}

fn wall_with_opening(spec: OpeningSpec) -> (Document, ElementId, ElementId) {
    let mut document = Document::new("Boundary cache regression").unwrap();
    let wall_id = create_wall(&mut document, "create-wall");
    let opening_id = create_opening(&mut document, "create-opening", wall_id, spec);
    (document, wall_id, opening_id)
}

fn assert_near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < EPSILON,
        "expected {expected}, got {actual}"
    );
}

fn through_wall_ray(x: f64, z: f64) -> Ray {
    Ray::new(Vec3::new(x, -1.0, z), Vec3::new(0.0, 1.0, 0.0))
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
        .expect("valid hosted openings should derive wall geometry")
}

fn vertex_key(vertex: Vec3) -> [i64; 3] {
    const SCALE: f64 = 1_000_000_000.0;
    [
        (vertex.x * SCALE).round() as i64,
        (vertex.y * SCALE).round() as i64,
        (vertex.z * SCALE).round() as i64,
    ]
}

fn assert_closed_oriented_manifold(mesh: &Mesh) {
    let mut edges: BTreeMap<([i64; 3], [i64; 3]), (usize, i64)> = BTreeMap::new();
    for triangle in mesh.triangles() {
        for index in 0..3 {
            let start = vertex_key(triangle.vertices[index]);
            let end = vertex_key(triangle.vertices[(index + 1) % 3]);
            assert_ne!(start, end, "triangle must not contain a collapsed edge");
            let (key, orientation) = if start < end {
                ((start, end), 1)
            } else {
                ((end, start), -1)
            };
            let incidence = edges.entry(key).or_default();
            incidence.0 += 1;
            incidence.1 += orientation;
        }
    }
    assert!(!edges.is_empty(), "a nonempty wall mesh is required");
    for (edge, (count, orientation_balance)) in edges {
        assert_eq!(
            count, 2,
            "welded edge {edge:?} must have two incident faces"
        );
        assert_eq!(
            orientation_balance, 0,
            "welded edge {edge:?} must have opposite face orientations"
        );
    }
}

#[test]
fn a_full_wall_opening_keeps_the_semantic_wall_without_rendered_geometry() {
    let mut document = Document::new("Full wall opening").unwrap();
    let wall_id = create_wall(&mut document, "create-wall");
    let opening_id = create_opening(
        &mut document,
        "create-full-wall-opening",
        wall_id,
        opening_spec(3.0, 0.0, 6.0, 3.0),
    );
    let mut cache = GeometryCache::new();

    project_dirty_document(&document, &mut cache);

    let wall = cache.inspect(wall_id).expect("host record is retained");
    assert!(wall.valid);
    assert_eq!(wall.openings, vec![opening_id]);
    assert_near(wall.volume.expect("valid cut has volume"), 0.0);
    assert_near(wall.surface_area.expect("valid cut has area"), 0.0);
    assert!(wall.bounds.is_none());
    assert_eq!(wall.triangle_count, 0);
    assert!(cache.scene().get(object_id_for(wall_id)).is_none());
    assert!(matches!(
        document.element(wall_id).unwrap().kind,
        ElementKind::Wall(_)
    ));
}

#[test]
fn edge_touching_openings_form_one_hole_with_a_closed_oriented_mesh() {
    let mut document = Document::new("Edge touching openings").unwrap();
    let wall_id = create_wall(&mut document, "create-wall");
    let first = create_opening(
        &mut document,
        "create-left-opening",
        wall_id,
        opening_spec(2.0, 0.5, 1.0, 1.0),
    );
    let second = create_opening(
        &mut document,
        "create-right-opening",
        wall_id,
        opening_spec(3.0, 0.5, 1.0, 1.0),
    );
    let mut cache = GeometryCache::new();

    project_dirty_document(&document, &mut cache);

    let wall = cache.inspect(wall_id).unwrap();
    assert!(wall.valid);
    assert_eq!(wall.openings, vec![first, second]);
    assert_near(wall.volume.unwrap(), 3.2);
    let derived = derive_host_directly(&document, wall_id, &[first, second]);
    assert_near(derived.volume.unwrap(), 3.2);
    let mesh = &cache.scene().get(object_id_for(wall_id)).unwrap().mesh;
    assert!(
        mesh.pick(object_id_for(wall_id), through_wall_ray(2.5, 1.0), 2.0)
            .is_none(),
        "the shared opening edge must remain part of the through-hole"
    );
    assert_closed_oriented_manifold(mesh);
}

#[test]
fn corner_touching_openings_report_a_structured_failure_and_hide_the_host() {
    let mut document = Document::new("Corner touching openings").unwrap();
    let wall_id = create_wall(&mut document, "create-wall");
    let first = create_opening(
        &mut document,
        "create-lower-left-opening",
        wall_id,
        opening_spec(2.0, 0.5, 1.0, 1.0),
    );
    let second = create_opening(
        &mut document,
        "create-upper-right-opening",
        wall_id,
        opening_spec(3.0, 1.5, 1.0, 1.0),
    );
    let mut cache = GeometryCache::new();

    project_dirty_document(&document, &mut cache);

    let wall = cache.inspect(wall_id).unwrap();
    assert!(!wall.valid);
    let failure = wall.failure.as_ref().expect("corner pinch has a failure");
    assert_eq!(failure.error_code, "nonmanifold_void_union");
    assert_eq!(failure.element_id, wall_id);
    assert_eq!(failure.host_id, Some(wall_id));
    assert!(
        failure
            .opening_id
            .is_some_and(|id| id == first || id == second),
        "failure should identify one of the corner-touching openings"
    );
    assert!(cache.scene().get(object_id_for(wall_id)).is_none());
    assert!(matches!(
        document.element(wall_id).unwrap().kind,
        ElementKind::Wall(_)
    ));
}

#[test]
fn changing_only_wall_height_does_not_change_the_opening_comparison() {
    let (mut document, wall_id, opening_id) = wall_with_opening(opening_spec(2.0, 0.8, 1.2, 1.5));
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let before_revision = cache.inspect(wall_id).unwrap().geometry_revision;
    let mut wall_spec = match document.element(wall_id).unwrap().kind.clone() {
        ElementKind::Wall(spec) => spec,
        other => panic!("expected wall, got {other:?}"),
    };
    wall_spec.height_m = 3.2;

    execute(
        &mut document,
        "raise-wall-height",
        vec![Command::UpdateElement {
            element_id: wall_id,
            kind: ElementKind::Wall(wall_spec),
        }],
    );
    project_dirty_document(&document, &mut cache);

    let after = cache.inspect(wall_id).unwrap();
    let comparison = cache
        .compare(wall_id, before_revision, after.geometry_revision)
        .expect("both wall geometry revisions should be available");
    assert!(!comparison.openings_changed);
    assert_eq!(comparison.openings_before, vec![opening_id]);
    assert_eq!(comparison.openings_after, vec![opening_id]);
    assert_near(comparison.volume_delta.unwrap(), 0.24);
}

#[test]
fn skipped_document_revisions_force_one_catch_up_scan_for_all_updates() {
    let mut document = Document::new("Skipped cache revisions").unwrap();
    let first_wall = create_wall(&mut document, "create-first-wall");
    let second_wall = create_wall(&mut document, "create-second-wall");
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);

    let mut first_spec = match document.element(first_wall).unwrap().kind.clone() {
        ElementKind::Wall(spec) => spec,
        other => panic!("expected wall, got {other:?}"),
    };
    first_spec.thickness_m = 0.3;
    execute(
        &mut document,
        "resize-first-wall",
        vec![Command::UpdateElement {
            element_id: first_wall,
            kind: ElementKind::Wall(first_spec),
        }],
    );
    let mut second_spec = match document.element(second_wall).unwrap().kind.clone() {
        ElementKind::Wall(spec) => spec,
        other => panic!("expected wall, got {other:?}"),
    };
    second_spec.thickness_m = 0.4;
    execute(
        &mut document,
        "resize-second-wall",
        vec![Command::UpdateElement {
            element_id: second_wall,
            kind: ElementKind::Wall(second_spec),
        }],
    );

    let catch_up = project_dirty_document(&document, &mut cache);
    assert_eq!(catch_up.total_document_scan_count, 1);
    assert_eq!(catch_up.scanned_element_count, 2);
    assert_eq!(catch_up.rebuilt_element_ids, vec![first_wall, second_wall]);
    assert_near(cache.inspect(first_wall).unwrap().volume.unwrap(), 5.4);
    assert_near(cache.inspect(second_wall).unwrap().volume.unwrap(), 7.2);

    let unchanged = project_dirty_document(&document, &mut cache);
    assert_eq!(unchanged.total_document_scan_count, 0);
    assert_eq!(unchanged.scanned_element_count, 0);
    assert_eq!(unchanged.rebuilt_element_count, 0);
}

#[test]
fn semantically_accepted_tiny_opening_rotation_still_cuts_the_host() {
    let (mut document, wall_id, opening_id) = wall_with_opening(opening_spec(2.0, 0.8, 1.2, 1.5));
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);
    let mut opening = match document.element(opening_id).unwrap().kind.clone() {
        ElementKind::Opening(spec) => spec,
        other => panic!("expected opening, got {other:?}"),
    };
    opening.rotation_rad = 5.0e-7;

    execute(
        &mut document,
        "rotate-opening-within-tolerance",
        vec![Command::UpdateElement {
            element_id: opening_id,
            kind: ElementKind::Opening(opening),
        }],
    );
    project_dirty_document(&document, &mut cache);

    let wall = cache.inspect(wall_id).unwrap();
    assert!(
        wall.valid,
        "the backend should accept the core's angular residual"
    );
    assert_near(wall.volume.unwrap(), 3.24);
    let mesh = &cache.scene().get(object_id_for(wall_id)).unwrap().mesh;
    assert!(
        mesh.pick(object_id_for(wall_id), through_wall_ray(2.0, 1.55), 2.0)
            .is_none(),
        "a small accepted rotation must preserve the opening cut"
    );
}

#[test]
fn backend_returns_structured_error_for_rotation_outside_core_tolerance() {
    let (document, wall_id, opening_id) = wall_with_opening(opening_spec(2.0, 0.8, 1.2, 1.5));
    let mut opening = document.effective_element(opening_id).unwrap();
    match &mut opening.kind {
        ElementKind::Opening(spec) => spec.rotation_rad = 2.0e-6,
        other => panic!("expected opening, got {other:?}"),
    }
    let input = GeometryInput {
        element: document.effective_element(wall_id).unwrap(),
        openings: vec![opening],
        derived_from: vec![wall_id, opening_id],
    };

    let failure = MeshGeometryBackend
        .derive_element(&input, GeometryOptions::default())
        .err()
        .expect("backend should reject rotation above the accepted residual");
    assert_eq!(failure.error_code, "invalid_opening_orientation");
    assert_eq!(failure.element_id, wall_id);
    assert_eq!(failure.host_id, Some(wall_id));
    assert_eq!(failure.opening_id, Some(opening_id));
}

#[test]
fn opening_volume_display_toggle_updates_target_scene_without_geometry_rebuild() {
    let (document, wall_id, opening_id) = wall_with_opening(opening_spec(2.0, 0.8, 1.2, 1.5));
    let mut cache = GeometryCache::new();
    let mut target_scene = Scene::new();
    let initial = project_dirty_document_into_scene(&document, &mut cache, &mut target_scene);
    assert_eq!(initial.rebuilt_element_count, 2);
    assert!(target_scene.get(object_id_for(wall_id)).is_some());
    assert!(target_scene.get(object_id_for(opening_id)).is_none());
    let document_revision = document.revision();
    let wall_revision = cache.inspect(wall_id).unwrap().geometry_revision;
    let opening_revision = cache.inspect(opening_id).unwrap().geometry_revision;
    let wall_mesh_revision = cache.inspect(wall_id).unwrap().mesh_revision;
    let opening_mesh_revision = cache.inspect(opening_id).unwrap().mesh_revision;
    let history_count = cache.geometry_history_entry_count();

    cache.set_show_opening_volumes(true);
    let shown = project_dirty_document_into_scene(&document, &mut cache, &mut target_scene);
    assert_eq!(shown.rebuilt_element_count, 0);
    assert_eq!(shown.display_changed_element_ids, vec![opening_id]);
    assert!(target_scene.get(object_id_for(opening_id)).is_some());
    assert_eq!(document.revision(), document_revision);
    assert_eq!(
        cache.inspect(wall_id).unwrap().geometry_revision,
        wall_revision
    );
    assert_eq!(
        cache.inspect(opening_id).unwrap().geometry_revision,
        opening_revision
    );
    assert_eq!(
        cache.inspect(wall_id).unwrap().mesh_revision,
        wall_mesh_revision
    );
    assert_eq!(
        cache.inspect(opening_id).unwrap().mesh_revision,
        opening_mesh_revision
    );
    assert_eq!(cache.geometry_history_entry_count(), history_count);

    cache.set_show_opening_volumes(false);
    let hidden = project_dirty_document_into_scene(&document, &mut cache, &mut target_scene);
    assert_eq!(hidden.rebuilt_element_count, 0);
    assert_eq!(hidden.display_changed_element_ids, vec![opening_id]);
    assert!(target_scene.get(object_id_for(opening_id)).is_none());
    assert_eq!(document.revision(), document_revision);
    assert_eq!(
        cache.inspect(wall_id).unwrap().geometry_revision,
        wall_revision
    );
    assert_eq!(
        cache.inspect(opening_id).unwrap().geometry_revision,
        opening_revision
    );
    assert_eq!(
        cache.inspect(wall_id).unwrap().mesh_revision,
        wall_mesh_revision
    );
    assert_eq!(
        cache.inspect(opening_id).unwrap().mesh_revision,
        opening_mesh_revision
    );
    assert_eq!(cache.geometry_history_entry_count(), history_count);
}

#[test]
fn invalid_opening_failure_provenance_tracks_remaining_duplicate_after_delete() {
    let mut document = Document::new("Duplicate invalid openings").unwrap();
    let wall_id = create_wall(&mut document, "create-wall");
    let mut shallow_spec = opening_spec(2.0, 0.8, 1.2, 1.5);
    shallow_spec.depth_m = 0.1;
    let first = create_opening(
        &mut document,
        "create-first-shallow-opening",
        wall_id,
        shallow_spec.clone(),
    );
    let second = create_opening(
        &mut document,
        "create-second-shallow-opening",
        wall_id,
        shallow_spec,
    );
    let mut cache = GeometryCache::new();
    project_dirty_document(&document, &mut cache);

    let before = cache.inspect(wall_id).unwrap().clone();
    let first_failure = before
        .failure
        .as_ref()
        .expect("shallow openings should fail the host cut");
    assert_eq!(first_failure.error_code, "opening_depth_insufficient");
    assert_eq!(first_failure.host_id, Some(wall_id));
    assert_eq!(first_failure.opening_id, Some(first));
    assert_eq!(before.openings, vec![first, second]);

    execute(
        &mut document,
        "delete-first-shallow-opening",
        vec![Command::DeleteElementWithStrategy {
            element_id: first,
            strategy: DeleteStrategy::Detach,
        }],
    );
    project_dirty_document(&document, &mut cache);

    let after = cache.inspect(wall_id).unwrap();
    let failure = after
        .failure
        .as_ref()
        .expect("remaining shallow opening still fails");
    assert_eq!(failure.error_code, "opening_depth_insufficient");
    assert_eq!(failure.host_id, Some(wall_id));
    assert_eq!(failure.opening_id, Some(second));
    assert_eq!(after.openings, vec![second]);
    let comparison = cache
        .compare(wall_id, before.geometry_revision, after.geometry_revision)
        .expect("the failed wall revisions should remain comparable");
    assert!(comparison.openings_changed);
    assert_eq!(comparison.openings_before, vec![first, second]);
    assert_eq!(comparison.openings_after, vec![second]);
}
