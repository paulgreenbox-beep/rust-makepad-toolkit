use crate::*;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Component ordering everywhere is [ux, uy, uz, rx, ry, rz] or [Fx,Fy,Fz,Mx,My,Mz].
pub type Six = [f64; 6];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuralSpace {
    Truss2dXz,
    Frame2dXz,
    Frame3d,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralNode {
    pub id: StructuralNodeId,
    pub position_m: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralMaterial {
    pub id: StructuralMaterialId,
    pub name: String,
    pub elastic_modulus_kn_m2: f64,
    pub shear_modulus_kn_m2: f64,
    /// Weight density, not mass density; own weight requires no implicit gravity factor.
    pub weight_density_kn_m3: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralSection {
    pub id: StructuralSectionId,
    pub name: String,
    pub area_m2: f64,
    pub iy_m4: f64,
    pub iz_m4: f64,
    pub torsional_constant_m4: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberFormulation {
    Truss2d,
    Frame2d,
    Frame3d,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralMember {
    pub id: StructuralMemberId,
    pub start: StructuralNodeId,
    pub end: StructuralNodeId,
    pub material: StructuralMaterialId,
    pub section: StructuralSectionId,
    pub formulation: MemberFormulation,
    /// Frame3d: reference for local +y, projected normal to x; must not be parallel.
    /// Plane members use global +Y and require this to be None.
    pub local_y_reference: Option<[f64; 3]>,
    /// Releases and offsets deliberately fail closed in this stage.
    #[serde(default)]
    pub end_releases: Option<[[bool; 6]; 2]>,
    #[serde(default)]
    pub rigid_offset_m: Option<[[f64; 3]; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralSupport {
    pub node: StructuralNodeId,
    /// Zero prescribed displacement only. No spring or settlement interpretation.
    pub restrained: [bool; 6],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadCaseKind {
    Dead,
    SuperimposedDead,
    Live,
    RoofLive,
    Snow,
    WindXPlus,
    WindXMinus,
    WindYPlus,
    WindYMinus,
    /// Reserved in the interchange schema; explicitly rejected by the solver.
    Temperature,
    Special,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NormativeStatus {
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodalLoad {
    pub node: StructuralNodeId,
    /// kN for force components; kN m for moments. Global axes.
    pub force_moment: Six,
    pub source_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberUniformLoad {
    pub member: StructuralMemberId,
    /// Constant local [qx,qy,qz], kN/m. Distributed moments are unsupported.
    pub local_force_kn_m: [f64; 3],
    pub source_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfWeightLoad {
    /// Stable ledger identity, e.g. "body:self_weight:steel-frame:v1".
    pub source_id: String,
    /// Each selected member is charged once; duplicates are rejected.
    pub members: Vec<StructuralMemberId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralLoadCase {
    pub id: LoadCaseId,
    pub name: String,
    pub kind: LoadCaseKind,
    pub normative_status: NormativeStatus,
    pub provenance: String,
    pub nodal_loads: Vec<NodalLoad>,
    pub uniform_member_loads: Vec<MemberUniformLoad>,
    pub self_weight: Vec<SelfWeightLoad>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CombinationTerm {
    pub case: LoadCaseId,
    pub coefficient: f64,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralLoadCombination {
    pub id: LoadCombinationId,
    pub name: String,
    pub normative_status: NormativeStatus,
    pub provenance: String,
    pub terms: Vec<CombinationTerm>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralModel {
    pub space: StructuralSpace,
    pub nodes: Vec<StructuralNode>,
    pub materials: Vec<StructuralMaterial>,
    pub sections: Vec<StructuralSection>,
    pub members: Vec<StructuralMember>,
    pub supports: Vec<StructuralSupport>,
    pub load_cases: Vec<StructuralLoadCase>,
    pub combinations: Vec<StructuralLoadCombination>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum AnalysisTarget {
    Case(LoadCaseId),
    Combination(LoadCombinationId),
}
impl From<LoadCaseId> for AnalysisTarget {
    fn from(id: LoadCaseId) -> Self {
        Self::Case(id)
    }
}
impl From<LoadCombinationId> for AnalysisTarget {
    fn from(id: LoadCombinationId) -> Self {
        Self::Combination(id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuralNodeResult {
    pub node: StructuralNodeId,
    pub position_m: [f64; 3],
    /// Metres followed by radians; excluded plane DOFs are exactly zero by definition.
    pub displacement: Six,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuralReaction {
    pub node: StructuralNodeId,
    pub position_m: [f64; 3],
    pub restrained: [bool; 6],
    /// All six global components; reactions only on constrained active DOFs.
    pub force_moment: Six,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuralMemberResult {
    pub member: StructuralMemberId,
    pub length_m: f64,
    /// Rows are local unit x,y,z expressed in global components.
    pub local_axes: [[f64; 3]; 3],
    /// Each end in local axes, [u,v,w,rx,ry,rz].
    pub local_end_displacements: [Six; 2],
    /// k_local d_local - f_equivalent_local. [Fx,Fy,Fz,Mx,My,Mz], i then j.
    pub local_end_forces: [Six; 2],
    pub local_uniform_load_kn_m: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadSourceReceipt {
    pub case: LoadCaseId,
    pub source_id: String,
    pub coefficient: f64,
    pub kind: String,
    pub member: Option<StructuralMemberId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NumericalStatus {
    LinearSolutionResidualsVerified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NumericalDiagnostics {
    pub active_dofs: usize,
    pub free_dofs: usize,
    /// L characteristic used to scale translations, before diagonal equilibration.
    pub characteristic_length_m: f64,
    pub min_equilibrated_pivot: f64,
    pub max_equilibrated_pivot: f64,
    /// Pivot spread is a diagnostic; it is not a spectral condition number.
    pub pivot_ratio_indicator: f64,
    pub relative_free_dof_residual: f64,
    pub global_force_residual_kn: [f64; 3],
    /// About global origin, including nodal moments and r × F.
    pub global_moment_residual_kn_m: [f64; 3],
    pub relative_force_balance_residual: f64,
    pub relative_moment_balance_residual: f64,
    pub status: NumericalStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuralAnalysisResult {
    /// SHA256 of canonical authoritative inputs + target + solver schema/units.
    /// This binds inputs; it is not execution attestation or a code-check pass.
    pub input_fingerprint: String,
    pub target: AnalysisTarget,
    pub normative_status: NormativeStatus,
    pub nodes: Vec<StructuralNodeResult>,
    pub reactions: Vec<StructuralReaction>,
    pub members: Vec<StructuralMemberResult>,
    pub load_sources: Vec<LoadSourceReceipt>,
    pub diagnostics: NumericalDiagnostics,
    /// Explicit scope boundary: no lateral, buckling, ULS, SLS or RC acceptance.
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StructuralError {
    InvalidModel {
        message: String,
    },
    Unsupported {
        message: String,
    },
    CapacityExceeded {
        entity: String,
        count: usize,
        limit: usize,
    },
    MissingReference {
        entity: String,
        id: String,
    },
    DuplicateId {
        entity: String,
        id: String,
    },
    DuplicateLoadSource {
        case: LoadCaseId,
        source_id: String,
    },
    DegenerateMember {
        member: StructuralMemberId,
        message: String,
    },
    SingularMechanism {
        free_dof_index: usize,
        node: StructuralNodeId,
        component: usize,
        pivot: f64,
    },
    NumericalFailure {
        message: String,
    },
}

impl fmt::Display for StructuralError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for StructuralError {}

pub const MAX_NODES: usize = 64;
pub const MAX_MEMBERS: usize = 128;
pub const MAX_LOAD_CASES: usize = 64;
pub const MAX_COMBINATIONS: usize = 64;
