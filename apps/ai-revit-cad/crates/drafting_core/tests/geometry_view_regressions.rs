//! Regressions for analytic geometry and camera-relative display/picking.
//! CHEAPEST_VERIFICATION_PATH: cargo test -p drafting_core --offline --test geometry_view_regressions -j2
use drafting_core::*;

fn create_geometry(document: &mut LabDocument, geometry: EntityGeometry) -> DraftEntityId {
    document
        .apply(CommandEnvelope {
            command_id: "geometry-view-regression".into(),
            expected_revision: document.revision(),
            operations: vec![DraftOperation::Create {
                drawing_id: DrawingId::new(1),
                layer_id: LayerId::new(1),
                geometry,
                style: DraftStyle::default(),
            }],
        })
        .unwrap()
        .created_ids[0]
}

#[test]
fn analytic_arc_keeps_a_small_sweep_after_a_large_start_angle() {
    for sweep_angle in [1e-6, -1e-6, 1e-5, -1e-5] {
        let geometry = EntityGeometry::Arc {
            center: Point2::default(),
            radius: 1e8,
            start_angle: 1e12,
            sweep_angle,
        };
        geometry.validate(GeometryLimits::default()).unwrap();
        let arc = AnalyticArc {
            center: Point2::default(),
            radius: 1e8,
            start_angle: 1e12,
            sweep_angle,
        };
        let start = arc.point(0.);
        let end = arc.point(1.);
        assert!(
            start.distance(end) > 1e8 * sweep_angle.abs() * 0.99,
            "sweep {sweep_angle} collapsed"
        );

        let mut camera = Camera2D::new(1000., 700.);
        camera.center = start;
        camera.pixels_per_metre = 0.01;
        let segments = tessellate_geometry(&geometry, &camera, &DisplayOptions::default()).unwrap();
        assert_eq!(segments.len(), 1);
        assert!(segments[0].start.distance(Point2::new(500., 350.)) < 1e-6);
        let expected_pixels = 2. * 1e8 * (sweep_angle.abs() * 0.5).sin() * 0.01;
        assert!(segments[0].start.distance(segments[0].end) > expected_pixels * 0.99);
    }
}

#[test]
fn large_start_angle_arcs_remain_pickable_on_both_sweep_directions() {
    for sweep_angle in [1e-6, -1e-6, 1e-5, -1e-5] {
        let mut document = LabDocument::new();
        let id = create_geometry(
            &mut document,
            EntityGeometry::Arc {
                center: Point2::default(),
                radius: 1e8,
                start_angle: 1e12,
                sweep_angle,
            },
        );
        let snapshot = document.snapshot(DrawingId::new(1)).unwrap();
        let point = AnalyticArc {
            center: Point2::default(),
            radius: 1e8,
            start_angle: 1e12,
            sweep_angle,
        }
        .point(0.5);
        let mut camera = Camera2D::new(1000., 700.);
        camera.center = point;
        camera.pixels_per_metre = 100.;

        let hits = pick(&snapshot, &camera, Point2::new(500., 350.), 0.01).unwrap();
        assert_eq!(hits.first().map(|hit| hit.entity_id), Some(id), "sweep={sweep_angle}");
    }
}

#[test]
fn huge_arc_projection_rebases_before_scaling_and_clipping() {
    let geometry = EntityGeometry::Arc {
        center: Point2::default(),
        radius: 1e12,
        start_angle: 0.,
        sweep_angle: 1.5e-12,
    };
    let mut camera = Camera2D::new(1000., 700.);
    camera.center = Point2::new(1e12, 0.);
    camera.pixels_per_metre = 1e12;

    let segments = tessellate_geometry(&geometry, &camera, &DisplayOptions::default()).unwrap();
    assert_eq!(segments.len(), 1);
    assert!((segments[0].start.x - 500.).abs() < 1e-6);
    assert!((segments[0].end.x - 500.).abs() < 1e-6);
    assert!((segments[0].start.y - 350.).abs() < 1e-6);
    assert!(segments[0].end.y <= 1e-6);
}

#[test]
fn large_origin_pick_uses_screen_relative_aperture_and_distance() {
    let mut document = LabDocument::new();
    let id = create_geometry(
        &mut document,
        EntityGeometry::Line {
            start: Point2::new(1e12, -1.),
            end: Point2::new(1e12, 1.),
        },
    );
    let snapshot = document.snapshot(DrawingId::new(1)).unwrap();
    let mut camera = Camera2D::new(1000., 700.);
    camera.center = Point2::new(1e12, 0.);
    camera.pixels_per_metre = 1e12;

    let on_line = pick(&snapshot, &camera, Point2::new(500., 350.), 5.).unwrap();
    assert_eq!(on_line.len(), 1);
    assert_eq!(on_line[0].entity_id, id);

    // At this scale screen_to_world rounds every x offset here back to 1e12.
    let off_line = pick(&snapshot, &camera, Point2::new(900., 350.), 5.).unwrap();
    assert!(off_line.is_empty(), "a point 400 px away must not pick the line");
}

