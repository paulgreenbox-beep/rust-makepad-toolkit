use geotech_core::*;
use std::sync::atomic::AtomicBool;

fn input() -> SlopeAnalysisInput {
    SlopeAnalysisInput {
        section: triangular_section(10.0, 45.0, 30.0, 1.0).unwrap(),
        soil: SoilMaterial {
            name: "Explicit analytical fixture".into(),
            unit_weight_kn_per_m3: 18.0,
            effective_cohesion_kpa: 0.0,
            effective_friction_angle_deg: 30.0,
            provenance: "GEO-SLOPE-001 analytical test; not design soil".into(),
        },
        units: InputUnits::default(),
        conditions: AnalysisConditions::default(),
        required_factor_of_safety: None,
        standard: None,
    }
}
fn near(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "actual={actual:.16e}, expected={expected:.16e}, tolerance={tolerance}"
    );
}
fn translate(input: &mut SlopeAnalysisInput, offset: f64) {
    for p in &mut input.section.terrain_polyline {
        p.x += offset;
        p.z += offset;
    }
    input.section.slope_toe.x += offset;
    input.section.slope_toe.z += offset;
    input.section.crest.x += offset;
    input.section.crest.z += offset;
    input.section.slip_surface.lower.x += offset;
    input.section.slip_surface.lower.z += offset;
    input.section.slip_surface.upper.x += offset;
    input.section.slip_surface.upper.z += offset;
}

