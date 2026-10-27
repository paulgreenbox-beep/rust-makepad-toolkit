//! One Stage 1 prototype only; this is not a code-designed building constructor.
use crate::*;

/// 10 × 12 m, 4 m high, single-storey steel frame with side diagonals.
/// All member joints are rigid and four bases are fixed. These are explicit
/// analytical idealizations, not evidence that real bracing/joints are adequate.
pub fn braced_single_storey_10x12() -> StructuralModel {
    let node = |v| StructuralNodeId::new(v).expect("fixture ID");
    let member = |v| StructuralMemberId::new(v).expect("fixture ID");
    let material = StructuralMaterialId::new(1).expect("fixture ID");
    let section = StructuralSectionId::new(1).expect("fixture ID");
    let dead = LoadCaseId::new(1).expect("fixture ID");
    let live = LoadCaseId::new(2).expect("fixture ID");
    let points = [
        [0.0, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.0, 12.0, 0.0],
        [0.0, 12.0, 0.0],
        [0.0, 0.0, 4.0],
        [10.0, 0.0, 4.0],
        [10.0, 12.0, 4.0],
        [0.0, 12.0, 4.0],
    ];
    let connections = [
        (1, 5),
        (2, 6),
        (3, 7),
        (4, 8),
        (5, 6),
        (6, 7),
        (7, 8),
        (8, 5),
        (1, 6),
        (3, 8),
        (2, 7),
        (4, 5),
    ];
    let members: Vec<_> = connections
        .iter()
        .enumerate()
        .map(|(i, (a, b))| {
            let vertical = points[*a as usize - 1][0] == points[*b as usize - 1][0]
                && points[*a as usize - 1][1] == points[*b as usize - 1][1];
            StructuralMember {
                id: member(i as u64 + 1),
                start: node(*a),
                end: node(*b),
                material,
                section,
                formulation: MemberFormulation::Frame3d,
                local_y_reference: Some(if vertical {
                    [1.0, 0.0, 0.0]
                } else {
                    [0.0, 0.0, 1.0]
                }),
                end_releases: None,
                rigid_offset_m: None,
            }
        })
        .collect();
    let member_ids = members.iter().map(|m| m.id).collect();
    StructuralModel {
        space: StructuralSpace::Frame3d,
        nodes: points
            .into_iter()
            .enumerate()
            .map(|(i, position_m)| StructuralNode {
                id: node(i as u64 + 1),
                position_m,
            })
            .collect(),
        materials: vec![StructuralMaterial {
            id: material,
            name: "Prototype steel, not grade certification".into(),
            elastic_modulus_kn_m2: 200_000_000.0,
            shear_modulus_kn_m2: 76_923_076.923_076_93,
            weight_density_kn_m3: 78.5,
        }],
        sections: vec![StructuralSection {
            id: section,
            name: "Prototype elastic section; no strength classification".into(),
            area_m2: 0.015,
            iy_m4: 0.0005,
            iz_m4: 0.0003,
            torsional_constant_m4: 0.00005,
        }],
        members,
        supports: (1..=4)
            .map(|id| StructuralSupport {
                node: node(id),
                restrained: [true; 6],
            })
            .collect(),
        load_cases: vec![
            StructuralLoadCase {
                id: dead,
                name: "Prototype own weight".into(),
                kind: LoadCaseKind::Dead,
                normative_status: NormativeStatus::Unverified,
                provenance:
                    "Stage 1 fixture: supplied weight density and geometry; no code acceptance"
                        .into(),
                nodal_loads: vec![],
                uniform_member_loads: vec![],
                self_weight: vec![SelfWeightLoad {
                    source_id: "prototype/body/self_weight/v1".into(),
                    members: member_ids,
                }],
            },
            StructuralLoadCase {
                id: live,
                name: "Prototype roof joint loading".into(),
                kind: LoadCaseKind::Live,
                normative_status: NormativeStatus::Unverified,
                provenance: "Stage 1 demonstration: manually supplied 10 kN per roof joint".into(),
                nodal_loads: (5..=8)
                    .map(|id| NodalLoad {
                        node: node(id),
                        force_moment: [0.0, 0.0, -10.0, 0.0, 0.0, 0.0],
                        source_id: format!("prototype/roof/joint/{id}"),
                    })
                    .collect(),
                uniform_member_loads: vec![],
                self_weight: vec![],
            },
        ],
        combinations: vec![StructuralLoadCombination {
            id: LoadCombinationId::new(1).expect("fixture ID"),
            name: "Prototype G+Q, user coefficients only".into(),
            normative_status: NormativeStatus::Unverified,
            provenance: "Demonstration coefficients; not a normative ULS/SLS combination".into(),
            terms: vec![
                CombinationTerm {
                    case: dead,
                    coefficient: 1.0,
                    provenance: "Fixture author supplied 1.0".into(),
                },
                CombinationTerm {
                    case: live,
                    coefficient: 1.0,
                    provenance: "Fixture author supplied 1.0".into(),
                },
            ],
        }],
    }
}