#[test]
fn small_circular_bounds_at_large_origins_stay_conservative_for_picking() {
    let geometries = [
        EntityGeometry::Circle {
            center: Point2::new(1e12, 0.),
            radius: 1e-5,
        },
        EntityGeometry::Arc {
            center: Point2::new(1e12, 0.),
            radius: 1e-5,
            start_angle: -0.5,
            sweep_angle: 1.,
        },
    ];
    let mut camera = Camera2D::new(1000., 700.);
    camera.center = Point2::new(1e12, 0.);
    camera.pixels_per_metre = 2e7;

    for geometry in geometries {
        let bounds = geometry.bounds().unwrap();
        let mut document = LabDocument::new();
        let id = create_geometry(&mut document, geometry);
        let snapshot = document.snapshot(DrawingId::new(1)).unwrap();
        assert!(bounds.min.x < 1e12 && bounds.max.x > 1e12);

        // This screen point lies on the radius at 200 px from the center.
        // At x=1e12, radius=1e-5 rounds away in center +/- radius unless the
        // analytic bounds round outward before the spatial-index query.
        let hits = pick(&snapshot, &camera, Point2::new(700., 350.), 0.01).unwrap();
        assert_eq!(hits.first().map(|hit| hit.entity_id), Some(id));
    }
}

#[test]
fn large_radius_arc_picking_respects_the_analytic_endpoint() {
    let mut document = LabDocument::new();
    create_geometry(
        &mut document,
        EntityGeometry::Arc {
            center: Point2::default(),
            radius: 1e12,
            start_angle: 0.,
            sweep_angle: std::f64::consts::FRAC_PI_4,
        },
    );
    let snapshot = document.snapshot(DrawingId::new(1)).unwrap();
    let theta = std::f64::consts::FRAC_PI_4 + 0.6e-12;
    let query = Point2::new(1e12 * theta.cos(), 1e12 * theta.sin());
    let mut camera = Camera2D::new(1000., 700.);
    camera.center = query;
    camera.pixels_per_metre = 1.;

    assert!(pick(&snapshot, &camera, Point2::new(500., 350.), 0.5)
        .unwrap()
        .is_empty());
}

#[test]
fn shallow_bulge_validates_from_curve_bounds_not_derived_center() {
    let start = Point2::new(0., 1e12);
    let end = Point2::new(1., 1e12);
    let bulge = 2e-12;
    let limits = GeometryLimits::default();
    let derived_arc = bulge_arc(start, end, bulge).unwrap().unwrap();
    assert!(derived_arc.center.y > limits.max_abs_coordinate_m);
    assert!(derived_arc.radius <= limits.max_abs_coordinate_m);

    let geometry = EntityGeometry::Polyline {
        vertices: vec![
            PolylineVertex::new(start, bulge),
            PolylineVertex::new(end, 0.),
        ],
        closed: false,
    };
    geometry.validate(limits).unwrap();

    let bounds = geometry.bounds().unwrap();
    let adjacent_below_limit = f64::from_bits(limits.max_abs_coordinate_m.to_bits() - 1);
    let adjacent_above_limit = f64::from_bits(limits.max_abs_coordinate_m.to_bits() + 1);
    assert_eq!(bounds.min.y, adjacent_below_limit);
    assert_eq!(bounds.max.y, adjacent_above_limit);
}

#[test]
fn negative_bulge_on_closed_polyline_closing_segment_is_analytic() {
    let geometry = EntityGeometry::Polyline {
        vertices: vec![
            PolylineVertex::new(Point2::new(-1., 0.), 0.),
            PolylineVertex::new(Point2::new(1., 0.), 0.),
            PolylineVertex::new(Point2::new(0., 1.), -0.5),
        ],
        closed: true,
    };
    geometry.validate(GeometryLimits::default()).unwrap();
    let closing = bulge_curve(Point2::new(0., 1.), Point2::new(-1., 0.), -0.5)
        .unwrap()
        .unwrap();
    let on_closing_arc = closing.point(0.5);
    assert!(geometry.distance(on_closing_arc).unwrap() < 1e-12);
    let bounds = geometry.bounds().unwrap();
    assert!(bounds.min.x <= on_closing_arc.x && on_closing_arc.x <= bounds.max.x);
    assert!(bounds.min.y <= on_closing_arc.y && on_closing_arc.y <= bounds.max.y);

    let mut document = LabDocument::new();
    let id = create_geometry(&mut document, geometry);
    let snapshot = document.snapshot(DrawingId::new(1)).unwrap();
    let mut camera = Camera2D::new(1000., 700.);
    camera.center = on_closing_arc;
    camera.pixels_per_metre = 100.;
    let hits = pick(&snapshot, &camera, Point2::new(500., 350.), 0.01).unwrap();
    assert_eq!(hits.first().map(|hit| hit.entity_id), Some(id));
}
