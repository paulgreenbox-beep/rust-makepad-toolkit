use geotech_core::{InputUnits, Point2, SoilMaterial, circular::*};
use serde_json::Value;
use std::sync::atomic::AtomicBool;

// Authored independently by the root auditor in Python Decimal80, never by Rust.
fn reference() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/golden/reference-decimal80-v01.json"
    ))
    .unwrap()
}
fn n(v: &Value) -> f64 {
    v.as_str()
        .map(|s| s.parse().unwrap())
        .or_else(|| v.as_f64())
        .unwrap()
}
fn near(a: f64, b: f64, t: f64) {
    assert!(
        (a - b).abs() <= t,
        "actual={a:.17e} expected={b:.17e} tolerance={t:.3e}"
    );
}
fn config(target: usize) -> CircularAnalysisConfig {
    CircularAnalysisConfig {
        slices: SliceConfig {
            target_slices: target,
            verify_refinement: false,
        },
        ..Default::default()
    }
}
fn fixture(layered: bool) -> CircularAnalysisInput {
    let materials = [(18.0, 3.0, 28.0), (20.0, 8.0, 22.0), (22.0, 12.0, 18.0)]
        .into_iter()
        .enumerate()
        .take(if layered { 3 } else { 1 })
        .map(|(i, (gamma, c, phi))| GroundMaterial {
            material_id: SoilMaterialId(format!("soil-{i}")),
            properties: SoilMaterial {
                name: format!("Synthetic material {i}"),
                unit_weight_kn_per_m3: gamma,
                effective_cohesion_kpa: c,
                effective_friction_angle_deg: phi,
                provenance: "Independent Decimal80 synthetic case, not design data".into(),
            },
        })
        .collect::<Vec<_>>();
    CircularAnalysisInput {
        analysis_id: "synthetic-quarter-segment".into(),
        source_revision: "synthetic-reference-v01".into(),
        ground: GroundSection2D {
            section_id: "section-1".into(),
            coordinate_system: SectionCoordinateSystem::CartesianXzMetres,
            terrain_polyline: vec![Point2::new(-5.0, 15.0 / 7.0), Point2::new(4.0, 6.0 / 7.0)],
            layer_interfaces: if layered {
                vec![
                    LayerInterface {
                        interface_id: "interface-1".into(),
                        polyline: vec![Point2::new(-5.0, 0.8), Point2::new(4.0, 0.8)],
                    },
                    LayerInterface {
                        interface_id: "interface-2".into(),
                        polyline: vec![Point2::new(-5.0, 0.2), Point2::new(4.0, 0.38)],
                    },
                ]
            } else {
                vec![]
            },
            soil_regions: materials
                .iter()
                .enumerate()
                .map(|(i, m)| SoilRegion {
                    region_id: SoilRegionId(format!("region-{i}")),
                    material_id: m.material_id.clone(),
                })
                .collect(),
            bottom_boundary: vec![Point2::new(-5.0, -2.0), Point2::new(4.0, -2.0)],
            materials,
            model_revision: 1,
        },
        slip_surface: CircularSlipSurface::from_center_radius(
            "circle-1".into(),
            Point2::new(0.0, 5.0),
            5.0,
            SlideDirection::PositiveX,
        )
        .unwrap(),
        basis: AnalysisBasis::DryStatic,
        units: InputUnits::default(),
        extrusion_width_m: 1.0,
    }
}
fn cohesionless_fixture(layered: bool) -> CircularAnalysisInput {
    let mut input = fixture(layered);
    for material in &mut input.ground.materials {
        material.properties.effective_cohesion_kpa = 0.0;
    }
    input
}
fn mixed_top_cohesionless_fixture() -> CircularAnalysisInput {
    let mut input = fixture(true);
    input.ground.materials[0].properties.effective_cohesion_kpa = 0.0;
    input
}
fn thin_cohesionless_top_fixture() -> CircularAnalysisInput {
    let mut input = fixture(false);
    input.ground.materials[0].properties.effective_cohesion_kpa = 0.0;
    input.ground.layer_interfaces = vec![LayerInterface {
        interface_id: "thin-top-interface".into(),
        polyline: vec![
            Point2::new(-5.0, 15.0 / 7.0 - 0.01),
            Point2::new(4.0, 6.0 / 7.0 - 0.01),
        ],
    }];
    input.ground.materials.push(GroundMaterial {
        material_id: SoilMaterialId("soil-1".into()),
        properties: SoilMaterial {
            name: "Synthetic lower material".into(),
            unit_weight_kn_per_m3: 20.0,
            effective_cohesion_kpa: 3.0,
            effective_friction_angle_deg: 28.0,
            provenance: "Thin top-layer contact regression, not design data".into(),
        },
    });
    input.ground.soil_regions.push(SoilRegion {
        region_id: SoilRegionId("region-1".into()),
        material_id: SoilMaterialId("soil-1".into()),
    });
    input
}
fn manual_table(reference: &Value) -> Vec<SliceMechanicsInput> {
    reference["result"]["terms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| SliceMechanicsInput {
            horizontal_width_m: n(&t["b"]),
            base_length_m: n(&t["l"]),
            base_inclination_rad: n(&t["alpha_deg"]).to_radians(),
            weight_kn_per_m: n(&t["weight"]),
            effective_cohesion_kpa: n(&t["cohesion"]),
            effective_friction_angle_rad: n(&t["phi_deg"]).to_radians(),
        })
        .collect()
}

fn manufactured_zero_friction_table(first_weight: f64) -> Vec<SliceMechanicsInput> {
    [
        (std::f64::consts::FRAC_PI_3, first_weight, 1.0),
        (std::f64::consts::FRAC_PI_6, 10.0, 9.0),
    ]
    .into_iter()
    .map(|(alpha, weight, cohesion)| SliceMechanicsInput {
        horizontal_width_m: alpha.cos(),
        base_length_m: 1.0,
        base_inclination_rad: alpha,
        weight_kn_per_m: weight,
        effective_cohesion_kpa: cohesion,
        effective_friction_angle_rad: 0.0,
    })
    .collect()
}

fn bishop_force_pair(s: &SliceMechanicsInput, factor: f64) -> (f64, f64) {
    let (sin, cos) = s.base_inclination_rad.sin_cos();
    let tan_phi = s.effective_friction_angle_rad.tan();
    let normal = (s.weight_kn_per_m
        - s.effective_cohesion_kpa * s.base_length_m * sin / factor)
        / (cos + sin * tan_phi / factor);
    let shear = (s.effective_cohesion_kpa * s.base_length_m + normal * tan_phi) / factor;
    (normal, shear)
}

#[test]
fn circular_manual_reference() {
    let r = reference();
    let t = &r["manual_table"];
    let actual = fellenius(&manual_table(t), &config(8)).unwrap();
    let expected = &t["result"];
    near(actual.factor_of_safety, n(&expected["fellenius_fs"]), 2e-14);
    near(
        actual.driving_sum_kn_per_m,
        n(&expected["driving_sum"]),
        5e-13,
    );
    near(
        actual.resisting_sum_kn_per_m,
        n(&expected["resisting_sum"]),
        5e-13,
    );
    for (a, e) in actual
        .slice_contributions
        .iter()
        .zip(expected["terms"].as_array().unwrap())
    {
        near(a.driving_kn_per_m, n(&e["driving"]), 2e-13);
        near(a.normal_kn_per_m, n(&e["normal_ordinary"]), 2e-13);
        near(a.cohesion_kn_per_m, n(&e["cohesion_ordinary"]), 2e-13);
        near(a.friction_kn_per_m, n(&e["friction_ordinary"]), 2e-13);
    }
    assert!(actual.slice_contributions[3].driving_kn_per_m < 0.0);
    assert_eq!(actual.method, CircularMethod::FelleniusOrdinary);
    let z = &r["zero_friction"];
    near(
        fellenius(&manual_table(z), &config(8))
            .unwrap()
            .factor_of_safety,
        n(&z["result"]["fellenius_fs"]),
        2e-14,
    );
}

