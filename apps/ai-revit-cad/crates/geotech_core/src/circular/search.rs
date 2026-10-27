//! SerialOnly bounded critical-circle search. Phase barriers and identities are
//! assigned before filtering; incomplete jobs never publish accepted minima.
use super::{
    analysis::{source_fingerprint, CIRCULAR_SOLVER_VERSION},
    arithmetic_environment::require_search_arithmetic_environment,
    search_contact_filter::enclose_terrain_contacts,
    search_evaluate::{evaluate_fixed, prepare_search_geometry, SearchPassEvaluation},
    search_grid::*, search_meter::{SearchMeterScope, SearchWorkLimits}, search_types::*,
    types::*, validation::{error, validate_config, CResult},
};
use crate::Point2;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{cmp::Ordering as Cmp, collections::{BTreeMap, BTreeSet}, io::{self, Write},
    sync::atomic::{AtomicBool, Ordering}, time::Instant};

const BINDING: &str = "SearchBindingV1";
const MIB: u64 = 1024 * 1024;
const PROFILE_MAX_CANDIDATES: u64 = 4096;
const ENGINE_DEADLINE_REASON: &str = "DeadlineExhausted: critical-circle search deadline";

fn classify_stop_status(error: &CircularError) -> SearchRunStatus {
    match error.code {
        CircularErrorCode::Cancelled => SearchRunStatus::Cancelled,
        CircularErrorCode::StaleResult => SearchRunStatus::Stale,
        CircularErrorCode::ResourceLimit
            if error.message == ENGINE_DEADLINE_REASON
                || super::search_meter::is_meter_deadline_error(error) =>
        {
            SearchRunStatus::DeadlineExhausted
        }
        CircularErrorCode::ResourceLimit => SearchRunStatus::BudgetExhausted,
        _ => SearchRunStatus::InternalFailure,
    }
}

/// The coordinator's reviewed increment grants this private capability. It is
/// deliberately neither deserializable nor constructible through a request.
struct OwnerCapability;
impl OwnerCapability {
    fn provenance(&self) -> OwnerPrerequisiteProvenance {
        OwnerPrerequisiteProvenance {
            decision: "ACCEPT_BOUNDED_CONDITIONAL_PHASE_A_CONTACT_SCREEN_ALLOW_NEXT_IMPLEMENTATION".into(),
            review_status: "PASS_BOUNDED_CONDITIONAL_WITH_DIAGNOSTIC_LIMITATION".into(),
            receipt_path: "artifacts/geotech-circular-v02/independent/root-gate7-contact-binding01.json".into(),
            receipt_sha256: "220c1150f7360edefe7257075727a0a72a86630063c1b4be0d9dc89564dbad96".into(),
            accepted_scope: "Dry static 2D self-weight; accepted conditional Phase A whole-arc policy; bounded serial Phase B implementation; no independent search or engineering acceptance".into(),
        }
    }
}

struct DigestSink<'a> { hash: Sha256, count: u64, limit: u64, cancel: &'a AtomicBool, enforce_meter: bool }
impl Write for DigestSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) { return Err(io::Error::other("Cancelled")); }
        if self.enforce_meter && super::search_meter::check().is_err(){return Err(io::Error::other("Search serialization deadline/work budget"));}
        let next = self.count.checked_add(bytes.len() as u64).ok_or_else(|| io::Error::other("CountOverflow"))?;
        if next > self.limit { return Err(io::Error::other("Search serialization byte budget exhausted")); }
        self.hash.update(bytes); self.count = next; Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
fn digest<T: Serialize + ?Sized>(value: &T, limit: u64, cancel: &AtomicBool) -> CResult<(String, u64)> {
    digest_with_policy(value,limit,cancel,true)
}
fn digest_with_policy<T:Serialize+?Sized>(value:&T,limit:u64,cancel:&AtomicBool,enforce_meter:bool)->CResult<(String,u64)>{
    let mut sink = DigestSink { hash: Sha256::new(), count: 0, limit, cancel, enforce_meter };
    let encoded=serde_json::to_writer(&mut sink,value);
    if enforce_meter{super::search_meter::check()?;}
    encoded.map_err(|e| error(
        if cancel.load(Ordering::Relaxed) { CircularErrorCode::Cancelled } else { CircularErrorCode::ResourceLimit },
        format!("Search streamed serialization: {e}")))?;
    Ok((format!("sha256:{:x}", sink.hash.finalize()), sink.count))
}
fn reason(code: impl Into<String>, stage: &str, method: Option<CircularMethod>, detail: impl Into<String>) -> ReasonRecord {
    let mut detail = detail.into();
    if detail.len() > 384 {
        let mut end = 384; while !detail.is_char_boundary(end) { end -= 1; }
        detail.truncate(end);
    }
    ReasonRecord { code: code.into(), stage: stage.into(), method, detail }
}
fn outcome(state: CheckState) -> CheckOutcome { CheckOutcome { state, reason: None } }
fn failed(state: CheckState, why: ReasonRecord) -> CheckOutcome { CheckOutcome { state, reason: Some(why) } }
fn blank(method: CircularMethod) -> MethodObservation {
    MethodObservation { method, raw_factor: None, accepted_factor: None,
        bishop_convergence_status:None,bishop_physical_status:None,endpoint_statuses:vec![],whole_arc_status:None,
        convergence: outcome(CheckState::NotAssessed), discretization: outcome(CheckState::NotAssessed),
        physical: outcome(CheckState::NotAssessed), own_prerequisites: outcome(CheckState::NotAssessed),
        selected_family_eligibility: outcome(CheckState::NotAssessed), slice_passes: vec![], error: None }
}
fn empty_methods<T>() -> ByMethod<Option<T>> { ByMethod { fellenius: None, bishop: None } }