#[test]
fn golden_geometry() {
    let original = input();
    let copy = original.clone();
    let r = analyze(&original).unwrap();
    near(r.geometry.area_m2, 36.60254037844387, 1e-10);
    near(r.geometry.slip_length_m, 20.0, 1e-12);
    near(r.geometry.volume_m3, 36.60254037844387, 1e-10);
    near(r.forces_per_m.weight_kn_per_m, 658.8457268119896, 1e-9);
    near(r.forces_per_m.normal_kn_per_m, 570.5771365940052, 1e-9);
    near(r.forces_per_m.driving_kn_per_m, 329.4228634059948, 1e-9);
    near(r.factor_of_safety, 1.0, 1e-14);
    assert_eq!(copy, original);
    assert!(!r.status.numerically_verified);
    assert!(r.status.engineering_review_required);
    assert!(polygon_area_m2(&r.geometry.polygon).unwrap() > 0.0);
}
#[test]
fn width_scales_only_extensive_totals() {
    let mut case = input();
    let a = analyze(&case).unwrap();
    case.section.extrusion_width_m = 10.0;
    let b = analyze(&case).unwrap();
    near(a.geometry.area_m2, b.geometry.area_m2, 0.0);
    near(b.geometry.volume_m3, a.geometry.volume_m3 * 10.0, 1e-9);
    near(
        b.total_forces.weight_kn,
        a.total_forces.weight_kn * 10.0,
        1e-9,
    );
    near(
        a.forces_per_m.weight_kn_per_m,
        b.forces_per_m.weight_kn_per_m,
        0.0,
    );
    near(a.factor_of_safety, b.factor_of_safety, 0.0);
}
#[test]
fn cohesion_and_friction_raise_resistance() {
    let mut case = input();
    let a = analyze(&case).unwrap();
    case.soil.effective_cohesion_kpa = 10.0;
    let b = analyze(&case).unwrap();
    assert!(b.factor_of_safety > a.factor_of_safety);
    near(
        b.forces_per_m.resistance_kn_per_m - a.forces_per_m.resistance_kn_per_m,
        200.0,
        1e-10,
    );
    case.soil.effective_friction_angle_deg = 35.0;
    let c = analyze(&case).unwrap();
    assert!(c.factor_of_safety > b.factor_of_safety);
}
#[test]
fn vertical_slope_reverse_order_and_left_facing_are_supported() {
    let mut case = input();
    case.section = triangular_section(10.0, 90.0, 30.0, 2.0).unwrap();
    let a = analyze(&case).unwrap();
    near(a.geometry.area_m2, 86.60254037844386, 1e-10);
    case.section.terrain_polyline.reverse();
    let b = analyze(&case).unwrap();
    near(a.factor_of_safety, b.factor_of_safety, 0.0);
    for p in &mut case.section.terrain_polyline {
        p.x = -p.x;
    }
    case.section.crest.x = -case.section.crest.x;
    case.section.slope_toe.x = -case.section.slope_toe.x;
    case.section.slip_surface.upper.x = -case.section.slip_surface.upper.x;
    let c = analyze(&case).unwrap();
    near(c.geometry.area_m2, a.geometry.area_m2, 1e-10);
    near(c.factor_of_safety, a.factor_of_safety, 1e-14);
}
#[test]
fn large_coordinates_use_local_area_without_modifying_input() {
    let mut case = input();
    translate(&mut case, 1e9);
    let preserved = case.clone();
    let r = analyze(&case).unwrap();
    near(r.geometry.area_m2, 36.60254037844387, 1e-6);
    near(r.factor_of_safety, 1.0, 1e-8);
    assert_eq!(preserved, case);
    assert!(r.geometry.numeric_tolerance_m > 1e-7);
}
#[test]
fn thin_prism_is_resolved_locally_or_rejected_at_world_resolution() {
    let mut case = input();
    case.section = triangular_section(10.0, 45.0, 44.9999999, 1.0).unwrap();
    let r = analyze(&case).unwrap();
    assert!(r.geometry.area_m2 > 0.0 && r.geometry.area_m2 < 1e-5);
    translate(&mut case, 1e9);
    assert!(analyze(&case).is_err());
}
#[test]
fn geometry_scaling_preserves_friction_only_fs() {
    let mut case = input();
    let a = analyze(&case).unwrap();
    for p in &mut case.section.terrain_polyline {
        p.x *= 0.001;
        p.z *= 0.001;
    }
    case.section.crest.x *= 0.001;
    case.section.crest.z *= 0.001;
    case.section.slip_surface.upper.x *= 0.001;
    case.section.slip_surface.upper.z *= 0.001;
    let b = analyze(&case).unwrap();
    near(b.geometry.area_m2, a.geometry.area_m2 * 1e-6, 1e-16);
    near(a.factor_of_safety, b.factor_of_safety, 1e-14);
}
#[test]
fn unsupported_actions_and_wrong_units_fail_explicitly() {
    let mut case = input();
    case.units.length = LengthUnit::Millimetre;
    assert_eq!(analyze(&case).unwrap_err().code, ErrorCode::InvalidUnits);
    case = input();
    case.conditions.pore_pressure = Some(PorePressureContract {
        resultant_kn_per_m: 0.0,
        model: "not enabled".into(),
    });
    assert_eq!(
        analyze(&case).unwrap_err().code,
        ErrorCode::UnsupportedConditions
    );
    case = input();
    case.conditions.anchors = true;
    assert_eq!(
        analyze(&case).unwrap_err().code,
        ErrorCode::UnsupportedConditions
    );
    case = input();
    case.conditions.external_loads.push(ExternalLoad {
        description: "zero load still unsupported".into(),
        force_x_kn_per_m: 0.0,
        force_z_kn_per_m: 0.0,
    });
    assert_eq!(
        analyze(&case).unwrap_err().code,
        ErrorCode::UnsupportedConditions
    );
}
#[test]
fn invalid_numbers_and_soil_are_rejected_before_serialization() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut case = input();
        case.soil.unit_weight_kn_per_m3 = value;
        assert_eq!(analyze(&case).unwrap_err().code, ErrorCode::NonFiniteInput);
        assert!(input_fingerprint(&case).is_err());
    }
    for value in [-1.0, 0.0] {
        let mut case = input();
        case.soil.unit_weight_kn_per_m3 = value;
        assert!(analyze(&case).is_err());
    }
    for value in [-0.1, 90.0, 100.0] {
        let mut case = input();
        case.soil.effective_friction_angle_deg = value;
        assert!(analyze(&case).is_err());
    }
}
#[test]
fn zero_driving_and_degenerate_input_have_no_fabricated_fs() {
    assert!(triangular_section(10.0, 45.0, 0.0, 1.0).is_err());
    assert!(triangular_section(10.0, 45.0, 45.0, 1.0).is_err());
    let mut case = input();
    case.section.slip_surface.upper = Point2::new(1.0, 0.0);
    assert_eq!(analyze(&case).unwrap_err().code, ErrorCode::NoDrivingForce);
}
#[test]
fn terrain_crossings_intruding_tails_and_multiple_intersections_are_diagnostic() {
    let mut case = input();
    case.section.terrain_polyline = vec![
        Point2::new(0.0, 0.0),
        Point2::new(2.0, 2.0),
        Point2::new(0.0, 2.0),
        Point2::new(2.0, 0.0),
    ];
    assert_eq!(
        analyze(&case).unwrap_err().code,
        ErrorCode::SelfIntersection
    );
    case = input();
    case.section.terrain_polyline = vec![
        Point2::new(1.0, 1.0),
        Point2::new(0.0, 0.0),
        Point2::new(2.0, 3.0),
        Point2::new(4.0, 2.0),
        Point2::new(5.0, 2.0),
    ];
    case.section.crest = Point2::new(2.0, 3.0);
    case.section.slip_surface.upper = Point2::new(4.0, 2.0);
    let e = analyze(&case).unwrap_err();
    assert_eq!(e.code, ErrorCode::UnsupportedTerrain);
    assert!(!e.diagnostic_geometry.is_empty());
    case = input();
    case.section.terrain_polyline = vec![
        Point2::new(0.0, 0.0),
        Point2::new(2.0, 3.0),
        Point2::new(4.0, 1.0),
        Point2::new(6.0, 4.0),
        Point2::new(8.0, 4.0),
    ];
    case.section.crest = Point2::new(2.0, 3.0);
    case.section.slip_surface.upper = Point2::new(8.0, 4.0);
    let e = analyze(&case).unwrap_err();
    assert_eq!(e.code, ErrorCode::AmbiguousIntersection);
    assert!(e.diagnostic_geometry.len() > 2);
}

