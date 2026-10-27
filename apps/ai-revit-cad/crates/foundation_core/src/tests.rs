use crate::*;

fn close(actual: f64, expected: f64, absolute_tolerance: f64) {
    assert!(
        (actual - expected).abs() <= absolute_tolerance,
        "actual={actual:.15e}, expected={expected:.15e}, tolerance={absolute_tolerance:.3e}"
    );
}

fn eccentric(ex: f64, ey: f64) -> RigidMatInput {
    let mut input = RigidMatInput::default();
    let n = 2941.995;
    input.load = PressureLoad::manual(n, n * ey, n * ex, "manual-eccentric-synthetic");
    input.mesh = MeshSpec { nx: 1000, ny: 8 };
    input
}

#[test]
fn centered_reference_mass_matches_force_area_and_uniform_spring_solution() {
    let input = RigidMatInput::default();
    let result = solve(&input).unwrap();
    close(input.load.n_kn, 2941.995, 1e-10);
    close(result.contact.sampled_qmin_kpa, 24.516625, 1e-10);
    close(result.contact.sampled_qmax_kpa, 24.516625, 1e-10);
    close(result.plane.w0_m, 24.516625 / 10_000.0, 1e-13);
    close(result.plane.dw_dx, 0.0, 1e-13);
    close(result.plane.dw_dy, 0.0, 1e-13);
    close(result.contact.active_sampled_area_m2, 120.0, 1e-10);
    assert_eq!(result.contact.state, ContactState::FullSampledContact);
    assert!(result.equilibrium.normalized_residual < 1e-12);
    assert_eq!(
        input.load.components[0].kind,
        LoadComponentKind::SyntheticReferenceMass
    );
    assert!(
        result
            .self_weight_ledger
            .original_structural_reactions
            .is_empty()
    );
    assert_eq!(
        result.engineering_limits.ultimate_bearing,
        AssessmentStatus::UltimateBearingNotVerified
    );
    assert_eq!(
        result.engineering_limits.settlement,
        AssessmentStatus::PreliminaryEstimate
    );
    assert!(!result.engineering_limits.engineering_site_verified);
}

#[test]
fn raft_self_weight_is_a_named_900_kn_component_added_exactly_once() {
    let mut input = RigidMatInput::default();
    input.self_weight_policy = SelfWeightPolicy::IncludeOnce {
        source_id: "raft-1-weight".into(),
    };
    let result = solve(&input).unwrap();
    close(result.self_weight_ledger.foundation_weight_kn, 900.0, 1e-10);
    close(
        result.self_weight_ledger.foundation_weight_added_kn,
        900.0,
        1e-10,
    );
    close(result.contact.sampled_qmax_kpa, 32.016625, 1e-10);
    assert_eq!(result.self_weight_ledger.applied_components.len(), 2);
    assert_eq!(
        result.self_weight_ledger.applied_components[1].source_id,
        "raft-1-weight"
    );
    assert_eq!(
        result.self_weight_ledger.applied_components[1].kind,
        LoadComponentKind::FoundationSelfWeight
    );

    input.load.n_kn = result.self_weight_ledger.total_n_kn;
    input.load.components = result.self_weight_ledger.applied_components.clone();
    assert!(matches!(
        solve(&input),
        Err(FoundationError::InvalidInput { .. })
    ));
    input.self_weight_policy = SelfWeightPolicy::IncludedInComponent {
        source_id: "raft-1-weight".into(),
    };
    let already_included = solve(&input).unwrap();
    close(
        already_included
            .self_weight_ledger
            .foundation_weight_added_kn,
        0.0,
        0.0,
    );
    close(already_included.contact.sampled_qmax_kpa, 32.016625, 1e-10);
}