#[derive(Clone, Copy)]
struct Admission { proposals: u64, main: u64, evaluations: u64, working_bytes: u64 }
fn profile_budgets(max_candidates:u64,max_wall_time_ms:u64)->SearchBudgets {
    SearchBudgets {
        max_generated_proposals: 32768, max_circle_candidates: max_candidates,
        max_candidate_evaluations: 131072, max_refinement_levels: 8, max_axis_points: 4097,
        max_sensitivity_branches: 8, max_total_geometry_preparations: 163840,
        max_total_geometry_work: 200_000_000_000, max_total_slice_rows: 536_870_912,
        max_total_material_integrations: 4_294_967_296,
        max_bishop_function_evaluations_per_pass: 65536,
        max_total_bishop_slice_terms: 35_184_372_088_832,
        max_total_contact_intervals: 1_073_741_824,
        max_total_contact_boundary_evaluations: 34_359_738_368,
        max_total_contact_material_checks: 34_359_738_368,
        max_summary_bytes: 64*MIB, max_working_bytes: 512*MIB,
        max_retained_table_bytes: 256*MIB, max_result_bytes: 256*MIB,
        max_in_flight: 1, max_wall_time_ms,
    }
}
fn bounded_profile(public: SearchPublicRequestV1) -> SearchRequestV1 {
    let budgets=profile_budgets(public.max_candidates,public.max_wall_time_ms);
    SearchRequestV1 { public, budgets, retention: SearchRetention { winner_full_tables: true,
        max_additional_full_tables: 16, max_reason_bytes_per_candidate: 4096,
        max_provenance_origins_per_candidate: 16, max_progress_events: 1024 },
        profile_status: "PROVISIONAL_UNCALIBRATED_SERIAL_V1: explicit implementation/calibration ceilings; not measured capacity".into() }
}
fn validate_pair(pair: FinalComparisonPair, lower: u32, max_slices: usize) -> CResult<()> {
    if pair.coarse_requested == 0 || pair.coarse_requested < lower
        || pair.coarse_requested.checked_mul(2) != Some(pair.fine_requested)
        || pair.fine_requested as usize > max_slices {
        return Err(error(CircularErrorCode::InvalidParameter,
            "Common slice pair requires positive fine=2*coarse, ordered policy lower bound and admitted actual-slice ceiling"));
    }
    Ok(())
}
fn admit(request: &SearchPublicRequestV1, cancel: &AtomicBool) -> CResult<(SearchRequestV1, Admission)> {
    if request.schema_version != 1 || request.search_id.trim().is_empty() || request.search_id.len() > 256
        || request.source_revision.trim().is_empty() || request.source_revision.len() > 256
        || request.max_candidates == 0 || request.max_candidates > PROFILE_MAX_CANDIDATES
        || request.max_wall_time_ms == 0 || request.max_wall_time_ms > 3_600_000
        || !request.near_tie_abs.is_finite() || !request.near_tie_rel.is_finite()
        || !(0.0..=1e-2).contains(&request.near_tie_abs) || !(0.0..=1e-2).contains(&request.near_tie_rel)
        || request.sensitivity.additional_slice_pairs.len() > 8 {
        return Err(error(CircularErrorCode::InvalidParameter, "Search header/profile/string/tie/sensitivity admission failed"));
    }
    let e = request.exploration_slices;
    if e.initial_requested == 0 || !(2..=16).contains(&e.max_passes)
        || !e.require_joint_consecutive_convergence || request.refinement.levels > 8
        || request.refinement.promising_per_method > 16
        || (request.refinement.levels > 0 && request.refinement.promising_per_method < 2) {
        return Err(error(CircularErrorCode::InvalidParameter, "Search exploration or multiple-seed refinement policy is unsupported"));
    }
    let max_exploration = e.initial_requested.checked_mul(1_u32.checked_shl(e.max_passes-1).ok_or_else(count_overflow)?)
        .ok_or_else(count_overflow)?;
    validate_config(&request.shared_analysis.fixed(e.initial_requested))?;
    validate_pair(request.final_pair, max_exploration, request.shared_analysis.limits.max_slices)?;
    let mut previous_fine = request.final_pair.fine_requested;
    for pair in &request.sensitivity.additional_slice_pairs {
        validate_pair(*pair, request.final_pair.fine_requested, request.shared_analysis.limits.max_slices)?;
        if pair.fine_requested <= previous_fine { return Err(error(CircularErrorCode::InvalidParameter,
            "Additional slice pairs require strictly increasing fine targets")); }
        previous_fine = pair.fine_requested;
    }
    for range in [request.domain.allowed_entry_x, request.domain.allowed_exit_x].into_iter().flatten() {
        if !range.min_m.is_finite() || !range.max_m.is_finite() || range.min_m > range.max_m {
            return Err(error(CircularErrorCode::InvalidParameter, "Filter ranges require finite ordered closed bounds"));
        }
    }
    let mut counts = [axis_count(request.domain.center_x)?, axis_count(request.domain.center_z)?, axis_count(request.domain.radius)?];
    if request.domain.radius.min <= 0.0 { return Err(error(CircularErrorCode::InvalidCircle, "Search radii must be positive")); }
    let b=profile_budgets(request.max_candidates,request.max_wall_time_ms);
    if counts.iter().any(|n| *n > b.max_axis_points) { return Err(error(CircularErrorCode::ResourceLimit, "Search axis point cap exceeded")); }
    let grid = checked_mul(checked_mul(counts[0], counts[1])?, counts[2])?;
    let neighborhoods = checked_mul(checked_mul(u64::from(request.refinement.levels),
        checked_mul(2, u64::from(request.refinement.promising_per_method))?)?, 27)?;
    let main = checked_add(grid, neighborhoods)?;
    let mut proposals = main;
    let mut evaluations = checked_mul(main, u64::from(e.max_passes) + 2)?;
    let branch_count = checked_add(checked_add(u64::from(request.sensitivity.uniform_grid_bisections),
        request.sensitivity.additional_slice_pairs.len() as u64)?,u64::from(request.sensitivity.uniform_grid_bisections>0))?;
    if branch_count > u64::from(b.max_sensitivity_branches) { return Err(error(CircularErrorCode::ResourceLimit, "Sensitivity branch cap exceeded")); }
    if request.sensitivity.uniform_grid_bisections > 0 { evaluations = checked_add(evaluations, checked_mul(grid, 2)?)?; }
    for _ in 0..request.sensitivity.uniform_grid_bisections {
        for n in &mut counts { if *n > 1 { *n = checked_add(checked_mul(*n-1, 2)?, 1)?; } }
        if counts.iter().any(|n| *n > b.max_axis_points) { return Err(error(CircularErrorCode::ResourceLimit, "Bisected axis point cap exceeded before allocation")); }
        let size = checked_mul(checked_mul(counts[0], counts[1])?, counts[2])?;
        proposals = checked_add(proposals, size)?;
        evaluations = checked_add(evaluations, checked_mul(size, 2)?)?;
    }
    evaluations = checked_add(evaluations, checked_mul(checked_mul(main, 2)?, request.sensitivity.additional_slice_pairs.len() as u64)?)?;
    let preparations = checked_add(proposals, evaluations)?;
    let ground = &request.ground;
    if ground.layer_interfaces.len() > request.shared_analysis.limits.max_soil_regions
        || ground.soil_regions.len() > request.shared_analysis.limits.max_soil_regions
        || ground.materials.len() > request.shared_analysis.limits.max_soil_regions
        || ground.section_id.0.len() > 256 { return Err(error(CircularErrorCode::ResourceLimit, "Ground collection/header cap exceeded before allocation")); }
    let vertices = ground.layer_interfaces.iter().try_fold(
        checked_add(ground.terrain_polyline.len() as u64, ground.bottom_boundary.len() as u64)?,
        |sum, i| checked_add(sum, i.polyline.len() as u64))?;
    if vertices > request.shared_analysis.limits.max_boundary_points as u64 || vertices > 32768
        || ground.materials.iter().any(|v| v.material_id.0.len() > 256
            ||v.properties.name.len()>4096||v.properties.provenance.len()>4096)
        || ground.soil_regions.iter().any(|v| v.region_id.0.len() > 256 || v.material_id.0.len() > 256)
        || ground.layer_interfaces.iter().any(|v| v.interface_id.0.len() > 256) {
        return Err(error(CircularErrorCode::ResourceLimit, "Ground vertex/string cap exceeded before allocation"));
    }
    let regions = ground.soil_regions.len() as u64;
    let geometry_intervals = checked_add(checked_mul(vertices,3)?,2)?;
    let actual_slice_bound=checked_add(u64::from(previous_fine),geometry_intervals)?
        .min(request.shared_analysis.limits.max_slices as u64);
    let rows=checked_mul(evaluations,actual_slice_bound)?;
    let materials = checked_add(checked_mul(rows, regions)?, checked_mul(checked_mul(preparations, geometry_intervals)?, regions)?)?;
    // Covers validation/crossing/classification and bounded comparator work;
    // actual numerical hooks remain authoritative and fail closed on a breach.
    let per_preparation = checked_mul(checked_mul(checked_add(vertices, 16)?, checked_add(vertices, 16)?)?, 64)?;
    let slice_sort_bound=checked_mul(evaluations,checked_mul(checked_add(actual_slice_bound,1)?,checked_add(actual_slice_bound,1)?)?)?;
    let geometry_work = checked_add(checked_add(checked_mul(preparations, per_preparation)?,
        checked_mul(checked_mul(rows, checked_add(vertices, 16)?)?, 32)?)?,checked_mul(slice_sort_bound,2)?)?;
    let bishop_terms = checked_mul(rows, u64::from(b.max_bishop_function_evaluations_per_pass))?;
    let contact = request.shared_analysis.contact;
    let contact_intervals = checked_mul(evaluations, contact.max_intervals as u64)?;
    let contact_boundaries = checked_mul(evaluations, contact.max_boundary_evaluations as u64)?;
    let contact_materials = checked_mul(evaluations, contact.max_material_checks as u64)?;
    let summary_per_candidate = checked_add(checked_add(16384, checked_mul(u64::from(e.max_passes)*2+4,
        2048)?)?,checked_mul(branch_count,8192)?)?;
    let summary = checked_add(checked_mul(proposals, summary_per_candidate)?, checked_mul(proposals, 1536)?)?;
    let max_id_bytes=ground.materials.iter().map(|v|v.material_id.0.len() as u64).max().unwrap_or(0);
    let escaped_id_bytes=checked_add(checked_mul(max_id_bytes,6)?,2)?;
    // This per-row envelope covers Slice/material allocations, independent
    // Fellenius and Bishop rows, force diagnostics, native Vec capacities and
    // worst-case JSON escaping of the actual admitted material identifiers.
    let per_row=checked_add(checked_add(6144,checked_mul(escaped_id_bytes,2)?)?,
        checked_mul(regions,checked_add(128,escaped_id_bytes)?)?)?;
    let table_rows=checked_mul(actual_slice_bound,per_row)?;
    let report_interval_bytes=checked_add(4096,checked_mul(regions,checked_add(128,escaped_id_bytes)?)?)?;
    let contact_details=checked_mul(contact.max_report_intervals as u64,report_interval_bytes)?;
    let table_bytes=checked_add(checked_add(table_rows,contact_details)?,checked_add(checked_mul(vertices,256)?,
        checked_add(checked_mul(request.shared_analysis.limits.max_solver_iterations as u64,512)?,131072)?)?)?;
    let retained_count = checked_add(18, checked_mul(branch_count, 2)?)?;
    let retained = checked_mul(retained_count, table_bytes)?;
    let mut input_string_bytes=checked_add(request.search_id.len() as u64,request.source_revision.len() as u64)?;
    input_string_bytes=checked_add(input_string_bytes,ground.section_id.0.len() as u64)?;
    for v in &ground.materials{input_string_bytes=checked_add(input_string_bytes,checked_add(v.material_id.0.len() as u64,
        checked_add(v.properties.name.len() as u64,v.properties.provenance.len() as u64)?)?)?;}
    for v in &ground.soil_regions{input_string_bytes=checked_add(input_string_bytes,checked_add(v.region_id.0.len() as u64,v.material_id.0.len() as u64)?)?;}
    for v in &ground.layer_interfaces{input_string_bytes=checked_add(input_string_bytes,v.interface_id.0.len() as u64)?;}
    let metadata_items=checked_add(checked_add(ground.materials.len() as u64,regions)?,checked_add(ground.layer_interfaces.len() as u64,8)?)?;
    let input_bytes=checked_add(checked_mul(vertices,128)?,checked_add(checked_mul(input_string_bytes,6)?,checked_mul(metadata_items,2048)?)?)?;
    let working = checked_add(checked_add(summary, retained)?, checked_add(checked_mul(table_bytes, 3)?, checked_mul(input_bytes, 8)?)?)?;
    for (actual, cap, name) in [
        (proposals,b.max_generated_proposals,"generated proposals"),(proposals,b.max_circle_candidates,"conservative unique candidate bound"),
        (evaluations,b.max_candidate_evaluations,"fixed pass evaluations"),(preparations,b.max_total_geometry_preparations,"geometry preparations"),
        (geometry_work,b.max_total_geometry_work,"geometry work"),(rows,b.max_total_slice_rows,"actual slice rows"),
        (materials,b.max_total_material_integrations,"material integrations"),(bishop_terms,b.max_total_bishop_slice_terms,"Bishop slice terms"),
        (contact_intervals,b.max_total_contact_intervals,"contact intervals"),(contact_boundaries,b.max_total_contact_boundary_evaluations,"contact boundary work"),
        (contact_materials,b.max_total_contact_material_checks,"contact material work"),(summary,b.max_summary_bytes,"summary bytes"),
        (retained,b.max_retained_table_bytes,"retained table bytes"),(working,b.max_working_bytes,"working allocation bound"),
        (checked_add(summary,retained)?,b.max_result_bytes,"result bound") ] {
        if actual > cap || usize::try_from(actual).is_err() { return Err(error(CircularErrorCode::ResourceLimit,
            format!("Combined search admission exceeded {name}: {actual} > {cap}"))); }
    }
    // Canonicalize every finite signed zero in the complete effective binding,
    // after collection/combined-allocation bounds and before any grid allocation.
    fn zeros(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Number(n) if n.is_f64() && n.as_f64() == Some(0.0) => {
                *n = serde_json::Number::from_f64(0.0).expect("finite zero");
            }
            serde_json::Value::Array(a) => for v in a { zeros(v); },
            serde_json::Value::Object(a) => for v in a.values_mut() { zeros(v); }, _ => {}
        }
    }
    let mut effective=bounded_profile(request.clone());
    let mut value = serde_json::to_value(&effective).map_err(|e| error(CircularErrorCode::InvalidParameter, e.to_string()))?;
    zeros(&mut value);
    effective = serde_json::from_value(value).map_err(|e| error(CircularErrorCode::InvalidParameter,
        format!("Nonfinite/invalid effective search input: {e}")))?;
    for axis in [effective.public.domain.center_x, effective.public.domain.center_z, effective.public.domain.radius] {
        validate_axis_stream(axis, cancel)?;
    }
    Ok((effective, Admission { proposals, main, evaluations, working_bytes: working }))
}

