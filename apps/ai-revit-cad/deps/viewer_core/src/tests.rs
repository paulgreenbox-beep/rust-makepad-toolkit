use crate::{
    procedural_test_scene, Aabb, Camera, Mesh, Quat, Ray, Scene, SceneObject, Transform3, Vec2,
    Vec3, Viewer, ViewerEvent, ViewerState, Viewport,
};

#[test]
fn bvh_picks_closest_triangle_with_stable_ids_and_world_geometry() {
    let mesh = Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0));
    let mut object = SceneObject::new(42, "scaled cube", mesh);
    object.transform = Transform3::new(
        Vec3::new(0.0, 0.0, -2.0),
        Quat::IDENTITY,
        Vec3::new(2.0, 1.0, 0.5),
    );
    let mut scene = Scene::new();
    scene.insert(object);

    let ray = Ray::new(Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0));
    let first = scene.pick(ray, 100.0).expect("ray should hit scaled cube");
    let repeated = scene
        .pick(ray, 100.0)
        .expect("repeated ray should hit scaled cube");
    assert_eq!(
        first, repeated,
        "tie-breaking and traversal should be stable"
    );
    assert_eq!(first.object_id, 42);
    assert_eq!(
        first.triangle_id, 9,
        "center of +Z face lies on the stable first triangle"
    );
    assert!((first.world_point - Vec3::new(0.0, 0.0, -1.5)).length() < 1.0e-9);
    assert!((first.world_normal - Vec3::Z).length() < 1.0e-9);
    assert!((first.distance - 6.5).abs() < 1.0e-9);
}

#[test]
fn camera_ray_navigation_anchor_and_fit_bounds_are_consistent() {
    let viewport = Viewport::new(800.0, 600.0);
    let mut camera = Camera::new(crate::CameraState {
        position: Vec3::new(0.0, 0.0, 5.0),
        target: Vec3::ZERO,
        pivot: Vec3::ZERO,
        up: Vec3::Y,
        projection: crate::CameraProjection::Perspective,
        vertical_fov_degrees: 50.0,
        near: 0.01,
        far: 1000.0,
    })
    .unwrap();
    let center_ray = camera
        .ray_from_screen(Vec2::new(400.0, 300.0), viewport)
        .unwrap();
    assert!((center_ray.direction - Vec3::new(0.0, 0.0, -1.0)).length() < 1.0e-9);

    let anchor = Vec3::new(0.7, 0.25, 0.0);
    let screen_before = camera
        .project_point(anchor, viewport)
        .unwrap()
        .unwrap()
        .screen;
    camera
        .dolly_at(1.7, Some(crate::SurfaceAnchor { point: anchor }))
        .unwrap();
    let screen_after = camera
        .project_point(anchor, viewport)
        .unwrap()
        .unwrap()
        .screen;
    assert!(
        (screen_before - screen_after).length() < 1.0e-8,
        "surface anchor should remain at the same pixel"
    );

    let prior_pivot = camera.pivot;
    camera.orbit(0.35, 0.2).unwrap();
    assert_eq!(camera.pivot, prior_pivot, "orbit should preserve the pivot");
    let distance = (camera.position - camera.target).length();
    camera.pan_pixels(Vec2::new(32.0, -18.0), viewport).unwrap();
    assert!((camera.position - camera.target).length() - distance < 1.0e-9);

    let bounds = Aabb::new(Vec3::new(-2.0, -1.0, -3.0), Vec3::new(2.0, 3.0, 1.0));
    camera.fit_bounds(bounds, viewport, 1.2).unwrap();
    for index in 0..8 {
        let projected = camera
            .project_point(bounds.corner(index), viewport)
            .unwrap()
            .unwrap();
        assert!(projected.screen.x >= 0.0 && projected.screen.x <= viewport.width);
        assert!(projected.screen.y >= 0.0 && projected.screen.y <= viewport.height);
    }
}

#[test]
fn dolly_respects_near_and_far_navigation_limits() {
    let mut camera = Camera::default();
    camera.dolly_at(1.0e12, None).unwrap();
    assert!(((camera.position - camera.pivot).length() - camera.near * 2.0).abs() < 1.0e-9);

    camera.dolly_at(1.0e-12, None).unwrap();
    assert!(((camera.position - camera.pivot).length() - camera.far * 0.8).abs() < 1.0e-6);
}

