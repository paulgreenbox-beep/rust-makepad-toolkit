use crate::*;

fn n(v: u64) -> StructuralNodeId {
    StructuralNodeId::new(v).unwrap()
}
fn m(v: u64) -> StructuralMemberId {
    StructuralMemberId::new(v).unwrap()
}
fn mat() -> StructuralMaterialId {
    StructuralMaterialId::new(1).unwrap()
}
fn sec() -> StructuralSectionId {
    StructuralSectionId::new(1).unwrap()
}
fn case() -> LoadCaseId {
    LoadCaseId::new(1).unwrap()
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() <= 1e-9 * b.abs() + 1e-12, "{a:e} != {b:e}");
}

fn basic(space: StructuralSpace, formulation: MemberFormulation, length: f64) -> StructuralModel {
    StructuralModel {
        space,
        nodes: vec![
            StructuralNode {
                id: n(1),
                position_m: [0.0, 0.0, 0.0],
            },
            StructuralNode {
                id: n(2),
                position_m: [length, 0.0, 0.0],
            },
        ],
        materials: vec![StructuralMaterial {
            id: mat(),
            name: "Analytical E=200 GPa".into(),
            elastic_modulus_kn_m2: 200_000_000.0,
            shear_modulus_kn_m2: 80_000_000.0,
            weight_density_kn_m3: 78.5,
        }],
        sections: vec![StructuralSection {
            id: sec(),
            name: "Analytical section".into(),
            area_m2: 0.01,
            iy_m4: 0.00008,
            iz_m4: 0.00004,
            torsional_constant_m4: 0.00001,
        }],
        members: vec![StructuralMember {
            id: m(1),
            start: n(1),
            end: n(2),
            material: mat(),
            section: sec(),
            formulation,
            local_y_reference: if formulation == MemberFormulation::Frame3d {
                Some([0.0, 1.0, 0.0])
            } else {
                None
            },
            end_releases: None,
            rigid_offset_m: None,
        }],
        supports: vec![StructuralSupport {
            node: n(1),
            restrained: [true; 6],
        }],
        load_cases: vec![StructuralLoadCase {
            id: case(),
            name: "Analytical".into(),
            kind: LoadCaseKind::Live,
            normative_status: NormativeStatus::Unverified,
            provenance: "Analytical production-author test, not external validation".into(),
            nodal_loads: vec![],
            uniform_member_loads: vec![],
            self_weight: vec![],
        }],
        combinations: vec![],
    }
}
fn point(model: &mut StructuralModel, node: StructuralNodeId, force_moment: Six) {
    model.load_cases[0].nodal_loads.push(NodalLoad {
        node,
        force_moment,
        source_id: "test/point/1".into(),
    });
}
fn uniform(model: &mut StructuralModel, member: StructuralMemberId, q: [f64; 3], source: &str) {
    model.load_cases[0]
        .uniform_member_loads
        .push(MemberUniformLoad {
            member,
            local_force_kn_m: q,
            source_id: source.into(),
        });
}
fn balanced(result: &StructuralAnalysisResult) {
    assert!(result.diagnostics.relative_free_dof_residual < 1e-10);
    assert!(result.diagnostics.relative_force_balance_residual < 1e-10);
    assert!(result.diagnostics.relative_moment_balance_residual < 1e-10);
}

#[test]
fn ids_are_positive_canonical_decimal_strings() {
    assert_eq!(serde_json::to_string(&n(9)).unwrap(), "\"9\"");
    for bad in [
        "0",
        "01",
        "-1",
        "+1",
        "1.0",
        " 1",
        "18446744073709551616",
        "",
    ] {
        assert!(bad.parse::<StructuralNodeId>().is_err(), "{bad}");
    }
    assert!(serde_json::from_str::<StructuralNodeId>("1").is_err());
    let id = "18446744073709551615"
        .parse::<StructuralMemberId>()
        .unwrap();
    assert_eq!(id.get(), u64::MAX);
}