#[test]
fn uniaxial_partial_contact_matches_independent_triangular_continuum_limit() {
    // Rectangle B=10, L=12, ex=2: c=3*(B/2-ex)=9, A=108,
    // qmax=2*N/(L*c)=54.4813888888889. These are continuum formulas;
    // the lumped midpoint model is required to converge, not equal them exactly.
    let fine = solve(&eccentric(2.0, 0.0)).unwrap();
    assert_eq!(fine.contact.state, ContactState::PartialSampledContact);
    close(fine.contact.active_sampled_area_m2, 108.0, 1e-8);
    close(fine.contact.sampled_qmin_kpa, 0.0, 0.0);
    let physical_edge = fine
        .continuum_full_contact
        .solved_plane_corner_qmax_kpa
        .unwrap();
    close(physical_edge, 54.4813888888889, 1e-4);
    assert!(fine.contact.sampled_qmax_kpa < physical_edge);
    assert!(fine.continuum_full_contact.candidate_has_tension);
    assert!(fine.continuum_full_contact.candidate_qmin_kpa < 0.0);
    assert!(fine.cells.iter().all(|c| c.q_kpa >= 0.0));
    assert!(fine.equilibrium.normalized_residual < 1e-11);
    assert!(fine.equilibrium.maximum_sign_violation_m < 1e-12);

    let mut coarse_input = eccentric(2.0, 0.0);
    coarse_input.mesh.nx = 40;
    let coarse = solve(&coarse_input).unwrap();
    let coarse_edge = coarse
        .continuum_full_contact
        .solved_plane_corner_qmax_kpa
        .unwrap();
    assert!(
        (physical_edge - 54.4813888888889).abs() < (coarse_edge - 54.4813888888889).abs() / 100.0
    );
}

#[test]
fn adding_centered_raft_weight_can_restore_full_contact_without_changing_moment() {
    let mut input = eccentric(2.0, 0.0);
    input.self_weight_policy = SelfWeightPolicy::IncludeOnce {
        source_id: "raft-weight".into(),
    };
    let result = solve(&input).unwrap();
    assert_eq!(result.contact.state, ContactState::FullSampledContact);
    close(result.equilibrium.applied_my_pressure_knm, 5883.99, 1e-9);
    close(
        result.continuum_full_contact.candidate_qmin_kpa,
        2.596675,
        1e-10,
    );
    close(
        result.continuum_full_contact.candidate_qmax_kpa,
        61.436575,
        1e-10,
    );
    assert!(!result.continuum_full_contact.candidate_has_tension);
    // Midpoint samples have no sample at the physical edge.
    assert!(result.contact.sampled_qmax_kpa < result.continuum_full_contact.candidate_qmax_kpa);
}

#[test]
fn reversed_eccentricity_mirrors_contact_and_changes_tilt_sign() {
    let positive = solve(&eccentric(2.0, 0.0)).unwrap();
    let negative = solve(&eccentric(-2.0, 0.0)).unwrap();
    close(positive.plane.w0_m, negative.plane.w0_m, 1e-11);
    close(positive.plane.dw_dx, -negative.plane.dw_dx, 1e-11);
    close(
        positive.contact.active_sampled_area_m2,
        negative.contact.active_sampled_area_m2,
        1e-10,
    );
    for iy in 0..positive.mesh.ny {
        for ix in 0..positive.mesh.nx {
            close(
                positive.cells[iy * positive.mesh.nx + ix].q_kpa,
                negative.cells[iy * negative.mesh.nx + (negative.mesh.nx - 1 - ix)].q_kpa,
                1e-8,
            );
        }
    }
}