#[test]
fn viewer_snapshot_captures_pivot_hover_counts_isolation_and_projection() {
    let mut scene = Scene::new();
    scene.insert(SceneObject::new(
        81,
        "front cube",
        Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0)),
    ));
    let mut rear = SceneObject::new(82, "rear cube", Mesh::cuboid(Vec3::new(1.0, 1.0, 1.0)));
    rear.transform.translation = Vec3::new(4.0, 0.0, 0.0);
    scene.insert(rear);
    let camera = Camera::new(crate::CameraState {
        position: Vec3::new(0.0, 0.0, 5.0),
        target: Vec3::ZERO,
        pivot: Vec3::ZERO,
        up: Vec3::Y,
        projection: crate::CameraProjection::Perspective,
        vertical_fov_degrees: 50.0,
        near: 0.01,
        far: 1000.0,
    })
    .unwrap();
    let mut viewer = Viewer::new(camera, scene, Viewport::new(800.0, 600.0)).unwrap();

    viewer
        .dolly_at_screen(Vec2::new(400.0, 300.0), 1.25)
        .unwrap();
    viewer.isolate(81);
    viewer.select(Some(81), false);
    viewer.set_highlighted(Some(81));
    let state = viewer.get_viewer_state();

    assert_eq!(state.navigation_pivot.object_id, Some(81));
    assert_eq!(state.navigation_pivot.world_point, viewer.camera().pivot);
    assert_eq!(state.hover_hit.unwrap().object_id, 81);
    assert_eq!((state.visible_count, state.hidden_count), (1, 1));
    assert_eq!(state.isolated_object_ids, vec![81]);
    assert_eq!(state.highlighted_object_ids, vec![81]);
    assert_eq!(state.selected_object_ids, vec![81]);
    assert_eq!(
        state.camera.projection,
        crate::CameraProjection::Perspective
    );
    assert_eq!(state.camera.pivot, state.navigation_pivot.world_point);

    let json = viewer.snapshot_json().unwrap();
    viewer.show_all();
    viewer.select(None, false);
    viewer.set_highlighted(None);
    viewer.restore_snapshot_json(&json).unwrap();
    let restored = viewer.get_viewer_state();
    assert_eq!(restored.visible_count, 1);
    assert_eq!(restored.hidden_count, 1);
    assert_eq!(restored.isolated_object_ids, vec![81]);
    assert_eq!(restored.navigation_pivot, state.navigation_pivot);
    assert_eq!(restored.hover_hit, state.hover_hit);
    let events = viewer.drain_events();
    assert!(events
        .iter()
        .any(|event| event.kind() == crate::ViewerEventKind::StateRestored));
    assert!(events.iter().any(|event| matches!(
        event,
        ViewerEvent::ObjectIsolated { object_ids } if object_ids == &[81]
    )));
    assert!(viewer.drain_events().is_empty());
}

#[test]
fn batch_selection_isolation_and_event_names_are_stable() {
    let mut viewer = Viewer::demo();
    viewer.select_many(&[1001, 1002, 9999]);
    assert_eq!(viewer.scene().selected_ids(), vec![1001, 1002]);

    viewer.isolate_many(&[1002, 1003, 1002]);
    let state = viewer.get_viewer_state();
    assert_eq!(state.isolated_object_ids, vec![1002, 1003]);
    assert_eq!((state.visible_count, state.hidden_count), (2, 2));
    assert_eq!(viewer.scene().selected_ids(), vec![1002]);

    viewer.unisolate();
    assert_eq!(viewer.scene().visible_count(), 4);
    assert!(viewer.get_viewer_state().isolated_object_ids.is_empty());

    let pivot_event = ViewerEvent::NavigationPivotChanged {
        pivot: crate::NavigationPivot {
            object_id: Some(1002),
            world_point: Vec3::new(1.0, 2.0, 3.0),
        },
    };
    let json = serde_json::to_value(pivot_event).unwrap();
    assert_eq!(json["type"], "pivot_changed");
}

