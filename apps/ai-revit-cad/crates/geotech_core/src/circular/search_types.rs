//! Versioned serial bounded-search records. No request field grants owner acceptance.
use super::types::*;
use super::search_evaluate::SearchPassEvaluation;
use super::search_meter::SearchWorkCounters;
use super::arithmetic_environment::ArithmeticEnvironmentReport;
use crate::{InputUnits, Point2};
use serde::{Deserialize, Serialize};

pub type CandidateId = u64;
pub type ProposalId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode { SearchCriticalCircle }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FamilyPolicy { JointlyEligibleV1 }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetMethod { Fellenius, Bishop, Both }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorePolicyProfile { ProvisionalSerialV1 }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchOutputMode { Strict, Diagnostic }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InclusiveAxis { pub min: f64, pub max: f64, pub intervals: u32 }
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosedXRange { pub min_m: f64, pub max_m: f64 }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchDomain {
    pub center_x: InclusiveAxis, pub center_z: InclusiveAxis, pub radius: InclusiveAxis,
    pub allowed_entry_x: Option<ClosedXRange>, pub allowed_exit_x: Option<ClosedXRange>,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchAnalysisConfig {
    pub tolerances: NumericalTolerances, pub limits: ResourceLimits,
    pub bishop: BishopConfig, pub contact: ContactScreenConfig,
}
impl SearchAnalysisConfig {
    pub fn fixed(self, target_slices: u32) -> CircularAnalysisConfig {
        CircularAnalysisConfig { slices: SliceConfig { target_slices: target_slices as usize,
            verify_refinement: false }, tolerances: self.tolerances, limits: self.limits,
            bishop: self.bishop, contact: self.contact }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplorationSlices {
    pub initial_requested: u32, pub max_passes: u32,
    pub require_joint_consecutive_convergence: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinalComparisonPair { pub coarse_requested: u32, pub fine_requested: u32 }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeighborhoodRefinement { pub levels: u32, pub promising_per_method: u32 }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensitivityPlan {
    pub uniform_grid_bisections: u32, pub additional_slice_pairs: Vec<FinalComparisonPair>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchPublicRequestV1 {
    pub schema_version: u32, pub mode: SearchMode,
    pub search_id: String, pub source_revision: String,
    pub ground: GroundSection2D, pub basis: AnalysisBasis, pub units: InputUnits,
    pub direction: SlideDirection, pub extrusion_width_m: f64,
    pub domain: SearchDomain, pub family_policy: FamilyPolicy, pub target_method: TargetMethod,
    pub shared_analysis: SearchAnalysisConfig, pub exploration_slices: ExplorationSlices,
    pub final_pair: FinalComparisonPair, pub refinement: NeighborhoodRefinement,
    pub sensitivity: SensitivityPlan, pub core_policy_profile: CorePolicyProfile,
    pub output_mode: SearchOutputMode, pub max_candidates: u64, pub max_wall_time_ms: u64,
    pub near_tie_abs: f64, pub near_tie_rel: f64,
}
impl SearchPublicRequestV1 {
    /// A convenience builder; admission still validates every resulting field.
    pub fn from_specified(input: &CircularAnalysisInput, config: &CircularAnalysisConfig,
        domain: SearchDomain) -> Self {
        let initial = u32::try_from(config.slices.target_slices).unwrap_or(u32::MAX);
        Self { schema_version: 1, mode: SearchMode::SearchCriticalCircle,
            search_id: input.analysis_id.clone(), source_revision: input.source_revision.clone(),
            ground: input.ground.clone(), basis: input.basis, units: input.units,
            direction: input.slip_surface.slide_direction, extrusion_width_m: input.extrusion_width_m,
            domain, family_policy: FamilyPolicy::JointlyEligibleV1, target_method: TargetMethod::Both,
            shared_analysis: SearchAnalysisConfig { tolerances: config.tolerances,
                limits: config.limits, bishop: config.bishop, contact: config.contact },
            exploration_slices: ExplorationSlices { initial_requested: initial, max_passes: 3,
                require_joint_consecutive_convergence: true },
            final_pair: FinalComparisonPair { coarse_requested: initial.saturating_mul(4),
                fine_requested: initial.saturating_mul(8) },
            refinement: NeighborhoodRefinement { levels: 0, promising_per_method: 2 },
            sensitivity: SensitivityPlan { uniform_grid_bisections: 0, additional_slice_pairs: vec![] },
            core_policy_profile: CorePolicyProfile::ProvisionalSerialV1,
            output_mode: SearchOutputMode::Strict, max_candidates: 4096, max_wall_time_ms: 120_000,
            near_tie_abs: 1e-10, near_tie_rel: 1e-10 }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchBudgets {
    pub max_generated_proposals: u64, pub max_circle_candidates: u64,
    pub max_candidate_evaluations: u64, pub max_refinement_levels: u32,
    pub max_axis_points: u64, pub max_sensitivity_branches: u32,
    pub max_total_geometry_preparations: u64, pub max_total_geometry_work: u64,
    pub max_total_slice_rows: u64, pub max_total_material_integrations: u64,
    pub max_bishop_function_evaluations_per_pass: u32, pub max_total_bishop_slice_terms: u64,
    pub max_total_contact_intervals: u64, pub max_total_contact_boundary_evaluations: u64,
    pub max_total_contact_material_checks: u64, pub max_summary_bytes: u64,
    pub max_working_bytes: u64, pub max_retained_table_bytes: u64, pub max_result_bytes: u64,
    pub max_in_flight: u32, pub max_wall_time_ms: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRetention {
    pub winner_full_tables: bool, pub max_additional_full_tables: u32,
    pub max_reason_bytes_per_candidate: u32, pub max_provenance_origins_per_candidate: u32,
    pub max_progress_events: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequestV1 {
    pub public: SearchPublicRequestV1, pub budgets: SearchBudgets, pub retention: SearchRetention,
    pub profile_status: String,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateKey {
    pub center_x_bits: u64, pub center_z_bits: u64, pub radius_bits: u64,
    pub branch: String, pub direction: String, pub ground_fingerprint: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProposalOrigin {
    BaseGrid { indices: [u32; 3] },
    Neighborhood { level: u32, parent: CandidateId, local_indices: [i8; 3] },
    UniformGrid { bisection: u32, indices: [u32; 3] },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProposalDisposition { Unique { candidate_id: CandidateId }, DuplicateOf { candidate_id: CandidateId } }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalOutcome {
    pub proposal_id: ProposalId, pub origin: ProposalOrigin, pub disposition: ProposalDisposition,
    pub branch: String, pub clipped: [bool; 3],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState { Passed, Rejected, Unresolved, NotAssessed }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasonRecord {
    pub code: String, pub stage: String, pub method: Option<CircularMethod>, pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckOutcome { pub state: CheckState, pub reason: Option<ReasonRecord> }
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericalXInterval { pub lower_m: f64, pub upper_m: f64 }
/// Explicit provenance of contact-X bounds. A missing legacy field remains
/// unavailable; a nominal contact or a null enclosure is never a certificate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum ContactXInterval {
    Computed { interval: NumericalXInterval },
    NotRequested,
    Unavailable,
    Unresolved,
}
impl Default for ContactXInterval {
    fn default() -> Self { Self::Unavailable }
}
impl ContactXInterval {
    fn is_unavailable(&self) -> bool { matches!(self, Self::Unavailable) }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ResolvedContactWire")]
pub struct ResolvedContact {
    pub nominal: Point2,
    /// Compatibility projection. Read contact_x_interval before using bounds.
    pub x_enclosure: Option<NumericalXInterval>,
    /// Omitted for legacy unavailable records to preserve archive roundtrips.
    #[serde(default, skip_serializing_if = "ContactXInterval::is_unavailable")]
    pub contact_x_interval: ContactXInterval,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolvedContactWire {
    nominal: Point2,
    x_enclosure: Option<NumericalXInterval>,
    #[serde(default)]
    contact_x_interval: ContactXInterval,
}
impl TryFrom<ResolvedContactWire> for ResolvedContact {
    type Error = String;
    fn try_from(value: ResolvedContactWire) -> Result<Self, Self::Error> {
        let valid_interval = |interval: NumericalXInterval| {
            interval.lower_m.is_finite() && interval.upper_m.is_finite()
                && interval.lower_m <= interval.upper_m
                && interval.lower_m <= value.nominal.x && value.nominal.x <= interval.upper_m
        };
        if !value.nominal.x.is_finite() || !value.nominal.z.is_finite()
            || value.x_enclosure.is_some_and(|interval| !valid_interval(interval))
        {
            return Err("Contact-X bounds must be finite, ordered and contain the nominal contact".into());
        }
        match value.contact_x_interval {
            ContactXInterval::Computed { interval }
                if !valid_interval(interval) || value.x_enclosure != Some(interval) =>
            {
                return Err("COMPUTED contact-X bounds must match the compatibility enclosure".into());
            }
            ContactXInterval::NotRequested | ContactXInterval::Unresolved
                if value.x_enclosure.is_some() =>
            {
                return Err("Noncomputed contact-X state cannot contain an enclosure".into());
            }
            _ => {}
        }
        Ok(Self { nominal: value.nominal, x_enclosure: value.x_enclosure,
            contact_x_interval: value.contact_x_interval })
    }
}
impl ResolvedContact {
    pub(crate) fn from_search_geometry(nominal: Point2,
        x_enclosure: Option<NumericalXInterval>, enclosure_requested: bool) -> Self {
        let contact_x_interval = match x_enclosure {
            Some(interval) => ContactXInterval::Computed { interval },
            None if enclosure_requested => ContactXInterval::Unresolved,
            None => ContactXInterval::NotRequested,
        };
        Self { nominal, x_enclosure, contact_x_interval }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedContacts {
    pub entry: ResolvedContact, pub exit: ResolvedContact, pub uncertainty_scope: String,
    pub geometry_evaluation_fingerprint: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlicePassSummary {
    pub requested_slices: u32, pub actual_slices: u64, pub input_fingerprint: String,
    pub raw_factor: Option<f64>, pub own_accepted_factor: Option<f64>,
    pub closure_passed: bool, pub error_code: Option<CircularErrorCode>,
    pub bishop_iterations: u64, pub contact_input_fingerprint: Option<String>,
    pub own_failure: Option<ReasonRecord>, pub bishop_convergence_status: Option<BishopConvergenceStatus>,
    pub bishop_physical_status: Option<BishopPhysicalStatus>, pub endpoint_statuses: Vec<BishopEndpointStatus>,
    pub whole_arc_status: Option<WholeArcContactStatus>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodObservation {
    pub method: CircularMethod, pub raw_factor: Option<f64>, pub accepted_factor: Option<f64>,
    pub bishop_convergence_status: Option<BishopConvergenceStatus>,
    pub bishop_physical_status: Option<BishopPhysicalStatus>,
    pub endpoint_statuses: Vec<BishopEndpointStatus>, pub whole_arc_status: Option<WholeArcContactStatus>,
    pub convergence: CheckOutcome, pub discretization: CheckOutcome, pub physical: CheckOutcome,
    pub own_prerequisites: CheckOutcome, pub selected_family_eligibility: CheckOutcome,
    pub slice_passes: Vec<SlicePassSummary>, pub error: Option<ReasonRecord>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    NotInMainFamily, GeometryRejected, FilterRejected, FilterUnresolved,
    MethodsRejected, MethodsUnresolved, JointlyEligible, NotEvaluatedBudget, Cancelled, Stale,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryFlags {
    pub original_box: [bool; 6], pub neighborhood_clipped: [bool; 3],
    pub entry_filter_edge: bool, pub exit_filter_edge: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSummary {
    pub id: CandidateId, pub key: CandidateKey, pub center: Point2, pub radius_m: f64,
    pub first_origin: ProposalId, pub origins: Vec<ProposalOrigin>, pub truncated_origins: u64,
    pub branches: Vec<String>, pub main_family_member: bool,
    pub contacts: Option<ResolvedContacts>, pub geometry: CheckOutcome,
    pub entry_filter: CheckOutcome, pub exit_filter: CheckOutcome,
    pub fellenius: MethodObservation, pub bishop: MethodObservation,
    pub exploratory_fellenius: MethodObservation, pub exploratory_bishop: MethodObservation,
    pub final_status: CandidateStatus, pub reasons: Vec<ReasonRecord>,
    pub boundary: BoundaryFlags, pub final_input_fingerprint: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByMethod<T> { pub fellenius: T, pub bishop: T }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodMinimum {
    pub method: CircularMethod, pub label: String, pub candidate_id: CandidateId,
    pub fine_factor: f64, pub table_hash: String, pub evaluation_key: String,
    pub final_pair: FinalComparisonPair, pub near_ties: Vec<CandidateId>, pub boundary: BoundaryFlags,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundCandidateTable {
    pub candidate_id: CandidateId, pub key: CandidateKey, pub branch: String,
    pub evaluation_key: String, pub source_fingerprint: String, pub table_hash: String,
    pub config: CircularAnalysisConfig, pub evaluation: SearchPassEvaluation,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundDiagnosticCandidate {
    pub candidate_id: CandidateId, pub key: CandidateKey, pub evaluation_key: String,
    pub source_fingerprint: String, pub observation: MethodObservation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerPrerequisiteProvenance {
    pub decision: String, pub review_status: String, pub receipt_path: String,
    pub receipt_sha256: String, pub accepted_scope: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchRunStatus {
    CompletedMinimum, CompletedNoEligibleCandidates, InvalidRequest, PrerequisiteNotAccepted,
    BudgetExhausted, DeadlineExhausted, Cancelled, Stale, InternalFailure,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchConclusion { MinimumWithinExaminedCircularFamily, NoStabilityEvidence, IncompleteDiagnostic }
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchCounters {
    pub admitted_proposals_upper_bound: u64, pub admitted_evaluations_upper_bound: u64,
    pub generated_proposals: u64, pub duplicate_proposals: u64, pub unique_candidates: u64,
    pub geometry_rejected: u64, pub filter_rejected: u64, pub filter_unresolved: u64,
    pub candidate_evaluations: u64, pub evaluated_candidates: u64,
    pub fellenius_own_eligible: u64, pub bishop_own_eligible: u64, pub jointly_eligible: u64,
    pub methods_rejected: u64, pub methods_unresolved: u64, pub not_evaluated: u64,
    pub actual_slice_rows: u64, pub bishop_iterations: u64,
    pub retained_table_bytes: u64, pub retained_table_peak_bytes: u64,
    pub summary_bytes: u64, pub working_upper_bound_bytes: u64, pub output_bytes: u64,
    pub progress_events: u32, pub progress_events_coalesced: u64,
    pub work: SearchWorkCounters,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FamilyManifest {
    pub binding_version: String, pub proposal_manifest_hash: String, pub eligible_set_hash: String,
    pub main_candidate_ids: Vec<CandidateId>, pub eligible_candidate_ids: Vec<CandidateId>,
    pub classification: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefinementStageSummary {
    pub level: u32, pub available_fellenius_seeds: u64, pub available_bishop_seeds: u64,
    pub seeds: Vec<CandidateId>, pub proposal_count: u64, pub limitation: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchCandidateObservation {
    pub candidate_id: CandidateId, pub fellenius: MethodObservation,
    pub bishop: MethodObservation, pub status: CandidateStatus,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensitivityResult {
    pub branch: String, pub kind: String, pub status: String, pub final_pair: FinalComparisonPair,
    pub candidate_ids: Vec<CandidateId>, pub eligible_candidate_ids: Vec<CandidateId>,
    pub eligible_set_hash: String, pub minima: ByMethod<Option<MethodMinimum>>,
    pub observations: Vec<BranchCandidateObservation>, pub winner_changed: ByMethod<Option<bool>>,
    pub minimum_factor_delta: ByMethod<Option<f64>>, pub eligibility_changed: bool,
    pub common_candidate_ids: Vec<CandidateId>, pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchResultV1 {
    pub schema_version: u32, pub mode: SearchMode, pub status: SearchRunStatus,
    pub conclusion: SearchConclusion, pub accepted: bool, pub diagnostic_only: bool,
    pub request: SearchRequestV1, pub request_fingerprint: String, pub source_fingerprint: String,
    pub solver_version: String, pub prerequisite_provenance: OwnerPrerequisiteProvenance,
    pub arithmetic_environment: ArithmeticEnvironmentReport,
    pub execution_policy: String, pub parallel_determinism: String,
    pub examined_family: FamilyManifest, pub family_policy: FamilyPolicy,
    pub counters: SearchCounters, pub candidates: Vec<CandidateSummary>,
    pub proposal_outcomes: Vec<ProposalOutcome>, pub minima: ByMethod<Option<MethodMinimum>>,
    pub diagnostic_best_so_far: ByMethod<Option<BoundDiagnosticCandidate>>,
    pub retained_tables: Vec<BoundCandidateTable>, pub refinement: Vec<RefinementStageSummary>,
    pub sensitivity: Vec<SensitivityResult>, pub warnings: Vec<String>,
    pub terminal_reason: Option<ReasonRecord>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchProgress {
    pub stage: String, pub generated_proposals: u64, pub unique_candidates: u64,
    pub completed_evaluations: u64, pub evaluated_candidates: u64, pub actual_slice_rows: u64,
    pub terminal_status: Option<SearchRunStatus>,
}