#[test]
fn biaxial_full_contact_recovers_both_pressure_moments_and_physical_axis_signs() {
    let mut input = eccentric(0.5, -0.5);
    input.mesh = MeshSpec { nx: 80, ny: 96 };
    let r = solve(&input).unwrap();
    assert_eq!(r.contact.state, ContactState::FullSampledContact);
    close(
        r.equilibrium.recovered_my_pressure_knm,
        2941.995 * 0.5,
        1e-8,
    );
    close(
        r.equilibrium.recovered_mx_pressure_knm,
        -2941.995 * 0.5,
        1e-8,
    );
    close(r.equilibrium.original_external_mx_knm, 2941.995 * 0.5, 1e-8);
    close(r.equilibrium.original_external_my_knm, 2941.995 * 0.5, 1e-8);
    assert!(r.plane.dw_dx > 0.0);
    assert!(r.plane.dw_dy < 0.0);
    close(r.plane.rotation_about_x_rad, -r.plane.dw_dy, 0.0);
    close(r.plane.rotation_about_y_rad, r.plane.dw_dx, 0.0);
}

#[test]
fn scaling_nonuniform_explicit_k_preserves_pressures_and_scales_settlement() {
    let mut input = eccentric(0.3, -0.2);
    input.mesh = MeshSpec { nx: 20, ny: 24 };
    let count = input.mesh.nx * input.mesh.ny;
    let k: Vec<_> = (0..count)
        .map(|i| {
            if i % input.mesh.nx < input.mesh.nx / 2 {
                8000.0
            } else {
                12_000.0
            }
        })
        .collect();
    input.soil.support = SupportField::PerCell {
        k_kn_m3: k.clone(),
        gap_m: vec![0.0; count],
    };
    let first = solve(&input).unwrap();
    input.soil.support = SupportField::PerCell {
        k_kn_m3: k.iter().map(|v| v * 2.5).collect(),
        gap_m: vec![0.0; count],
    };
    let scaled = solve(&input).unwrap();
    close(first.plane.w0_m, scaled.plane.w0_m * 2.5, 1e-12);
    close(first.plane.dw_dx, scaled.plane.dw_dx * 2.5, 1e-12);
    close(first.plane.dw_dy, scaled.plane.dw_dy * 2.5, 1e-12);
    for (a, b) in first.cells.iter().zip(&scaled.cells) {
        close(a.q_kpa, b.q_kpa, 1e-9);
    }
    assert_eq!(
        first.continuum_full_contact.applicability,
        ContinuumApplicability::NotApplicableToNonuniformStiffnessOrGaps
    );
    assert!(
        first
            .continuum_full_contact
            .solved_plane_corner_pressures
            .is_none()
    );
    assert!(first.equilibrium.normalized_residual < 1e-12);
}

#[test]
fn changing_illustrative_layer_modulus_does_not_silently_change_bedding_k() {
    let mut input = RigidMatInput::default();
    let first = solve(&input).unwrap();
    input.soil.layers[0].young_modulus_kpa *= 100.0;
    let other = solve(&input).unwrap();
    close(first.plane.w0_m, other.plane.w0_m, 0.0);
    close(
        first.contact.sampled_qmax_kpa,
        other.contact.sampled_qmax_kpa,
        0.0,
    );
    assert_ne!(first.input_sha256, other.input_sha256);
    for s in synthetic_scenarios() {
        assert_eq!(s.source_type, SourceType::SyntheticAssumption);
        assert!(!s.engineering_site_verified);
        assert!(s.layers.iter().all(|l| !l.engineering_site_verified));
    }
}

#[test]
fn specified_three_layer_metadata_is_preserved_without_claiming_a_verified_bottom_depth() {
    let scenarios = synthetic_scenarios();
    let s = &scenarios[2];
    assert_eq!(s.id.as_str(), "3");
    let expected = [
        (0.0, Some(1.5), 18.0, 5.0, 22.0, 8000.0),
        (1.5, Some(5.0), 19.0, 18.0, 24.0, 18000.0),
        (5.0, None, 19.5, 0.0, 32.0, 30000.0),
    ];
    for (l, (top, bottom, gamma, c, phi, e)) in s.layers.iter().zip(expected) {
        close(l.depth_top_m, top, 0.0);
        assert_eq!(l.depth_bottom_m, bottom);
        close(l.unit_weight_kn_m3, gamma, 0.0);
        close(l.effective_cohesion_kpa, c, 0.0);
        close(l.effective_friction_angle_deg, phi, 0.0);
        close(l.young_modulus_kpa, e, 0.0);
    }
    close(s.layers[2].visualization_bottom_depth_m, 12.0, 0.0);
    assert!(
        s.layers[2]
            .parameter_assumption_note
            .contains("display assumption")
    );
    let mut input = RigidMatInput::default();
    input.soil = s.clone();
    close(
        solve(&input).unwrap().contact.sampled_qmax_kpa,
        24.516625,
        1e-10,
    );
}