#[test]
fn hide_show_isolate_and_restore_emit_explicit_semantic_events() {
    let mut viewer = Viewer::demo();

    let hidden = viewer.hide(1001);
    assert!(hidden
        .iter()
        .any(|event| matches!(event, ViewerEvent::ObjectHidden { object_id: 1001 })));
    let hidden_json = serde_json::to_value(
        hidden
            .iter()
            .find(|event| event.kind() == crate::ViewerEventKind::ObjectHidden)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(hidden_json["type"], "object_hidden");
    assert_eq!(hidden_json["data"]["object_id"], 1001);

    let shown = viewer.show(1001);
    assert!(shown
        .iter()
        .any(|event| matches!(event, ViewerEvent::ObjectShown { object_id: 1001 })));
    let shown_json = serde_json::to_value(
        shown
            .iter()
            .find(|event| event.kind() == crate::ViewerEventKind::ObjectShown)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(shown_json["type"], "object_shown");
    assert_eq!(shown_json["data"]["object_id"], 1001);

    let isolated = viewer.isolate_many(&[1002, 1003]);
    assert!(isolated.iter().any(|event| matches!(
        event,
        ViewerEvent::ObjectIsolated { object_ids } if object_ids == &[1002, 1003]
    )));
    let isolated_json = serde_json::to_value(
        isolated
            .iter()
            .find(|event| event.kind() == crate::ViewerEventKind::ObjectIsolated)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(isolated_json["type"], "object_isolated");
    assert_eq!(
        isolated_json["data"]["object_ids"],
        serde_json::json!([1002, 1003])
    );

    let unisolated = viewer.unisolate();
    assert_eq!(viewer.scene().visible_count(), 4);
    assert!(unisolated
        .iter()
        .any(|event| matches!(event, ViewerEvent::ObjectShown { object_id: 1001 })));
    assert!(unisolated.iter().any(|event| matches!(
        event,
        ViewerEvent::IsolationChanged { object_ids } if object_ids.is_empty()
    )));

    viewer.hide(1004);
    let shown_all = viewer.show_all();
    assert!(shown_all
        .iter()
        .any(|event| matches!(event, ViewerEvent::ObjectShown { object_id: 1004 })));
    assert!(viewer
        .drain_events()
        .iter()
        .any(|event| event.kind() == crate::ViewerEventKind::ObjectHidden));
}

#[test]
fn selection_visibility_events_and_json_state_restore_round_trip() {
    let camera = Camera::default();
    let viewport = Viewport::new(900.0, 600.0);
    let mut viewer = Viewer::new(camera, procedural_test_scene(), viewport).unwrap();
    let selected = viewer.select(Some(1001), false);
    assert!(
        matches!(selected.as_slice(), [ViewerEvent::SelectionChanged { selected_object_ids }] if selected_object_ids == &[1001])
    );
    viewer.select(Some(1004), true);
    viewer.set_highlighted(Some(1003));
    viewer.set_visible(1002, false);

    let saved = viewer.capture_state();
    let json = saved.to_json().unwrap();
    let decoded = ViewerState::from_json(&json).unwrap();
    assert_eq!(decoded, saved);

    viewer.select(None, false);
    viewer.set_visible(1002, true);
    viewer.set_visible(1003, false);
    viewer.set_highlighted(None);
    let events = viewer.restore_state(&decoded).unwrap();

    assert_eq!(viewer.scene().selected_ids(), vec![1001, 1004]);
    assert!(!viewer.scene().get(1002).unwrap().visible);
    assert!(viewer.scene().get(1003).unwrap().visible);
    assert_eq!(viewer.scene().highlighted_id(), Some(1003));
    assert!(events
        .iter()
        .any(|event| matches!(event, ViewerEvent::StateRestored)));
}

#[test]
fn section_bounds_validate_and_include_all_six_planes() {
    let bounds = crate::SectionBounds::new(-1.0, 2.0, -3.0, 4.0, -5.0, 6.0).unwrap();
    for point in [
        Vec3::new(-1.0, 0.0, 0.0),
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::new(0.0, -3.0, 0.0),
        Vec3::new(0.0, 4.0, 0.0),
        Vec3::new(0.0, 0.0, -5.0),
        Vec3::new(0.0, 0.0, 6.0),
    ] {
        assert!(bounds.contains(point), "boundary point {point:?} is inside");
    }
    for point in [
        Vec3::new(-1.0001, 0.0, 0.0),
        Vec3::new(2.0001, 0.0, 0.0),
        Vec3::new(0.0, -3.0001, 0.0),
        Vec3::new(0.0, 4.0001, 0.0),
        Vec3::new(0.0, 0.0, -5.0001),
        Vec3::new(0.0, 0.0, 6.0001),
    ] {
        assert!(
            !bounds.contains(point),
            "outside point {point:?} is clipped"
        );
    }
    assert_eq!(
        crate::SectionBounds::new(f64::NAN, 2.0, -3.0, 4.0, -5.0, 6.0),
        Err(crate::SectionBoxError::InvalidBounds)
    );
    assert_eq!(
        crate::SectionBounds::new(2.0, 2.0, -3.0, 4.0, -5.0, 6.0),
        Err(crate::SectionBoxError::InvalidBounds)
    );
}

#[test]
fn section_box_fit_and_expand_use_world_bounds_and_emit_semantic_changes() {
    let mut scene = Scene::new();
    scene.insert(SceneObject::new(
        501,
        "selected beam",
        Mesh::cuboid(Vec3::new(2.0, 4.0, 6.0)),
    ));
    let mut second = SceneObject::new(502, "second beam", Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0)));
    second.transform.translation = Vec3::new(10.0, 0.0, 0.0);
    scene.insert(second);
    let mut hidden = SceneObject::new(503, "hidden object", Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0)));
    hidden.transform.translation = Vec3::new(100.0, 0.0, 0.0);
    hidden.visible = false;
    scene.insert(hidden);
    let mut viewer = Viewer::new(Camera::default(), scene, Viewport::new(800.0, 600.0)).unwrap();

    viewer.select(Some(501), false);
    let events = viewer.fit_section_box_to_selection(1.0).unwrap();
    assert_eq!(
        viewer.section_box().bounds,
        crate::SectionBounds::new(-2.0, 2.0, -3.0, 3.0, -4.0, 4.0).unwrap()
    );
    assert!(viewer.section_box().enabled);
    assert!(events.iter().any(|event| matches!(
        event,
        ViewerEvent::SectionBoxChanged { section_box } if *section_box == viewer.section_box()
    )));

    viewer
        .fit_section_box_to_objects(&[501, 502, 9999], 2.0)
        .unwrap();
    assert_eq!(
        viewer.section_box().bounds,
        crate::SectionBounds::new(-3.0, 13.0, -4.0, 4.0, -5.0, 5.0).unwrap()
    );
    viewer.expand_section_box(0.5).unwrap();
    assert_eq!(
        viewer.section_box().bounds,
        crate::SectionBounds::new(-3.5, 13.5, -4.5, 4.5, -5.5, 5.5).unwrap()
    );

    viewer.fit_section_box_to_model(0.0).unwrap();
    assert_eq!(
        viewer.section_box().bounds,
        crate::SectionBounds::new(-1.0, 11.0, -2.0, 2.0, -3.0, 3.0).unwrap()
    );
    viewer.fit_section_box_to_objects(&[503], 0.0).unwrap();
    assert_eq!(
        viewer.section_box().bounds,
        crate::SectionBounds::new(99.0, 101.0, -1.0, 1.0, -1.0, 1.0).unwrap()
    );
    assert!(!viewer
        .fit_section_box_to_objects(&[9999], 0.0)
        .unwrap()
        .iter()
        .any(|event| event.kind() == crate::ViewerEventKind::SectionBoxChanged));
    assert_eq!(
        viewer.expand_section_box(f64::NAN),
        Err(crate::SectionBoxError::InvalidMargin)
    );
    viewer.reset_section_box();
    assert!(!viewer.section_box().enabled);
    assert_eq!(
        viewer.section_box().bounds,
        crate::SectionBounds::new(-1.0, 11.0, -2.0, 2.0, -3.0, 3.0).unwrap()
    );
}