#[test]
fn circular_geometry_and_three_point_constructor() {
    let input = fixture(false);
    let c = config(8);
    let reference = reference();
    let mass = build_failure_mass(&input, &c).unwrap();
    near(
        mass.area_m2,
        n(&reference["geometry"]["analytic_segment_area"]),
        2e-13,
    );
    near(mass.circle.left_point.x, -4.0, 2e-14);
    near(mass.circle.left_point.z, 2.0, 2e-14);
    near(mass.circle.right_point.x, 3.0, 2e-14);
    near(mass.circle.right_point.z, 1.0, 2e-14);
    near(
        mass.circle.arc_length_m,
        5.0 * std::f64::consts::FRAC_PI_2,
        2e-14,
    );
    for points in [
        [
            Point2::new(-4.0, 2.0),
            Point2::new(0.0, 0.0),
            Point2::new(3.0, 1.0),
        ],
        [
            Point2::new(3.0, 1.0),
            Point2::new(-4.0, 2.0),
            Point2::new(0.0, 0.0),
        ],
    ] {
        let circle = CircularSlipSurface::from_three_points(
            "three".into(),
            points,
            SlideDirection::PositiveX,
            &c,
        )
        .unwrap();
        near(circle.center.x, 0.0, 2e-14);
        near(circle.center.z, 5.0, 2e-14);
        near(circle.radius_m, 5.0, 2e-14);
    }
    assert_eq!(
        CircularSlipSurface::from_three_points(
            "bad".into(),
            [
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 1.0),
                Point2::new(2.0, 2.0)
            ],
            SlideDirection::PositiveX,
            &c
        )
        .unwrap_err()
        .code,
        CircularErrorCode::InvalidCircle
    );
    let ring = render_boundary(&mass, 512, 1024).unwrap();
    near(
        geotech_core::polygon_area_m2(&ring).unwrap(),
        mass.area_m2,
        4e-5,
    );
    assert!(render_boundary(&mass, 512, 32).is_err());
}

#[test]
fn circular_homogeneous_and_layered_independent_reference() {
    let reference = reference();
    for layered in [false, true] {
        let input = fixture(layered);
        let key = if layered { "layered" } else { "homogeneous" };
        for expected in reference[key].as_array().unwrap() {
            let c = config(expected["target_slices"].as_u64().unwrap() as usize);
            let actual = diagnose_specified_circle(&input, &c).unwrap();
            assert!(actual.diagnostic_only);
            assert!(!actual.accepted);
            assert_eq!(actual.bishop_result.accepted_factor_of_safety, None);
            assert_eq!(
                actual.bishop_result.physical_assessment.scope,
                BishopPhysicalScope::CurrentDiscreteSliceTableOnly
            );
            assert!(!actual
                .bishop_result
                .physical_assessment
                .physical_contact_convergence_verified);
            assert!(!actual
                .bishop_result
                .physical_assessment
                .eligible_for_critical_minimum);
            assert_eq!(
                actual.slices.len(),
                expected["actual_slices"].as_u64().unwrap() as usize
            );
            near(actual.total_area_m2, n(&expected["total_area"]), 5e-12);
            near(
                actual.total_weight_kn_per_m,
                n(&expected["total_weight"]),
                5e-11,
            );
            near(
                actual.fellenius_result.factor_of_safety,
                n(&expected["result"]["fellenius_fs"]),
                2e-11,
            );
            near(
                actual.bishop_result.factor_of_safety,
                n(&expected["result"]["bishop_fs"]),
                3e-9,
            );
            assert_eq!(
                actual.bishop_result.method,
                CircularMethod::BishopSimplified
            );
            assert_eq!(
                actual.bishop_result.convergence_status,
                BishopConvergenceStatus::Converged
            );
            assert!(actual.bishop_result.relative_residual <= c.bishop.residual_rel);
            for (a, e) in actual
                .soil_breakdown
                .iter()
                .zip(expected["material_areas"].as_array().unwrap())
            {
                near(a.area_m2, n(e), 5e-12);
            }
            for ((a, t), e) in actual
                .slices
                .iter()
                .zip(&actual.fellenius_result.slice_contributions)
                .zip(expected["result"]["terms"].as_array().unwrap())
            {
                near(a.x_left, n(&e["x_left"]), 5e-12);
                near(a.x_right, n(&e["x_right"]), 5e-12);
                near(a.horizontal_width_m, n(&e["b"]), 5e-12);
                near(a.base_length_m, n(&e["l"]), 5e-12);
                near(a.weight_kn_per_m, n(&e["weight"]), 5e-11);
                near(t.driving_kn_per_m, n(&e["driving"]), 5e-11);
                near(t.normal_kn_per_m, n(&e["normal_ordinary"]), 5e-11);
                near(t.cohesion_kn_per_m, n(&e["cohesion_ordinary"]), 5e-11);
                near(t.friction_kn_per_m, n(&e["friction_ordinary"]), 5e-11);
                assert_eq!(
                    a.base_material_id.0,
                    format!("soil-{}", e["base_material_index"].as_u64().unwrap())
                );
                for (area, value) in a
                    .area_by_material
                    .iter()
                    .zip(e["area_by_material"].as_array().unwrap())
                {
                    near(area.area_m2, n(value), 5e-12);
                }
                let b = &actual.bishop_result.slice_contributions[a.slice_id];
                near(b.m_alpha, n(&e["bishop_m_alpha"]), 3e-9);
                near(b.contribution_kn_per_m, n(&e["bishop_contribution"]), 3e-8);
                near(
                    a.base_length_m * a.base_inclination_rad.cos(),
                    a.horizontal_width_m,
                    2e-14,
                );
            }
            assert!(!actual.numerically_verified && actual.engineering_review_required);
            assert!(!actual.discretization.refinement_checked);
            let error = analyze_specified_circle(&input, &c).unwrap_err();
            assert_eq!(error.code, CircularErrorCode::BishopPhysicallyInadmissible);
            let failure = error.bishop_diagnostic.as_ref().unwrap();
            assert_eq!(failure.status, BishopConvergenceStatus::Converged);
            let root = failure.mathematical_root.as_deref().unwrap();
            assert_eq!(root.convergence_status, BishopConvergenceStatus::Converged);
            near(
                root.factor_of_safety,
                n(&expected["result"]["bishop_fs"]),
                3e-9,
            );
            assert_eq!(root.accepted_factor_of_safety, None);
            let endpoints = root.endpoint_assessment.as_ref().unwrap();
            assert!(!endpoints.passed);
            assert!(!endpoints.whole_arc_contact_verified);
            assert_eq!(endpoints.endpoints.len(), 2);
            let entry = endpoints
                .endpoints
                .iter()
                .find(|endpoint| endpoint.endpoint_type == BishopEndpointType::Entry)
                .unwrap();
            assert_eq!(entry.status, BishopEndpointStatus::TensileInterval);
            assert!(entry.sin_alpha.unwrap() > 0.0);
            let exit = endpoints
                .endpoints
                .iter()
                .find(|endpoint| endpoint.endpoint_type == BishopEndpointType::Exit)
                .unwrap();
            assert_eq!(exit.status, BishopEndpointStatus::PositiveLimitLocalScreenPassed);
            if !layered && c.slices.target_slices == 128 {
                assert_eq!(
                    root.physical_assessment.status,
                    BishopPhysicalStatus::InadmissibleTension
                );
                assert!(root.physical_assessment.minimum_normal_kn_per_m < -0.0030);
                assert!(root.physical_assessment.minimum_normal_kn_per_m > -0.0031);
                assert!(!root.physical_assessment.tensile_slice_ids.is_empty());
            }
            if layered && c.slices.target_slices == 128 {
                assert!(root.physical_assessment.minimum_normal_kn_per_m > 0.0);
            }
        }
    }
}

