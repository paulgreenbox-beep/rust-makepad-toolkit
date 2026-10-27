use crate::{InputUnits, Point2, SoilMaterial};
use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.into())
            }
        }
    };
}
id!(GroundSectionId);
id!(SoilMaterialId);
id!(SoilRegionId);
id!(LayerInterfaceId);
id!(CircularSlipId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionCoordinateSystem {
    CartesianXzMetres,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundMaterial {
    pub material_id: SoilMaterialId,
    pub properties: SoilMaterial,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerInterface {
    pub interface_id: LayerInterfaceId,
    pub polyline: Vec<Point2>,
}

/// Regions are ordered from the terrain downwards. Region i lies between
/// boundary i and i+1; the boundaries are terrain, interfaces, bottom.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoilRegion {
    pub region_id: SoilRegionId,
    pub material_id: SoilMaterialId,
}

/// Authoritative, immutable world X/Z coordinates in metres, Z upwards.
/// v0.2 supports a closed, full-span, non-crossing stratigraphic stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundSection2D {
    pub section_id: GroundSectionId,
    pub coordinate_system: SectionCoordinateSystem,
    pub terrain_polyline: Vec<Point2>,
    pub layer_interfaces: Vec<LayerInterface>,
    pub soil_regions: Vec<SoilRegion>,
    pub bottom_boundary: Vec<Point2>,
    pub materials: Vec<GroundMaterial>,
    pub model_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArcBranch {
    Lower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlideDirection {
    PositiveX,
    NegativeX,
}
impl SlideDirection {
    pub(crate) fn sign(self) -> f64 {
        match self {
            Self::PositiveX => 1.0,
            Self::NegativeX => -1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircleEndpoints {
    pub entry_point: Point2,
    pub exit_point: Point2,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircularSlipSurface {
    pub slip_id: CircularSlipId,
    pub center: Point2,
    pub radius_m: f64,
    pub arc_branch: ArcBranch,
    pub slide_direction: SlideDirection,
    /// Optional explicit selection, verified against the unique terrain contacts.
    pub selected_endpoints: Option<CircleEndpoints>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisBasis {
    DryStatic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircularAnalysisInput {
    pub analysis_id: String,
    pub source_revision: String,
    pub ground: GroundSection2D,
    pub slip_surface: CircularSlipSurface,
    pub basis: AnalysisBasis,
    pub units: InputUnits,
    pub extrusion_width_m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericalTolerances {
    pub intersection_abs_m: f64,
    pub area_abs_m2: f64,
    pub area_rel: f64,
    pub integral_abs_m2: f64,
    pub integral_rel: f64,
    pub angle_abs_rad: f64,
    pub min_base_cos: f64,
    pub force_abs_kn_per_m: f64,
    pub force_rel: f64,
    pub fs_abs: f64,
    pub fs_rel: f64,
}
impl Default for NumericalTolerances {
    fn default() -> Self {
        Self {
            intersection_abs_m: 1e-9,
            area_abs_m2: 1e-8,
            area_rel: 1e-9,
            integral_abs_m2: 1e-9,
            integral_rel: 1e-10,
            angle_abs_rad: 1e-10,
            min_base_cos: 1e-6,
            force_abs_kn_per_m: 1e-9,
            force_rel: 1e-10,
            fs_abs: 1e-5,
            fs_rel: 1e-4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    pub max_soil_regions: usize,
    pub max_terrain_segments: usize,
    pub max_boundary_points: usize,
    pub max_slices: usize,
    pub max_solver_iterations: usize,
    pub max_result_bytes: usize,
}
impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_soil_regions: 32,
            max_terrain_segments: 2048,
            max_boundary_points: 8192,
            max_slices: 4096,
            max_solver_iterations: 1024,
            max_result_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceConfig {
    pub target_slices: usize,
    pub verify_refinement: bool,
}
impl Default for SliceConfig {
    fn default() -> Self {
        Self {
            target_slices: 64,
            verify_refinement: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CircularAnalysisConfig {
    pub slices: SliceConfig,
    pub tolerances: NumericalTolerances,
    pub limits: ResourceLimits,
    pub bishop: BishopConfig,
    #[serde(default)]
    pub contact: ContactScreenConfig,
}

/// Work and uncertainty bounds for the numerical whole-arc contact screen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContactScreenConfig {
    pub max_intervals: usize,
    pub max_depth: usize,
    pub max_boundary_evaluations: usize,
    pub max_material_checks: usize,
    pub max_report_intervals: usize,
    pub min_interval_width_m: f64,
    pub geometry_margin_multiplier: f64,
    pub friction_uncertainty_rad: f64,
}
impl Default for ContactScreenConfig {
    fn default() -> Self {
        Self {
            max_intervals: 8192, max_depth: 40,
            max_boundary_evaluations: 262144, max_material_checks: 262144,
            max_report_intervals: 256, min_interval_width_m: 1e-9,
            geometry_margin_multiplier: 4.0, friction_uncertainty_rad: 1e-12,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BishopConfig {
    pub initial_factor: f64,
    pub min_factor: f64,
    pub max_factor: f64,
    pub max_iterations: usize,
    pub step_abs: f64,
    pub step_rel: f64,
    pub residual_abs: f64,
    pub residual_rel: f64,
    pub min_m_alpha: f64,
}
impl Default for BishopConfig {
    fn default() -> Self {
        Self {
            initial_factor: 1.0,
            min_factor: 1e-6,
            max_factor: 1e6,
            max_iterations: 128,
            step_abs: 1e-10,
            step_rel: 1e-10,
            residual_abs: 1e-10,
            residual_rel: 1e-10,
            min_m_alpha: 1e-12,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialArea {
    pub material_id: SoilMaterialId,
    pub area_m2: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericalQuality {
    pub local_origin: Point2,
    pub world_resolution_m: f64,
    pub intersection_tolerance_m: f64,
    pub estimated_area_roundoff_m2: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedCircularSlip {
    pub surface: CircularSlipSurface,
    pub left_point: Point2,
    pub right_point: Point2,
    pub entry_point: Point2,
    pub exit_point: Point2,
    pub arc_length_m: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureMassGeometry {
    pub circle: ResolvedCircularSlip,
    /// Terrain segment from left to right. The exact closing edge is the arc.
    pub terrain_boundary: Vec<Point2>,
    pub area_m2: f64,
    pub material_areas: Vec<MaterialArea>,
    pub numerical_quality: NumericalQuality,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slice {
    pub slice_id: usize,
    pub x_left: f64,
    pub x_right: f64,
    pub horizontal_width_m: f64,
    pub area_by_material: Vec<MaterialArea>,
    pub total_area_m2: f64,
    pub weight_kn_per_m: f64,
    pub base_left: Point2,
    pub base_right: Point2,
    pub base_inclination_rad: f64,
    /// Chord length, never the circular arc length. l = b/cos(alpha).
    pub base_length_m: f64,
    pub arc_length_m: f64,
    pub base_material_id: SoilMaterialId,
    pub numerical_quality: NumericalQuality,
}

/// Public formula boundary for independently authored tables, without geometry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceMechanicsInput {
    pub horizontal_width_m: f64,
    pub base_length_m: f64,
    pub base_inclination_rad: f64,
    pub weight_kn_per_m: f64,
    pub effective_cohesion_kpa: f64,
    pub effective_friction_angle_rad: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircularMethod {
    FelleniusOrdinary,
    BishopSimplified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BishopConvergenceStatus {
    Converged,
    NoPhysicalRoot,
    InvalidDenominator,
    MaxIterations,
    FactorOutOfBounds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BishopStep {
    FixedPoint,
    Bisection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BishopIteration {
    pub iteration: usize,
    pub step: BishopStep,
    pub factor_before: f64,
    pub factor_after: f64,
    pub step_delta: f64,
    pub equation_residual: f64,
    pub relative_residual: f64,
    pub minimum_m_alpha: f64,
    pub bracket_lower: f64,
    pub bracket_upper: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BishopSliceContribution {
    pub slice_id: usize,
    pub mechanics: SliceMechanicsInput,
    pub m_alpha: f64,
    pub numerator_kn_per_m: f64,
    pub contribution_kn_per_m: f64,
    pub base_forces: BishopBaseForces,
}

/// Contact admissibility is independent of convergence of H(F)=0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BishopPhysicalStatus {
    Admissible,
    InadmissibleTension,
    NumericallyIndeterminate,
    VerticalEquilibriumNotSatisfied,
    ShearLawNotSatisfied,
    MomentEquilibriumNotSatisfied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BishopPhysicalScope {
    CurrentDiscreteSliceTableOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BishopBaseForces {
    pub normal_kn_per_m: f64,
    pub mobilized_shear_kn_per_m: f64,
    pub available_shear_strength_kn_per_m: f64,
    pub normal_roundoff_bound_kn_per_m: f64,
    pub minimum_bracket_normal_kn_per_m: f64,
    pub vertical_residual_kn_per_m: f64,
    pub vertical_residual_bound_kn_per_m: f64,
    pub shear_law_residual_kn_per_m: f64,
    pub shear_law_residual_bound_kn_per_m: f64,
    pub physical_status: BishopPhysicalStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BishopPhysicalAssessment {
    pub status: BishopPhysicalStatus,
    pub scope: BishopPhysicalScope,
    pub physical_contact_convergence_verified: bool,
    pub eligible_for_critical_minimum: bool,
    pub tensile_slice_ids: Vec<usize>,
    pub indeterminate_slice_ids: Vec<usize>,
    pub vertical_failure_slice_ids: Vec<usize>,
    pub shear_law_failure_slice_ids: Vec<usize>,
    pub minimum_normal_kn_per_m: f64,
    pub maximum_absolute_vertical_residual_kn_per_m: f64,
    pub maximum_absolute_shear_law_residual_kn_per_m: f64,
    pub root_bracket_lower: f64,
    pub root_bracket_upper: f64,
    pub roundoff_multiplier: f64,
    pub force_sum_identity_residual_kn_per_m: f64,
    pub force_sum_identity_bound_kn_per_m: f64,
    pub moment_equilibrium_residual_kn_per_m: f64,
    pub moment_equilibrium_bound_kn_per_m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BishopEndpointType { Entry, Exit }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BishopEndpointStatus {
    TensileInterval,
    Unresolved,
    CohesionlessLocalScreenPassed,
    PositiveLimitLocalScreenPassed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BishopEndpointContact {
    pub contact_id: String,
    pub endpoint_type: BishopEndpointType,
    pub point: Point2,
    pub inward_x_direction: i8,
    pub material_id: Option<SoilMaterialId>,
    pub material_separation_m: Option<f64>,
    pub cohesion_kpa: Option<f64>,
    pub friction_angle_rad: Option<f64>,
    pub sin_alpha: Option<f64>,
    pub cos_alpha: Option<f64>,
    pub angle_uncertainty_rad: Option<f64>,
    pub factor: f64,
    pub factor_lower: f64,
    pub factor_upper: f64,
    pub m_alpha: Option<f64>,
    pub denominator_f_cos_plus_sin_tan: Option<f64>,
    pub denominator_lower_bound: Option<f64>,
    pub normal_stress_limit_kpa: Option<f64>,
    pub status: BishopEndpointStatus,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BishopEndpointAssessment {
    pub passed: bool,
    pub endpoints: Vec<BishopEndpointContact>,
    /// Endpoint screens do not establish contact admissibility on the whole arc.
    pub whole_arc_contact_verified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WholeArcContactStatus {
    PassedDeclaredModel,
    Violation,
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactIntervalStatus {
    Passed,
    OutsideFailureMass,
    Violation,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContactIntervalReport {
    pub x_left_m: f64,
    pub x_right_m: f64,
    pub depth: usize,
    pub status: ContactIntervalStatus,
    pub material_ids: Vec<SoilMaterialId>,
    pub material_union_exhaustive: bool,
    pub structural_nonnegative_b: bool,
    pub b_lower_kpa: Option<f64>,
    pub b_upper_kpa: Option<f64>,
    pub d_lower: Option<f64>,
    pub d_upper: Option<f64>,
    pub cos_lower: Option<f64>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WholeArcContactReport {
    pub status: WholeArcContactStatus,
    pub scope: String,
    pub root_range_scope: String,
    pub contact_input_fingerprint: String,
    pub source_fingerprint: String,
    pub config: ContactScreenConfig,
    pub factor_lower: f64,
    pub factor_upper: f64,
    pub geometry_envelope_m: f64,
    pub domain_x_left_m: f64,
    pub domain_x_right_m: f64,
    pub coverage_complete: bool,
    pub minimum_b_lower_kpa: Option<f64>,
    pub minimum_d_lower: Option<f64>,
    pub minimum_cos_lower: Option<f64>,
    pub intervals_evaluated: usize,
    pub leaf_intervals: usize,
    pub subdivisions: usize,
    pub boundary_evaluations: usize,
    pub material_checks: usize,
    pub maximum_depth_reached: usize,
    pub details_truncated: bool,
    pub intervals: Vec<ContactIntervalReport>,
    pub reasons: Vec<String>,
    /// A passed numerical envelope does not independently certify the root,
    /// discretization error, or engineering suitability.
    pub independent_whole_arc_acceptance: bool,
    pub eligible_for_critical_minimum: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BishopResult {
    pub method: CircularMethod,
    pub convergence_status: BishopConvergenceStatus,
    pub config: BishopConfig,
    pub initial_factor_requested: f64,
    pub initial_factor_used: f64,
    pub admissible_lower_factor: f64,
    pub factor_of_safety: f64,
    /// Present only for a converged, contact-admissible result. The raw field
    /// above remains available in explicit mathematical diagnostics.
    pub accepted_factor_of_safety: Option<f64>,
    pub physical_assessment: BishopPhysicalAssessment,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_assessment: Option<BishopEndpointAssessment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whole_arc_contact: Option<WholeArcContactReport>,
    pub driving_sum_kn_per_m: f64,
    pub bishop_sum_kn_per_m: f64,
    pub iterations: Vec<BishopIteration>,
    pub final_step_delta: f64,
    pub equation_residual: f64,
    pub relative_residual: f64,
    pub step_tolerance: f64,
    pub residual_tolerance: f64,
    pub slice_contributions: Vec<BishopSliceContribution>,
    pub assumptions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BishopFailureDiagnostic {
    pub status: BishopConvergenceStatus,
    pub admissible_lower_factor: Option<f64>,
    pub last_factor: Option<f64>,
    pub equation_residual: Option<f64>,
    pub relative_residual: Option<f64>,
    pub faulty_slice_id: Option<usize>,
    pub m_alpha: Option<f64>,
    pub iterations: Vec<BishopIteration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mathematical_root: Option<Box<BishopResult>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FelleniusSliceContribution {
    pub slice_id: usize,
    pub mechanics: SliceMechanicsInput,
    pub normal_kn_per_m: f64,
    pub driving_kn_per_m: f64,
    pub cohesion_kn_per_m: f64,
    pub friction_kn_per_m: f64,
    pub resistance_kn_per_m: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FelleniusResult {
    pub method: CircularMethod,
    pub factor_of_safety: f64,
    pub driving_sum_kn_per_m: f64,
    pub resisting_sum_kn_per_m: f64,
    pub slice_contributions: Vec<FelleniusSliceContribution>,
    pub assumptions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscretizationQuality {
    pub refinement_checked: bool,
    pub coarse_slice_count: usize,
    pub fine_slice_count: Option<usize>,
    pub coarse_factor_of_safety: f64,
    pub fine_factor_of_safety: Option<f64>,
    pub factor_delta: Option<f64>,
    pub accepted_tolerance: f64,
    pub coarse_bishop_factor_of_safety: f64,
    pub fine_bishop_factor_of_safety: Option<f64>,
    pub bishop_factor_delta: Option<f64>,
    pub bishop_accepted_tolerance: f64,
    pub area_closure_m2: f64,
    pub weight_closure_kn_per_m: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircularSlopeResult {
    pub analysis_id: String,
    pub source_revision: String,
    pub input_fingerprint: String,
    pub source_fingerprint: String,
    pub solver_version: String,
    pub geometry: FailureMassGeometry,
    pub slices: Vec<Slice>,
    pub total_area_m2: f64,
    pub total_weight_kn_per_m: f64,
    pub soil_breakdown: Vec<MaterialArea>,
    pub fellenius_result: FelleniusResult,
    pub bishop_result: BishopResult,
    pub discretization: DiscretizationQuality,
    pub warnings: Vec<String>,
    /// Solver execution cannot grant independent verification.
    pub numerically_verified: bool,
    pub engineering_review_required: bool,
    /// Explicit diagnostic execution is never an accepted search candidate.
    pub diagnostic_only: bool,
    /// Numerical/physical specified-circle acceptance, never independent
    /// numerical verification or engineering/regulatory approval.
    pub accepted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CircularErrorCode {
    NonFiniteInput,
    InvalidParameter,
    InvalidUnits,
    UnsupportedConditions,
    UnsupportedGroundTopology,
    MissingMaterial,
    InvalidGround,
    AmbiguousIntersection,
    InvalidCircle,
    NoTerrainIntersection,
    SlipOutsideGround,
    DegenerateMass,
    AmbiguousBaseMaterial,
    NearVerticalBase,
    NoDrivingForce,
    NumericOverflow,
    ResourceLimit,
    DiscretizationNotConverged,
    Cancelled,
    StaleResult,
    BishopInvalidDenominator,
    BishopNoPhysicalRoot,
    BishopNotConverged,
    BishopFactorOutOfBounds,
    BishopPhysicallyInadmissible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircularError {
    pub code: CircularErrorCode,
    pub message: String,
    pub diagnostic_geometry: Vec<Point2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bishop_diagnostic: Option<BishopFailureDiagnostic>,
}
impl CircularError {
    pub fn new(code: CircularErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            diagnostic_geometry: vec![],
            bishop_diagnostic: None,
        }
    }
    pub(crate) fn at(mut self, points: Vec<Point2>) -> Self {
        self.diagnostic_geometry = points;
        self
    }
}
impl fmt::Display for CircularError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for CircularError {}