#[test]
fn uniform_gap_changes_plane_translation_not_pressure_or_tilt() {
    let mut input = eccentric(0.3, 0.1);
    let first = solve(&input).unwrap();
    input.soil.support = SupportField::Uniform {
        k_kn_m3: 10_000.0,
        gap_m: 0.015,
    };
    let gap = solve(&input).unwrap();
    close(gap.plane.w0_m - first.plane.w0_m, 0.015, 1e-12);
    close(gap.plane.dw_dx, first.plane.dw_dx, 1e-12);
    close(
        gap.contact.sampled_qmax_kpa,
        first.contact.sampled_qmax_kpa,
        1e-8,
    );
}

#[test]
fn original_six_component_actions_are_retained_and_translated_with_r_cross_f() {
    let original = LocatedStructuralReaction {
        source_id: "column-A".into(),
        location_m: [2.0, 3.0, 4.0],
        external_force_kn: [10.0, 20.0, -100.0],
        external_free_moment_knm: [1.0, 2.0, 3.0],
    };
    let load = load_from_structural_reactions(vec![original.clone()]).unwrap();
    close(load.n_kn, 100.0, 0.0);
    close(load.mx_pressure_knm, 379.0, 0.0);
    close(load.my_pressure_knm, 242.0, 0.0);
    close(load.fx_kn, 10.0, 0.0);
    close(load.fy_kn, 20.0, 0.0);
    close(load.mz_knm, 13.0, 0.0);
    assert_eq!(load.original_structural_reactions, vec![original]);
    let mut input = RigidMatInput::default();
    input.load = load;
    let result = solve(&input).unwrap();
    assert_eq!(
        result.horizontal_actions_status.status,
        AssessmentStatus::NotImplemented
    );
    assert!(result.horizontal_actions_status.nonzero_unconsumed_actions);
    close(result.horizontal_actions_status.fx_kn, 10.0, 0.0);
    close(result.horizontal_actions_status.fy_kn, 20.0, 0.0);
    close(result.horizontal_actions_status.mz_knm, 13.0, 0.0);
    assert_eq!(
        result.self_weight_ledger.original_structural_reactions,
        input.load.original_structural_reactions
    );
}

#[test]
fn invalid_ids_nonfinite_loads_tension_and_outside_hull_are_refused() {
    for text in ["0", "01", "-1", "+1", "1.0", " 1", "1 ", ""] {
        assert!(FoundationId::new(text).is_err());
    }
    assert!(serde_json::from_str::<FoundationId>("1").is_err());
    assert!(serde_json::from_str::<FoundationId>("\"01\"").is_err());
    assert_eq!(
        serde_json::to_string(&FoundationId::new("123456789012345678901234567890").unwrap())
            .unwrap(),
        "\"123456789012345678901234567890\""
    );
    let mut input = RigidMatInput::default();
    input.load = PressureLoad::manual(-10.0, 0.0, 0.0, "uplift");
    input.self_weight_policy = SelfWeightPolicy::IncludeOnce {
        source_id: "raft".into(),
    };
    assert!(matches!(
        solve(&input),
        Err(FoundationError::NegativeVerticalLoad { .. })
    ));
    input = RigidMatInput::default();
    input.load = PressureLoad::manual(0.0, 0.0, 0.0, "zero");
    assert!(matches!(
        solve(&input),
        Err(FoundationError::ZeroCompression)
    ));
    input.load = PressureLoad::manual(f64::NAN, 0.0, 0.0, "nonfinite");
    assert!(matches!(
        solve(&input),
        Err(FoundationError::InvalidInput { .. })
    ));
    assert!(matches!(
        solve(&eccentric(5.0, 0.0)),
        Err(FoundationError::ResultantOutsideSupportHull { .. })
    ));
    assert!(matches!(
        solve(&eccentric(0.0, 6.0)),
        Err(FoundationError::ResultantOutsideSupportHull { .. })
    ));
}

