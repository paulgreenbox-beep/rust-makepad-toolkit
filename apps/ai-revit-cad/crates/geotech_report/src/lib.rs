//! Isolated pure archived-evidence report draft. No solver or Document calls.
use geotech_core::circular::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
mod finite;
mod preflight;
mod render;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod regression_tests;

pub const ARCHIVE_FORMAT: &str = "circular-engineering-report-archive-v1";
pub const FORMATTER_VERSION: &str = "circular-engineering-markdown-v1";
pub const INPUT_DIGEST_ALGORITHM: &str = "archived-solver-input-config-json-tuple-v1";
const SEARCH_BINDING: &str = "SearchBindingV1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    NonFinite,
    InputBudget,
    OutputBudget,
    ArchiveBudget,
    InvalidArchive,
    UnsupportedFormat,
    Binding(&'static str),
    MissingWinnerTable(u64),
}
#[derive(Debug, Clone, Copy)]
pub struct ReportLimits {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub archive_bytes: usize,
    pub string_bytes: usize,
    pub collection_items: usize,
    pub nesting: usize,
}
impl Default for ReportLimits {
    fn default() -> Self {
        Self { input_bytes: 64 << 20, output_bytes: 64 << 20,
            archive_bytes: 160 << 20, string_bytes: 16 << 10,
            collection_items: 100_000, nesting: 96 }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectId(pub String);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseId(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundId(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceOperationId(pub u64);
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportContext {
    pub project_id: ProjectId,
    pub case_id: CaseId,
    pub ground_id: GroundId,
    /// Provenance only; unrelated BIM edits must not stale this evidence.
    pub captured_global_revision: u64,
    pub ground_source_operation: SourceOperationId,
    pub case_source_operation: SourceOperationId,
    pub archived_solver_version: String,
    pub archived_solver_source_fingerprint: String,
    /// Must be the actual calculation worker's saved report, never a formatter probe.
    /// Specified core currently has no such field. Absence remains UNKNOWN.
    pub saved_calculation_worker_arithmetic: Option<ArithmeticEnvironmentReport>,
}
impl ReportContext {
    pub fn source_revision(&self) -> String {
        format!("ground-operation:{}/case-operation:{}",
            self.ground_source_operation.0, self.case_source_operation.0)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReportEvidence {
    Specified { input: CircularAnalysisInput, config: CircularAnalysisConfig,
        result: CircularSlopeResult },
    Search { request: SearchPublicRequestV1, result: SearchResultV1 },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceAvailability { ProvidedUnverified, Unavailable, Conflicting, Missing }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceReference {
    pub title: String,
    pub locator: Option<String>,
    pub publication_date: Option<String>,
    pub section_or_page: Option<String>,
    pub availability: ReferenceAvailability,
    pub note: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportBundle {
    pub context: ReportContext,
    pub evidence: ReportEvidence,
    pub source_references: Vec<SourceReference>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportArchive {
    pub format: String,
    pub formatter: String,
    pub input_digest_algorithm: String,
    pub bundle: ReportBundle,
    pub markdown: String,
    pub bundle_digest: String,
    pub report_digest: String,
    pub archive_digest: String,
}
/// A caller-supplied live source identity; no Document or solver is opened here.
#[derive(Debug, Clone)]
pub struct CurrentSource {
    pub project_id: ProjectId,
    pub case_id: CaseId,
    pub ground_id: GroundId,
    pub ground_source_operation: SourceOperationId,
    pub case_source_operation: SourceOperationId,
    pub solver_version: String,
    pub solver_source_fingerprint: String,
    /// Specified archived input/config digest OR SearchBindingV1 effective request digest.
    pub calculation_digest: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Currentness { Current, Stale }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceConclusion {
    ConditionalSpecifiedEvidence,
    ConditionalMinimumWithinExaminedCircularFamily,
    NoAcceptedStabilityEvidence,
}

struct BytesSink { bytes: Vec<u8>, cap: usize }
impl Write for BytesSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.cap.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("bounded output exceeded"));
        }
        self.bytes.extend_from_slice(bytes); Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
struct HashSink { hash: Sha256, bytes: usize, cap: usize }
impl HashSink {
    fn new(cap: usize) -> Self { Self { hash: Sha256::new(), bytes: 0, cap } }
    fn finish(self) -> String { format!("sha256:{:x}", self.hash.finalize()) }
}
impl Write for HashSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.cap.saturating_sub(self.bytes) {
            return Err(io::Error::other("bounded hash exceeded"));
        }
        self.hash.update(bytes); self.bytes += bytes.len(); Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
fn digest(value: &impl Serialize, cap: usize) -> Result<String, ReportError> {
    let mut sink = HashSink::new(cap);
    serde_json::to_writer(&mut sink, value).map_err(|_| ReportError::InputBudget)?;
    Ok(sink.finish())
}
/// Pure serialization contract: exact compact serde_json tuple field order,
/// UTF-8 and float_roundtrip, archived solver string, SHA256 with sha256: prefix.
/// This deliberately never calls input_fingerprint or prepares a failure mass.
pub fn archived_input_digest(version: &str, input: &CircularAnalysisInput,
    config: &CircularAnalysisConfig, limits: ReportLimits) -> Result<String, ReportError> {
    audit(&(version, input, config), limits)?;
    digest(&(version, input, config), limits.input_bytes)
}
fn audit(value: &impl Serialize, limits: ReportLimits) -> Result<(), ReportError> {
    finite::audit_finite(value, limits.input_bytes, limits.string_bytes,
        limits.collection_items, limits.nesting)
}
fn eq(actual: &str, expected: &str, name: &'static str) -> Result<(), ReportError> {
    if actual == expected { Ok(()) } else { Err(ReportError::Binding(name)) }
}
impl ReportBundle {
    pub fn calculation_digest(&self) -> &str {
        match &self.evidence {
            ReportEvidence::Specified { result, .. } => &result.input_fingerprint,
            ReportEvidence::Search { result, .. } => &result.request_fingerprint,
        }
    }
    pub fn conclusion(&self) -> EvidenceConclusion {
        match &self.evidence {
            ReportEvidence::Specified { result, .. }
                if result.accepted && !result.diagnostic_only =>
                EvidenceConclusion::ConditionalSpecifiedEvidence,
            ReportEvidence::Search { result, .. }
                if result.accepted && !result.diagnostic_only
                && result.status == SearchRunStatus::CompletedMinimum
                && result.conclusion == SearchConclusion::MinimumWithinExaminedCircularFamily
                && (result.minima.fellenius.is_some() || result.minima.bishop.is_some()) =>
                EvidenceConclusion::ConditionalMinimumWithinExaminedCircularFamily,
            _ => EvidenceConclusion::NoAcceptedStabilityEvidence,
        }
    }
    pub fn currentness(&self, current: &CurrentSource) -> Currentness {
        let c = &self.context;
        if c.project_id == current.project_id && c.case_id == current.case_id
            && c.ground_id == current.ground_id
            && c.ground_source_operation == current.ground_source_operation
            && c.case_source_operation == current.case_source_operation
            && c.archived_solver_version == current.solver_version
            && c.archived_solver_source_fingerprint == current.solver_source_fingerprint
            && self.calculation_digest() == current.calculation_digest {
            Currentness::Current
        } else { Currentness::Stale }
    }
    pub fn validate(&self, limits: ReportLimits) -> Result<(), ReportError> {
        audit(self, limits)?;
        // Traversal counts are not encoded bytes (one finite f64 may use many
        // JSON digits). Enforce actual whole typed input cap for format_report
        // as well as archive creation, without allocating a JSON copy.
        digest(self, limits.input_bytes)?;
        let c = &self.context;
        if c.project_id.0.is_empty() || c.case_id.0 == 0 || c.ground_id.0 == 0
            || c.ground_source_operation.0 == 0 || c.case_source_operation.0 == 0
            || c.archived_solver_version.is_empty()
            || c.archived_solver_source_fingerprint.is_empty() {
            return Err(ReportError::Binding("host identity"));
        }
        let (id, revision, ground) = match &self.evidence {
            ReportEvidence::Specified { input, config, result } => {
                eq(&result.analysis_id, &input.analysis_id, "analysis ID")?;
                eq(&result.source_revision, &input.source_revision, "result source revision")?;
                eq(&result.solver_version, &c.archived_solver_version, "solver version")?;
                eq(&result.source_fingerprint, &c.archived_solver_source_fingerprint, "solver source")?;
                eq(&result.input_fingerprint,
                    &archived_input_digest(&c.archived_solver_version, input, config, limits)?,
                    "specified input/config")?;
                if result.geometry.circle.surface != input.slip_surface {
                    return Err(ReportError::Binding("specified circle identity"));
                }
                validate_bishop_binding(input, config, &result.bishop_result,
                    &result.source_fingerprint, limits)?;
                (&input.analysis_id, &input.source_revision, &input.ground)
            }
            ReportEvidence::Search { request, result } => {
                if request != &result.request.public {
                    return Err(ReportError::Binding("public/effective search request"));
                }
                eq(&result.solver_version, &c.archived_solver_version, "solver version")?;
                eq(&result.source_fingerprint, &c.archived_solver_source_fingerprint, "solver source")?;
                eq(&result.request_fingerprint,
                    &digest(&(SEARCH_BINDING, &result.request), limits.input_bytes)?,
                    "effective search request")?;
                validate_search(result, limits)?;
                (&request.search_id, &request.source_revision, &request.ground)
            }
        };
        eq(id, &c.case_id.0.to_string(), "host case ID")?;
        eq(revision, &c.source_revision(), "host source stamps")?;
        eq(&ground.section_id.0, &c.ground_id.0.to_string(), "host ground ID")?;
        if ground.model_revision != c.ground_source_operation.0 {
            return Err(ReportError::Binding("ground source operation"));
        }
        Ok(())
    }
}
fn validate_bishop_binding(input: &CircularAnalysisInput, config: &CircularAnalysisConfig,
    bishop: &BishopResult, source: &str, limits: ReportLimits) -> Result<(), ReportError> {
    if bishop.config != config.bishop { return Err(ReportError::Binding("Bishop config")); }
    if let Some(contact) = &bishop.whole_arc_contact {
        eq(&contact.source_fingerprint, source, "contact source")?;
        if contact.config != config.contact { return Err(ReportError::Binding("contact config")); }
        eq(&contact.contact_input_fingerprint, &digest(&(
            "whole-arc-contact-envelope-v1", input, config, contact.factor_lower,
            contact.factor_upper, &contact.root_range_scope, source), limits.input_bytes)?,
            "whole-arc input/config/range")?;
    }
    Ok(())
}
fn validate_search(result: &SearchResultV1, limits: ReportLimits) -> Result<(), ReportError> {
    let public = &result.request.public;
    if result.schema_version != 1 || public.schema_version != 1
        || result.examined_family.binding_version != SEARCH_BINDING {
        return Err(ReportError::UnsupportedFormat);
    }
    if result.accepted && (result.status != SearchRunStatus::CompletedMinimum
        || result.diagnostic_only || !result.arithmetic_environment.accepted
        || result.conclusion != SearchConclusion::MinimumWithinExaminedCircularFamily) {
        return Err(ReportError::Binding("search accepted status"));
    }
    if !result.accepted && (result.minima.fellenius.is_some() || result.minima.bishop.is_some()) {
        return Err(ReportError::Binding("unaccepted main minima"));
    }
    if result.accepted && match public.target_method {
        TargetMethod::Both => result.minima.fellenius.is_none() || result.minima.bishop.is_none(),
        TargetMethod::Fellenius => result.minima.fellenius.is_none(),
        TargetMethod::Bishop => result.minima.bishop.is_none(),
    } { return Err(ReportError::Binding("accepted main minima missing")); }
    let ground_hash = digest(&(SEARCH_BINDING, &public.ground), limits.input_bytes)?;
    let mut candidates = BTreeMap::new();
    for candidate in &result.candidates {
        if candidates.insert(candidate.id, candidate).is_some() {
            return Err(ReportError::Binding("duplicate candidate ID"));
        }
    }
    let mut saved_tables = BTreeMap::new();
    // Reconstruct only public identity serialization, never geometry or solver work.
    // Ground cloning occurs after the whole evidence has passed finite/size admission.
    for table in &result.retained_tables {
        let candidate = candidates.get(&table.candidate_id).copied()
            .ok_or(ReportError::Binding("table candidate"))?;
        if saved_tables.insert((table.candidate_id, table.table_hash.as_str(),
            table.evaluation_key.as_str()), table).is_some() {
            return Err(ReportError::Binding("duplicate saved table"));
        }
        if table.key != candidate.key || table.key.ground_fingerprint != ground_hash
            || table.key.center_x_bits != candidate.center.x.to_bits()
            || table.key.center_z_bits != candidate.center.z.to_bits()
            || table.key.radius_bits != candidate.radius_m.to_bits()
            || table.key.branch != "lower"
            || table.key.direction != match public.direction {
                SlideDirection::PositiveX => "positive_x", SlideDirection::NegativeX => "negative_x" } {
            return Err(ReportError::Binding("candidate key"));
        }
        eq(&table.source_fingerprint, &result.source_fingerprint, "table source")?;
        let input = CircularAnalysisInput {
            analysis_id: format!("{}:candidate:{}", public.search_id, candidate.id),
            source_revision: public.source_revision.clone(), ground: public.ground.clone(),
            slip_surface: CircularSlipSurface {
                slip_id: CircularSlipId(format!("{}:circle:{}", public.search_id, candidate.id)),
                center: candidate.center, radius_m: candidate.radius_m,
                arc_branch: ArcBranch::Lower, slide_direction: public.direction,
                selected_endpoints: None },
            basis: public.basis, units: public.units, extrusion_width_m: public.extrusion_width_m,
        };
        if table.evaluation.geometry.circle.surface != input.slip_surface {
            return Err(ReportError::Binding("candidate circle identity"));
        }
        if let Ok(bishop) = &table.evaluation.bishop {
            validate_bishop_binding(&input, &table.config, bishop,
                &result.source_fingerprint, limits)?;
        }
        eq(&table.evaluation.input_fingerprint,
            &archived_input_digest(&result.solver_version, &input, &table.config, limits)?,
            "candidate input/config")?;
        eq(&table.evaluation_key, &digest(&(SEARCH_BINDING, &candidate.key,
            &result.source_fingerprint, &result.solver_version,
            &table.evaluation.input_fingerprint, &table.config), limits.input_bytes)?,
            "table evaluation key")?;
        eq(&table.table_hash, &digest(&(SEARCH_BINDING, candidate.id, &candidate.key,
            &result.source_fingerprint, &table.config, &table.evaluation), limits.input_bytes)?,
            "table content")?;
    }
    let unique = |ids: &[u64]| -> Result<BTreeSet<u64>, ReportError> {
        let set: BTreeSet<_> = ids.iter().copied().collect();
        if set.len() != ids.len() || set.iter().any(|id| !candidates.contains_key(id)) {
            return Err(ReportError::Binding("family candidate IDs"));
        }
        Ok(set)
    };
    let main_ids = unique(&result.examined_family.main_candidate_ids)?;
    let main_eligible = unique(&result.examined_family.eligible_candidate_ids)?;
    if !main_eligible.is_subset(&main_ids) || main_eligible.iter().any(|id|
        !candidates[id].main_family_member || candidates[id].final_status != CandidateStatus::JointlyEligible) {
        return Err(ReportError::Binding("main eligible family"));
    }
    if result.accepted && (main_eligible.is_empty() || main_ids.iter().any(|id| !candidates[id].main_family_member)) {
        return Err(ReportError::Binding("accepted main family"));
    }
    if result.status == SearchRunStatus::CompletedNoEligibleCandidates
        && (!main_eligible.is_empty() || result.conclusion != SearchConclusion::NoStabilityEvidence) {
        return Err(ReportError::Binding("empty main status/family"));
    }
    for (scope, minima) in std::iter::once((None, &result.minima))
        .chain(result.sensitivity.iter().map(|s| (Some(s), &s.minima))) {
        let (ids, eligible, pair) = if let Some(branch) = scope {
            let ids = unique(&branch.candidate_ids)?; let eligible = unique(&branch.eligible_candidate_ids)?;
            if !eligible.is_subset(&ids) { return Err(ReportError::Binding("sensitivity eligible family")); }
            match branch.kind.as_str() {
                "controlled_original_uniform_grid" => {
                    if branch.branch != "uniform_grid_baseline" || public.sensitivity.uniform_grid_bisections == 0
                        || branch.final_pair != public.final_pair || !ids.is_subset(&main_ids) {
                        return Err(ReportError::Binding("uniform baseline scope"));
                    }
                }
                "complete_nested_uniform_grid" => {
                    let level = branch.branch.strip_prefix("uniform_grid_bisection_")
                        .and_then(|v| v.parse::<u32>().ok()).ok_or(ReportError::Binding("uniform branch name"))?;
                    if level == 0 || level > public.sensitivity.uniform_grid_bisections || branch.final_pair != public.final_pair
                        || ids.iter().any(|id| !candidates[id].branches.contains(&branch.branch)) {
                        return Err(ReportError::Binding("nested uniform scope"));
                    }
                }
                "same_main_geometric_family_finer_slice_pair" => {
                    let index = branch.branch.strip_prefix("slice_pair_").and_then(|v| v.parse::<usize>().ok())
                        .and_then(|v| v.checked_sub(1)).ok_or(ReportError::Binding("slice branch name"))?;
                    if public.sensitivity.additional_slice_pairs.get(index) != Some(&branch.final_pair) || ids != main_ids {
                        return Err(ReportError::Binding("slice sensitivity scope"));
                    }
                }
                _ => return Err(ReportError::UnsupportedFormat),
            }
            let observations: BTreeSet<_> = branch.observations.iter().map(|o| o.candidate_id).collect();
            if observations.len() != branch.observations.len() || observations != ids
                || eligible.iter().any(|id| !branch.observations.iter().any(|o|
                    o.candidate_id == *id && o.status == CandidateStatus::JointlyEligible)) {
                return Err(ReportError::Binding("sensitivity observations"));
            }
            let both = branch.minima.fellenius.is_some() && branch.minima.bishop.is_some();
            if (branch.status == "Completed" && (eligible.is_empty() || !both))
                || (branch.status == "CompletedNoEligibleCandidates" && (!eligible.is_empty()
                    || branch.minima.fellenius.is_some() || branch.minima.bishop.is_some()))
                || !["Completed", "CompletedNoEligibleCandidates"].contains(&branch.status.as_str()) {
                return Err(ReportError::Binding("sensitivity status/minima"));
            }
            (ids, eligible, branch.final_pair)
        } else { (main_ids.clone(), main_eligible.clone(), public.final_pair) };
        if pair.coarse_requested == 0 || pair.fine_requested <= pair.coarse_requested {
            return Err(ReportError::Binding("comparison pair order"));
        }
        for (minimum, method) in [(&minima.fellenius, CircularMethod::FelleniusOrdinary),
            (&minima.bishop, CircularMethod::BishopSimplified)] {
            if let Some(minimum) = minimum {
                if minimum.method != method { return Err(ReportError::Binding("minimum method")); }
                if minimum.final_pair != pair || !ids.contains(&minimum.candidate_id)
                    || !eligible.contains(&minimum.candidate_id)
                    || minimum.near_ties.iter().any(|id| !eligible.contains(id)) {
                    return Err(ReportError::Binding("minimum family/pair"));
                }
                let candidate = candidates[&minimum.candidate_id];
                let observation = if let Some(branch) = scope {
                    let observation = branch.observations.iter().find(|o| o.candidate_id == minimum.candidate_id)
                        .ok_or(ReportError::Binding("minimum branch observation"))?;
                    if observation.status != CandidateStatus::JointlyEligible { return Err(ReportError::Binding("branch winner eligibility")); }
                    match method { CircularMethod::FelleniusOrdinary => &observation.fellenius,
                        CircularMethod::BishopSimplified => &observation.bishop }
                } else {
                    if !candidate.main_family_member || candidate.final_status != CandidateStatus::JointlyEligible
                        || !candidate.branches.iter().any(|b| b == "main") {
                        return Err(ReportError::Binding("main winner eligibility"));
                    }
                    match method { CircularMethod::FelleniusOrdinary => &candidate.fellenius,
                        CircularMethod::BishopSimplified => &candidate.bishop }
                };
                if observation.accepted_factor != Some(minimum.fine_factor)
                    || observation.selected_family_eligibility.state != CheckState::Passed {
                    return Err(ReportError::Binding("winner observation"));
                }
                let table = saved_tables.get(&(minimum.candidate_id,
                    minimum.table_hash.as_str(), minimum.evaluation_key.as_str())).copied()
                    .ok_or(ReportError::MissingWinnerTable(minimum.candidate_id))?;
                if table.config.slices.target_slices != minimum.final_pair.fine_requested as usize {
                    return Err(ReportError::Binding("minimum fine table"));
                }
                if scope.is_none() && table.branch != "main" { return Err(ReportError::Binding("main table scope")); }
                // Sensitivity can intentionally reuse an identical MAIN/baseline
                // table: bind its own membership/observations/pair/key, not a
                // blanket table.branch equality. Coarse table retention is optional.
                let input = CircularAnalysisInput {
                    analysis_id: format!("{}:candidate:{}", public.search_id, candidate.id),
                    source_revision: public.source_revision.clone(), ground: public.ground.clone(),
                    slip_surface: table.evaluation.geometry.circle.surface.clone(),
                    basis: public.basis, units: public.units, extrusion_width_m: public.extrusion_width_m,
                };
                for requested in [pair.coarse_requested, pair.fine_requested] {
                    let mut passes = observation.slice_passes.iter().filter(|p| p.requested_slices == requested);
                    let pass = passes.next().ok_or(ReportError::Binding("winner coarse/fine passes"))?;
                    if passes.next().is_some() { return Err(ReportError::Binding("winner coarse/fine passes")); }
                    eq(&pass.input_fingerprint, &archived_input_digest(&result.solver_version,
                        &input, &public.shared_analysis.fixed(requested), limits)?, "winner coarse/fine input")?;
                    if !pass.closure_passed || pass.error_code.is_some() || pass.own_accepted_factor.is_none() {
                        return Err(ReportError::Binding("winner pass eligibility"));
                    }
                    if requested == pair.fine_requested && (pass.input_fingerprint != table.evaluation.input_fingerprint
                        || pass.actual_slices != table.evaluation.slices.len() as u64) {
                        return Err(ReportError::Binding("winner fine pass/table"));
                    }
                }
                let saved_factor = match method {
                    CircularMethod::FelleniusOrdinary => table.evaluation.fellenius.as_ref()
                        .ok().map(|r| r.factor_of_safety),
                    CircularMethod::BishopSimplified => table.evaluation.bishop.as_ref()
                        .ok().and_then(|r| r.accepted_factor_of_safety),
                };
                if saved_factor != Some(minimum.fine_factor) {
                    return Err(ReportError::Binding("minimum saved factor"));
                }
            }
        }
    }
    Ok(())
}

pub fn format_report(bundle: &ReportBundle, limits: ReportLimits) -> Result<String, ReportError> {
    bundle.validate(limits)?;
    let mut sink = BytesSink { bytes: Vec::new(), cap: limits.output_bytes };
    render::render(bundle, &mut sink)?;
    String::from_utf8(sink.bytes).map_err(|_| ReportError::InvalidArchive)
}
pub fn create_archive(bundle: ReportBundle, limits: ReportLimits) -> Result<ReportArchive, ReportError> {
    let markdown = format_report(&bundle, limits)?;
    let bundle_digest = digest(&(ARCHIVE_FORMAT, INPUT_DIGEST_ALGORITHM, &bundle), limits.input_bytes)?;
    let report_digest = format!("sha256:{:x}", Sha256::digest(markdown.as_bytes()));
    let archive_digest = digest(&(ARCHIVE_FORMAT, FORMATTER_VERSION, INPUT_DIGEST_ALGORITHM,
        &bundle_digest, &report_digest), limits.input_bytes)?;
    let archive = ReportArchive { format: ARCHIVE_FORMAT.into(), formatter: FORMATTER_VERSION.into(),
        input_digest_algorithm: INPUT_DIGEST_ALGORITHM.into(), bundle, markdown,
        bundle_digest, report_digest, archive_digest };
    check_archive_size(&archive, limits)?;
    Ok(archive)
}
fn check_archive_size(archive: &ReportArchive, limits: ReportLimits) -> Result<(), ReportError> {
    serde_json::to_writer(HashSink::new(limits.archive_bytes), archive)
        .map_err(|_| ReportError::ArchiveBudget)
}
pub fn verify_archive(archive: &ReportArchive, limits: ReportLimits) -> Result<(), ReportError> {
    if archive.format != ARCHIVE_FORMAT || archive.formatter != FORMATTER_VERSION
        || archive.input_digest_algorithm != INPUT_DIGEST_ALGORITHM {
        return Err(ReportError::UnsupportedFormat);
    }
    if archive.markdown.len() > limits.output_bytes { return Err(ReportError::OutputBudget); }
    archive.bundle.validate(limits)?;
    eq(&archive.bundle_digest, &digest(&(ARCHIVE_FORMAT, INPUT_DIGEST_ALGORITHM,
        &archive.bundle), limits.input_bytes)?, "archive bundle digest")?;
    eq(&archive.report_digest, &format!("sha256:{:x}", Sha256::digest(archive.markdown.as_bytes())),
        "report digest")?;
    eq(&archive.archive_digest, &digest(&(ARCHIVE_FORMAT, FORMATTER_VERSION,
        INPUT_DIGEST_ALGORITHM, &archive.bundle_digest, &archive.report_digest), limits.input_bytes)?,
        "archive digest")?;
    // Stream deterministic report to SHA only: no solver and no duplicate output allocation.
    let mut sink = HashSink::new(limits.output_bytes);
    render::render(&archive.bundle, &mut sink)?;
    eq(&archive.report_digest, &sink.finish(), "formatter/report binding")?;
    check_archive_size(archive, limits)
}
pub fn encode_archive(archive: &ReportArchive, limits: ReportLimits) -> Result<Vec<u8>, ReportError> {
    verify_archive(archive, limits)?;
    let mut sink = BytesSink { bytes: Vec::new(), cap: limits.archive_bytes };
    serde_json::to_writer(&mut sink, archive).map_err(|_| ReportError::ArchiveBudget)?;
    Ok(sink.bytes)
}
pub fn decode_archive(bytes: &[u8], limits: ReportLimits) -> Result<ReportArchive, ReportError> {
    preflight::check(bytes, limits)?;
    let archive: ReportArchive = serde_json::from_slice(bytes).map_err(|_| ReportError::InvalidArchive)?;
    verify_archive(&archive, limits)?;
    Ok(archive)
}