#[test]
fn flat_geometry_gets_one_millimeter_minimum_section_extent() {
    let flat = crate::Aabb::new(Vec3::new(-1.0, 2.0, -3.0), Vec3::new(1.0, 2.0, 3.0));
    let section = crate::SectionBounds::from_aabb(flat).unwrap();
    assert_eq!(section.xmin, -1.0);
    assert_eq!(section.xmax, 1.0);
    assert!((section.ymin - 1.9995).abs() < 1.0e-12);
    assert!((section.ymax - 2.0005).abs() < 1.0e-12);
    assert_eq!(section.zmin, -3.0);
    assert_eq!(section.zmax, 3.0);
}

#[test]
fn active_section_box_skips_clipped_near_triangle_and_picks_deeper_inside_hit() {
    let triangles = vec![
        crate::Triangle::new(
            1,
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
        ),
        crate::Triangle::new(
            2,
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, -1.0, -1.0),
            Vec3::new(0.0, 1.0, -1.0),
        ),
    ];
    let mut scene = Scene::new();
    scene.insert(SceneObject::new(700, "two surfaces", Mesh::new(triangles)));
    let mut viewer = Viewer::new(Camera::default(), scene, Viewport::new(800.0, 600.0)).unwrap();
    viewer
        .set_section_box(crate::SectionBox {
            enabled: true,
            bounds: crate::SectionBounds::new(-0.5, 0.5, -0.5, 0.5, -2.0, 0.0).unwrap(),
            mode: crate::SectionMode::Inside,
        })
        .unwrap();

    let hit = viewer
        .raycast_ray(Ray::new(Vec3::new(0.0, 0.0, 5.0), -Vec3::Z))
        .expect("the deeper surface remains inside the active box");
    assert_eq!(hit.object_id, 700);
    assert_eq!(hit.triangle_id, 2);
    assert_eq!(hit.world_point, Vec3::new(0.0, 0.0, -1.0));

    viewer
        .set_section_box_bounds(-0.5, 0.5, -0.5, 0.5, -3.0, -2.0)
        .unwrap();
    assert_eq!(
        viewer.raycast_ray(Ray::new(Vec3::new(0.0, 0.0, 5.0), -Vec3::Z)),
        None
    );
}