#[test]
fn mesh_budget_unsupported_water_and_iteration_budget_are_explicit_errors() {
    let mut input = RigidMatInput::default();
    input.mesh = MeshSpec { nx: 1, ny: 8 };
    assert!(matches!(
        solve(&input),
        Err(FoundationError::InvalidInput { .. })
    ));
    input.mesh = MeshSpec { nx: 1024, ny: 1024 };
    assert!(matches!(
        solve(&input),
        Err(FoundationError::MeshBudgetExceeded { .. })
    ));
    input = RigidMatInput::default();
    input.groundwater = Groundwater::WaterTable {
        depth_below_base_m: 1.0,
        source_id: "synthetic-water".into(),
    };
    assert_eq!(solve(&input), Err(FoundationError::UnsupportedGroundwater));
    input = eccentric(2.0, 0.0);
    input.max_iterations = 1;
    assert!(matches!(
        solve(&input),
        Err(FoundationError::DidNotConverge { iterations: 1 })
    ));
}

#[test]
fn nearly_collinear_effective_support_is_refused_by_scaled_condition_guard() {
    let mut input = RigidMatInput::default();
    input.mesh = MeshSpec { nx: 4, ny: 4 };
    let mut k = vec![1e-25; 16];
    // Only two cells in one row dominate, so the third rigid-plane DOF is
    // effectively unrestrained even though every input k is formally positive.
    k[5] = 1e15;
    k[6] = 1e15;
    input.soil.support = SupportField::PerCell {
        k_kn_m3: k,
        gap_m: vec![0.0; 16],
    };
    assert!(matches!(
        solve(&input),
        Err(FoundationError::IllConditionedStiffness { .. })
            | Err(FoundationError::SingularStiffness { .. })
    ));
}

#[test]
fn source_component_mismatch_and_duplicate_source_ids_are_refused() {
    let mut input = RigidMatInput::default();
    input.load.n_kn += 100.0;
    assert!(matches!(
        validate_input(&input),
        Err(FoundationError::InvalidInput { .. })
    ));
    input = RigidMatInput::default();
    input.load.components.push(input.load.components[0].clone());
    assert!(matches!(
        validate_input(&input),
        Err(FoundationError::InvalidInput { .. })
    ));
    input = RigidMatInput::default();
    input.self_weight_policy = SelfWeightPolicy::IncludedInComponent {
        source_id: "missing".into(),
    };
    assert!(matches!(
        validate_input(&input),
        Err(FoundationError::InvalidInput { .. })
    ));
    input = RigidMatInput::default();
    input.soil.engineering_site_verified = true;
    assert!(matches!(
        validate_input(&input),
        Err(FoundationError::InvalidInput { .. })
    ));
}