struct Candidate {
    summary: CandidateSummary, geometry_ready: bool, filter_ready: bool,
    exploration: ByMethod<MethodObservation>,
}
struct StoredTable { table: BoundCandidateTable, bytes: u64 }
struct Branch {
    ids: Vec<CandidateId>, observations: Vec<BranchCandidateObservation>,
    minima: ByMethod<Option<MethodMinimum>>, eligible: Vec<CandidateId>,
}
struct Engine<'a> {
    result: SearchResultV1, candidates: Vec<Candidate>, indices: BTreeMap<CandidateId, usize>,
    exact: BTreeMap<(u64,u64,u64), CandidateId>, tables: Vec<StoredTable>,
    evaluated_ids: BTreeSet<CandidateId>,
    extra_tables: u32, cancel: &'a AtomicBool, progress: &'a mut dyn FnMut(SearchProgress),
    started: Instant, source: String,
}
impl Engine<'_> {
    fn check(&self) -> CResult<()> {
        if self.cancel.load(Ordering::Relaxed) { return Err(error(CircularErrorCode::Cancelled,"Critical-circle search cancelled")); }
        if self.started.elapsed().as_millis() >= u128::from(self.result.request.budgets.max_wall_time_ms) {
            return Err(error(CircularErrorCode::ResourceLimit,ENGINE_DEADLINE_REASON));
        }
        // Progress callbacks execute on this owned worker; reject an altered
        // arithmetic environment before resuming any numerical operation.
        require_search_arithmetic_environment()?;
        Ok(())
    }
    fn stop(&mut self,e:CircularError,stage:&str){
        self.result.accepted=false;self.result.diagnostic_only=true;self.result.minima=empty_methods();
        self.result.conclusion=SearchConclusion::IncompleteDiagnostic;
        self.result.status=classify_stop_status(&e);
        self.result.terminal_reason=Some(reason(format!("{:?}",e.code),stage,None,e.message));
        for branch in &mut self.result.sensitivity{branch.minima=empty_methods();branch.status="IncompleteDiagnostic".into();}
    }
    fn emit(&mut self, stage: &str, terminal: Option<SearchRunStatus>) {
        let limit = self.result.request.retention.max_progress_events;
        if terminal.is_none() && self.result.counters.progress_events >= limit.saturating_sub(1) {
            self.result.counters.progress_events_coalesced += 1; return;
        }
        self.result.counters.progress_events += 1;
        (self.progress)(SearchProgress { stage: stage.into(),
            generated_proposals: self.result.counters.generated_proposals,
            unique_candidates: self.result.counters.unique_candidates,
            completed_evaluations: self.result.counters.candidate_evaluations,
            evaluated_candidates: self.result.counters.evaluated_candidates,
            actual_slice_rows: self.result.counters.actual_slice_rows, terminal_status: terminal });
    }
    fn input(&self, index: usize) -> CircularAnalysisInput {
        let p = &self.result.request.public; let candidate = &self.candidates[index].summary;
        CircularAnalysisInput { analysis_id: format!("{}:candidate:{}",p.search_id,candidate.id),
            source_revision: p.source_revision.clone(), ground: p.ground.clone(),
            slip_surface: CircularSlipSurface { slip_id: CircularSlipId(format!("{}:circle:{}",p.search_id,candidate.id)),
                center: candidate.center, radius_m: candidate.radius_m, arc_branch: ArcBranch::Lower,
                slide_direction: p.direction, selected_endpoints: None }, basis:p.basis, units:p.units,
            extrusion_width_m:p.extrusion_width_m }
    }
    fn propose(&mut self, point: [f64;3], origin: ProposalOrigin, branch: &str,
        main: bool, clipped: [bool;3]) -> CResult<(CandidateId,bool)> {
        self.check()?;
        let b = self.result.request.budgets;
        if self.result.counters.generated_proposals >= b.max_generated_proposals {
            return Err(error(CircularErrorCode::ResourceLimit,"Generated proposal budget exhausted"));
        }
        let proposal_id = self.result.counters.generated_proposals;
        self.result.counters.generated_proposals += 1;
        let point = point.map(canonical); let tuple = (point[0].to_bits(),point[1].to_bits(),point[2].to_bits());
        let (id,unique) = if let Some(id) = self.exact.get(&tuple).copied() {
            self.result.counters.duplicate_proposals += 1;
            let index = self.indices[&id]; let existing = &mut self.candidates[index].summary;
            if existing.origins.len() < self.result.request.retention.max_provenance_origins_per_candidate as usize {
                existing.origins.push(origin.clone());
            } else { existing.truncated_origins += 1; }
            if !existing.branches.iter().any(|v| v == branch) { existing.branches.push(branch.into()); }
            existing.main_family_member |= main;
            for (old,new) in existing.boundary.neighborhood_clipped.iter_mut().zip(clipped) { *old |= new; }
            (id,false)
        } else {
            if self.candidates.len() as u64 >= b.max_circle_candidates { return Err(error(CircularErrorCode::ResourceLimit,"Unique circle budget exhausted")); }
            let p = &self.result.request.public;
            let ground_fingerprint = digest(&(BINDING,&p.ground),b.max_summary_bytes,self.cancel)?.0;
            let summary = CandidateSummary { id:proposal_id,
                key:CandidateKey {center_x_bits:tuple.0,center_z_bits:tuple.1,radius_bits:tuple.2,
                    branch:"lower".into(),direction:match p.direction {SlideDirection::PositiveX=>"positive_x",SlideDirection::NegativeX=>"negative_x"}.into(),ground_fingerprint},
                center:Point2::new(point[0],point[1]),radius_m:point[2],first_origin:proposal_id,
                origins:vec![origin.clone()],truncated_origins:0,branches:vec![branch.into()],main_family_member:main,
                contacts:None,geometry:outcome(CheckState::NotAssessed),entry_filter:outcome(CheckState::NotAssessed),exit_filter:outcome(CheckState::NotAssessed),
                fellenius:blank(CircularMethod::FelleniusOrdinary),bishop:blank(CircularMethod::BishopSimplified),
                exploratory_fellenius:blank(CircularMethod::FelleniusOrdinary),exploratory_bishop:blank(CircularMethod::BishopSimplified),
                final_status:if main {CandidateStatus::NotEvaluatedBudget}else{CandidateStatus::NotInMainFamily},reasons:vec![],
                boundary:BoundaryFlags {original_box:box_flags(point,&p.domain),neighborhood_clipped:clipped,..BoundaryFlags::default()},final_input_fingerprint:None };
            self.indices.insert(proposal_id,self.candidates.len()); self.exact.insert(tuple,proposal_id);
            self.candidates.push(Candidate {summary,geometry_ready:false,filter_ready:false,
                exploration:ByMethod {fellenius:blank(CircularMethod::FelleniusOrdinary),bishop:blank(CircularMethod::BishopSimplified)}});
            self.result.counters.unique_candidates += 1; (proposal_id,true)
        };
        self.result.proposal_outcomes.push(ProposalOutcome {proposal_id,origin,
            disposition:if unique {ProposalDisposition::Unique{candidate_id:id}}else{ProposalDisposition::DuplicateOf{candidate_id:id}},
            branch:branch.into(),clipped});
        Ok((id,unique))
    }
    fn prepare(&mut self, index: usize, stage: &str) -> CResult<()> {
        self.check()?;
        let input = self.input(index);
        let cfg = self.result.request.public.shared_analysis.fixed(self.result.request.public.exploration_slices.initial_requested);
        let geometry = match prepare_search_geometry(&input,&cfg,self.cancel) {
            Ok(value)=>value,
            Err(e) if e.code == CircularErrorCode::Cancelled || e.code == CircularErrorCode::ResourceLimit => return Err(e),
            Err(e)=> {
                let why = reason(format!("{:?}",e.code),stage,None,e.message);
                let candidate = &mut self.candidates[index].summary;
                candidate.geometry = failed(CheckState::Rejected,why.clone()); candidate.reasons.push(why);
                candidate.final_status=CandidateStatus::GeometryRejected; return Ok(());
            }
        };
        self.candidates[index].geometry_ready=true;
        let p = &self.result.request.public;
        let filters = p.domain.allowed_entry_x.is_some() || p.domain.allowed_exit_x.is_some();
        let enclosures = if filters {enclose_terrain_contacts(&input,&cfg,&geometry,self.cancel)?}else{None};
        let (entry,exit,scope) = if let Some(e)=enclosures {
            let (a,b)=match p.direction {SlideDirection::PositiveX=>(e.left_x,e.right_x),SlideDirection::NegativeX=>(e.right_x,e.left_x)};
            (Some(NumericalXInterval {lower_m:a[0],upper_m:a[1]}),Some(NumericalXInterval {lower_m:b[0],upper_m:b[1]}),e.scope)
        } else {(None,None,if filters {"No supported conservative contact-X enclosure; filter inclusion unresolved"}else{"No contact-X filter requested; nominal resolved geometry only"}.into())};
        let geometry_fingerprint=digest(&(BINDING,&input,&cfg,&geometry),self.result.request.budgets.max_result_bytes,self.cancel)?.0;
        let contacts=ResolvedContacts {entry:ResolvedContact::from_search_geometry(geometry.circle.entry_point,entry,filters),
            exit:ResolvedContact::from_search_geometry(geometry.circle.exit_point,exit,filters),uncertainty_scope:scope,
            geometry_evaluation_fingerprint:geometry_fingerprint};
        let (entry_check,entry_edge)=filter_check(entry,p.domain.allowed_entry_x,"entry",stage);
        let (exit_check,exit_edge)=filter_check(exit,p.domain.allowed_exit_x,"exit",stage);
        let ready=entry_check.state==CheckState::Passed && exit_check.state==CheckState::Passed;
        let rejected=entry_check.state==CheckState::Rejected || exit_check.state==CheckState::Rejected;
        let candidate=&mut self.candidates[index]; candidate.filter_ready=ready;
        candidate.summary.contacts=Some(contacts); candidate.summary.geometry=outcome(CheckState::Passed);
        candidate.summary.entry_filter=entry_check;candidate.summary.exit_filter=exit_check;
        candidate.summary.boundary.entry_filter_edge=entry_edge;candidate.summary.boundary.exit_filter_edge=exit_edge;
        if !ready {candidate.summary.final_status=if rejected {CandidateStatus::FilterRejected}else{CandidateStatus::FilterUnresolved};}
        Ok(())
    }
    fn evaluate(&mut self,index:usize,target:u32,stage:&str)->CResult<SearchPassEvaluation> {
        self.check()?;
        if self.result.counters.candidate_evaluations>=self.result.request.budgets.max_candidate_evaluations {
            return Err(error(CircularErrorCode::ResourceLimit,"Fixed-pass evaluation budget exhausted"));
        }
        self.result.counters.candidate_evaluations+=1;
        let input=self.input(index);let cfg=self.result.request.public.shared_analysis.fixed(target);
        let value=evaluate_fixed(&input,&cfg,target as usize,self.cancel);
        if let Ok(v)=&value {
            self.result.counters.actual_slice_rows=checked_add(self.result.counters.actual_slice_rows,v.slices.len() as u64)?;
            if let Ok(b)=&v.bishop {self.result.counters.bishop_iterations=checked_add(self.result.counters.bishop_iterations,b.iterations.len() as u64)?;}
        }
        // This callback follows real work, enabling deterministic in-flight cancellation.
        self.emit(stage,None);self.check()?;value
    }
    fn exploration(&mut self,index:usize,stage:&str)->CResult<()> {
        self.prepare(index,stage)?;
        if !self.candidates[index].geometry_ready || !self.candidates[index].filter_ready {return Ok(());}
        let schedule=self.result.request.public.exploration_slices;
        let mut observations=ByMethod {fellenius:blank(CircularMethod::FelleniusOrdinary),bishop:blank(CircularMethod::BishopSimplified)};
        let mut previous:Option<ByMethod<MethodObservation>>=None;
        let mut history=ByMethod{fellenius:Vec::new(),bishop:Vec::new()};
        for pass in 0..schedule.max_passes {
            self.check()?;let target=schedule.initial_requested.checked_mul(1_u32.checked_shl(pass).ok_or_else(count_overflow)?).ok_or_else(count_overflow)?;
            let current=match self.evaluate(index,target,stage) {
                Ok(v)=>observations_from_pass(&v,target,stage),
                Err(e) if is_job_error(&e)=>return Err(e),
                Err(e)=>observations_from_error(&e,target,stage),
            };
            if let Some(old)=&previous { observations=paired(old,&current,self.result.request.public.shared_analysis.tolerances,stage); }
            else { observations=current.clone(); }
            history.fellenius.extend(current.fellenius.slice_passes.iter().cloned());
            history.bishop.extend(current.bishop.slice_passes.iter().cloned());
            observations.fellenius.slice_passes=history.fellenius.clone();
            observations.bishop.slice_passes=history.bishop.clone();
            if observations.fellenius.accepted_factor.is_some() && observations.bishop.accepted_factor.is_some() {break;}
            previous=Some(current);
        }
        self.candidates[index].exploration=observations.clone();
        self.candidates[index].summary.exploratory_fellenius=observations.fellenius.clone();
        self.candidates[index].summary.exploratory_bishop=observations.bishop.clone();
        self.candidates[index].summary.fellenius=observations.fellenius;
        self.candidates[index].summary.bishop=observations.bishop;
        if self.evaluated_ids.insert(self.candidates[index].summary.id){self.result.counters.evaluated_candidates+=1;}
        self.emit("exploratory_candidate_completed",None);
        self.check()?;
        Ok(())
    }
    fn enumerate_grid(&mut self,grid:&Grid,branch:&str,bisection:u32,explore:bool)->CResult<Vec<CandidateId>> {
        let count=usize::try_from(grid.count()?).map_err(|_|count_overflow())?;
        let mut ids=Vec::new();ids.try_reserve_exact(count).map_err(|_|error(CircularErrorCode::ResourceLimit,"Grid membership allocation failed"))?;
        for (x,cx) in grid.axes[0].iter().enumerate(){for (z,cz) in grid.axes[1].iter().enumerate(){for (r,radius) in grid.axes[2].iter().enumerate(){
            let indices=[x as u32,z as u32,r as u32];
            let origin=if bisection==0 {ProposalOrigin::BaseGrid{indices}}else{ProposalOrigin::UniformGrid{bisection,indices}};
            let (id,unique)=self.propose([*cx,*cz,*radius],origin,branch,explore,[false;3])?;
            ids.push(id);let index=self.indices[&id];
            if unique {if explore {self.exploration(index,branch)?;}else{self.prepare(index,branch)?;}}
        }}}
        Ok(ids)
    }
    fn ranking(&self,method:CircularMethod)->CResult<Vec<CandidateId>> {
        let mut eligible:Vec<_>=self.candidates.iter().filter(|v|v.summary.main_family_member
            && v.exploration.fellenius.accepted_factor.is_some() && v.exploration.bishop.accepted_factor.is_some()).collect();
        super::search_meter::sort_by(&mut eligible,self.cancel,|a,b|compare_factor(factor(&a.exploration,method).unwrap(),&a.summary,
            factor(&b.exploration,method).unwrap(),&b.summary))?;
        Ok(eligible.into_iter().map(|v|v.summary.id).collect())
    }
    fn refine(&mut self)->CResult<()> {
        let policy=self.result.request.public.refinement;
        for level in 1..=policy.levels {
            self.check()?;
            let ff=self.ranking(CircularMethod::FelleniusOrdinary)?;let bishop=self.ranking(CircularMethod::BishopSimplified)?;
            let mut seeds:BTreeSet<_>=ff.iter().take(policy.promising_per_method as usize).chain(bishop.iter().take(policy.promising_per_method as usize)).copied().collect();
            let mut ordered:Vec<_>=seeds.iter().copied().collect();seeds.clear();
            super::search_meter::sort_by(&mut ordered,self.cancel,|a,b|compare_coordinates(&self.candidates[self.indices[a]].summary,&self.candidates[self.indices[b]].summary))?;
            let before=self.result.counters.generated_proposals;
            let mut limitation=None;
            for id in &ordered {
                self.check()?;let candidate=&self.candidates[self.indices[id]].summary;
                let stencil=stencil([candidate.center.x,candidate.center.z,candidate.radius_m],&self.result.request.public.domain,level)?;
                for (point,local_indices,clipped) in stencil {
                    let (id,unique)=self.propose(point,ProposalOrigin::Neighborhood{level,parent:*id,local_indices},"main",true,clipped)?;
                    if unique {let index=self.indices[&id];self.exploration(index,"neighborhood")?;}
                }
            }
            if ff.len()<(policy.promising_per_method as usize) || bishop.len()<(policy.promising_per_method as usize) {
                limitation=Some("Fewer distinct jointly admissible exploratory seeds than requested; available count reported".into());
            }
            self.result.refinement.push(RefinementStageSummary{level,available_fellenius_seeds:ff.len() as u64,
                available_bishop_seeds:bishop.len() as u64,seeds:ordered,
                proposal_count:self.result.counters.generated_proposals-before,limitation:limitation.clone()});
            if limitation.as_ref().is_some_and(|s|s.contains("representable")||s.contains("nonfinite")) {
                return Err(error(CircularErrorCode::DiscretizationNotConverged,limitation.unwrap()));
            }
        }
        Ok(())
    }
    fn make_table(&self,index:usize,branch:&str,target:u32,evaluation:SearchPassEvaluation)->CResult<BoundCandidateTable> {
        let candidate=&self.candidates[index].summary;let config=self.result.request.public.shared_analysis.fixed(target);
        let evaluation_key=digest(&(BINDING,&candidate.key,&self.source,CIRCULAR_SOLVER_VERSION,
            &evaluation.input_fingerprint,&config),self.result.request.budgets.max_result_bytes,self.cancel)?.0;
        let table_hash=digest(&(BINDING,candidate.id,&candidate.key,&self.source,&config,&evaluation),
            self.result.request.budgets.max_result_bytes,self.cancel)?.0;
        Ok(BoundCandidateTable{candidate_id:candidate.id,key:candidate.key.clone(),branch:branch.into(),
            evaluation_key,source_fingerprint:self.source.clone(),table_hash,config,evaluation})
    }
    fn table_present(&self,current:&[StoredTable],key:&str)->bool {
        self.tables.iter().chain(current).any(|v|v.table.evaluation_key==key)
    }
    fn retain(&mut self,table:BoundCandidateTable,current:&mut Vec<StoredTable>,global:bool)->CResult<()> {
        self.check()?;
        if self.table_present(current,&table.evaluation_key){return Ok(());}
        let bytes=digest(&table,self.result.request.budgets.max_retained_table_bytes,self.cancel)?.1;
        let existing=self.tables.iter().chain(current.iter()).try_fold(0u64,|sum,v|checked_add(sum,v.bytes))?;
        let total=checked_add(existing,bytes)?;
        if total>self.result.request.budgets.max_retained_table_bytes {return Err(error(CircularErrorCode::ResourceLimit,"Required winner/additional table retention budget exhausted"));}
        self.result.counters.retained_table_peak_bytes=self.result.counters.retained_table_peak_bytes.max(total);
        let stored=StoredTable{table,bytes};if global{self.tables.push(stored);}else{current.push(stored);}Ok(())
    }
    fn common_pair(&mut self,ids:Vec<CandidateId>,pair:FinalComparisonPair,branch:&str,main:bool)->CResult<Branch> {
        let mut observations=Vec::new();let mut best:ByMethod<Option<(CandidateId,f64,String)>>=empty_methods();
        let mut current_tables:Vec<StoredTable>=Vec::new();let mut eligible=Vec::new();
        for id in &ids {
            self.check()?;let index=self.indices[id];
            if !self.candidates[index].geometry_ready || !self.candidates[index].filter_ready {
                let candidate=&self.candidates[index].summary;
                observations.push(BranchCandidateObservation{candidate_id:*id,fellenius:blank(CircularMethod::FelleniusOrdinary),
                    bishop:blank(CircularMethod::BishopSimplified),status:candidate.final_status});
                continue;
            }
            let coarse=self.evaluate(index,pair.coarse_requested,branch);
            let coarse_observations=match &coarse {Ok(v)=>observations_from_pass(v,pair.coarse_requested,branch),Err(e) if is_job_error(e)=>return Err(e.clone()),Err(e)=>observations_from_error(e,pair.coarse_requested,branch)};
            if main && self.extra_tables<self.result.request.retention.max_additional_full_tables {
                if let Ok(v)=coarse {let table=self.make_table(index,branch,pair.coarse_requested,v)?;self.retain(table,&mut current_tables,true)?;self.extra_tables+=1;}
            }
            let fine=self.evaluate(index,pair.fine_requested,branch);
            let fine_observations=match &fine {Ok(v)=>observations_from_pass(v,pair.fine_requested,branch),Err(e) if is_job_error(e)=>return Err(e.clone()),Err(e)=>observations_from_error(e,pair.fine_requested,branch)};
            let combined=paired(&coarse_observations,&fine_observations,self.result.request.public.shared_analysis.tolerances,branch);
            let joint=combined.fellenius.accepted_factor.is_some()&&combined.bishop.accepted_factor.is_some();
            let status=if joint{CandidateStatus::JointlyEligible}else if combined.fellenius.own_prerequisites.state==CheckState::Unresolved
                ||combined.bishop.own_prerequisites.state==CheckState::Unresolved{CandidateStatus::MethodsUnresolved}else{CandidateStatus::MethodsRejected};
            let mut winner=false;
            if joint {
                eligible.push(*id);
                for method in [CircularMethod::FelleniusOrdinary,CircularMethod::BishopSimplified] {
                    let f=factor(&combined,method).unwrap();let old=method_get(&best,method);
                    let improve=old.as_ref().is_none_or(|(old_id,old_f,_)|compare_factor(f,&self.candidates[index].summary,*old_f,
                        &self.candidates[self.indices[old_id]].summary)==Cmp::Less);
                    if improve {winner=true;*method_get_mut(&mut best,method)=Some((*id,f,String::new()));}
                }
            }
            let extra=main&&self.extra_tables<self.result.request.retention.max_additional_full_tables;
            let mut evaluation_key=None;
            if winner||extra {
                if let Ok(v)=fine {let table=self.make_table(index,branch,pair.fine_requested,v)?;let key=table.evaluation_key.clone();evaluation_key=Some(key.clone());
                    for method in [CircularMethod::FelleniusOrdinary,CircularMethod::BishopSimplified] {
                        if let Some((best_id,_,best_key))=method_get_mut(&mut best,method) {if *best_id==*id{*best_key=key.clone();}}
                    }
                    // Release superseded transient winners before reserving the next.
                    current_tables.retain(|t|[&best.fellenius,&best.bishop].iter().any(|b|b.as_ref().is_some_and(|x|x.2==t.table.evaluation_key)));
                    self.retain(table,&mut current_tables,extra)?;if extra{self.extra_tables+=1;}
                }
            }
            if main {
                let candidate=&mut self.candidates[index].summary;candidate.fellenius=combined.fellenius.clone();candidate.bishop=combined.bishop.clone();
                candidate.final_status=status;candidate.final_input_fingerprint=fine_observations.fellenius.slice_passes.last().map(|v|v.input_fingerprint.clone());
                if candidate.final_input_fingerprint.as_ref().is_some_and(|v|v.is_empty()){candidate.final_input_fingerprint=None;}
            }
            let _=evaluation_key;
            observations.push(BranchCandidateObservation{candidate_id:*id,fellenius:combined.fellenius,bishop:combined.bishop,status});
            if self.evaluated_ids.insert(*id){self.result.counters.evaluated_candidates+=1;}
            self.emit("common_pair_candidate_completed",None);self.check()?;
        }
        // A branch's winner tables become retained evidence for that branch.
        self.tables.extend(current_tables);
        let mut minima=empty_methods();
        for method in [CircularMethod::FelleniusOrdinary,CircularMethod::BishopSimplified] {
            if let Some((id,f,key))=method_get(&best,method) {
                let table=self.tables.iter().find(|t|t.table.evaluation_key==*key).ok_or_else(||error(CircularErrorCode::ResourceLimit,"Winner table evidence missing"))?;
                let tolerance=self.result.request.public.near_tie_abs+self.result.request.public.near_tie_rel*f.abs();
                let near_ties=observations.iter().filter(|v|v.status==CandidateStatus::JointlyEligible).filter_map(|v|{
                    let value=if method==CircularMethod::FelleniusOrdinary{v.fellenius.accepted_factor}else{v.bishop.accepted_factor}?;
                    ((value-f).abs()<=tolerance).then_some(v.candidate_id)}).collect();
                *method_get_mut(&mut minima,method)=Some(MethodMinimum{method,label:if method==CircularMethod::FelleniusOrdinary{
                    "fellenius_minimum_within_jointly_eligible_examined_family"}else{"bishop_minimum_within_jointly_eligible_examined_family"}.into(),
                    candidate_id:*id,fine_factor:*f,table_hash:table.table.table_hash.clone(),evaluation_key:key.clone(),final_pair:pair,
                    near_ties,boundary:self.candidates[self.indices[id]].summary.boundary});
            }
        }
        super::search_meter::sort_by(&mut eligible,self.cancel,|a,b|a.cmp(b))?;
        Ok(Branch{ids,observations,minima,eligible})
    }
    fn sensitivity_record(&self,branch:Branch,name:&str,kind:&str,pair:FinalComparisonPair,baseline:Option<&SensitivityResult>)->CResult<SensitivityResult> {
        let reference=baseline.map(|v|&v.minima).unwrap_or(&self.result.minima);
        let compare=|method|{let a=method_get(reference,method);let b=method_get(&branch.minima,method);
            (a.as_ref().zip(b.as_ref()).map(|(a,b)|a.candidate_id!=b.candidate_id),a.as_ref().zip(b.as_ref()).map(|(a,b)|b.fine_factor-a.fine_factor))};
        let ff=compare(CircularMethod::FelleniusOrdinary);let bb=compare(CircularMethod::BishopSimplified);
        let reference_eligible=baseline.map(|v|v.eligible_candidate_ids.as_slice()).unwrap_or(self.result.examined_family.eligible_candidate_ids.as_slice());
        let reference_ids=baseline.map(|v|v.candidate_ids.as_slice()).unwrap_or(self.result.examined_family.main_candidate_ids.as_slice());
        let common_candidate_ids=branch.ids.iter().copied().filter(|id|reference_ids.contains(id)).collect();
        let eligible_set_hash=digest(&(BINDING,&branch.eligible),self.result.request.budgets.max_summary_bytes,self.cancel)?.0;
        let eligibility_changed=reference_eligible!=branch.eligible.as_slice();
        let empty=branch.eligible.is_empty();
        Ok(SensitivityResult{branch:name.into(),kind:kind.into(),status:if empty{"CompletedNoEligibleCandidates"}else{"Completed"}.into(),final_pair:pair,
            candidate_ids:branch.ids,eligible_candidate_ids:branch.eligible,eligible_set_hash,minima:branch.minima,
            observations:branch.observations,winner_changed:ByMethod{fellenius:ff.0,bishop:bb.0},minimum_factor_delta:ByMethod{fellenius:ff.1,bishop:bb.1},
            eligibility_changed,common_candidate_ids,warnings:vec!["SEARCH_DOMAIN_LIMITED".into(),if empty{"NO_STABILITY_EVIDENCE"}else{"Discretization deltas are not rigorous error bounds"}.into()]})
    }
    fn run(&mut self)->CResult<()> {
        let domain=self.result.request.public.domain.clone();let mut grid=Grid::new(&domain,self.cancel)?;
        let base=self.enumerate_grid(&grid,"main",0,true)?;self.refine()?;
        let main_ids:Vec<_>=self.candidates.iter().filter(|v|v.summary.main_family_member).map(|v|v.summary.id).collect();
        let pair=self.result.request.public.final_pair;
        let branch=self.common_pair(main_ids.clone(),pair,"main",true)?;
        self.result.minima=branch.minima;self.result.examined_family.main_candidate_ids=main_ids.clone();
        self.result.examined_family.eligible_candidate_ids=branch.eligible;
        self.result.examined_family.eligible_set_hash=digest(&(BINDING,&self.result.examined_family.eligible_candidate_ids),self.result.request.budgets.max_summary_bytes,self.cancel)?.0;
        let sensitivity=self.result.request.public.sensitivity.clone();
        if sensitivity.uniform_grid_bisections>0 {
            let baseline=self.common_pair(base,pair,"uniform_grid_baseline",false)?;
            let baseline=self.sensitivity_record(baseline,"uniform_grid_baseline","controlled_original_uniform_grid",pair,None)?;
            for level in 1..=sensitivity.uniform_grid_bisections {
                self.check()?;grid=grid.bisect(self.result.request.budgets.max_axis_points,self.cancel)?;
                let name=format!("uniform_grid_bisection_{level}");let ids=self.enumerate_grid(&grid,&name,level,false)?;
                let branch=self.common_pair(ids,pair,&name,false)?;
                let report=self.sensitivity_record(branch,&name,"complete_nested_uniform_grid",pair,Some(&baseline))?;self.result.sensitivity.push(report);
            }
            self.result.sensitivity.insert(0,baseline);
        }
        for (index,pair) in sensitivity.additional_slice_pairs.into_iter().enumerate() {
            self.check()?;let name=format!("slice_pair_{}",index+1);
            let branch=self.common_pair(main_ids.clone(),pair,&name,false)?;
            let report=self.sensitivity_record(branch,&name,"same_main_geometric_family_finer_slice_pair",pair,None)?;self.result.sensitivity.push(report);
        }
        if self.result.sensitivity.is_empty(){self.result.warnings.push("SENSITIVITY_NOT_REQUESTED".into());}
        self.check()?;
        if source_fingerprint()!=self.source{return Err(error(CircularErrorCode::StaleResult,"Integrated source identity changed before publication"));}
        if self.result.examined_family.eligible_candidate_ids.is_empty(){
            self.result.status=SearchRunStatus::CompletedNoEligibleCandidates;self.result.conclusion=SearchConclusion::NoStabilityEvidence;
            self.result.minima=empty_methods();self.result.warnings.push("NO_STABILITY_EVIDENCE".into());
        }else{
            self.result.status=SearchRunStatus::CompletedMinimum;self.result.conclusion=SearchConclusion::MinimumWithinExaminedCircularFamily;
            self.result.accepted=self.result.request.public.output_mode==SearchOutputMode::Strict;
        }
        Ok(())
    }
    fn finalize(&mut self,work:super::search_meter::SearchWorkCounters,serialize_cancel:&AtomicBool)->CResult<()> {
        self.result.counters.work=work;
        self.result.counters.actual_slice_rows=work.slice_rows;
        self.result.counters.retained_table_bytes=self.tables.iter().try_fold(0u64,|sum,t|checked_add(sum,t.bytes))?;
        self.result.retained_tables=std::mem::take(&mut self.tables).into_iter().map(|t|t.table).collect();
        let mut diagnostic=empty_methods();
        for method in [CircularMethod::FelleniusOrdinary,CircularMethod::BishopSimplified] {
            let mut best:Option<&CandidateSummary>=None;
            for candidate in &self.candidates {
                let observation=if method==CircularMethod::FelleniusOrdinary{&candidate.summary.fellenius}else{&candidate.summary.bishop};
                let Some(f)=observation.accepted_factor else{continue;};
                if best.is_none_or(|b|{let o=if method==CircularMethod::FelleniusOrdinary{&b.fellenius}else{&b.bishop};
                    compare_factor(f,&candidate.summary,o.accepted_factor.unwrap(),b)==Cmp::Less}){best=Some(&candidate.summary);}
            }
            if let Some(best)=best {let observation=if method==CircularMethod::FelleniusOrdinary{best.fellenius.clone()}else{best.bishop.clone()};
                let evaluation_key=if let Some(pass)=observation.slice_passes.last(){
                    let cfg=self.result.request.public.shared_analysis.fixed(pass.requested_slices);
                    digest_with_policy(&(BINDING,&best.key,&self.source,CIRCULAR_SOLVER_VERSION,&pass.input_fingerprint,&cfg),
                        self.result.request.budgets.max_result_bytes,serialize_cancel,self.result.accepted)?.0
                }else{String::new()};
                *method_get_mut(&mut diagnostic,method)=Some(BoundDiagnosticCandidate{candidate_id:best.id,key:best.key.clone(),
                    evaluation_key,source_fingerprint:self.source.clone(),observation});}
        }
        self.result.diagnostic_best_so_far=diagnostic;
        for candidate in &mut self.candidates {
            if candidate.summary.main_family_member {
                match candidate.summary.final_status {
                    CandidateStatus::GeometryRejected=>self.result.counters.geometry_rejected+=1,
                    CandidateStatus::FilterRejected=>self.result.counters.filter_rejected+=1,
                    CandidateStatus::FilterUnresolved=>self.result.counters.filter_unresolved+=1,
                    CandidateStatus::JointlyEligible=>self.result.counters.jointly_eligible+=1,
                    CandidateStatus::MethodsRejected=>self.result.counters.methods_rejected+=1,
                    CandidateStatus::MethodsUnresolved=>self.result.counters.methods_unresolved+=1,
                    _=>{self.result.counters.not_evaluated+=1;candidate.summary.final_status=match self.result.status {
                        SearchRunStatus::Cancelled=>CandidateStatus::Cancelled,SearchRunStatus::Stale=>CandidateStatus::Stale,_=>CandidateStatus::NotEvaluatedBudget};}
                }
                if candidate.summary.fellenius.own_prerequisites.state==CheckState::Passed{self.result.counters.fellenius_own_eligible+=1;}
                if candidate.summary.bishop.own_prerequisites.state==CheckState::Passed{self.result.counters.bishop_own_eligible+=1;}
            }
        }
        self.result.candidates=std::mem::take(&mut self.candidates).into_iter().map(|v|v.summary).collect();
        let live=self.result.accepted;
        let manifest=digest_with_policy(&(BINDING,&self.result.proposal_outcomes,
            self.result.candidates.iter().map(|v|(&v.id,&v.key,&v.branches,&v.final_status)).collect::<Vec<_>>(),
            &self.result.examined_family.eligible_candidate_ids),self.result.request.budgets.max_result_bytes,serialize_cancel,live)?.0;
        self.result.examined_family.proposal_manifest_hash=manifest;
        self.result.counters.summary_bytes=digest_with_policy(&(&self.result.candidates,&self.result.proposal_outcomes,&self.result.sensitivity),
            self.result.request.budgets.max_summary_bytes,serialize_cancel,live)?.1;
        for _ in 0..4 {
            let size=digest_with_policy(&self.result,self.result.request.budgets.max_result_bytes,serialize_cancel,live)?.1;
            if size==self.result.counters.output_bytes{break;}self.result.counters.output_bytes=size;
        }
        Ok(())
    }
}