#[test]
fn section_box_state_round_trips_and_v01_state_defaults_to_disabled() {
    let mut viewer = Viewer::demo();
    viewer
        .set_section_box_bounds(-4.0, 8.0, -5.0, 9.0, -6.0, 10.0)
        .unwrap();
    viewer.enable_section_box();
    let saved = viewer.capture_state();
    let json = saved.to_json().unwrap();
    let restored = ViewerState::from_json(&json).unwrap();
    assert_eq!(restored.section_box, viewer.section_box());

    viewer.disable_section_box();
    let events = viewer.restore_state(&restored).unwrap();
    assert_eq!(viewer.section_box(), restored.section_box);
    assert!(events.iter().any(|event| matches!(
        event,
        ViewerEvent::SectionBoxChanged { section_box } if *section_box == restored.section_box
    )));
    let event_json = serde_json::to_value(
        events
            .iter()
            .find(|event| event.kind() == crate::ViewerEventKind::SectionBoxChanged)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(event_json["type"], "section_box_changed");

    let mut legacy: serde_json::Value = serde_json::from_str(&json).unwrap();
    legacy["schema_version"] = serde_json::json!(1);
    legacy.as_object_mut().unwrap().remove("section_box");
    let legacy_state = ViewerState::from_json(&legacy.to_string()).unwrap();
    assert_eq!(legacy_state.section_box, crate::SectionBox::default());
    assert!(!legacy_state.section_box.enabled);

    let invalid = json.replace("\"xmin\": -4.0", "\"xmin\": 100.0");
    assert!(ViewerState::from_json(&invalid).is_err());
}

#[test]
fn aabb_collision_candidates_are_conservative_and_have_stable_ids() {
    fn scene(insert_reversed: bool) -> Scene {
        let mut scene = Scene::new();
        let low_id = SceneObject::new(1001, "display A", Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0)));
        let mut high_id =
            SceneObject::new(9002, "display B", Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0)));
        high_id.transform.translation = Vec3::new(1.0, 0.0, 0.0);
        if insert_reversed {
            scene.insert(high_id);
            scene.insert(low_id);
        } else {
            scene.insert(low_id);
            scene.insert(high_id);
        }
        scene
    }

    let first = crate::detect_aabb_collisions(&scene(false));
    let reversed = crate::detect_aabb_collisions(&scene(true));
    assert_eq!(first.len(), 1);
    assert_eq!(reversed.len(), 1);
    let collision = &first[0];
    assert_eq!(collision, &reversed[0]);
    assert_eq!(collision.collision_id, "COL-1001-9002");
    assert_eq!((collision.object_a, collision.object_b), (1001, 9002));
    assert_eq!(collision.status, crate::CollisionStatus::Candidate);
    assert_eq!(collision.method, crate::CollisionMethod::Aabb);
    assert_eq!(
        collision.severity,
        crate::CollisionSeverity::PotentialInterference
    );
    assert_eq!(
        collision.intersection_bounds,
        Some(Aabb::new(
            Vec3::new(0.0, -1.0, -1.0),
            Vec3::new(1.0, 1.0, 1.0)
        ))
    );
    assert_eq!(collision.intersection_volume, Some(4.0));
    assert_eq!(collision.location, Vec3::new(0.5, 0.0, 0.0));

    let summary = collision.summary();
    assert_eq!(summary.collision_id, collision.collision_id);
    assert_eq!(summary.objects, [1001, 9002]);
    assert_eq!(summary.location, collision.location);
    assert_eq!(
        summary.severity,
        crate::CollisionSeverity::PotentialInterference
    );
}

#[test]
fn aabb_clearance_warning_reports_nearby_disjoint_bounds_only() {
    let mut scene = Scene::new();
    scene.insert(SceneObject::new(
        7,
        "first",
        Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0)),
    ));
    let mut nearby = SceneObject::new(3, "second", Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0)));
    nearby.transform.translation = Vec3::new(2.25, 0.0, 0.0);
    scene.insert(nearby);

    let collisions = crate::detect_aabb_collisions_with_clearance(&scene, 0.3);
    assert_eq!(collisions.len(), 1);
    let warning = &collisions[0];
    assert_eq!(warning.collision_id, "COL-3-7");
    assert_eq!((warning.object_a, warning.object_b), (3, 7));
    assert_eq!(warning.status, crate::CollisionStatus::ClearanceWarning);
    assert_eq!(warning.severity, crate::CollisionSeverity::ClearanceWarning);
    assert_eq!(warning.intersection_bounds, None);
    assert_eq!(warning.intersection_volume, None);
    assert_eq!(warning.location, Vec3::new(1.125, 0.0, 0.0));

    assert!(
        crate::detect_aabb_collisions_with_clearance(&scene, 0.2).is_empty(),
        "pairs farther apart than the clearance are not reported"
    );
    assert!(
        crate::detect_aabb_collisions_with_clearance(&scene, f64::NAN).is_empty(),
        "non-finite clearance is safely treated as zero"
    );
}

