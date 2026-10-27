//! Small, named fixtures for geometry and bounded-projection contract checks.
//!
//! All material and section values in these fixtures are synthetic inputs. They
//! do not identify a catalogue section or establish design adequacy.

use crate::{
    AnalysisLoadScope, BuildingConfig, ConnectionIdealization, Corner, GridSpacing,
    InterfaceAnchor, InterfaceKind, InterfaceSpec, LocalTransform, MemberProfile, MemberRule,
    MemberRules, Provenance, RoofKind, RoofSpec, SectionConfig, SectionGrid, SemanticId,
    SteelMaterial, StructuralSystem,
};
use serde::{Deserialize, Serialize};

/// Deterministic fixture families used by the crate's integration tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FixtureKind {
    /// Adjacent gable and mono-pitch sections with an explicit rigid interface.
    SteppedConnected,
    /// The same connected plan with equal eaves at the shared interface.
    EqualHeightConnected,
    /// Adjacent sections separated by an explicit 0.1 m joint.
    IndependentJoint,
    /// Coincident geometry with no declared structural load path.
    GeometryOnly,
}

/// Build one of the frozen, wholly synthetic Stage 1 fixtures.
pub fn fixture(kind: FixtureKind) -> BuildingConfig {
    let (b_eave, b_high) = match kind {
        FixtureKind::EqualHeightConnected => (7.0, 7.6),
        _ => (4.2, 4.8),
    };

    let interface_kind = match kind {
        FixtureKind::SteppedConnected | FixtureKind::EqualHeightConnected => {
            InterfaceKind::StructurallyConnected {
                idealization: ConnectionIdealization::Rigid,
            }
        }
        FixtureKind::IndependentJoint => InterfaceKind::IndependentWithJoint {
            width_m: 0.1,
            offset_b_direction: [1.0, 0.0],
        },
        FixtureKind::GeometryOnly => InterfaceKind::GeometryOnlyContact,
    };

    let anchors = match kind {
        FixtureKind::SteppedConnected | FixtureKind::EqualHeightConnected => vec![
            InterfaceAnchor {
                id: sid("s0"),
                fraction: 0.0,
            },
            InterfaceAnchor {
                id: sid("s1"),
                fraction: 0.5,
            },
            InterfaceAnchor {
                id: sid("s2"),
                fraction: 1.0,
            },
        ],
        FixtureKind::IndependentJoint | FixtureKind::GeometryOnly => Vec::new(),
    };

    let material_id = sid("material-fixture-steel");
    let profile_id = sid("profile-fixture-a");
    let rules = member_rules(material_id.clone(), profile_id.clone());

    BuildingConfig {
        id: sid("building-configurator-fixture"),
        transform: LocalTransform::default(),
        sections: vec![section_a(rules.clone()), section_b(rules, b_eave, b_high)],
        interfaces: vec![InterfaceSpec {
            id: sid("interface-a-b"),
            section_a: sid("A"),
            edge_a: [sid("a1"), sid("a2")],
            section_b: sid("B"),
            edge_b: [sid("b3"), sid("b0")],
            kind: interface_kind,
            anchors,
        }],
        materials: vec![SteelMaterial {
            id: material_id,
            name: "Synthetic fixture steel".into(),
            elastic_modulus_kn_m2: 200.0e6,
            shear_modulus_kn_m2: 76.9230769e6,
            weight_density_kn_m3: 78.5,
            provenance: Provenance::Synthetic {
                note: "Explicit fixture values only; no grade or design-standard claim".into(),
            },
        }],
        profiles: vec![MemberProfile {
            id: profile_id,
            name: "Manual synthetic fixture profile A".into(),
            area_m2: 0.015,
            iy_m4: 0.0005,
            iz_m4: 0.0003,
            torsional_constant_m4: 0.00005,
            provenance: Provenance::Synthetic {
                note: "Explicit synthetic fixture properties; not a catalogue section".into(),
            },
            visualization: None,
        }],
        foundation_assignments: Vec::new(),
        allow_disconnected_analysis: kind == FixtureKind::IndependentJoint,
        analysis_load_scope: AnalysisLoadScope::FrameSelfWeightOnly,
    }
}

fn sid(value: &str) -> SemanticId {
    SemanticId::new(value).expect("fixture semantic ID is a valid constant")
}

fn member_rules(material_id: SemanticId, profile_id: SemanticId) -> MemberRules {
    let make = |local_y_reference| MemberRule {
        material_id: material_id.clone(),
        profile_id: profile_id.clone(),
        local_y_reference,
        end_releases: None,
    };
    MemberRules {
        column: make([0.0, 1.0, 0.0]),
        rafter: make([0.0, 1.0, 0.0]),
        beam: make([0.0, 0.0, 1.0]),
        brace: make([0.0, 1.0, 0.0]),
        purlin: make([0.0, 0.0, 1.0]),
        wall_girt: make([0.0, 0.0, 1.0]),
    }
}

fn section_a(members: MemberRules) -> SectionConfig {
    SectionConfig {
        id: sid("A"),
        structural_system: StructuralSystem::SteelFrame,
        corners: vec![
            Corner {
                id: sid("a0"),
                xy_m: [0.0, 0.0],
            },
            Corner {
                id: sid("a1"),
                xy_m: [6.0, 0.0],
            },
            Corner {
                id: sid("a2"),
                xy_m: [6.0, 12.0],
            },
            Corner {
                id: sid("a3"),
                xy_m: [0.0, 12.0],
            },
        ],
        roof: RoofSpec {
            kind: RoofKind::Gable,
            base_abs_m: 0.0,
            eave_abs_m: 7.0,
            ridge_abs_m: 8.5,
            unsupported_details: None,
        },
        grid: SectionGrid {
            span_direction: [1.0, 0.0],
            frame_stations: GridSpacing::Uniform { pitch_m: 6.0 },
            column_stations: GridSpacing::Uniform { pitch_m: 6.0 },
            purlin_pitch_m: 3.0,
            wall_girt_pitch_m: 4.0,
            braces: true,
        },
        members,
        base_restraints: [true; 6],
        roof_panels: None,
        wall_panels: None,
    }
}

fn section_b(members: MemberRules, eave_abs_m: f64, ridge_abs_m: f64) -> SectionConfig {
    SectionConfig {
        id: sid("B"),
        structural_system: StructuralSystem::SteelFrame,
        corners: vec![
            Corner {
                id: sid("b0"),
                xy_m: [6.0, 0.0],
            },
            Corner {
                id: sid("b1"),
                xy_m: [10.0, 0.0],
            },
            Corner {
                id: sid("b2"),
                xy_m: [10.0, 12.0],
            },
            Corner {
                id: sid("b3"),
                xy_m: [6.0, 12.0],
            },
        ],
        roof: RoofSpec {
            kind: RoofKind::MonoPitch,
            base_abs_m: 0.0,
            eave_abs_m,
            ridge_abs_m,
            unsupported_details: None,
        },
        grid: SectionGrid {
            span_direction: [1.0, 0.0],
            frame_stations: GridSpacing::Uniform { pitch_m: 6.0 },
            column_stations: GridSpacing::Uniform { pitch_m: 4.0 },
            purlin_pitch_m: 4.0,
            wall_girt_pitch_m: 4.0,
            braces: true,
        },
        members,
        base_restraints: [true; 6],
        roof_panels: None,
        wall_panels: None,
    }
}