#[test]
fn p1_resolved_bends_on_long_endpoint_edges_are_never_dropped() {
    // Independently derived exact binary64 polygon area 2.25 m2.
    // The old dimensionless 1e-10 index tolerance dropped a resolved bend.
    for upper_boundary in [false, true] {
        let mut case = input();
        if upper_boundary {
            case.section.terrain_polyline = vec![
                Point2::new(0.0, 0.0),
                Point2::new(0.5, 0.5),
                Point2::new(1.5, 2.5),
                Point2::new(2e10, 2.5),
            ];
            case.section.slope_toe = Point2::new(0.0, 0.0);
            case.section.crest = Point2::new(1.5, 2.5);
            case.section.slip_surface = PlanarSlipSurface {
                lower: Point2::new(0.0, 0.0),
                upper: Point2::new(3.5, 2.5),
            };
        } else {
            case.section.terrain_polyline = vec![
                Point2::new(0.0, 0.0),
                Point2::new(1e10, 1e10),
                Point2::new(1e10 + 1.0, 1e10 + 2.0),
                Point2::new(2e10, 1e10 + 2.0),
            ];
            case.section.slope_toe = Point2::new(1e10 - 0.5, 1e10 - 0.5);
            case.section.crest = Point2::new(1e10 + 1.0, 1e10 + 2.0);
            case.section.slip_surface = PlanarSlipSurface {
                lower: case.section.slope_toe,
                upper: Point2::new(1e10 + 3.0, 1e10 + 2.0),
            };
        }
        let preserved = case.clone();
        for reverse in [false, true] {
            let mut sample = case.clone();
            if reverse {
                sample.section.terrain_polyline.reverse();
            }
            let result = analyze(&sample).unwrap();
            assert_eq!(result.geometry.polygon.len(), 4);
            near(result.geometry.area_m2, 2.25, 1e-12);
            near(result.forces_per_m.weight_kn_per_m, 40.5, 1e-12);
            near(result.factor_of_safety, 0.8082903768654761, 1e-14);
            sample.soil.effective_cohesion_kpa = 5.0;
            near(
                analyze(&sample).unwrap().factor_of_safety,
                1.7218706237790563,
                1e-14,
            );
            assert!(extrude(&result.geometry).is_ok());
        }
        assert_eq!(case, preserved);
    }
}