#[test]
fn aabb_collision_detection_uses_world_bounds_and_includes_hidden_objects() {
    let mut scene = Scene::new();
    let mut first = SceneObject::new(10, "first", Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0)));
    first.visible = false;
    first.transform.translation = Vec3::new(10.0, 0.0, 0.0);
    scene.insert(first);
    let mut second = SceneObject::new(20, "second", Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0)));
    second.transform.translation = Vec3::new(11.0, 0.0, 0.0);
    scene.insert(second);

    let collisions = crate::detect_aabb_collisions(&scene);
    assert_eq!(collisions.len(), 1);
    assert_eq!(
        collisions[0].intersection_bounds,
        Some(Aabb::new(
            Vec3::new(10.0, -1.0, -1.0),
            Vec3::new(11.0, 1.0, 1.0)
        ))
    );
}
#[test]
fn collision_demo_fixture_has_expected_candidate_clearance_occlusion_and_context_objects() {
    let scene = crate::collision_demo_scene();
    assert_eq!(
        scene.object_ids().collect::<Vec<_>>(),
        (3001..=3007).collect::<Vec<_>>()
    );

    let collisions = crate::detect_aabb_collisions_with_clearance(&scene, 0.05);
    let find_pair = |a, b| {
        collisions
            .iter()
            .find(|collision| (collision.object_a, collision.object_b) == (a, b))
            .expect("expected fixture pair")
    };
    assert_eq!(
        find_pair(3001, 3002).status,
        crate::CollisionStatus::Candidate
    );
    assert_eq!(
        find_pair(3003, 3004).status,
        crate::CollisionStatus::ClearanceWarning
    );
    assert!(
        collisions
            .iter()
            .all(|collision| (collision.object_a, collision.object_b) != (3005, 3006)),
        "the projected occlusion pair remains separated in world bounds"
    );
    assert!(
        collisions
            .iter()
            .all(|collision| collision.object_a != 3007 && collision.object_b != 3007),
        "the remote context object is unrelated to the local collision examples"
    );

    let viewer = Viewer::collision_demo();
    assert_eq!(viewer.scene().visible_count(), 7);
    let screen_bounds = |object_id| {
        let bounds = viewer
            .scene()
            .get(object_id)
            .unwrap()
            .world_bounds()
            .unwrap();
        let mut min_x = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for index in 0..8 {
            let point = viewer
                .camera()
                .project_point(bounds.corner(index), viewer.viewport())
                .unwrap()
                .unwrap();
            min_x = min_x.min(point.screen.x);
            max_x = max_x.max(point.screen.x);
            min_y = min_y.min(point.screen.y);
            max_y = max_y.max(point.screen.y);
        }
        (min_x, max_x, min_y, max_y)
    };
    let rear = screen_bounds(3005);
    let front = screen_bounds(3006);
    let overlap_x = rear.1.min(front.1) - rear.0.max(front.0);
    let overlap_y = rear.3.min(front.3) - rear.2.max(front.2);
    assert!(overlap_x > 0.0 && overlap_y > 0.0);
    assert!(overlap_x < rear.1 - rear.0 && overlap_x < front.1 - front.0);
    let rear_depth = viewer
        .camera()
        .project_point(
            viewer.scene().get(3005).unwrap().transform.translation,
            viewer.viewport(),
        )
        .unwrap()
        .unwrap()
        .depth;
    let front_depth = viewer
        .camera()
        .project_point(
            viewer.scene().get(3006).unwrap().transform.translation,
            viewer.viewport(),
        )
        .unwrap()
        .unwrap()
        .depth;
    assert!(front_depth < rear_depth, "the partial occluder is in front");
}

fn reduced_3d_tetra_mesh() -> Mesh {
    let a = Vec3::new(-0.5, -0.5, -0.5);
    let b = Vec3::new(0.5, -0.5, -0.5);
    let c = Vec3::new(0.0, 0.5, -0.5);
    let d = Vec3::new(0.0, 0.0, 0.5);
    Mesh::new(vec![
        crate::Triangle::new(0, a, c, b),
        crate::Triangle::new(1, a, b, d),
        crate::Triangle::new(2, b, c, d),
        crate::Triangle::new(3, c, a, d),
    ])
}