fn filter_check(enclosure:Option<NumericalXInterval>,range:Option<ClosedXRange>,name:&str,stage:&str)->(CheckOutcome,bool) {
    let Some(range)=range else{return(outcome(CheckState::Passed),false);};
    let Some(x)=enclosure else{return(failed(CheckState::Unresolved,reason("FILTER_UNRESOLVED",stage,None,format!("{name}: no supported conservative X enclosure"))),false);};
    let edge=x.lower_m<=range.min_m&&x.upper_m>=range.min_m || x.lower_m<=range.max_m&&x.upper_m>=range.max_m;
    if x.upper_m<range.min_m||x.lower_m>range.max_m {(failed(CheckState::Rejected,reason("FILTER_EXCLUDED",stage,None,format!("{name}: enclosure disjoint from closed allowed range"))),edge)}
    else if x.lower_m>=range.min_m&&x.upper_m<=range.max_m {(outcome(CheckState::Passed),edge)}
    else {(failed(CheckState::Unresolved,reason("FILTER_UNRESOLVED",stage,None,format!("{name}: conservative enclosure overlaps allowed-range edge"))),edge)}
}
fn is_job_error(e:&CircularError)->bool {matches!(e.code,CircularErrorCode::Cancelled|CircularErrorCode::ResourceLimit|CircularErrorCode::StaleResult)}
fn method_get<T>(methods:&ByMethod<T>,method:CircularMethod)->&T {match method{CircularMethod::FelleniusOrdinary=>&methods.fellenius,CircularMethod::BishopSimplified=>&methods.bishop}}
fn method_get_mut<T>(methods:&mut ByMethod<T>,method:CircularMethod)->&mut T {match method{CircularMethod::FelleniusOrdinary=>&mut methods.fellenius,CircularMethod::BishopSimplified=>&mut methods.bishop}}
fn factor(methods:&ByMethod<MethodObservation>,method:CircularMethod)->Option<f64>{method_get(methods,method).accepted_factor}
fn compare_coordinates(a:&CandidateSummary,b:&CandidateSummary)->Cmp {a.center.x.total_cmp(&b.center.x).then(a.center.z.total_cmp(&b.center.z))
    .then(a.radius_m.total_cmp(&b.radius_m)).then(a.id.cmp(&b.id))}