#[test]
fn serde_roundtrip_and_result_binding_preserve_unverified_provenance_and_reject_stale_input() {
    let input = RigidMatInput::default();
    let result = solve(&input).unwrap();
    let json = serde_json::to_string(&input).unwrap();
    assert!(json.contains("SYNTHETIC_ASSUMPTION"));
    assert_eq!(serde_json::from_str::<RigidMatInput>(&json).unwrap(), input);
    assert_eq!(
        serde_json::from_str::<RigidMatResult>(&serde_json::to_string(&result).unwrap()).unwrap(),
        result
    );
    validate_result_binding(&input, &result).unwrap();
    let mut changed = input.clone();
    changed.soil.layers[0].young_modulus_kpa += 1.0;
    assert_eq!(
        validate_result_binding(&changed, &result),
        Err(FoundationError::ResultBindingMismatch)
    );
    let mut damaged = result;
    damaged.cells[0].q_kpa = -1.0;
    assert_eq!(
        validate_result_binding(&input, &damaged),
        Err(FoundationError::ResultBindingMismatch)
    );
}

fn reject_forged_continuum_archive(
    input: &RigidMatInput,
    result: &RigidMatResult,
    forge: impl FnOnce(&mut serde_json::Value),
) {
    let mut archive = serde_json::to_value(result).unwrap();
    forge(&mut archive["continuum_full_contact"]);
    let forged: RigidMatResult = serde_json::from_value(archive).unwrap();
    // The producer input identity still matches. A result-content change must
    // fail its own semantic binding rather than relying on an outer file hash.
    assert_eq!(forged.input_sha256, result.input_sha256);
    assert_eq!(
        validate_result_binding(input, &forged),
        Err(FoundationError::ResultBindingMismatch)
    );
}

#[test]
fn continuum_archive_applicability_is_bound_to_the_authoritative_support_field() {
    let mut input = RigidMatInput::default();
    input.mesh = MeshSpec { nx: 4, ny: 4 };
    for support in [
        SupportField::Uniform {
            k_kn_m3: 10_000.0,
            gap_m: 0.0,
        },
        SupportField::Uniform {
            k_kn_m3: 10_000.0,
            gap_m: 0.015,
        },
        SupportField::PerCell {
            k_kn_m3: (0..16)
                .map(|i| if i < 8 { 8000.0 } else { 12_000.0 })
                .collect(),
            gap_m: vec![0.0; 16],
        },
    ] {
        input.soil.support = support;
        let result = solve(&input).unwrap();
        let reopened: RigidMatResult =
            serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
        validate_result_binding(&input, &reopened).unwrap();
        let expected = match &input.soil.support {
            SupportField::Uniform { gap_m, .. } if *gap_m == 0.0 => {
                ContinuumApplicability::HomogeneousFullContactCandidate
            }
            _ => ContinuumApplicability::NotApplicableToNonuniformStiffnessOrGaps,
        };
        assert_eq!(result.continuum_full_contact.applicability, expected);
        let forged = match expected {
            ContinuumApplicability::HomogeneousFullContactCandidate => {
                ContinuumApplicability::NotApplicableToNonuniformStiffnessOrGaps
            }
            ContinuumApplicability::NotApplicableToNonuniformStiffnessOrGaps => {
                ContinuumApplicability::HomogeneousFullContactCandidate
            }
        };
        reject_forged_continuum_archive(&input, &result, |comparison| {
            comparison["applicability"] = serde_json::to_value(forged).unwrap()
        });
    }
}