fn reduced_3d_cylinder_mesh() -> Mesh {
    const SEGMENTS: usize = 8;
    let lower = |index: usize| {
        let angle = std::f64::consts::TAU * index as f64 / SEGMENTS as f64;
        Vec3::new(angle.cos(), angle.sin(), -1.0)
    };
    let upper = |index: usize| {
        let angle = std::f64::consts::TAU * index as f64 / SEGMENTS as f64;
        Vec3::new(angle.cos(), angle.sin(), 1.0)
    };
    let mut triangles = Vec::new();
    let mut triangle_id = 0;
    for index in 0..SEGMENTS {
        let next = (index + 1) % SEGMENTS;
        let low = lower(index);
        let low_next = lower(next);
        let high = upper(index);
        let high_next = upper(next);
        triangles.push(crate::Triangle::new(
            triangle_id,
            Vec3::new(0.0, 0.0, -1.0),
            low,
            low_next,
        ));
        triangle_id += 1;
        triangles.push(crate::Triangle::new(
            triangle_id,
            Vec3::new(0.0, 0.0, 1.0),
            high_next,
            high,
        ));
        triangle_id += 1;
        triangles.push(crate::Triangle::new(triangle_id, low, low_next, high_next));
        triangle_id += 1;
        triangles.push(crate::Triangle::new(triangle_id, low, high_next, high));
        triangle_id += 1;
    }
    Mesh::new(triangles)
}

#[test]
fn reduced_3d_prefers_analytic_boxes_and_preserves_transform_and_id() {
    let mut object = SceneObject::new(7101, "box", Mesh::cuboid(Vec3::new(2.0, 3.0, 4.0)));
    object.transform = Transform3::new(
        Vec3::new(5.0, -2.0, 1.0),
        Quat::from_axis_angle(Vec3::Z, 0.25),
        Vec3::new(1.0, 2.0, 1.0),
    );
    let mut scene = Scene::new();
    scene.insert(object);

    let observation =
        crate::capture_reduced_3d(&scene, &[7101, 7101], 0.0, crate::Reduced3DComplexity::Low)
            .unwrap();
    assert_eq!(observation.requested_ids, vec![7101]);
    assert_eq!(observation.objects.len(), 1);
    let reduced = &observation.objects[0];
    let wire = serde_json::to_value(&observation).unwrap();
    assert_eq!(wire["objects"][0]["representation"], "box");
    assert!(wire["objects"][0].get("size").is_some());
    assert!(wire["objects"][0].get("quality").is_some());
    assert_eq!(reduced.object_id, 7101);
    assert_eq!(reduced.transform.translation, Vec3::new(5.0, -2.0, 1.0));
    assert_eq!(
        reduced.quality,
        crate::Reduced3DQuality::RecognizedPrimitiveApproximation
    );
    assert!(matches!(
        &reduced.representation,
        crate::Reduced3DRepresentation::Box { size, .. }
            if *size == Vec3::new(2.0, 3.0, 4.0)
    ));
    assert_eq!(
        observation.geometry_quality,
        crate::Reduced3DGeometryQuality::ApproximateRenderMesh
    );
}

#[test]
fn reduced_3d_recognizes_cylinders_extrusions_planes_and_polylines() {
    let mut scene = Scene::new();
    scene.insert(SceneObject::new(
        7201,
        "cylinder",
        reduced_3d_cylinder_mesh(),
    ));

    let a0 = Vec3::new(0.0, 0.0, -1.0);
    let b0 = Vec3::new(2.0, 0.0, -1.0);
    let c0 = Vec3::new(0.0, 1.0, -1.0);
    let a1 = Vec3::new(0.0, 0.0, 1.0);
    let b1 = Vec3::new(2.0, 0.0, 1.0);
    let c1 = Vec3::new(0.0, 1.0, 1.0);
    let extrusion = Mesh::new(vec![
        crate::Triangle::new(0, a0, b0, c0),
        crate::Triangle::new(1, a1, b1, c1),
        crate::Triangle::new(2, a0, b0, b1),
        crate::Triangle::new(3, a0, b1, a1),
        crate::Triangle::new(4, b0, c0, c1),
        crate::Triangle::new(5, b0, c1, b1),
        crate::Triangle::new(6, c0, a0, a1),
        crate::Triangle::new(7, c0, a1, c1),
    ]);
    scene.insert(SceneObject::new(7202, "extrusion", extrusion));

    let flat = Mesh::new(vec![
        crate::Triangle::new(
            0,
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
        ),
        crate::Triangle::new(
            1,
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(-1.0, 1.0, 0.0),
        ),
    ]);
    scene.insert(SceneObject::new(7203, "plane", flat));
    let line = Mesh::new(vec![
        crate::Triangle::new(
            0,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
        ),
        crate::Triangle::new(
            1,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
        ),
    ]);
    scene.insert(SceneObject::new(7204, "polyline", line));

    let check = |id, expected: fn(&crate::Reduced3DRepresentation) -> bool| {
        let observation =
            crate::capture_reduced_3d(&scene, &[id], 0.0, crate::Reduced3DComplexity::Medium)
                .unwrap();
        let object = &observation.objects[0];
        assert_eq!(
            object.quality,
            if id == 7204 {
                crate::Reduced3DQuality::PolylineApproximation
            } else {
                crate::Reduced3DQuality::RecognizedPrimitiveApproximation
            }
        );
        assert!(expected(&object.representation));
    };
    check(7201, |representation| {
        matches!(
            representation,
            crate::Reduced3DRepresentation::Cylinder { .. }
        )
    });
    check(
        7202,
        |representation| matches!(representation, crate::Reduced3DRepresentation::Extrusion { profile, .. } if profile.len() == 3),
    );
    check(7203, |representation| {
        matches!(representation, crate::Reduced3DRepresentation::Plane { .. })
    });
    check(
        7204,
        |representation| matches!(representation, crate::Reduced3DRepresentation::Polyline { points, closed: false } if points.len() == 3),
    );
}