fn compare_factor(a:f64,ac:&CandidateSummary,b:f64,bc:&CandidateSummary)->Cmp {a.total_cmp(&b).then_with(||compare_coordinates(ac,bc))}
fn observations_from_error(e:&CircularError,target:u32,stage:&str)->ByMethod<MethodObservation> {
    let one=|method|{let mut o=blank(method);let why=reason(format!("{:?}",e.code),stage,Some(method),e.message.clone());
        o.own_prerequisites=failed(CheckState::Unresolved,why.clone());o.error=Some(why);o.slice_passes.push(SlicePassSummary{
            requested_slices:target,actual_slices:0,input_fingerprint:String::new(),raw_factor:None,own_accepted_factor:None,
            closure_passed:false,error_code:Some(e.code),bishop_iterations:0,contact_input_fingerprint:None,
            own_failure:o.error.clone(),bishop_convergence_status:None,bishop_physical_status:None,endpoint_statuses:vec![],whole_arc_status:None});o};
    ByMethod{fellenius:one(CircularMethod::FelleniusOrdinary),bishop:one(CircularMethod::BishopSimplified)}
}
fn observations_from_pass(value:&SearchPassEvaluation,target:u32,stage:&str)->ByMethod<MethodObservation> {
    let mut ff=blank(CircularMethod::FelleniusOrdinary);let mut bishop=blank(CircularMethod::BishopSimplified);
    for method in [CircularMethod::FelleniusOrdinary,CircularMethod::BishopSimplified] {
        let observation=if method==CircularMethod::FelleniusOrdinary{&mut ff}else{&mut bishop};
        let (raw,accepted,error_code,iterations,contact)=match method {
            CircularMethod::FelleniusOrdinary=>match &value.fellenius {
                Ok(v)=>(Some(v.factor_of_safety),Some(v.factor_of_safety),None,0,None),
                Err(e)=>{observation.error=Some(reason(format!("{:?}",e.code),stage,Some(method),e.message.clone()));(None,None,Some(e.code),0,None)}},
            CircularMethod::BishopSimplified=>match &value.bishop {
                Ok(v)=>(Some(v.factor_of_safety),v.accepted_factor_of_safety,None,v.iterations.len() as u64,v.whole_arc_contact.as_ref().map(|v|v.contact_input_fingerprint.clone())),
                Err(e)=>{let root=e.bishop_diagnostic.as_ref().and_then(|d|d.mathematical_root.as_ref());
                    observation.error=Some(reason(format!("{:?}",e.code),stage,Some(method),e.message.clone()));
                    (root.map(|v|v.factor_of_safety).or_else(||e.bishop_diagnostic.as_ref().and_then(|v|v.last_factor)),None,Some(e.code),root.map_or(0,|v|v.iterations.len() as u64),root.and_then(|v|v.whole_arc_contact.as_ref()).map(|v|v.contact_input_fingerprint.clone()))}},
        };
        if method==CircularMethod::BishopSimplified {
            let root=match &value.bishop{Ok(v)=>Some(v),Err(e)=>e.bishop_diagnostic.as_ref().and_then(|v|v.mathematical_root.as_deref())};
            if let Some(root)=root{
                observation.bishop_convergence_status=Some(root.convergence_status);
                observation.bishop_physical_status=Some(root.physical_assessment.status);
                observation.endpoint_statuses=root.endpoint_assessment.as_ref().map(|v|v.endpoints.iter().map(|v|v.status).collect()).unwrap_or_default();
                observation.whole_arc_status=root.whole_arc_contact.as_ref().map(|v|v.status);
            }else if let Err(e)=&value.bishop{observation.bishop_convergence_status=e.bishop_diagnostic.as_ref().map(|v|v.status);}
        }
        let finite_positive=|f:f64|f.is_finite()&&f>0.0;
        observation.raw_factor=raw.filter(|v|v.is_finite());
        let accepted=accepted.filter(|v|finite_positive(*v)).filter(|_|value.closure_passed);
        observation.convergence=outcome(if (method==CircularMethod::FelleniusOrdinary&&raw.is_some()&&error_code.is_none())
            ||observation.bishop_convergence_status==Some(BishopConvergenceStatus::Converged){CheckState::Passed}
            else if matches!(observation.bishop_convergence_status,Some(BishopConvergenceStatus::MaxIterations)){CheckState::Unresolved}else{CheckState::Rejected});
        let unresolved=if method==CircularMethod::BishopSimplified{match &value.bishop{
            Ok(v)=>v.physical_assessment.status==BishopPhysicalStatus::NumericallyIndeterminate
                ||v.whole_arc_contact.as_ref().is_some_and(|v|v.status==WholeArcContactStatus::Unresolved)
                ||v.endpoint_assessment.as_ref().is_some_and(|v|v.endpoints.iter().any(|e|e.status==BishopEndpointStatus::Unresolved)),
            Err(e)=>matches!(e.code,CircularErrorCode::BishopNotConverged|CircularErrorCode::NumericOverflow|CircularErrorCode::ResourceLimit)
        }}else{!value.closure_passed};
        let state=if accepted.is_some(){CheckState::Passed}else if unresolved||!value.closure_passed{CheckState::Unresolved}else{CheckState::Rejected};
        observation.physical=outcome(state);observation.own_prerequisites=outcome(state);
        if accepted.is_none()&&observation.error.is_none(){observation.error=Some(reason(if unresolved{"METHOD_UNRESOLVED"}else{"METHOD_REJECTED"},stage,Some(method),
            if !value.closure_passed{"Independent slice area/weight closure failed"}else if unresolved{"Existing endpoint, finite-table or whole-arc check is unresolved"}else{"Existing own-method physical or factor check rejected the result"}));}
        observation.slice_passes.push(SlicePassSummary{requested_slices:target,actual_slices:value.slices.len() as u64,
            input_fingerprint:value.input_fingerprint.clone(),raw_factor:observation.raw_factor,own_accepted_factor:accepted,
            closure_passed:value.closure_passed,error_code,bishop_iterations:iterations,contact_input_fingerprint:contact,
            own_failure:observation.error.clone(),bishop_convergence_status:observation.bishop_convergence_status,
            bishop_physical_status:observation.bishop_physical_status,endpoint_statuses:observation.endpoint_statuses.clone(),
            whole_arc_status:observation.whole_arc_status});
        // A single fixed pass cannot establish search discretization acceptance.
        observation.accepted_factor=None;
    }
    ByMethod{fellenius:ff,bishop}
}
fn paired(coarse:&ByMethod<MethodObservation>,fine:&ByMethod<MethodObservation>,t:NumericalTolerances,stage:&str)->ByMethod<MethodObservation> {
    let one=|method|{let old=method_get(coarse,method);let new=method_get(fine,method);let mut observation=new.clone();
        observation.slice_passes=old.slice_passes.iter().rev().take(1).cloned().chain(new.slice_passes.iter().rev().take(1).cloned()).collect();
        let a=old.slice_passes.last().and_then(|v|v.own_accepted_factor);let b=new.slice_passes.last().and_then(|v|v.own_accepted_factor);
        let converged=a.zip(b).is_some_and(|(a,b)|(a-b).abs()<=t.fs_abs+t.fs_rel*b.abs());
        observation.discretization=if converged{outcome(CheckState::Passed)}else{failed(CheckState::Unresolved,
            reason("DISCRETIZATION_NOT_CONVERGED",stage,Some(method),"Both own-method checks and consecutive common-pair factor tolerance are required"))};
        observation.accepted_factor=if converged{b}else{None};
        if !converged&&observation.own_prerequisites.state==CheckState::Passed{observation.own_prerequisites=outcome(CheckState::Unresolved);}
        observation};
    let mut methods=ByMethod{fellenius:one(CircularMethod::FelleniusOrdinary),bishop:one(CircularMethod::BishopSimplified)};
    let joint=methods.fellenius.accepted_factor.is_some()&&methods.bishop.accepted_factor.is_some();
    for method in [CircularMethod::FelleniusOrdinary,CircularMethod::BishopSimplified] {
        method_get_mut(&mut methods,method).selected_family_eligibility=if joint{outcome(CheckState::Passed)}else{failed(CheckState::Rejected,
            reason("JOINT_FAMILY_EXCLUDED",stage,Some(method),"JointlyEligibleV1 requires both independent methods on this same pair"))};
    }
    methods
}