#[test]
fn arbitrary_simple_nonmonotone_profile_matches_independent_fixture() {
    let mut case = input();
    case.section.terrain_polyline = vec![
        Point2::new(0.0, 0.0),
        Point2::new(3.0, 4.0),
        Point2::new(2.0, 5.0),
        Point2::new(5.0, 6.5),
        Point2::new(8.0, 7.0),
        Point2::new(24.0, 7.0),
    ];
    case.section.crest = Point2::new(8.0, 7.0);
    case.section.slip_surface.upper = Point2::new(19.232341936182355, 7.0);
    case.soil.effective_cohesion_kpa = 2.0;
    case.soil.effective_friction_angle_deg = 26.0;
    let preserved = case.clone();
    let result = analyze(&case).unwrap();
    near(result.geometry.area_m2, 50.313196776638236, 1e-10);
    near(result.geometry.slip_length_m, 20.466630801141608, 1e-11);
    near(
        result.forces_per_m.weight_kn_per_m,
        905.63754197948822,
        1e-9,
    );
    near(
        result.forces_per_m.normal_kn_per_m,
        851.02091530481334,
        1e-9,
    );
    near(
        result.forces_per_m.driving_kn_per_m,
        309.74628190893094,
        1e-9,
    );
    near(
        result.forces_per_m.resistance_kn_per_m,
        456.00389554758857,
        1e-9,
    );
    near(result.factor_of_safety, 1.4721852115133995, 1e-12);
    assert!(extrude(&result.geometry).is_ok());
    assert_eq!(case, preserved);
    case.section.terrain_polyline.reverse();
    let reversed = analyze(&case).unwrap();
    near(reversed.geometry.area_m2, result.geometry.area_m2, 1e-12);
    for p in &mut case.section.terrain_polyline {
        p.x = -p.x;
    }
    case.section.crest.x = -case.section.crest.x;
    case.section.slip_surface.upper.x = -case.section.slip_surface.upper.x;
    near(
        analyze(&case).unwrap().factor_of_safety,
        result.factor_of_safety,
        1e-12,
    );
}

#[test]
fn offchain_terrain_intrusion_is_rejected_in_both_orders() {
    let mut case = input();
    case.section.terrain_polyline = vec![
        Point2::new(1.0, 1.0),
        Point2::new(0.0, 0.0),
        Point2::new(2.0, 3.0),
        Point2::new(4.0, 2.0),
        Point2::new(5.0, 2.0),
    ];
    case.section.crest = Point2::new(2.0, 3.0);
    case.section.slip_surface.upper = Point2::new(4.0, 2.0);
    for reverse in [false, true] {
        let mut sample = case.clone();
        if reverse {
            sample.section.terrain_polyline.reverse();
        }
        let error = analyze(&sample).unwrap_err();
        assert_eq!(error.code, ErrorCode::UnsupportedTerrain);
        assert!(!error.diagnostic_geometry.is_empty());
    }
}