#[test]
fn reduced_3d_adds_only_nearby_visible_context_with_stable_order() {
    let mut scene = Scene::new();
    scene.insert(SceneObject::new(
        7301,
        "requested",
        Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0)),
    ));
    let mut near = SceneObject::new(7302, "near", reduced_3d_tetra_mesh());
    near.transform.translation = Vec3::new(3.0, 0.0, 0.0);
    scene.insert(near);
    let mut hidden = SceneObject::new(7303, "hidden", reduced_3d_tetra_mesh());
    hidden.visible = false;
    hidden.transform.translation = Vec3::new(1.0, 0.0, 0.0);
    scene.insert(hidden);
    let mut remote = SceneObject::new(7304, "remote", reduced_3d_tetra_mesh());
    remote.transform.translation = Vec3::new(12.0, 0.0, 0.0);
    scene.insert(remote);

    let observation =
        crate::capture_reduced_3d(&scene, &[7301], 2.0, crate::Reduced3DComplexity::Low).unwrap();
    assert_eq!(observation.context_ids, vec![7302]);
    assert_eq!(observation.objects[1].role, crate::Reduced3DRole::Context);
    assert_eq!(observation.objects[1].object_id, 7302);
    assert!((observation.objects[1].distance_to_requested.unwrap() - 1.5).abs() < 1.0e-9);
}

#[test]
fn reduced_3d_fallback_obeys_global_budgets_and_labels_bounds_only() {
    let mut scene = Scene::new();
    let ids: Vec<_> = (0..24).map(|offset| 7400 + offset).collect();
    for object_id in &ids {
        scene.insert(SceneObject::new(
            *object_id,
            format!("tetra {object_id}"),
            reduced_3d_tetra_mesh(),
        ));
    }

    let observation =
        crate::capture_reduced_3d(&scene, &ids, 0.0, crate::Reduced3DComplexity::Low).unwrap();
    assert_eq!(observation.objects.len(), ids.len());
    assert!(observation.point_count <= observation.budget.max_points);
    assert!(observation.triangle_count <= observation.budget.max_triangles);
    assert!(observation.objects.iter().any(|object| {
        object.quality == crate::Reduced3DQuality::BoundsOnlyApproximation
            && matches!(
                &object.representation,
                crate::Reduced3DRepresentation::BoundsOnly { .. }
            )
    }));
    assert_eq!(
        observation.geometry_quality,
        crate::Reduced3DGeometryQuality::ApproximateRenderMeshWithBoundsOnlyObjects
    );
}

#[test]
fn reduced_3d_rejects_invalid_capture_requests() {
    let mut scene = Scene::new();
    scene.insert(SceneObject::new(
        7501,
        "box",
        Mesh::cuboid(Vec3::new(1.0, 1.0, 1.0)),
    ));
    assert_eq!(
        crate::capture_reduced_3d(&scene, &[], 1.0, crate::Reduced3DComplexity::Low),
        Err(crate::Reduced3DError::EmptyObjectIds)
    );
    assert_eq!(
        crate::capture_reduced_3d(&scene, &[7501], f64::NAN, crate::Reduced3DComplexity::Low),
        Err(crate::Reduced3DError::InvalidContextRadius)
    );
    assert_eq!(
        crate::capture_reduced_3d(&scene, &[7502], 0.0, crate::Reduced3DComplexity::Low),
        Err(crate::Reduced3DError::UnknownObject(7502))
    );
}