pub fn search_critical_circle(request:&SearchPublicRequestV1)->CResult<SearchResultV1> {
    search_critical_circle_cancellable(request,&AtomicBool::new(false),&mut |_|{})
}
pub fn search_critical_circle_cancellable(request:&SearchPublicRequestV1,cancel:&AtomicBool,
    progress:&mut dyn FnMut(SearchProgress))->CResult<SearchResultV1> {
    let started=Instant::now();
    let arithmetic_environment=require_search_arithmetic_environment()?;
    let (effective,admission)=admit(request,cancel)?;
    let capability=OwnerCapability;let source=source_fingerprint();
    let request_fingerprint=digest(&(BINDING,&effective),effective.budgets.max_summary_bytes,cancel)?.0;
    let b=effective.budgets;
    let scope=SearchMeterScope::enter(SearchWorkLimits {geometry_preparations:b.max_total_geometry_preparations,
        geometry_work:b.max_total_geometry_work,slice_rows:b.max_total_slice_rows,material_integrations:b.max_total_material_integrations,
        bishop_function_evaluations_per_pass:u64::from(b.max_bishop_function_evaluations_per_pass),bishop_slice_terms:b.max_total_bishop_slice_terms,
        contact_intervals:b.max_total_contact_intervals,contact_boundary_evaluations:b.max_total_contact_boundary_evaluations,
        contact_material_checks:b.max_total_contact_material_checks},b.max_wall_time_ms)?;
    let result=SearchResultV1 {schema_version:1,mode:SearchMode::SearchCriticalCircle,status:SearchRunStatus::InternalFailure,
        conclusion:SearchConclusion::IncompleteDiagnostic,accepted:false,diagnostic_only:true,
        request:effective,request_fingerprint,source_fingerprint:source.clone(),solver_version:CIRCULAR_SOLVER_VERSION.into(),
        prerequisite_provenance:capability.provenance(),arithmetic_environment,execution_policy:"SerialOnly".into(),parallel_determinism:"NotTested".into(),
        examined_family:FamilyManifest{binding_version:BINDING.into(),proposal_manifest_hash:String::new(),eligible_set_hash:String::new(),
            main_candidate_ids:vec![],eligible_candidate_ids:vec![],classification:"minimum_within_examined_circular_family".into()},
        family_policy:FamilyPolicy::JointlyEligibleV1,counters:SearchCounters{admitted_proposals_upper_bound:admission.proposals,
            admitted_evaluations_upper_bound:admission.evaluations,working_upper_bound_bytes:admission.working_bytes,..SearchCounters::default()},
        candidates:vec![],proposal_outcomes:vec![],minima:empty_methods(),diagnostic_best_so_far:empty_methods(),retained_tables:vec![],
        refinement:vec![],sensitivity:vec![],warnings:vec!["SEARCH_DOMAIN_LIMITED".into(),"DRY_STATIC_2D_SELF_WEIGHT_ONLY".into(),
            "ENGINEERING_REVIEW_REQUIRED".into(),"Computed finite-table/contact/discretization evidence is conditional, not an exact continuum certificate".into()],terminal_reason:None};
    let mut engine=Engine{result,candidates:vec![],indices:BTreeMap::new(),exact:BTreeMap::new(),tables:vec![],evaluated_ids:BTreeSet::new(),extra_tables:0,
        cancel,progress,started,source};
    let operation=engine.run();
    if let Err(e)=operation {engine.stop(e,"terminal");}
    engine.result.diagnostic_only=!engine.result.accepted;
    if !engine.result.accepted{engine.result.minima=empty_methods();
        for branch in &mut engine.result.sensitivity{branch.minima=empty_methods();}}
    // This is the final cancellable progress callback. The later terminal
    // callback only observes the completed immutable core decision.
    engine.emit("publication_ready",None);
    if matches!(engine.result.status,SearchRunStatus::CompletedMinimum|SearchRunStatus::CompletedNoEligibleCandidates){
        if let Err(e)=engine.check(){engine.stop(e,"publication_ready");}
    }
    // Terminal diagnostics must remain serializable after cancellation. A fresh
    // local token is only used for serialization of already stopped work.
    let stopped=AtomicBool::new(false);
    let serialize_live=engine.result.accepted;
    let finalized=engine.finalize(scope.snapshot(),if serialize_live{cancel}else{&stopped});
    if let Err(e)=finalized {
        engine.stop(e,"serialization");
        // Releasing tables reduces diagnostic output deterministically; accepted
        // publication is already forbidden and the missing evidence is explicit.
        engine.result.retained_tables.clear();engine.result.counters.retained_table_bytes=0;
    }
    // Reserve the one terminal notification before the final counting sink.
    engine.result.counters.progress_events+=1;
    fn count_final(engine:&mut Engine<'_>,cancel:&AtomicBool,live:bool)->CResult<()>{
        for _ in 0..8{
            let size=digest_with_policy(&engine.result,engine.result.request.budgets.max_result_bytes,cancel,live)?.1;
            if size==engine.result.counters.output_bytes{return Ok(());}
            engine.result.counters.output_bytes=size;
        }
        Err(error(CircularErrorCode::ResourceLimit,"Final output byte count did not stabilize"))
    }
    let live=engine.result.accepted;
    if let Err(e)=count_final(&mut engine,if live{cancel}else{&stopped},live){
        engine.stop(e,"final_serialization");engine.result.retained_tables.clear();engine.result.counters.retained_table_bytes=0;
        count_final(&mut engine,&stopped,false)?;
    }
    if matches!(engine.result.status,SearchRunStatus::CompletedMinimum|SearchRunStatus::CompletedNoEligibleCandidates){
        if let Err(e)=engine.check(){engine.stop(e,"core_commit");count_final(&mut engine,&stopped,false)?;}
    }
    // Core decision commit: the following observer cannot rewrite this result.
    // Its token changes belong to a later owner publication/job decision.
    let terminal=SearchProgress{stage:"terminal".into(),generated_proposals:engine.result.counters.generated_proposals,
        unique_candidates:engine.result.counters.unique_candidates,completed_evaluations:engine.result.counters.candidate_evaluations,
        evaluated_candidates:engine.result.counters.evaluated_candidates,actual_slice_rows:engine.result.counters.actual_slice_rows,
        terminal_status:Some(engine.result.status.clone())};
    (engine.progress)(terminal);
    let _=admission.main;
    Ok(engine.result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_meter_charge_maps_to_deadline_exhausted() {
        let limits = SearchWorkLimits {
            geometry_preparations: u64::MAX,
            geometry_work: u64::MAX,
            slice_rows: u64::MAX,
            material_integrations: u64::MAX,
            bishop_function_evaluations_per_pass: u64::MAX,
            bishop_slice_terms: u64::MAX,
            contact_intervals: u64::MAX,
            contact_boundary_evaluations: u64::MAX,
            contact_material_checks: u64::MAX,
        };
        let _scope = super::super::search_meter::SearchMeterScope::enter_expired_for_test(limits).unwrap();
        let propagated = super::super::search_meter::charge(
            super::super::search_meter::WorkKind::GeometryWork,
            1,
        ).unwrap_err();

        assert_eq!(propagated.code, CircularErrorCode::ResourceLimit);
        assert!(super::super::search_meter::is_meter_deadline_error(&propagated));
        assert_eq!(classify_stop_status(&propagated), SearchRunStatus::DeadlineExhausted);
        assert_eq!(classify_stop_status(&CircularError::new(
            CircularErrorCode::ResourceLimit,
            "An ordinary budget message mentions a deadline",
        )), SearchRunStatus::BudgetExhausted);
        assert_eq!(classify_stop_status(&CircularError::new(
            CircularErrorCode::InvalidParameter,
            ENGINE_DEADLINE_REASON,
        )), SearchRunStatus::InternalFailure);
        assert_eq!(classify_stop_status(&CircularError::new(
            CircularErrorCode::Cancelled,
            ENGINE_DEADLINE_REASON,
        )), SearchRunStatus::Cancelled);
    }
}