#[test]
fn search_rejects_zero_and_unresolved_direction_before_signum() {
    let spec = CandidateSearchSpec {
        start_angle_deg: 20.0,
        end_angle_deg: 40.0,
        coarse_step_deg: 5.0,
        refinement_rounds: 0,
    };
    for x in [0.0, -0.0, 1e-15] {
        let mut case = input();
        case.section.slip_surface.upper.x = x;
        assert_eq!(
            search(&case, &spec).unwrap_err().code,
            ErrorCode::InvalidSearch
        );
    }
}
#[test]
fn negative_polygon_area_and_bowtie_are_rejected() {
    let cw = [
        Point2::new(0.0, 0.0),
        Point2::new(0.0, 1.0),
        Point2::new(1.0, 0.0),
    ];
    assert_eq!(
        polygon_area_m2(&cw).unwrap_err().code,
        ErrorCode::ClockwisePolygon
    );
    let bow = [
        Point2::new(0.0, 0.0),
        Point2::new(2.0, 2.0),
        Point2::new(0.0, 2.0),
        Point2::new(2.0, 0.0),
    ];
    assert_eq!(
        polygon_area_m2(&bow).unwrap_err().code,
        ErrorCode::SelfIntersection
    );
}
#[test]
fn independent_mesh_volume_matches_prism_and_slip_is_subset() {
    let mut case = input();
    case.section.extrusion_width_m = 3.0;
    let r = analyze(&case).unwrap();
    let mesh = extrude(&r.geometry).unwrap();
    let origin = mesh.vertices[0];
    let mut volume = 0.0;
    for t in &mesh.triangles {
        let a = mesh.vertices[t[0] as usize];
        let b = mesh.vertices[t[1] as usize];
        let c = mesh.vertices[t[2] as usize];
        let a = [a[0] - origin[0], a[1] - origin[1], a[2] - origin[2]];
        let b = [b[0] - origin[0], b[1] - origin[1], b[2] - origin[2]];
        let c = [c[0] - origin[0], c[1] - origin[1], c[2] - origin[2]];
        volume += (a[0] * (b[1] * c[2] - b[2] * c[1])
            + a[1] * (b[2] * c[0] - b[0] * c[2])
            + a[2] * (b[0] * c[1] - b[1] * c[0]))
            / 6.0;
    }
    near(volume, r.geometry.volume_m3, 1e-10);
    assert_eq!(mesh.slip_triangles.len(), 2);
    assert!(
        mesh.slip_triangles
            .iter()
            .all(|s| mesh.triangles.contains(s))
    );
}
#[test]
fn bounded_search_retains_rejections_and_true_candidate_snapshot() {
    let case = input();
    let spec = CandidateSearchSpec {
        start_angle_deg: 10.0,
        end_angle_deg: 60.0,
        coarse_step_deg: 10.0,
        refinement_rounds: 2,
    };
    let s = search(&case, &spec).unwrap();
    assert_eq!(s.classification, "minimum_within_examined_planar_family");
    assert_eq!(s.tested_surfaces, s.candidates.len());
    assert!(s.candidates.iter().any(|c| c.rejection.is_some()));
    let r = s.minimum.as_ref().unwrap();
    let snapshot = s.minimum_input.as_ref().unwrap();
    assert_eq!(r.input_fingerprint, input_fingerprint(snapshot).unwrap());
    assert_eq!(analyze(snapshot).unwrap(), *r);
    assert!(
        report(snapshot, r, Some(&s))
            .unwrap()
            .contains("minimum_within_examined_planar_family")
    );
    let invalid = CandidateSearchSpec {
        coarse_step_deg: 1e-20,
        ..spec
    };
    assert_eq!(
        search(&case, &invalid).unwrap_err().code,
        ErrorCode::InvalidSearch
    );
}
#[test]
fn cancellation_stale_report_unknown_json_and_explicit_density() {
    let case = input();
    assert_eq!(
        analyze_cancellable(&case, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    let spec = CandidateSearchSpec {
        start_angle_deg: 20.0,
        end_angle_deg: 40.0,
        coarse_step_deg: 1.0,
        refinement_rounds: 0,
    };
    assert_eq!(
        search_cancellable(&case, &spec, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    let r = analyze(&case).unwrap();
    let mut changed = case.clone();
    changed.soil.effective_cohesion_kpa = 1.0;
    assert_eq!(
        report(&changed, &r, None).unwrap_err().code,
        ErrorCode::StaleResult
    );
    let mut json = serde_json::to_value(&case).unwrap();
    json.as_object_mut()
        .unwrap()
        .insert("groundwater".into(), serde_json::json!(10.0));
    assert!(serde_json::from_value::<SlopeAnalysisInput>(json).is_err());
    let soil = SoilMaterial::from_density_kg_m3(
        "Fixture".into(),
        1800.0,
        9.80665,
        0.0,
        30.0,
        "Density fixture".into(),
    )
    .unwrap();
    near(soil.unit_weight_kn_per_m3, 17.65197, 1e-12);
    assert!(soil.provenance.contains("9.80665"));
}

#[test]
fn collinear_and_concave_caps_preserve_manifold_boundary() {
    for terrain in [
        vec![
            Point2::new(0.0, 0.0),
            Point2::new(5.0, 5.0),
            Point2::new(10.0, 10.0),
            Point2::new(30.0, 10.0),
        ],
        vec![
            Point2::new(0.0, 0.0),
            Point2::new(3.0, 5.0),
            Point2::new(6.0, 4.0),
            Point2::new(9.0, 7.0),
            Point2::new(15.0, 7.0),
        ],
    ] {
        let mut case = input();
        case.section.terrain_polyline = terrain;
        if case.section.terrain_polyline.len() == 5 {
            case.section.crest = Point2::new(9.0, 7.0);
            case.section.slip_surface.upper = Point2::new(14.0, 7.0);
        }
        let r = analyze(&case).unwrap();
        let mesh = extrude(&r.geometry).unwrap();
        assert_eq!(
            mesh.cross_section_triangles.len(),
            r.geometry.polygon.len() - 2
        );
        let mut edges = std::collections::BTreeMap::new();
        for t in mesh.triangles {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        assert!(
            edges.values().all(|count| *count == 2),
            "Mesh has a T-junction or open boundary"
        );
    }
}
