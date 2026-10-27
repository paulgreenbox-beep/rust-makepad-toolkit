//! Persisted, typed search policy for a circular engineering case.
//!
//! This validates the durable request shape and bounded policy. Search admission,
//! aggregate runtime budgets, geometry checks, and solver acceptance remain in
//! their existing layers.
use crate::engineering::operation_source_id;
use crate::{AnalysisCaseId, CoreError, Document, OperationId};
use geotech_core::circular::{
    CircularAnalysisConfig, CorePolicyProfile, ExplorationSlices, FamilyPolicy,
    FinalComparisonPair, InclusiveAxis, NeighborhoodRefinement, SearchDomain, SearchMode,
    SearchOutputMode, SearchPublicRequestV1, SensitivityPlan, TargetMethod,
};
use serde::{Deserialize, Serialize};

/// Durable policy for one bounded circular search, attached to its owning case.
/// The search mode is deliberately implicit and resolves to the core's fixed
/// `SearchCriticalCircle` mode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircularSearchSpecV1 {
    pub schema_version: u32,
    pub domain: SearchDomain,
    pub family_policy: FamilyPolicy,
    pub target_method: TargetMethod,
    pub exploration_slices: ExplorationSlices,
    pub final_pair: FinalComparisonPair,
    pub refinement: NeighborhoodRefinement,
    pub sensitivity: SensitivityPlan,
    pub core_policy_profile: CorePolicyProfile,
    pub output_mode: SearchOutputMode,
    pub max_candidates: u64,
    pub max_wall_time_ms: u64,
    pub near_tie_abs: f64,
    pub near_tie_rel: f64,
}

impl CircularSearchSpecV1 {
    /// Check only persisted search policy and its relationship to the owning
    /// case's actual analysis limits. This does not admit or run a search.
    pub(crate) fn validate_for_config(
        &self,
        config: &CircularAnalysisConfig,
    ) -> Result<(), CoreError> {
        if self.schema_version != 1 {
            return Err(invalid_spec("unsupported schema version"));
        }
        if !(1..=4096).contains(&self.max_candidates)
            || !(1..=3_600_000).contains(&self.max_wall_time_ms)
            || !self.near_tie_abs.is_finite()
            || !self.near_tie_rel.is_finite()
            || !(0.0..=1e-2).contains(&self.near_tie_abs)
            || !(0.0..=1e-2).contains(&self.near_tie_rel)
        {
            return Err(invalid_spec("bounded header or tie policy is invalid"));
        }

        validate_axis(self.domain.center_x, false)?;
        validate_axis(self.domain.center_z, false)?;
        validate_axis(self.domain.radius, true)?;
        for range in [self.domain.allowed_entry_x, self.domain.allowed_exit_x]
            .into_iter()
            .flatten()
        {
            if !range.min_m.is_finite() || !range.max_m.is_finite() || range.min_m > range.max_m {
                return Err(invalid_spec(
                    "closed x range must have finite ordered bounds",
                ));
            }
        }

        let exploration = self.exploration_slices;
        if exploration.initial_requested == 0
            || !(2..=16).contains(&exploration.max_passes)
            || !exploration.require_joint_consecutive_convergence
            || self.refinement.levels > 8
            || self.refinement.promising_per_method > 16
            || (self.refinement.levels > 0 && self.refinement.promising_per_method < 2)
        {
            return Err(invalid_spec(
                "exploration or refinement policy is unsupported",
            ));
        }

        let shift = 1_u32
            .checked_shl(exploration.max_passes - 1)
            .ok_or_else(|| invalid_spec("maximum exploration count overflows"))?;
        let max_exploration = exploration
            .initial_requested
            .checked_mul(shift)
            .ok_or_else(|| invalid_spec("maximum exploration count overflows"))?;
        validate_pair(self.final_pair, max_exploration, config.limits.max_slices)?;

        if self.sensitivity.additional_slice_pairs.len() > 8 {
            return Err(invalid_spec("too many additional slice pairs"));
        }
        let mut previous_fine = self.final_pair.fine_requested;
        for pair in &self.sensitivity.additional_slice_pairs {
            validate_pair(
                *pair,
                self.final_pair.fine_requested,
                config.limits.max_slices,
            )?;
            if pair.fine_requested <= previous_fine {
                return Err(invalid_spec(
                    "additional fine targets must strictly increase",
                ));
            }
            previous_fine = pair.fine_requested;
        }

        let uniform = u64::from(self.sensitivity.uniform_grid_bisections);
        let additional = u64::try_from(self.sensitivity.additional_slice_pairs.len())
            .map_err(|_| invalid_spec("sensitivity branch count overflows"))?;
        let branch_count = uniform
            .checked_add(additional)
            .and_then(|count| count.checked_add(u64::from(uniform > 0)))
            .ok_or_else(|| invalid_spec("sensitivity branch count overflows"))?;
        if branch_count > 8 {
            return Err(invalid_spec("sensitivity branch cap exceeded"));
        }
        Ok(())
    }
}

fn invalid_spec(reason: &str) -> CoreError {
    CoreError::InvalidDocument(format!(
        "Circular search specification is invalid: {reason}"
    ))
}