#[test]
fn analytical_axial_bar_uses_kn_m_units() {
    let mut model = basic(StructuralSpace::Truss2dXz, MemberFormulation::Truss2d, 6.0);
    model.supports.push(StructuralSupport {
        node: n(2),
        restrained: [false, false, true, false, false, false],
    });
    point(&mut model, n(2), [10.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let r = analyze(&model, case()).unwrap();
    close(
        r.nodes[1].displacement[0],
        10.0 * 6.0 / (200_000_000.0 * 0.01),
    );
    close(r.reactions[0].force_moment[0], -10.0);
    close(r.members[0].local_end_forces[0][0], -10.0);
    close(r.members[0].local_end_forces[1][0], 10.0);
    assert_eq!(r.nodes[0].displacement, [0.0; 6]);
    balanced(&r);
}

#[test]
fn analytical_xz_cantilever_tip_force_and_moment_signs() {
    let mut model = basic(StructuralSpace::Frame2dXz, MemberFormulation::Frame2d, 4.0);
    point(&mut model, n(2), [0.0, 0.0, -10.0, 0.0, 0.0, 0.0]);
    let r = analyze(&model, case()).unwrap();
    let ei = 200_000_000.0 * 0.00008;
    close(
        r.nodes[1].displacement[2],
        -10.0 * 4.0_f64.powi(3) / (3.0 * ei),
    );
    close(
        r.nodes[1].displacement[4],
        10.0 * 4.0_f64.powi(2) / (2.0 * ei),
    );
    close(r.reactions[0].force_moment[2], 10.0);
    close(r.reactions[0].force_moment[4], -40.0);
    close(r.members[0].local_end_forces[0][4], -40.0);
    balanced(&r);
}

#[test]
fn analytical_simply_supported_udl_midpoint_and_end_reactions() {
    let length = 6.0;
    let mut model = basic(
        StructuralSpace::Frame2dXz,
        MemberFormulation::Frame2d,
        length,
    );
    model.nodes.push(StructuralNode {
        id: n(3),
        position_m: [length / 2.0, 0.0, 0.0],
    });
    model.members[0].end = n(3);
    let mut right = model.members[0].clone();
    right.id = m(2);
    right.start = n(3);
    right.end = n(2);
    model.members.push(right);
    model.supports = vec![
        StructuralSupport {
            node: n(1),
            restrained: [true, false, true, false, false, false],
        },
        StructuralSupport {
            node: n(2),
            restrained: [false, false, true, false, false, false],
        },
    ];
    uniform(&mut model, m(1), [0.0, 0.0, -2.0], "test/udl/left");
    uniform(&mut model, m(2), [0.0, 0.0, -2.0], "test/udl/right");
    let r = analyze(&model, case()).unwrap();
    let ei = 200_000_000.0 * 0.00008;
    close(
        r.nodes[2].displacement[2],
        -5.0 * 2.0 * length.powi(4) / (384.0 * ei),
    );
    close(
        r.nodes[0].displacement[4],
        2.0 * length.powi(3) / (24.0 * ei),
    );
    close(
        r.nodes[1].displacement[4],
        -2.0 * length.powi(3) / (24.0 * ei),
    );
    close(r.reactions[0].force_moment[2], 6.0);
    close(r.reactions[1].force_moment[2], 6.0);
    close(r.members[0].local_end_forces[0][4], 0.0);
    close(r.members[1].local_end_forces[1][4], 0.0);
    balanced(&r);
}

#[test]
fn analytical_triangle_truss_axial_forces_and_virtual_work_deflection() {
    let mut model = basic(StructuralSpace::Truss2dXz, MemberFormulation::Truss2d, 4.0);
    model.nodes.push(StructuralNode {
        id: n(3),
        position_m: [2.0, 0.0, 3.0],
    });
    for (id, start, end) in [(2, 1, 3), (3, 2, 3)] {
        let mut diagonal = model.members[0].clone();
        diagonal.id = m(id);
        diagonal.start = n(start);
        diagonal.end = n(end);
        model.members.push(diagonal);
    }
    model.supports.push(StructuralSupport {
        node: n(2),
        restrained: [false, false, true, false, false, false],
    });
    point(&mut model, n(3), [0.0, 0.0, -10.0, 0.0, 0.0, 0.0]);
    let r = analyze(&model, case()).unwrap();
    close(r.reactions[0].force_moment[2], 5.0);
    close(r.reactions[1].force_moment[2], 5.0);
    close(r.members[0].local_end_forces[1][0], 10.0 / 3.0);
    close(
        r.members[1].local_end_forces[1][0],
        -5.0 * 13.0_f64.sqrt() / 3.0,
    );
    let compliance = (13.0 * 13.0_f64.sqrt() / 18.0 + 4.0 / 9.0) / (200_000_000.0 * 0.01);
    close(r.nodes[2].displacement[2], -10.0 * compliance);
    balanced(&r);
}

#[test]
fn analytical_3d_rotated_cantilever_axial_biaxial_torsion() {
    let length = 5.0;
    let mut model = basic(StructuralSpace::Frame3d, MemberFormulation::Frame3d, length);
    let x = [1.0 / 3.0_f64.sqrt(); 3];
    let y = [
        -1.0 / 6.0_f64.sqrt(),
        -1.0 / 6.0_f64.sqrt(),
        2.0 / 6.0_f64.sqrt(),
    ];
    let z = [1.0 / 2.0_f64.sqrt(), -1.0 / 2.0_f64.sqrt(), 0.0];
    let axes = [x, y, z];
    model.nodes[0].position_m = [7.0, -2.0, 3.0];
    model.nodes[1].position_m =
        std::array::from_fn(|c| model.nodes[0].position_m[c] + length * x[c]);
    model.members[0].local_y_reference = Some([0.0, 0.0, 1.0]);
    let local = [3.0, 4.0, -6.0, 2.0, 1.5, -2.5];
    let global: Six = std::array::from_fn(|c| {
        (0..3)
            .map(|a| axes[a][c % 3] * local[a + 3 * (c / 3)])
            .sum()
    });
    point(&mut model, n(2), global);
    let r = analyze(&model, case()).unwrap();
    let e = 200_000_000.0;
    let g = 80_000_000.0;
    let iy = 0.00008;
    let iz = 0.00004;
    let expected = [
        local[0] * length / (e * 0.01),
        local[1] * length.powi(3) / (3.0 * e * iz) + local[5] * length.powi(2) / (2.0 * e * iz),
        local[2] * length.powi(3) / (3.0 * e * iy) - local[4] * length.powi(2) / (2.0 * e * iy),
        local[3] * length / (g * 0.00001),
        -local[2] * length.powi(2) / (2.0 * e * iy) + local[4] * length / (e * iy),
        local[1] * length.powi(2) / (2.0 * e * iz) + local[5] * length / (e * iz),
    ];
    for c in 0..6 {
        close(r.members[0].local_end_displacements[1][c], expected[c]);
        close(r.members[0].local_end_forces[1][c], local[c]);
    }
    let root = [
        -local[0],
        -local[1],
        -local[2],
        -local[3],
        -local[4] + length * local[2],
        -local[5] - length * local[1],
    ];
    for c in 0..6 {
        close(r.members[0].local_end_forces[0][c], root[c]);
    }
    balanced(&r);
}

#[test]
fn analytical_uniform_3d_cantilever_consistent_loads() {
    let l = 4.0;
    let mut model = basic(StructuralSpace::Frame3d, MemberFormulation::Frame3d, l);
    uniform(&mut model, m(1), [2.0, 3.0, -5.0], "test/3d/udl");
    let r = analyze(&model, case()).unwrap();
    let e = 200_000_000.0;
    close(r.nodes[1].displacement[0], 2.0 * l * l / (2.0 * e * 0.01));
    close(
        r.nodes[1].displacement[1],
        3.0 * l.powi(4) / (8.0 * e * 0.00004),
    );
    close(
        r.nodes[1].displacement[2],
        -5.0 * l.powi(4) / (8.0 * e * 0.00008),
    );
    close(
        r.nodes[1].displacement[4],
        5.0 * l.powi(3) / (6.0 * e * 0.00008),
    );
    close(
        r.nodes[1].displacement[5],
        3.0 * l.powi(3) / (6.0 * e * 0.00004),
    );
    for c in 0..6 {
        close(r.members[0].local_end_forces[1][c], 0.0);
    }
    balanced(&r);
}

#[test]
fn mechanism_is_an_error_even_for_zero_loading() {
    let mut model = basic(StructuralSpace::Truss2dXz, MemberFormulation::Truss2d, 4.0);
    // Node 2 transverse motion has zero stiffness; never silently return zeros.
    assert!(
        matches!(analyze(&model,case()),Err(StructuralError::SingularMechanism {node,component:2,..}) if node==n(2))
    );
    model.supports.clear();
    assert!(matches!(
        analyze(&model, case()),
        Err(StructuralError::SingularMechanism { .. })
    ));
}

#[test]
fn invalid_degenerate_and_unsupported_models_fail_closed() {
    let mut model = basic(StructuralSpace::Frame2dXz, MemberFormulation::Frame2d, 4.0);
    model.materials[0].elastic_modulus_kn_m2 = 0.0;
    assert!(matches!(
        validate(&model),
        Err(StructuralError::InvalidModel { .. })
    ));
    model.materials[0].elastic_modulus_kn_m2 = 200_000_000.0;
    model.nodes[1].position_m = model.nodes[0].position_m;
    assert!(matches!(
        validate(&model),
        Err(StructuralError::DegenerateMember { .. })
    ));
    model.nodes[1].position_m = [4.0, 0.0, 0.0];
    model.members[0].end_releases = Some([[false; 6]; 2]);
    assert!(matches!(
        validate(&model),
        Err(StructuralError::Unsupported { .. })
    ));
    model.members[0].end_releases = None;
    model.load_cases[0].kind = LoadCaseKind::Temperature;
    assert!(matches!(
        validate(&model),
        Err(StructuralError::Unsupported { .. })
    ));
}

#[test]
fn combinations_are_coherent_and_supplied_coefficients_only() {
    let mut model = basic(StructuralSpace::Frame2dXz, MemberFormulation::Frame2d, 4.0);
    point(&mut model, n(2), [0.0, 0.0, -10.0, 0.0, 0.0, 0.0]);
    let id = LoadCombinationId::new(1).unwrap();
    model.combinations.push(StructuralLoadCombination {
        id,
        name: "User term".into(),
        normative_status: NormativeStatus::Unverified,
        provenance: "User analytical coefficient".into(),
        terms: vec![CombinationTerm {
            case: case(),
            coefficient: 1.5,
            provenance: "User supplied 1.5".into(),
        }],
    });
    let single = analyze(&model, case()).unwrap();
    let combination = analyze(&model, id).unwrap();
    assert_eq!(combination.target, AnalysisTarget::Combination(id));
    close(
        combination.reactions[0].force_moment[2],
        single.reactions[0].force_moment[2] * 1.5,
    );
    assert_eq!(combination.normative_status, NormativeStatus::Unverified);
    assert_eq!(combination.load_sources[0].coefficient, 1.5);
}

#[test]
fn self_weight_is_global_minus_z_and_charged_once() {
    let mut model = basic(StructuralSpace::Frame3d, MemberFormulation::Frame3d, 4.0);
    model.load_cases[0].kind = LoadCaseKind::Dead;
    model.load_cases[0].self_weight.push(SelfWeightLoad {
        source_id: "body/steel/weight/v1".into(),
        members: vec![m(1)],
    });
    let r = analyze(&model, case()).unwrap();
    close(r.reactions[0].force_moment[2], 78.5 * 0.01 * 4.0);
    close(r.reactions[0].force_moment[4], -78.5 * 0.01 * 4.0 * 2.0);
    model.load_cases[0].self_weight[0].members.push(m(1));
    assert!(matches!(
        validate(&model),
        Err(StructuralError::DuplicateLoadSource { .. })
    ));
}

#[test]
fn scaling_preserves_a_very_small_unit_load_response() {
    let mut model = basic(StructuralSpace::Frame2dXz, MemberFormulation::Frame2d, 0.01);
    point(&mut model, n(2), [0.0, 0.0, -1e-8, 0.0, 0.0, 0.0]);
    let r = analyze(&model, case()).unwrap();
    let expected = -1e-8 * 0.01_f64.powi(3) / (3.0 * 200_000_000.0 * 0.00008);
    assert!((r.nodes[1].displacement[2] / expected - 1.0).abs() < 1e-10);
    balanced(&r);
}

#[test]
fn prototype_fixture_has_a_real_bounded_balanced_linear_solution() {
    let model = fixtures::braced_single_storey_10x12();
    let result = analyze(&model, LoadCombinationId::new(1).unwrap()).unwrap();
    assert_eq!(result.reactions.len(), 4);
    assert_eq!(result.members.len(), 12);
    assert_eq!(result.normative_status, NormativeStatus::Unverified);
    assert!(result.nodes.iter().skip(4).any(|n| n.displacement[2] < 0.0));
    balanced(&result);
}

#[test]
fn pure_validation_and_serialization_do_not_need_a_solve() {
    let model = fixtures::braced_single_storey_10x12();
    validate(&model).unwrap();
    let json = serde_json::to_string(&model).unwrap();
    let decoded: StructuralModel = serde_json::from_str(&json).unwrap();
    assert_eq!(model, decoded);
}

#[test]
fn archived_result_binding_accepts_exact_inputs_without_resolving() {
    let model = fixtures::braced_single_storey_10x12();
    let target = LoadCombinationId::new(1).unwrap();
    let result = analyze(&model, target).unwrap();
    validate_result_binding(&model, target, &result).unwrap();
    let decoded: StructuralAnalysisResult =
        serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
    assert_eq!(result, decoded);
    validate_result_binding(&model, target, &decoded).unwrap();
    assert_eq!(result.input_fingerprint.len(), 64);
}

#[test]
fn stale_result_cannot_be_rebound_to_changed_load_section_or_material() {
    let mut model = basic(StructuralSpace::Frame2dXz, MemberFormulation::Frame2d, 4.0);
    point(&mut model, n(2), [0.0, 0.0, -10.0, 0.0, 0.0, 0.0]);
    let result = analyze(&model, case()).unwrap();
    for variant in 0..3 {
        let mut changed = model.clone();
        match variant {
            0 => changed.load_cases[0].nodal_loads[0].force_moment[2] = -20.0,
            1 => changed.sections[0].iy_m4 *= 2.0,
            _ => changed.materials[0].elastic_modulus_kn_m2 *= 2.0,
        }
        assert_ne!(
            result.input_fingerprint,
            input_fingerprint(&changed, case()).unwrap()
        );
        assert!(validate_result_binding(&changed, case(), &result).is_err());
        let mut forged = result.clone();
        forged.input_fingerprint = input_fingerprint(&changed, case()).unwrap();
        assert!(
            validate_result_binding(&changed, case(), &forged).is_err(),
            "Input hash alone must not replace constitutive/equilibrium validation"
        );
    }
}

#[test]
fn archived_result_rejects_tampered_reaction_member_and_displacement() {
    let mut model = basic(StructuralSpace::Frame2dXz, MemberFormulation::Frame2d, 4.0);
    point(&mut model, n(2), [0.0, 0.0, -10.0, 0.0, 0.0, 0.0]);
    let result = analyze(&model, case()).unwrap();
    for variant in 0..3 {
        let mut changed = result.clone();
        match variant {
            0 => changed.reactions[0].force_moment[2] += 1.0,
            1 => changed.members[0].local_end_forces[0][2] += 1.0,
            _ => changed.nodes[1].displacement[2] += 0.001,
        }
        assert!(validate_result_binding(&model, case(), &changed).is_err());
    }
}

#[test]
fn input_fingerprint_and_binding_are_independent_of_entity_order() {
    let model = fixtures::braced_single_storey_10x12();
    let target = LoadCombinationId::new(1).unwrap();
    let result = analyze(&model, target).unwrap();
    let mut reordered = model.clone();
    reordered.nodes.reverse();
    reordered.members.reverse();
    reordered.supports.reverse();
    reordered.load_cases.reverse();
    reordered.combinations[0].terms.reverse();
    for case in &mut reordered.load_cases {
        case.nodal_loads.reverse();
        case.uniform_member_loads.reverse();
        case.self_weight.reverse();
        for source in &mut case.self_weight {
            source.members.reverse();
        }
    }
    assert_eq!(
        input_fingerprint(&model, target).unwrap(),
        input_fingerprint(&reordered, target).unwrap()
    );
    validate_result_binding(&reordered, target, &result).unwrap();
}