#[test]
fn continuum_archive_rejects_forged_reference_pressures_extrema_and_tension_status() {
    let mut input = eccentric(2.0, 0.1);
    input.mesh = MeshSpec { nx: 20, ny: 24 };
    let result = solve(&input).unwrap();
    assert!(result.continuum_full_contact.candidate_has_tension);
    reject_forged_continuum_archive(&input, &result, |c| {
        c["candidate_has_tension"] = false.into()
    });
    reject_forged_continuum_archive(&input, &result, |c| {
        c["candidate_qmin_kpa"] = (c["candidate_qmin_kpa"].as_f64().unwrap() + 1.0).into()
    });
    reject_forged_continuum_archive(&input, &result, |c| {
        c["candidate_qmax_kpa"] = (c["candidate_qmax_kpa"].as_f64().unwrap() + 1.0).into()
    });
    reject_forged_continuum_archive(&input, &result, |c| {
        c["candidate_corner_pressures"][0]["q_kpa"] = (c["candidate_corner_pressures"][0]["q_kpa"]
            .as_f64()
            .unwrap()
            + 1.0)
            .into();
    });
    reject_forged_continuum_archive(&input, &result, |c| {
        c["candidate_corner_pressures"][0]["x_m"] = 0.0.into()
    });
    reject_forged_continuum_archive(&input, &result, |c| {
        c["candidate_corner_pressures"]
            .as_array_mut()
            .unwrap()
            .pop();
    });
    reject_forged_continuum_archive(&input, &result, |c| {
        c["candidate_corner_pressures"][1] = c["candidate_corner_pressures"][0].clone()
    });
    // List order carries no mathematical meaning; all four physical corners do.
    let mut reordered = result;
    reordered
        .continuum_full_contact
        .candidate_corner_pressures
        .reverse();
    reordered
        .continuum_full_contact
        .solved_plane_corner_pressures
        .as_mut()
        .unwrap()
        .rotate_left(1);
    validate_result_binding(&input, &reordered).unwrap();
}

#[test]
fn continuum_archive_requires_complete_uniform_plane_values_and_refuses_per_cell_extensions() {
    let mut input = eccentric(2.0, 0.0);
    input.mesh = MeshSpec { nx: 20, ny: 24 };
    input.soil.support = SupportField::Uniform {
        k_kn_m3: 10_000.0,
        gap_m: 0.015,
    };
    let result = solve(&input).unwrap();
    assert_eq!(
        result.continuum_full_contact.applicability,
        ContinuumApplicability::NotApplicableToNonuniformStiffnessOrGaps
    );
    assert!(
        result
            .continuum_full_contact
            .solved_plane_corner_pressures
            .is_some()
    );
    reject_forged_continuum_archive(&input, &result, |c| {
        c["solved_plane_corner_pressures"][0]["q_kpa"] =
            (c["solved_plane_corner_pressures"][0]["q_kpa"]
                .as_f64()
                .unwrap()
                + 1.0)
                .into();
    });
    reject_forged_continuum_archive(&input, &result, |c| {
        c["solved_plane_corner_pressures"][0]["y_m"] = 0.0.into()
    });
    reject_forged_continuum_archive(&input, &result, |c| {
        c["solved_plane_corner_qmin_kpa"] =
            (c["solved_plane_corner_qmin_kpa"].as_f64().unwrap() + 1.0).into()
    });
    reject_forged_continuum_archive(&input, &result, |c| {
        c["solved_plane_corner_qmax_kpa"] =
            (c["solved_plane_corner_qmax_kpa"].as_f64().unwrap() + 1.0).into()
    });
    for field in [
        "solved_plane_corner_pressures",
        "solved_plane_corner_qmin_kpa",
        "solved_plane_corner_qmax_kpa",
    ] {
        reject_forged_continuum_archive(&input, &result, |c| c[field] = serde_json::Value::Null);
    }
    // Even identical per-cell values retain the conservative PerCell contract:
    // no physical corner extension is inferred from a sampled support field.
    input.soil.support = SupportField::PerCell {
        k_kn_m3: vec![10_000.0; 480],
        gap_m: vec![0.015; 480],
    };
    let per_cell = solve(&input).unwrap();
    assert!(
        per_cell
            .continuum_full_contact
            .solved_plane_corner_pressures
            .is_none()
    );
    let uniform_fields = serde_json::to_value(&result.continuum_full_contact).unwrap();
    for field in [
        "solved_plane_corner_pressures",
        "solved_plane_corner_qmin_kpa",
        "solved_plane_corner_qmax_kpa",
    ] {
        reject_forged_continuum_archive(&input, &per_cell, |c| {
            c[field] = uniform_fields[field].clone()
        });
    }
}