#[test]
fn circular_large_world_coordinates_are_local_and_immutable() {
    let c = config(64);
    let mut moved = cohesionless_fixture(true);
    let baseline = analyze_specified_circle(&moved, &c).unwrap();
    for p in moved
        .ground
        .terrain_polyline
        .iter_mut()
        .chain(moved.ground.bottom_boundary.iter_mut())
        .chain(
            moved
                .ground
                .layer_interfaces
                .iter_mut()
                .flat_map(|i| i.polyline.iter_mut()),
        )
    {
        p.x += 1e9;
        p.z += 1e9;
    }
    moved.slip_surface.center.x += 1e9;
    moved.slip_surface.center.z += 1e9;
    let preserved = moved.clone();
    let actual = analyze_specified_circle(&moved, &c).unwrap();
    assert_eq!(moved, preserved);
    near(actual.total_area_m2, baseline.total_area_m2, 3e-6);
    near(
        actual.total_weight_kn_per_m,
        baseline.total_weight_kn_per_m,
        6e-5,
    );
    near(
        actual.fellenius_result.factor_of_safety,
        baseline.fellenius_result.factor_of_safety,
        3e-6,
    );
    assert!(actual.geometry.numerical_quality.world_resolution_m > 1e-8);
}

#[test]
fn circular_direction_and_signed_driving_are_not_absolutized() {
    let mut input = cohesionless_fixture(false);
    input.slip_surface.slide_direction = SlideDirection::NegativeX;
    assert_eq!(
        analyze_specified_circle(&input, &config(16))
            .unwrap_err()
            .code,
        CircularErrorCode::NoDrivingForce
    );
    let mut input = cohesionless_fixture(false);
    for p in input
        .ground
        .terrain_polyline
        .iter_mut()
        .chain(input.ground.bottom_boundary.iter_mut())
    {
        p.x = -p.x;
    }
    input.ground.terrain_polyline.reverse();
    input.ground.bottom_boundary.reverse();
    input.slip_surface.slide_direction = SlideDirection::NegativeX;
    let reversed = analyze_specified_circle(&input, &config(16)).unwrap();
    let normal = analyze_specified_circle(&cohesionless_fixture(false), &config(16)).unwrap();
    near(
        reversed.fellenius_result.factor_of_safety,
        normal.fellenius_result.factor_of_safety,
        5e-13,
    );
    for (result, direction) in [
        (&normal, SlideDirection::PositiveX),
        (&reversed, SlideDirection::NegativeX),
    ] {
        let endpoints = result.bishop_result.endpoint_assessment.as_ref().unwrap();
        assert!(endpoints.passed);
        assert!(!endpoints.whole_arc_contact_verified);
        assert!(result.accepted);
        assert!(!result
            .bishop_result
            .physical_assessment
            .eligible_for_critical_minimum);
        assert!(endpoints.endpoints.iter().all(|endpoint| {
            endpoint.status == BishopEndpointStatus::CohesionlessLocalScreenPassed
        }));
        let entry = endpoints
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_type == BishopEndpointType::Entry)
            .unwrap();
        assert_eq!(entry.status, BishopEndpointStatus::CohesionlessLocalScreenPassed);
        assert!(entry.sin_alpha.unwrap() > 0.0);
        let expected_inward = if direction == SlideDirection::PositiveX {
            1
        } else {
            -1
        };
        assert_eq!(entry.inward_x_direction, expected_inward);
        near(
            entry.point.x,
            result.geometry.circle.entry_point.x,
            1e-12,
        );
    }
}

#[test]
fn circular_ground_topology_and_material_rejections() {
    let c = config(8);
    let original = fixture(true);
    let mut input = original.clone();
    input.ground.materials.pop();
    assert_eq!(
        validate_ground(&input.ground, &c).unwrap_err().code,
        CircularErrorCode::MissingMaterial
    );
    let mut input = original.clone();
    input.ground.layer_interfaces[1].polyline[1].z = 1.0;
    assert_eq!(
        validate_ground(&input.ground, &c).unwrap_err().code,
        CircularErrorCode::InvalidGround
    );
    let mut input = original.clone();
    input.ground.layer_interfaces[0].polyline[0].x += 0.1;
    assert_eq!(
        validate_ground(&input.ground, &c).unwrap_err().code,
        CircularErrorCode::UnsupportedGroundTopology
    );
    let mut input = original;
    input.ground.terrain_polyline.reverse();
    assert_eq!(
        validate_ground(&input.ground, &c).unwrap_err().code,
        CircularErrorCode::UnsupportedGroundTopology
    );
}

#[test]
fn circular_invalid_geometry_nonfinite_and_budget_rejections() {
    let c = config(8);
    for radius in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut input = fixture(false);
        input.slip_surface.radius_m = radius;
        assert!(build_failure_mass(&input, &c).is_err());
    }
    let mut input = fixture(false);
    input.slip_surface.center.z = 100.0;
    assert_eq!(
        build_failure_mass(&input, &c).unwrap_err().code,
        CircularErrorCode::NoTerrainIntersection
    );
    let mut input = fixture(false);
    input
        .ground
        .bottom_boundary
        .iter_mut()
        .for_each(|p| p.z = 0.1);
    assert_eq!(
        build_failure_mass(&input, &c).unwrap_err().code,
        CircularErrorCode::SlipOutsideGround
    );
    let mut limited = c;
    limited.limits.max_slices = 8;
    assert_eq!(
        generate_slices(&fixture(true), &limited).unwrap_err().code,
        CircularErrorCode::ResourceLimit
    );
    let mut bad = c;
    bad.tolerances.fs_rel = f64::NAN;
    assert_eq!(
        validate_config(&bad).unwrap_err().code,
        CircularErrorCode::NonFiniteInput
    );
    let mut bad = c;
    bad.limits.max_slices = usize::MAX;
    assert_eq!(
        validate_config(&bad).unwrap_err().code,
        CircularErrorCode::ResourceLimit
    );
    let mut bad = fixture(false);
    bad.ground.materials[0].properties.unit_weight_kn_per_m3 = f64::NAN;
    assert!(input_fingerprint(&bad, &c).is_err());
}

#[test]
fn circular_tangency_extra_contacts_and_selected_arc_rejected() {
    let c = config(8);
    let mut tangent = fixture(false);
    tangent
        .ground
        .terrain_polyline
        .iter_mut()
        .for_each(|p| p.z = 0.0);
    assert_eq!(
        build_failure_mass(&tangent, &c).unwrap_err().code,
        CircularErrorCode::NoTerrainIntersection
    );
    let mut extra = fixture(false);
    extra.ground.terrain_polyline = vec![
        Point2::new(-5.0, 2.0),
        Point2::new(-4.0, 2.0),
        Point2::new(-2.0, -1.0),
        Point2::new(0.0, 2.0),
        Point2::new(2.0, -1.0),
        Point2::new(3.0, 1.0),
        Point2::new(4.0, 2.0),
    ];
    assert_eq!(
        build_failure_mass(&extra, &c).unwrap_err().code,
        CircularErrorCode::AmbiguousIntersection
    );
    let mut selected = fixture(false);
    selected.slip_surface.selected_endpoints = Some(CircleEndpoints {
        entry_point: Point2::new(3.0, 1.0),
        exit_point: Point2::new(-4.0, 2.0),
    });
    assert_eq!(
        build_failure_mass(&selected, &c).unwrap_err().code,
        CircularErrorCode::InvalidCircle
    );
    selected.slip_surface.selected_endpoints = Some(CircleEndpoints {
        entry_point: Point2::new(-4.0, 2.0),
        exit_point: Point2::new(3.0, 1.0),
    });
    assert!(build_failure_mass(&selected, &c).is_ok());
}