fn validate_axis(axis: InclusiveAxis, positive: bool) -> Result<(), CoreError> {
    if !axis.min.is_finite()
        || !axis.max.is_finite()
        || axis.min > axis.max
        || (positive && axis.min <= 0.0)
    {
        return Err(invalid_spec(
            "axis bounds must be finite and ordered; radius must be positive",
        ));
    }
    if axis.min == axis.max {
        if axis.intervals != 0 {
            return Err(invalid_spec(
                "collapsed inclusive axis requires zero intervals",
            ));
        }
        return Ok(());
    }
    if !(1..=4096).contains(&axis.intervals) {
        return Err(invalid_spec(
            "noncollapsed inclusive axis requires 1 to 4096 intervals",
        ));
    }
    let span = axis.max - axis.min;
    let step = span / f64::from(axis.intervals);
    if !span.is_finite() || !step.is_finite() || step <= 0.0 {
        return Err(invalid_spec(
            "inclusive axis span or step is not representable",
        ));
    }
    Ok(())
}

fn validate_pair(
    pair: FinalComparisonPair,
    lower_bound: u32,
    max_slices: usize,
) -> Result<(), CoreError> {
    let max_slices = u32::try_from(max_slices).unwrap_or(u32::MAX);
    if pair.coarse_requested == 0
        || pair.coarse_requested < lower_bound
        || pair.coarse_requested.checked_mul(2) != Some(pair.fine_requested)
        || pair.fine_requested > max_slices
    {
        return Err(invalid_spec(
            "slice pair requires positive coarse, checked fine=2*coarse, ordered lower bound, and case slice limit",
        ));
    }
    Ok(())
}

/// Detached reproducibility request resolved against the actual case and
/// ground source operations. It records no calculation result or acceptance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedCircularSearch {
    pub request: SearchPublicRequestV1,
    #[serde(with = "operation_source_id")]
    pub ground_source_operation_id: OperationId,
    #[serde(with = "operation_source_id")]
    pub case_source_operation_id: OperationId,
}

impl Document {
    /// Resolve a configured search policy against the case's current source
    /// revisions without performing search admission or invoking a solver.
    pub fn resolve_circular_search(
        &self,
        id: AnalysisCaseId,
    ) -> Result<ResolvedCircularSearch, CoreError> {
        self.resolve_circular_search_with_archive_validation(id, false)
    }

    pub(crate) fn resolve_circular_search_after_archive_validation(
        &self,
        id: AnalysisCaseId,
    ) -> Result<ResolvedCircularSearch, CoreError> {
        self.resolve_circular_search_with_archive_validation(id, true)
    }

    /// Editor source projection only. Retained archive integrity is checked by
    /// the separate owned inspector before any archived evidence is displayed.
    /// This projection grants no current calculation or persistence authority.
    pub fn resolve_circular_editor_search(
        &self,
        id: AnalysisCaseId,
    ) -> Result<ResolvedCircularSearch, CoreError> {
        self.verify_circular_editor_sources(id)?;
        self.resolve_circular_search_after_archive_validation(id)
    }

    fn resolve_circular_search_with_archive_validation(
        &self,
        id: AnalysisCaseId,
        archive_checked_separately: bool,
    ) -> Result<ResolvedCircularSearch, CoreError> {
        let case = self
            .circular_case(id)
            .ok_or_else(|| CoreError::InvalidDocument(format!("Unknown circular analysis {id}")))?;
        let spec = case.definition.search_spec.as_ref().ok_or_else(|| {
            CoreError::InvalidDocument(
                "SEARCH_SPEC_NOT_CONFIGURED: circular analysis has no persisted search policy"
                    .into(),
            )
        })?;

        // This existing resolver performs case-local validation, rejects an
        // incomplete ground draft, and binds the actual two source operations.
        let resolved = if archive_checked_separately {
            self.resolve_circular_input_after_archive_validation(id)?
        } else {
            self.resolve_circular_input(id)?
        };
        let mut request = SearchPublicRequestV1::from_specified(
            &resolved.input,
            &resolved.config,
            spec.domain.clone(),
        );
        request.schema_version = spec.schema_version;
        request.mode = SearchMode::SearchCriticalCircle;
        request.domain = spec.domain.clone();
        request.family_policy = spec.family_policy;
        request.target_method = spec.target_method;
        request.exploration_slices = spec.exploration_slices;
        request.final_pair = spec.final_pair;
        request.refinement = spec.refinement;
        request.sensitivity = spec.sensitivity.clone();
        request.core_policy_profile = spec.core_policy_profile;
        request.output_mode = spec.output_mode;
        request.max_candidates = spec.max_candidates;
        request.max_wall_time_ms = spec.max_wall_time_ms;
        request.near_tie_abs = spec.near_tie_abs;
        request.near_tie_rel = spec.near_tie_rel;

        Ok(ResolvedCircularSearch {
            request,
            ground_source_operation_id: resolved.ground_source_operation_id,
            case_source_operation_id: resolved.case_source_operation_id,
        })
    }
}
