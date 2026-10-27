use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SemanticId(String);
impl SemanticId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 512
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.@:/%".contains(&b))
        {
            return Err("Semantic ID must contain 1..512 ASCII letters/digits or _-.@:/%".into());
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for SemanticId {
    type Error = String;
    fn try_from(v: String) -> Result<Self, String> {
        Self::new(v)
    }
}
impl TryFrom<&str> for SemanticId {
    type Error = String;
    fn try_from(v: &str) -> Result<Self, String> {
        Self::new(v)
    }
}
impl std::str::FromStr for SemanticId {
    type Err = String;
    fn from_str(v: &str) -> Result<Self, String> {
        Self::new(v)
    }
}
impl std::fmt::Display for SemanticId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}
impl AsRef<str> for SemanticId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl std::borrow::Borrow<str> for SemanticId {
    fn borrow(&self) -> &str {
        &self.0
    }
}
impl std::ops::Deref for SemanticId {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for SemanticId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalTransform {
    pub translation_m: [f64; 2],
    pub rotation_rad: f64,
}
impl Default for LocalTransform {
    fn default() -> Self {
        Self {
            translation_m: [0.0; 2],
            rotation_rad: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corner {
    pub id: SemanticId,
    pub xy_m: [f64; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Provenance {
    Synthetic { note: String },
    Manual { reference: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteelMaterial {
    pub id: SemanticId,
    pub name: String,
    pub elastic_modulus_kn_m2: f64,
    pub shear_modulus_kn_m2: f64,
    pub weight_density_kn_m3: f64,
    pub provenance: Provenance,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberProfile {
    pub id: SemanticId,
    pub name: String,
    pub area_m2: f64,
    pub iy_m4: f64,
    pub iz_m4: f64,
    pub torsional_constant_m4: f64,
    pub provenance: Provenance,
    #[serde(default)]
    pub visualization: Option<ProfileVisual>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileVisual {
    pub width_m: f64,
    pub depth_m: f64,
    pub web_thickness_m: f64,
    pub flange_thickness_m: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberRule {
    pub material_id: SemanticId,
    pub profile_id: SemanticId,
    /// Building-local reference, rotated into global XY; explicit for every role.
    pub local_y_reference: [f64; 3],
    pub end_releases: Option<[[bool; 6]; 2]>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberRules {
    pub column: MemberRule,
    pub rafter: MemberRule,
    pub beam: MemberRule,
    pub brace: MemberRule,
    pub purlin: MemberRule,
    pub wall_girt: MemberRule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RoofKind {
    Flat,
    MonoPitch,
    Gable,
    Hip,
    Curved,
    MultiFacet,
    Stepped,
    Custom,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RoofDetails {
    pub corner_tops_abs_m: BTreeMap<SemanticId, f64>,
    pub break_lines_m: Vec<Vec<[f64; 3]>>,
    pub ridge_lines_m: Vec<Vec<[f64; 3]>>,
    pub facets_m: Vec<Vec<[f64; 3]>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoofSpec {
    pub kind: RoofKind,
    pub base_abs_m: f64,
    pub eave_abs_m: f64,
    /// Flat equals eave; mono is high edge; gable is ridge.
    pub ridge_abs_m: f64,
    #[serde(default)]
    pub unsupported_details: Option<RoofDetails>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StructuralSystem {
    SteelFrame,
    Timber,
    Masonry,
    ReinforcedConcrete,
    Precast,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PanelLoadTransferStatus {
    NotVerified,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelSpec {
    pub thickness_m: f64,
    pub layout_width_m: f64,
    pub layout_length_m: f64,
    pub material_label: String,
    pub provenance: Provenance,
    pub mass_kg_m2: Option<f64>,
    pub stiffness_enabled: bool,
    pub load_transfer_status: PanelLoadTransferStatus,
    /// Reserved requests are reported, never silently applied to the host surface.
    #[serde(default)]
    pub requested_features: Vec<PanelFeature>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PanelFeature {
    StripLayout,
    Openings,
    SupportScheme,
    VerifiedLoadTransfer,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridStation {
    pub id: SemanticId,
    pub position_m: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GridSpacing {
    /// Distances from the section minimum projected coordinate. Includes both ends.
    Explicit { stations: Vec<GridStation> },
    /// Interior IDs are ordinal; endpoints keep stable start/end IDs.
    Uniform { pitch_m: f64 },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionGrid {
    /// Unit building-local span/slope direction u; v=(-uy,ux).
    pub span_direction: [f64; 2],
    pub frame_stations: GridSpacing,
    pub column_stations: GridSpacing,
    pub purlin_pitch_m: f64,
    pub wall_girt_pitch_m: f64,
    pub braces: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionConfig {
    pub id: SemanticId,
    pub structural_system: StructuralSystem,
    /// Ordered CCW, unclosed ring in building-local XY.
    pub corners: Vec<Corner>,
    pub roof: RoofSpec,
    pub grid: SectionGrid,
    pub members: MemberRules,
    pub base_restraints: [bool; 6],
    #[serde(default)]
    pub roof_panels: Option<PanelSpec>,
    #[serde(default)]
    pub wall_panels: Option<PanelSpec>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConnectionIdealization {
    Rigid,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InterfaceKind {
    StructurallyConnected {
        idealization: ConnectionIdealization,
    },
    /// Translate section B by width * unit direction, normal to the declared edge.
    IndependentWithJoint {
        width_m: f64,
        offset_b_direction: [f64; 2],
    },
    GeometryOnlyContact,
    NotDefined,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceAnchor {
    pub id: SemanticId,
    /// Fraction along edge_a in its declared order; all shared column stations required.
    pub fraction: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceSpec {
    pub id: SemanticId,
    pub section_a: SemanticId,
    pub edge_a: [SemanticId; 2],
    pub section_b: SemanticId,
    pub edge_b: [SemanticId; 2],
    pub kind: InterfaceKind,
    pub anchors: Vec<InterfaceAnchor>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationAssignment {
    pub section_id: SemanticId,
    pub support_node_id: SemanticId,
    pub foundation_id: SemanticId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingConfig {
    pub id: SemanticId,
    pub transform: LocalTransform,
    pub sections: Vec<SectionConfig>,
    pub interfaces: Vec<InterfaceSpec>,
    pub materials: Vec<SteelMaterial>,
    pub profiles: Vec<MemberProfile>,
    pub foundation_assignments: Vec<FoundationAssignment>,
    /// Explicit permission to analyze independent supported components together.
    /// Does not partition the dense model or relax its total capacity.
    pub allow_disconnected_analysis: bool,
    pub analysis_load_scope: AnalysisLoadScope,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AnalysisLoadScope {
    FrameSelfWeightOnly,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RoofHeightUpdate {
    PreserveRise,
    KeepRidgeAbsolute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IssueSeverity {
    Error,
    GenerationLimit,
    Warning,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub code: String,
    pub path: String,
    pub message: String,
    pub severity: IssueSeverity,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConfiguratorError {
    InvalidConfiguration {
        issues: Vec<ValidationIssue>,
    },
    UnsupportedGeneration {
        section_id: String,
        reason: String,
    },
    InterfaceConflict {
        interface_id: String,
        reason: String,
    },
    InterfaceLoadPathNotVerified {
        interface_ids: Vec<String>,
    },
    DisconnectedLoadPathsNotAcknowledged,
    AnalysisModelCapacityExceeded {
        nodes: usize,
        members: usize,
        max_nodes: usize,
        max_members: usize,
    },
    UnsupportedReleases {
        member_id: String,
    },
    InvalidMemberOrientation {
        member_id: String,
    },
    MissingReference {
        entity: String,
        id: String,
    },
    ResourceLimit {
        reason: String,
    },
    Serialization {
        message: String,
    },
    GeneratedBindingMismatch {
        message: String,
    },
    InvalidAnalysisScope {
        reason: String,
    },
    SemanticIdHashCollision {
        entity: String,
        first: String,
        second: String,
    },
}
impl std::fmt::Display for ConfiguratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ConfiguratorError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedNode {
    pub id: SemanticId,
    pub position_m: [f64; 3],
    pub owners: BTreeSet<SemanticId>,
    pub restrained: Option<[bool; 6]>,
    pub fingerprint: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MemberRole {
    Column,
    Rafter,
    Beam,
    Brace,
    Purlin,
    WallGirt,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedMember {
    pub id: SemanticId,
    pub start: SemanticId,
    pub end: SemanticId,
    pub role: MemberRole,
    pub owners: BTreeSet<SemanticId>,
    pub material_id: SemanticId,
    pub profile_id: SemanticId,
    pub local_y_reference: [f64; 3],
    pub end_releases: Option<[[bool; 6]; 2]>,
    pub length_m: f64,
    pub self_weight_source_id: SemanticId,
    pub fingerprint: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SurfaceRole {
    RoofFacet,
    WallFace,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedSurface {
    pub id: SemanticId,
    pub section_id: SemanticId,
    pub role: SurfaceRole,
    pub vertices_m: Vec<[f64; 3]>,
    pub area_m2: f64,
    pub stiffness_enabled: bool,
    pub load_source_id: SemanticId,
    pub fingerprint: String,
    pub panel_spec: Option<PanelSpec>,
    pub mass_kg: Option<f64>,
    pub load_transfer_status: PanelLoadTransferStatus,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionOutline {
    pub section_id: SemanticId,
    pub world_corners_m: Vec<[f64; 3]>,
    pub footprint_area_m2: f64,
    pub perimeter_m: f64,
    pub generation_limit: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InterfaceLoadPathStatus {
    ExplicitRigidIdealization,
    IndependentWithJoint,
    InterfaceLoadPathNotVerified,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedInterface {
    pub id: SemanticId,
    pub section_a: SemanticId,
    pub section_b: SemanticId,
    pub status: InterfaceLoadPathStatus,
    pub shared_node_ids: Vec<SemanticId>,
    pub shared_member_ids: Vec<SemanticId>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReactionRouting {
    pub support_node_id: SemanticId,
    pub section_owners: BTreeSet<SemanticId>,
    pub foundation_by_section: BTreeMap<SemanticId, SemanticId>,
    /// Shared physical reaction cannot be silently divided among section owners.
    pub requires_explicit_allocation: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedBuilding {
    pub id: SemanticId,
    pub generator_version: String,
    pub config_fingerprint: String,
    pub graph_fingerprint: String,
    #[serde(deserialize_with = "crate::unique_map::deserialize")]
    pub nodes: BTreeMap<SemanticId, GeneratedNode>,
    #[serde(deserialize_with = "crate::unique_map::deserialize")]
    pub members: BTreeMap<SemanticId, GeneratedMember>,
    #[serde(deserialize_with = "crate::unique_map::deserialize")]
    pub surfaces: BTreeMap<SemanticId, GeneratedSurface>,
    #[serde(deserialize_with = "crate::unique_map::deserialize")]
    pub outlines: BTreeMap<SemanticId, SectionOutline>,
    pub interfaces: Vec<GeneratedInterface>,
    pub materials: Vec<SteelMaterial>,
    pub profiles: Vec<MemberProfile>,
    pub reaction_routing: Vec<ReactionRouting>,
    pub allow_disconnected_analysis: bool,
    pub generation_limits: BTreeMap<SemanticId, String>,
    pub limitations: Vec<String>,
    pub analysis_status: AnalysisProjectionStatus,
    pub analysis_load_scope: AnalysisLoadScope,
    pub bim_quantities: BimQuantityLedger,
    pub analysis_load_ledger: AnalysisLoadLedger,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SteelQuantity {
    pub member_id: SemanticId,
    pub owners: BTreeSet<SemanticId>,
    pub volume_m3: f64,
    pub weight_kn: f64,
    pub source_id: SemanticId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BimQuantityLedger {
    pub steel: Vec<SteelQuantity>,
    pub steel_volume_m3: f64,
    pub steel_weight_kn: f64,
    pub panel_mass_kg: Option<f64>,
    pub unknown_panel_surface_count: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalysisLoadLedger {
    pub scope: AnalysisLoadScope,
    pub projected_member_ids: BTreeSet<SemanticId>,
    pub projected_steel_weight_kn: f64,
    pub excluded_loads: Vec<String>,
    /// A model load proposal, not a receipt that a solver actually applied it.
    pub status: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AnalysisProjectionStatus {
    ReadyForBoundedProjection,
    AnalysisModelCapacityExceeded {
        nodes: usize,
        members: usize,
        max_nodes: usize,
        max_members: usize,
    },
    Blocked {
        reasons: Vec<String>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedAnalysisScope {
    pub section_ids: BTreeSet<SemanticId>,
}
pub type Building = BuildingConfig;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DenseIdMap {
    pub nodes: BTreeMap<SemanticId, u64>,
    pub members: BTreeMap<SemanticId, u64>,
    pub materials: BTreeMap<SemanticId, u64>,
    pub profiles: BTreeMap<SemanticId, u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationDelta {
    pub added: BTreeSet<SemanticId>,
    pub removed: BTreeSet<SemanticId>,
    pub changed: BTreeSet<SemanticId>,
    pub unchanged: BTreeSet<SemanticId>,
}