#[test]
fn circular_mechanics_invalid_contract_and_near_vertical() {
    let r = reference();
    let mut table = manual_table(&r["manual_table"]);
    let c = config(8);
    table[0].base_length_m = table[0].horizontal_width_m;
    assert_eq!(
        fellenius(&table, &c).unwrap_err().code,
        CircularErrorCode::InvalidParameter
    );
    table = manual_table(&r["manual_table"]);
    table[0].base_inclination_rad = std::f64::consts::FRAC_PI_2 - 1e-9;
    assert_eq!(
        fellenius(&table, &c).unwrap_err().code,
        CircularErrorCode::NearVerticalBase
    );
    table = manual_table(&r["manual_table"]);
    table[0].weight_kn_per_m = f64::INFINITY;
    assert_eq!(
        fellenius(&table, &c).unwrap_err().code,
        CircularErrorCode::NonFiniteInput
    );
    assert!(fellenius(&[], &c).is_err());
}

#[test]
fn circular_refinement_cancellation_fingerprints_and_width() {
    let input = cohesionless_fixture(true);
    let mut c = config(16);
    c.slices.verify_refinement = true;
    let actual = analyze_specified_circle(&input, &c).unwrap();
    assert!(actual.discretization.refinement_checked);
    assert!(
        actual.discretization.factor_delta.unwrap() <= actual.discretization.accepted_tolerance
    );
    assert!(
        actual.discretization.bishop_factor_delta.unwrap()
            <= actual.discretization.bishop_accepted_tolerance
    );
    let mut limited = c;
    limited.limits.max_slices = 32;
    assert_eq!(
        analyze_specified_circle(&input, &limited).unwrap_err().code,
        CircularErrorCode::DiscretizationNotConverged
    );
    assert_eq!(
        analyze_specified_circle_cancellable(&input, &c, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        CircularErrorCode::Cancelled
    );
    let mut wide = input.clone();
    wide.extrusion_width_m = 50.0;
    let wide_result = analyze_specified_circle(&wide, &c).unwrap();
    near(
        wide_result.fellenius_result.factor_of_safety,
        actual.fellenius_result.factor_of_safety,
        0.0,
    );
    assert_ne!(wide_result.input_fingerprint, actual.input_fingerprint);
    let mut changed = input;
    changed.ground.materials[0]
        .properties
        .effective_cohesion_kpa += 1.0;
    assert_ne!(
        input_fingerprint(&changed, &c).unwrap(),
        actual.input_fingerprint
    );
    assert_eq!(source_fingerprint(), actual.source_fingerprint);
}

#[test]
fn circular_bishop_manual_high_precision_reference() {
    let r = reference();
    let table = manual_table(&r["manual_table"]);
    let mut c = config(8);
    c.bishop.step_abs = 1e-13;
    c.bishop.step_rel = 1e-13;
    c.bishop.residual_abs = 1e-13;
    c.bishop.residual_rel = 1e-13;
    let actual = bishop(&table, &c).unwrap();
    assert!(actual.accepted_factor_of_safety.is_some());
    assert!(actual.endpoint_assessment.is_none());
    let expected = &r["manual_table"]["result"];
    near(actual.factor_of_safety, n(&expected["bishop_fs"]), 2e-12);
    near(
        actual.driving_sum_kn_per_m,
        n(&expected["driving_sum"]),
        3e-13,
    );
    assert_eq!(actual.method, CircularMethod::BishopSimplified);
    assert_ne!(actual.method, fellenius(&table, &c).unwrap().method);
    assert!(
        (actual.factor_of_safety - fellenius(&table, &c).unwrap().factor_of_safety).abs() > 0.1
    );
    assert!(actual.final_step_delta <= actual.step_tolerance);
    assert!(actual.equation_residual.abs() <= actual.residual_tolerance);
    assert!(actual.relative_residual <= c.bishop.residual_rel);
    for (a, e) in actual
        .slice_contributions
        .iter()
        .zip(expected["terms"].as_array().unwrap())
    {
        near(a.m_alpha, n(&e["bishop_m_alpha"]), 2e-12);
        near(a.contribution_kn_per_m, n(&e["bishop_contribution"]), 3e-11);
        assert!(a.m_alpha > c.bishop.min_m_alpha);
    }
    for (index, iteration) in actual.iterations.iter().enumerate() {
        assert_eq!(iteration.iteration, index + 1);
        let f = iteration.factor_after;
        // Independently evaluate the published equation for each recorded F.
        let g = table
            .iter()
            .map(|s| {
                let (sin, cos) = s.base_inclination_rad.sin_cos();
                let tan = s.effective_friction_angle_rad.tan();
                (s.effective_cohesion_kpa * s.horizontal_width_m + s.weight_kn_per_m * tan)
                    / (cos + sin * tan / f)
            })
            .sum::<f64>()
            / n(&expected["driving_sum"]);
        near(iteration.equation_residual, f - g, 3e-15);
        near(
            iteration.step_delta,
            (iteration.factor_after - iteration.factor_before).abs(),
            0.0,
        );
        assert!(iteration.minimum_m_alpha > c.bishop.min_m_alpha);
        assert!(f >= c.bishop.min_factor && f <= c.bishop.max_factor);
    }
}

#[test]
fn circular_bishop_zero_friction_is_consistent_with_chord_fellenius() {
    let r = reference();
    let table = manual_table(&r["zero_friction"]);
    let c = config(8);
    let b = bishop(&table, &c).unwrap();
    let f = fellenius(&table, &c).unwrap();
    near(
        b.factor_of_safety,
        n(&r["zero_friction"]["result"]["bishop_fs"]),
        2e-14,
    );
    near(b.factor_of_safety, f.factor_of_safety, 2e-14);
    assert!(b.iterations.len() <= 3);
}

#[test]
fn circular_bishop_failure_status_and_poles_are_explicit() {
    let r = reference();
    let table = manual_table(&r["manual_table"]);
    let mut c = config(8);
    c.bishop.max_iterations = 1;
    let e = bishop(&table, &c).unwrap_err();
    assert_eq!(e.code, CircularErrorCode::BishopNotConverged);
    let d = e.bishop_diagnostic.unwrap();
    assert_eq!(d.status, BishopConvergenceStatus::MaxIterations);
    assert_eq!(d.iterations.len(), 1);
    assert!(d.last_factor.unwrap() > 0.0);
    let mut c = config(8);
    c.bishop.min_factor = 3.0;
    c.bishop.max_factor = 4.0;
    c.bishop.initial_factor = 3.5;
    let e = bishop(&table, &c).unwrap_err();
    assert_eq!(e.code, CircularErrorCode::BishopNoPhysicalRoot);
    assert_eq!(
        e.bishop_diagnostic.unwrap().status,
        BishopConvergenceStatus::NoPhysicalRoot
    );
    let mut c = config(8);
    c.bishop.min_factor = 0.01;
    c.bishop.max_factor = 0.09;
    c.bishop.initial_factor = 0.05;
    let e = bishop(&table, &c).unwrap_err();
    assert_eq!(e.code, CircularErrorCode::BishopNoPhysicalRoot);
    assert!(
        e.bishop_diagnostic
            .unwrap()
            .admissible_lower_factor
            .unwrap()
            > c.bishop.max_factor
    );
    let mut c = config(8);
    c.bishop.initial_factor = 0.001;
    let adjusted = bishop(&table, &c).unwrap();
    assert!(adjusted.initial_factor_used >= adjusted.admissible_lower_factor);
    assert_ne!(
        adjusted.initial_factor_requested,
        adjusted.initial_factor_used
    );
    near(
        adjusted.factor_of_safety,
        n(&r["manual_table"]["result"]["bishop_fs"]),
        1e-9,
    );
    let mut c = config(8);
    c.bishop.initial_factor = c.bishop.max_factor + 1.0;
    let e = bishop(&table, &c).unwrap_err();
    assert_eq!(e.code, CircularErrorCode::BishopFactorOutOfBounds);
    assert_eq!(
        e.bishop_diagnostic.unwrap().status,
        BishopConvergenceStatus::FactorOutOfBounds
    );
}

#[test]
fn circular_bishop_equation_residual_cannot_be_replaced_by_step_only() {
    let r = reference();
    let table = manual_table(&r["manual_table"]);
    let mut c = config(8);
    c.bishop.max_iterations = 1;
    c.bishop.step_abs = 1e9;
    c.bishop.residual_abs = 1e9;
    c.bishop.residual_rel = 1e-20;
    let e = bishop(&table, &c).unwrap_err();
    assert_eq!(e.code, CircularErrorCode::BishopNotConverged);
    let d = e.bishop_diagnostic.unwrap();
    let last = d.iterations.last().unwrap();
    assert!(last.step_delta < c.bishop.step_abs);
    assert!(last.equation_residual.abs() < c.bishop.residual_abs);
    assert!(last.relative_residual > c.bishop.residual_rel);
}

#[test]
fn circular_bishop_nonfinite_denominator_cancel_and_resource_rejections() {
    let r = reference();
    let table = manual_table(&r["manual_table"]);
    let mut c = config(8);
    c.bishop.min_factor = 1e-300;
    c.bishop.max_factor = 1e20;
    let alpha = 0.5_f64;
    let extreme = [SliceMechanicsInput {
        horizontal_width_m: 1.0,
        base_length_m: 1.0 / alpha.cos(),
        base_inclination_rad: alpha,
        weight_kn_per_m: 1.0,
        effective_cohesion_kpa: 1.0,
        effective_friction_angle_rad: std::f64::consts::FRAC_PI_2 - 1e-15,
    }];
    let e = bishop(&extreme, &c).unwrap_err();
    assert_eq!(e.code, CircularErrorCode::BishopInvalidDenominator);
    let d = e.bishop_diagnostic.unwrap();
    assert_eq!(d.status, BishopConvergenceStatus::InvalidDenominator);
    assert_eq!(d.faulty_slice_id, Some(0));
    assert_eq!(d.m_alpha, None);
    let c = config(8);
    assert_eq!(
        bishop_cancellable(&table, &c, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        CircularErrorCode::Cancelled
    );
    let mut c = config(8);
    c.bishop.max_iterations = c.limits.max_solver_iterations + 1;
    assert_eq!(
        bishop(&table, &c).unwrap_err().code,
        CircularErrorCode::ResourceLimit
    );
    let mut c = config(8);
    c.bishop.residual_abs = f64::NAN;
    assert_eq!(
        bishop(&table, &c).unwrap_err().code,
        CircularErrorCode::NonFiniteInput
    );
    let mut c = config(8);
    c.limits.max_result_bytes = 1024;
    assert_eq!(
        bishop(&table, &c).unwrap_err().code,
        CircularErrorCode::ResourceLimit
    );
}

#[test]
fn circular_bishop_failure_does_not_return_a_fellenius_result() {
    let input = fixture(true);
    let mut c = config(16);
    c.bishop.max_iterations = 1;
    let mechanics = slice_mechanics(&input.ground, &generate_slices(&input, &c).unwrap()).unwrap();
    assert!(fellenius(&mechanics, &c).is_ok());
    assert_eq!(
        analyze_specified_circle(&input, &c).unwrap_err().code,
        CircularErrorCode::BishopNotConverged
    );
    let a = config(16);
    let mut b = a;
    b.bishop.residual_rel *= 0.5;
    assert_ne!(
        input_fingerprint(&input, &a).unwrap(),
        input_fingerprint(&input, &b).unwrap()
    );
}

#[test]
fn circular_json_rejects_unknown_fields_and_unsupported_basis() {
    let input = fixture(false);
    let mut json = serde_json::to_value(&input).unwrap();
    json["pore_pressure"] = Value::from(1.0);
    assert!(serde_json::from_value::<CircularAnalysisInput>(json).is_err());
    let mut json = serde_json::to_value(&input).unwrap();
    json["basis"] = Value::from("wet_static");
    assert!(serde_json::from_value::<CircularAnalysisInput>(json).is_err());
    let mut json = serde_json::to_value(&input).unwrap();
    json["ground"]["bogus"] = Value::from(1);
    assert!(serde_json::from_value::<CircularAnalysisInput>(json).is_err());
}

#[test]
fn circular_bishop_positive_reference_table_has_accepted_base_forces() {
    let r = reference();
    let table = manual_table(&r["manual_table"]);
    let c = config(8);
    let result = bishop(&table, &c).unwrap();

    assert_eq!(result.convergence_status, BishopConvergenceStatus::Converged);
    assert_eq!(result.physical_assessment.status, BishopPhysicalStatus::Admissible);
    near(
        result.accepted_factor_of_safety.unwrap(),
        result.factor_of_safety,
        0.0,
    );
    for row in &result.slice_contributions {
        let forces = &row.base_forces;
        assert_eq!(forces.physical_status, BishopPhysicalStatus::Admissible);
        assert!(forces.normal_kn_per_m >= forces.normal_roundoff_bound_kn_per_m);
        assert!(forces.vertical_residual_kn_per_m.abs() <= forces.vertical_residual_bound_kn_per_m);
        assert!(forces.shear_law_residual_kn_per_m.abs() <= forces.shear_law_residual_bound_kn_per_m);

        let checked = check_bishop_base_forces(
            &row.mechanics,
            result.factor_of_safety,
            forces.normal_kn_per_m,
            forces.mobilized_shear_kn_per_m,
            &c,
        )
        .unwrap();
        assert_eq!(checked.physical_status, BishopPhysicalStatus::Admissible);
    }
}

#[test]
fn circular_bishop_tensile_root_is_diagnostic_only_and_strictly_rejected() {
    let c = config(8);
    let tensile_table = manufactured_zero_friction_table(0.1);
    let driving = 0.1 * std::f64::consts::FRAC_PI_3.sin()
        + 10.0 * std::f64::consts::FRAC_PI_6.sin();
    let expected_factor = 10.0 / driving;

    let error = bishop(&tensile_table, &c).unwrap_err();
    assert_eq!(error.code, CircularErrorCode::BishopPhysicallyInadmissible);
    let failure = error.bishop_diagnostic.as_ref().unwrap();
    assert_eq!(failure.status, BishopConvergenceStatus::Converged);
    let root = failure.mathematical_root.as_deref().unwrap();
    assert_eq!(root.convergence_status, BishopConvergenceStatus::Converged);
    near(root.factor_of_safety, expected_factor, 2e-12);
    assert_eq!(root.accepted_factor_of_safety, None);
    assert_eq!(
        root.physical_assessment.status,
        BishopPhysicalStatus::InadmissibleTension
    );
    assert!(root.slice_contributions[0].base_forces.normal_kn_per_m < 0.0);

    let diagnostic = bishop_diagnostic(&tensile_table, &c).unwrap();
    near(diagnostic.factor_of_safety, expected_factor, 2e-12);
    assert_eq!(diagnostic.accepted_factor_of_safety, None);
    assert_eq!(
        diagnostic.physical_assessment.status,
        BishopPhysicalStatus::InadmissibleTension
    );

    let positive_table = manufactured_zero_friction_table(1.0);
    let accepted = bishop(&positive_table, &c).unwrap();
    assert_eq!(accepted.physical_assessment.status, BishopPhysicalStatus::Admissible);
    assert!(accepted.accepted_factor_of_safety.is_some());
    assert!(accepted
        .slice_contributions
        .iter()
        .all(|row| row.base_forces.normal_kn_per_m > 0.0));
}

#[test]
fn circular_bishop_base_force_checker_rejects_vertical_and_shear_perturbations() {
    let c = config(8);
    let factor = 1.5;
    let alpha = 0.4_f64;
    let vertical_slice = SliceMechanicsInput {
        horizontal_width_m: alpha.cos(),
        base_length_m: 1.0,
        base_inclination_rad: alpha,
        weight_kn_per_m: 2.0,
        effective_cohesion_kpa: 0.5,
        effective_friction_angle_rad: 0.0,
    };
    let (normal, shear) = bishop_force_pair(&vertical_slice, factor);
    let vertical_failure = check_bishop_base_forces(
        &vertical_slice,
        factor,
        normal + 0.1,
        shear,
        &c,
    )
    .unwrap();
    assert_eq!(
        vertical_failure.physical_status,
        BishopPhysicalStatus::VerticalEquilibriumNotSatisfied
    );
    assert!(vertical_failure.vertical_residual_kn_per_m.abs()
        > vertical_failure.vertical_residual_bound_kn_per_m);
    assert!(vertical_failure.shear_law_residual_kn_per_m.abs()
        <= vertical_failure.shear_law_residual_bound_kn_per_m);

    let shear_slice = SliceMechanicsInput {
        horizontal_width_m: 1.0,
        base_length_m: 1.0,
        base_inclination_rad: 0.0,
        weight_kn_per_m: 2.0,
        effective_cohesion_kpa: 0.5,
        effective_friction_angle_rad: 0.2,
    };
    let (normal, shear) = bishop_force_pair(&shear_slice, factor);
    let shear_failure = check_bishop_base_forces(
        &shear_slice,
        factor,
        normal,
        shear + 0.1,
        &c,
    )
    .unwrap();
    assert_eq!(
        shear_failure.physical_status,
        BishopPhysicalStatus::ShearLawNotSatisfied
    );
    assert!(shear_failure.vertical_residual_kn_per_m.abs()
        <= shear_failure.vertical_residual_bound_kn_per_m);
    assert!(shear_failure.shear_law_residual_kn_per_m.abs()
        > shear_failure.shear_law_residual_bound_kn_per_m);

    let alpha = 0.8_f64.asin();
    let near_zero_normal = SliceMechanicsInput {
        horizontal_width_m: alpha.cos(),
        base_length_m: 1.0,
        base_inclination_rad: alpha,
        weight_kn_per_m: 10.0 * alpha.sin() / 2.0,
        effective_cohesion_kpa: 10.0,
        effective_friction_angle_rad: 0.0,
    };
    let factor = 2.0;
    let reference = check_bishop_base_forces(
        &near_zero_normal,
        factor,
        0.0,
        10.0 / factor,
        &c,
    )
    .unwrap();
    assert_eq!(reference.physical_status, BishopPhysicalStatus::Admissible);
    let perturbed = check_bishop_base_forces(
        &near_zero_normal,
        factor,
        1.5 * reference.normal_roundoff_bound_kn_per_m,
        10.0 / factor,
        &c,
    )
    .unwrap();
    assert_eq!(
        perturbed.physical_status,
        BishopPhysicalStatus::NumericallyIndeterminate
    );
    assert!(perturbed.vertical_residual_kn_per_m.abs()
        <= perturbed.vertical_residual_bound_kn_per_m);
    assert!(perturbed.shear_law_residual_kn_per_m.abs()
        <= perturbed.shear_law_residual_bound_kn_per_m);
}

#[test]
fn circular_bishop_base_force_checker_keeps_tiny_tension_indeterminate() {
    let c = config(8);
    let factor = 1.25;
    let alpha = 0.5_f64;
    let mut slice = SliceMechanicsInput {
        horizontal_width_m: alpha.cos(),
        base_length_m: 1.0,
        base_inclination_rad: alpha,
        weight_kn_per_m: 1.0,
        effective_cohesion_kpa: 1.0,
        effective_friction_angle_rad: 0.0,
    };
    let threshold_weight = slice.effective_cohesion_kpa
        * slice.base_length_m
        * alpha.sin()
        / factor;
    slice.weight_kn_per_m = f64::from_bits(threshold_weight.to_bits() - 4);
    let (normal, shear) = bishop_force_pair(&slice, factor);
    let tiny = check_bishop_base_forces(&slice, factor, normal, shear, &c).unwrap();
    assert!(tiny.normal_kn_per_m < 0.0);
    assert!(tiny.normal_kn_per_m.abs() <= tiny.normal_roundoff_bound_kn_per_m);
    assert_eq!(
        tiny.physical_status,
        BishopPhysicalStatus::NumericallyIndeterminate
    );

    let mut scaled = slice;
    scaled.effective_cohesion_kpa = 1e8;
    scaled.weight_kn_per_m = 0.5
        * scaled.effective_cohesion_kpa
        * scaled.base_length_m
        * alpha.sin()
        / factor;
    let (normal, shear) = bishop_force_pair(&scaled, factor);
    let tension = check_bishop_base_forces(&scaled, factor, normal, shear, &c).unwrap();
    assert!(tension.normal_kn_per_m < -tension.normal_roundoff_bound_kn_per_m);
    assert_eq!(
        tension.physical_status,
        BishopPhysicalStatus::InadmissibleTension
    );
}

#[test]
fn circular_bishop_force_tolerances_cannot_accept_tension() {
    let mut c = config(8);
    c.tolerances.force_abs_kn_per_m = 0.1;
    c.tolerances.force_rel = 0.09;
    let table = manufactured_zero_friction_table(0.1);

    let error = bishop(&table, &c).unwrap_err();
    assert_eq!(error.code, CircularErrorCode::BishopPhysicallyInadmissible);
    let root = error
        .bishop_diagnostic
        .as_ref()
        .unwrap()
        .mathematical_root
        .as_deref()
        .unwrap();
    assert_eq!(
        root.physical_assessment.status,
        BishopPhysicalStatus::InadmissibleTension
    );
    assert_eq!(root.accepted_factor_of_safety, None);
}

#[test]
fn circular_bishop_physical_classification_survives_json_roundtrips() {
    let c = config(8);
    let table = manufactured_zero_friction_table(0.1);
    let error = bishop(&table, &c).unwrap_err();
    let encoded_error = serde_json::to_vec(&error).unwrap();
    let decoded_error: CircularError = serde_json::from_slice(&encoded_error).unwrap();
    assert_eq!(
        decoded_error.code,
        CircularErrorCode::BishopPhysicallyInadmissible
    );
    let root = decoded_error
        .bishop_diagnostic
        .as_ref()
        .unwrap()
        .mathematical_root
        .as_deref()
        .unwrap();
    assert_eq!(
        root.physical_assessment.status,
        BishopPhysicalStatus::InadmissibleTension
    );
    assert_eq!(root.accepted_factor_of_safety, None);
    assert!(root.slice_contributions[0].base_forces.normal_kn_per_m < 0.0);

    let geometry_diagnostic = diagnose_specified_circle(&fixture(false), &config(128)).unwrap();
    let encoded_result = serde_json::to_vec(&geometry_diagnostic).unwrap();
    let decoded_result: CircularSlopeResult = serde_json::from_slice(&encoded_result).unwrap();
    assert!(decoded_result.diagnostic_only);
    assert!(!decoded_result.accepted);
    assert_eq!(
        decoded_result.bishop_result.physical_assessment.status,
        BishopPhysicalStatus::InadmissibleTension
    );
    assert_eq!(decoded_result.bishop_result.accepted_factor_of_safety, None);
}

#[test]
fn circular_refinement_does_not_claim_physical_contact_convergence() {
    let input = fixture(true);
    let mut c = config(64);
    c.slices.verify_refinement = true;
    let refined = diagnose_specified_circle(&input, &c).unwrap();

    assert_eq!(refined.discretization.coarse_slice_count, 68);
    assert_eq!(refined.discretization.fine_slice_count, Some(132));
    assert!(refined
        .discretization
        .bishop_factor_delta
        .unwrap()
        <= refined.discretization.bishop_accepted_tolerance);
    assert!(refined
        .discretization
        .factor_delta
        .unwrap()
        <= refined.discretization.accepted_tolerance);
    near(
        refined.bishop_result.physical_assessment.minimum_normal_kn_per_m,
        0.007076,
        2e-6,
    );
    assert_eq!(
        refined.bishop_result.physical_assessment.status,
        BishopPhysicalStatus::Admissible
    );
    let endpoint_assessment = refined.bishop_result.endpoint_assessment.as_ref().unwrap();
    assert!(!endpoint_assessment.passed);
    assert!(endpoint_assessment.endpoints.iter().any(|endpoint| {
        endpoint.endpoint_type == BishopEndpointType::Entry
            && endpoint.status == BishopEndpointStatus::TensileInterval
    }));
    assert!(refined.diagnostic_only);
    assert!(!refined.accepted);
    assert_eq!(
        refined.bishop_result.physical_assessment.scope,
        BishopPhysicalScope::CurrentDiscreteSliceTableOnly
    );
    assert!(!refined
        .bishop_result
        .physical_assessment
        .physical_contact_convergence_verified);
    assert!(!refined
        .bishop_result
        .physical_assessment
        .eligible_for_critical_minimum);

    let error = analyze_specified_circle(&input, &c).unwrap_err();
    assert_eq!(error.code, CircularErrorCode::BishopPhysicallyInadmissible);
    let error_root = error
        .bishop_diagnostic
        .as_ref()
        .unwrap()
        .mathematical_root
        .as_deref()
        .unwrap();
    assert_eq!(error_root.physical_assessment.status, BishopPhysicalStatus::Admissible);
    assert!(!error_root.endpoint_assessment.as_ref().unwrap().passed);
    assert_eq!(error_root.accepted_factor_of_safety, None);

    let mut c = config(256);
    c.slices.verify_refinement = false;
    let diagnostic = diagnose_specified_circle(&input, &c).unwrap();
    near(
        diagnostic.bishop_result.physical_assessment.minimum_normal_kn_per_m,
        -0.008578,
        2e-6,
    );
    assert!(diagnostic.diagnostic_only);
    assert!(!diagnostic.accepted);
    assert_eq!(
        diagnostic.bishop_result.physical_assessment.status,
        BishopPhysicalStatus::InadmissibleTension
    );
    assert!(!diagnostic
        .bishop_result
        .endpoint_assessment
        .as_ref()
        .unwrap()
        .passed);

    let error = analyze_specified_circle(&input, &c).unwrap_err();
    assert_eq!(error.code, CircularErrorCode::BishopPhysicallyInadmissible);
    assert_eq!(
        error
            .bishop_diagnostic
            .as_ref()
            .unwrap()
            .mathematical_root
            .as_deref()
            .unwrap()
            .physical_assessment
            .status,
        BishopPhysicalStatus::InadmissibleTension
    );
}

#[test]
fn circular_bishop_rejects_moment_identity_error_within_chord_tolerance() {
    let c = config(8);
    let alpha = std::f64::consts::FRAC_PI_6;
    let b = alpha.cos() + 5e-11;
    let table = [SliceMechanicsInput {
        horizontal_width_m: b,
        base_length_m: 1.0,
        base_inclination_rad: alpha,
        weight_kn_per_m: 10.0,
        effective_cohesion_kpa: 10.0,
        effective_friction_angle_rad: 0.0,
    }];

    let diagnostic = bishop_diagnostic(&table, &c).unwrap();
    let assessment = &diagnostic.physical_assessment;
    assert_eq!(
        assessment.status,
        BishopPhysicalStatus::MomentEquilibriumNotSatisfied
    );
    assert_eq!(diagnostic.accepted_factor_of_safety, None);
    let expected_factor =
        (10.0 * b / alpha.cos()) / (10.0 * alpha.sin());
    near(diagnostic.factor_of_safety, expected_factor, 3e-12);
    assert!(assessment.force_sum_identity_residual_kn_per_m.abs()
        > assessment.force_sum_identity_bound_kn_per_m);
    // The global moment residual is within the solver's configured root
    // tolerance; the independent force-sum identity exposes the chord error.
    near(
        assessment.moment_equilibrium_residual_kn_per_m.abs(),
        5.7735e-10,
        2e-12,
    );

    let error = bishop(&table, &c).unwrap_err();
    assert_eq!(error.code, CircularErrorCode::BishopPhysicallyInadmissible);
    assert_eq!(
        error
            .bishop_diagnostic
            .as_ref()
            .unwrap()
            .mathematical_root
            .as_deref()
            .unwrap()
            .physical_assessment
            .status,
        BishopPhysicalStatus::MomentEquilibriumNotSatisfied
    );
}

#[test]
fn circular_endpoint_guard_marks_near_vertical_limit_unresolved() {
    let input = cohesionless_fixture(false);
    let c = config(16);
    let baseline = diagnose_specified_circle(&input, &c).unwrap();
    let baseline_endpoints = baseline
        .bishop_result
        .endpoint_assessment
        .as_ref()
        .unwrap();
    let minimum_endpoint_cos = baseline_endpoints
        .endpoints
        .iter()
        .map(|endpoint| endpoint.cos_alpha.unwrap())
        .fold(f64::INFINITY, f64::min);
    let minimum_slice_cos = baseline
        .slices
        .iter()
        .map(|slice| slice.base_inclination_rad.cos())
        .fold(f64::INFINITY, f64::min);
    assert!(minimum_slice_cos > minimum_endpoint_cos);

    let mut guarded = c;
    guarded.tolerances.min_base_cos = (minimum_endpoint_cos + minimum_slice_cos) / 2.0;
    let diagnostic = diagnose_specified_circle(&input, &guarded).unwrap();
    let endpoints = diagnostic
        .bishop_result
        .endpoint_assessment
        .as_ref()
        .unwrap();
    assert!(!endpoints.passed);
    let entry = endpoints
        .endpoints
        .iter()
        .find(|endpoint| endpoint.endpoint_type == BishopEndpointType::Entry)
        .unwrap();
    assert_eq!(entry.status, BishopEndpointStatus::Unresolved);
    assert!(entry.cos_alpha.unwrap() <= guarded.tolerances.min_base_cos);

    let error = analyze_specified_circle(&input, &guarded).unwrap_err();
    assert_eq!(error.code, CircularErrorCode::BishopPhysicallyInadmissible);
    assert!(!error
        .bishop_diagnostic
        .as_ref()
        .unwrap()
        .mathematical_root
        .as_deref()
        .unwrap()
        .endpoint_assessment
        .as_ref()
        .unwrap()
        .passed);
}

#[test]
fn circular_whole_arc_screen_accepts_cohesionless_reference_with_closed_contact_domain() {
    let input = cohesionless_fixture(false);
    let screen = config(64);
    let report = screen_whole_arc_contact(&input, &screen, 3.0, 10.0).unwrap();

    assert_eq!(report.status, WholeArcContactStatus::PassedDeclaredModel);
    assert!(report.coverage_complete);
    assert!(report.minimum_d_lower.unwrap() > 0.0);
    assert!(!report.independent_whole_arc_acceptance);
    assert!(!report.eligible_for_critical_minimum);
    assert!(report
        .intervals
        .iter()
        .filter(|interval| interval.status == ContactIntervalStatus::Passed)
        .all(|interval| interval.structural_nonnegative_b));
    // The interval cover includes both closed terrain-contact ends, where the
    // failure-mass thickness (and therefore B) is exactly zero.
    assert_eq!(report.minimum_b_lower_kpa, Some(0.0));
    let diagnostic = diagnose_specified_circle(&input, &screen).unwrap();
    for contact_x in [
        diagnostic.geometry.circle.entry_point.x,
        diagnostic.geometry.circle.exit_point.x,
    ] {
        assert!(contact_x >= report.domain_x_left_m && contact_x <= report.domain_x_right_m);
    }
}

#[test]
fn circular_whole_arc_screen_reports_mixed_material_union_and_roundtrips() {
    let input = mixed_top_cohesionless_fixture();
    let original = input.clone();
    let analysis_config = config(128);
    let screen = &analysis_config;
    let report = screen_whole_arc_contact(&input, screen, 7.5, 7.6).unwrap();

    assert_eq!(report.status, WholeArcContactStatus::PassedDeclaredModel);
    assert!(report.coverage_complete);
    assert!(!report.eligible_for_critical_minimum);
    let mixed_interval = report
        .intervals
        .iter()
        .find(|interval| {
            interval.status == ContactIntervalStatus::Passed && interval.material_ids.len() >= 2
        })
        .expect("a passed interval reports the exhaustive material union it covers");
    assert!(mixed_interval.material_union_exhaustive);
    assert_eq!(input, original);

    let encoded = serde_json::to_vec(&report).unwrap();
    let decoded: WholeArcContactReport = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, report);

    let solution = analyze_specified_circle(&input, &analysis_config).unwrap();
    near(solution.bishop_result.factor_of_safety, 7.564582977103678, 2e-10);
    assert!(solution.accepted);
    let attached = solution.bishop_result.whole_arc_contact.as_ref().unwrap();
    assert_eq!(attached.status, WholeArcContactStatus::PassedDeclaredModel);
    assert!(attached.coverage_complete);
    assert!(!attached.eligible_for_critical_minimum);

    let mut translated = input.clone();
    let dx = 1.0e9;
    let dz = 1.0e9;
    for point in &mut translated.ground.terrain_polyline {
        point.x += dx;
        point.z += dz;
    }
    for interface in &mut translated.ground.layer_interfaces {
        for point in &mut interface.polyline {
            point.x += dx;
            point.z += dz;
        }
    }
    for point in &mut translated.ground.bottom_boundary {
        point.x += dx;
        point.z += dz;
    }
    translated.slip_surface.center.x += dx;
    translated.slip_surface.center.z += dz;
    let translated_before = translated.clone();
    let translated_report = screen_whole_arc_contact(&translated, screen, 7.5, 7.6).unwrap();
    assert_eq!(translated_report.status, WholeArcContactStatus::PassedDeclaredModel);
    assert!(translated_report.coverage_complete);
    assert!(translated_report.geometry_envelope_m > report.geometry_envelope_m);
    assert_eq!(translated, translated_before);
}

#[test]
fn circular_whole_arc_noninterval_budgets_and_details_remain_bounded() {
    let input = cohesionless_fixture(false);
    let mut c = config(64);
    c.contact.max_boundary_evaluations = 1;
    let report = screen_whole_arc_contact(&input, &c, 3.0, 10.0).unwrap();
    assert_eq!(report.status, WholeArcContactStatus::Unresolved);
    assert!(report.boundary_evaluations <= 1 && !report.coverage_complete);

    let mut c = config(64);
    c.contact.max_material_checks = 1;
    let report = screen_whole_arc_contact(&input, &c, 3.0, 10.0).unwrap();
    assert_eq!(report.status, WholeArcContactStatus::Unresolved);
    assert!(report.material_checks <= 1 && !report.coverage_complete);

    let mut c = config(64);
    c.contact.max_depth = 1;
    let report = screen_whole_arc_contact(&input, &c, 3.0, 10.0).unwrap();
    assert_eq!(report.status, WholeArcContactStatus::Unresolved);
    assert!(report.maximum_depth_reached <= 1 && !report.coverage_complete);

    let mut c = config(64);
    c.contact.max_report_intervals = 1;
    let report = screen_whole_arc_contact(&input, &c, 3.0, 10.0).unwrap();
    assert_eq!(report.status, WholeArcContactStatus::PassedDeclaredModel);
    assert!(report.coverage_complete && report.details_truncated);
    assert!(report.intervals.len() <= 1);

    let mut c = config(64);
    c.limits.max_result_bytes = 1024;
    let error = screen_whole_arc_contact(&input, &c, 3.0, 10.0).unwrap_err();
    assert_eq!(error.code, CircularErrorCode::ResourceLimit);
}

#[test]
fn circular_whole_arc_budget_is_unresolved_and_bound_into_fingerprint() {
    let input = cohesionless_fixture(false);
    let mut one_interval = config(64);
    one_interval.contact.max_intervals = 1;
    let report = screen_whole_arc_contact(&input, &one_interval, 3.0, 10.0).unwrap();

    assert_eq!(report.status, WholeArcContactStatus::Unresolved);
    assert!(report.intervals_evaluated <= 1);
    assert!(!report.coverage_complete);
    let mut deeper = one_interval;
    deeper.contact.max_intervals = CircularAnalysisConfig::default().contact.max_intervals;
    deeper.contact.max_depth -= 1;
    let deeper_report = screen_whole_arc_contact(&input, &deeper, 3.0, 10.0).unwrap();
    assert_ne!(
        report.contact_input_fingerprint,
        deeper_report.contact_input_fingerprint
    );

    let error = analyze_specified_circle(&input, &one_interval).unwrap_err();
    assert_eq!(error.code, CircularErrorCode::BishopPhysicallyInadmissible);
    let root = error
        .bishop_diagnostic
        .as_ref()
        .unwrap()
        .mathematical_root
        .as_deref()
        .unwrap();
    assert_eq!(root.convergence_status, BishopConvergenceStatus::Converged);
    assert_eq!(root.accepted_factor_of_safety, None);
    assert_eq!(
        root.whole_arc_contact.as_ref().unwrap().status,
        WholeArcContactStatus::Unresolved
    );
}

#[test]
fn circular_whole_arc_screen_d_pole_cannot_pass_from_structural_b_only() {
    let input = cohesionless_fixture(false);
    let c = config(64);
    let report = screen_whole_arc_contact(
        &input,
        &c,
        0.3987,
        0.3989,
    )
    .unwrap();

    assert_ne!(report.status, WholeArcContactStatus::PassedDeclaredModel);
    assert!(report.intervals.iter().any(|interval| {
        (interval.status == ContactIntervalStatus::Violation
            || interval.status == ContactIntervalStatus::Unresolved)
            && interval.structural_nonnegative_b
            && interval
                .d_lower
                .is_none_or(|lower_bound| lower_bound <= 0.0)
    }));
}

#[test]
fn circular_whole_arc_screen_cancellation_and_invalid_config_are_rejected() {
    let input = cohesionless_fixture(false);
    let c = config(64);
    let cancellation = AtomicBool::new(true);
    let cancelled = screen_whole_arc_contact_cancellable(
        &input,
        &c,
        3.0,
        10.0,
        &cancellation,
    )
    .unwrap_err();
    assert_eq!(cancelled.code, CircularErrorCode::Cancelled);

    let mut bad_float = c;
    bad_float.contact.friction_uncertainty_rad = f64::NAN;
    assert!(screen_whole_arc_contact(&input, &bad_float, 3.0, 10.0).is_err());

    let mut bad_limit = c;
    bad_limit.contact.max_intervals = 0;
    assert!(screen_whole_arc_contact(&input, &bad_limit, 3.0, 10.0).is_err());
}

#[test]
fn circular_whole_arc_finds_thin_cohesionless_top_layer_counterexample() {
    let input = thin_cohesionless_top_fixture();
    let c = config(64);
    let whole_arc = screen_whole_arc_contact(&input, &c, 6.0, 7.0).unwrap();
    assert_eq!(whole_arc.status, WholeArcContactStatus::Violation);

    let diagnostic = diagnose_specified_circle(&input, &c).unwrap();
    near(
        diagnostic.bishop_result.factor_of_safety,
        6.2174734477646805,
        2e-10,
    );
    assert_eq!(
        diagnostic.bishop_result.physical_assessment.status,
        BishopPhysicalStatus::Admissible
    );
    assert!(diagnostic
        .bishop_result
        .endpoint_assessment
        .as_ref()
        .unwrap()
        .passed);
    assert_eq!(
        diagnostic
            .bishop_result
            .whole_arc_contact
            .as_ref()
            .unwrap()
            .status,
        WholeArcContactStatus::Violation
    );
    assert_eq!(diagnostic.bishop_result.accepted_factor_of_safety, None);
    assert!(diagnostic.diagnostic_only);
    assert!(!diagnostic.accepted);

    let error = analyze_specified_circle(&input, &c).unwrap_err();
    assert_eq!(error.code, CircularErrorCode::BishopPhysicallyInadmissible);
    let root = error
        .bishop_diagnostic
        .as_ref()
        .unwrap()
        .mathematical_root
        .as_deref()
        .unwrap();
    near(root.factor_of_safety, 6.2174734477646805, 2e-10);
    assert_eq!(root.accepted_factor_of_safety, None);
    assert_eq!(
        root.whole_arc_contact.as_ref().unwrap().status,
        WholeArcContactStatus::Violation
    );
}
