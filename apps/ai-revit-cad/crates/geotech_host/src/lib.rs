//! Isolated host02 draft pinned to final durable-search foundation03 freeze.
//! Real Document -> exact jobs02; no cloned Document, second history or runner.
use ai_revit_core::{AnalysisCaseId, Document};
use geotech_core::circular::*;
use geotech_gate8_jobs_draft::{self as jobs, CircularJobManager, JobLimits,
    JobMode, JobPhase, JobRequest, JobStatus, SourceBinding};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, VecDeque}, io::{self, Write}};
mod decimal;
mod finite;
mod paging;
mod transport;
mod wire;
pub use decimal::DecimalU64;
pub use paging::{EncodedPage, PageKind, TableSelector};
pub use transport::{TransportHeader, encode_submission_line};
pub use wire::decode_submission_bytes;
#[cfg(test)]
mod tests;

pub const ENVELOPE_FORMAT: &str = "document-circular-full-submission-v1";
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
    InvalidContext, ContextMismatch, InvalidInput, InputBudget,
    RevisionConflict, SourceMismatch, NotReady, SearchSpecNotConfigured, UnknownCase, InvalidEnvelope,
    PageBudget, TransportBudget, InvalidPage, CursorGone, CursorMismatch,
    Job(jobs::JobError),
}
impl From<jobs::JobError> for HostError {
    fn from(value: jobs::JobError) -> Self { Self::Job(value) }
}
impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{self:?}") }
}
#[derive(Clone, Debug)]
pub struct HostLimits {
    pub jobs: JobLimits,
    pub max_status_bytes: usize,
    pub max_page_rows: usize,
    pub max_page_bytes: usize,
    pub max_cursors: usize,
}
impl Default for HostLimits {
    fn default() -> Self { Self { jobs: JobLimits::default(), max_status_bytes: 16 << 10,
        max_page_rows: 64, max_page_bytes: 64 << 10, max_cursors: 32 } }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostContext {
    pub session_id: String,
    pub open_generation: DecimalU64,
}
impl HostContext {
    fn validate(&self) -> Result<(), HostError> {
        if self.session_id.trim().is_empty() || self.session_id.len() > 256
            || self.open_generation.0 == 0 { Err(HostError::InvalidContext) } else { Ok(()) }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionMode { Specified, Search }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostBinding {
    pub document_uuid: String,
    pub runtime_id: String,
    pub session_id: String,
    pub open_generation: DecimalU64,
    pub case_id: DecimalU64,
    pub ground_id: DecimalU64,
    pub ground_source_operation: DecimalU64,
    pub case_source_operation: DecimalU64,
    pub solver_version: String,
    pub solver_source_fingerprint: String,
    pub mode: SubmissionMode,
}
impl HostBinding {
    fn job(&self) -> SourceBinding {
        SourceBinding { document_uuid: self.document_uuid.clone(), session_id: self.session_id.clone(),
            runtime_id: self.runtime_id.clone(), open_generation: self.open_generation.0,
            analysis_id: jobs::CaseId(self.case_id.0), ground_id: jobs::GroundId(self.ground_id.0),
            ground_source_operation: jobs::SourceOperationId(self.ground_source_operation.0),
            case_source_operation: jobs::SourceOperationId(self.case_source_operation.0),
            solver_version: self.solver_version.clone(), solver_source_fingerprint: self.solver_source_fingerprint.clone(),
            mode: match self.mode { SubmissionMode::Specified => JobMode::Specified, SubmissionMode::Search => JobMode::Search } }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "request", rename_all = "snake_case", deny_unknown_fields)]
pub enum FullRequest {
    Specified { #[serde(with = "wire::input")] input: CircularAnalysisInput, config: CircularAnalysisConfig },
    Search(#[serde(with = "wire::search")] SearchPublicRequestV1),
}
/// Caller keeps this ORIGINAL immutable envelope for unknown-response recovery.
/// Dispatcher checks token/session before invoking this controller; no token here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Submission {
    pub format: String,
    pub sequence: DecimalU64,
    pub expected_revision: DecimalU64,
    pub binding: HostBinding,
    /// Preserve the full case config even when search shares only a subset.
    pub resolved_case_config: CircularAnalysisConfig,
    pub request: FullRequest,
}
#[derive(Serialize)]
pub struct HostStatus {
    pub sequence: DecimalU64,
    pub phase: JobPhase,
    pub request_digest: String,
    pub captured_project_revision: DecimalU64,
    pub result_available: bool,
    pub worker_owned: bool,
    pub progress: Option<SearchProgress>,
    pub progress_updates: DecimalU64,
    pub failure: Option<jobs::JobFailure>,
    pub worker_arithmetic: Option<ArithmeticEnvironmentReport>,
    pub core_status: Option<SearchRunStatus>,
}
impl From<JobStatus> for HostStatus {
    fn from(s: JobStatus) -> Self { Self { sequence: DecimalU64(s.sequence), phase: s.phase,
        request_digest: s.request_digest, captured_project_revision: DecimalU64(s.captured_project_revision),
        result_available: s.result_available, worker_owned: s.worker_owned, progress: s.progress,
        progress_updates: DecimalU64(s.progress_updates), failure: s.failure,
        worker_arithmetic: s.worker_arithmetic, core_status: s.core_status } }
}
struct Admission {
    envelope: Submission,
    envelope_digest: String,
    binding: SourceBinding,
    request: JobRequest,
    job_digest: String,
}
pub struct CircularHostController {
    manager: CircularJobManager,
    limits: HostLimits,
    admissions: BTreeMap<u64, Admission>,
    cursors: BTreeMap<String, paging::CursorState>,
    cursor_order: VecDeque<String>,
}
impl CircularHostController {
    pub fn new(limits: HostLimits) -> Result<Self, HostError> {
        if limits.max_status_bytes < 4096 || limits.max_status_bytes > (64 << 10)
            || limits.jobs.max_receipts > 32 || limits.max_page_rows == 0 || limits.max_page_rows > 1024
            || limits.jobs.max_input_bytes > transport::MAX_REQUEST_LINE_BYTES
            || limits.max_page_bytes < 1024 || limits.max_page_bytes > (1 << 20)
            || limits.max_cursors == 0 || limits.max_cursors > 1024 {
            return Err(HostError::InvalidInput);
        }
        Ok(Self { manager: CircularJobManager::new(limits.jobs.clone())?, limits,
            admissions: BTreeMap::new(), cursors: BTreeMap::new(), cursor_order: VecDeque::new() })
    }
    pub fn next_sequence(&self) -> DecimalU64 { DecimalU64(self.manager.next_sequence()) }
    pub fn quote_specified(&self, doc: &Document, ctx: &HostContext, case_id: DecimalU64)
        -> Result<Submission, HostError> {
        let resolved = self.resolve_bounded(doc, ctx, case_id.0)?;
        let binding = self.binding(doc, ctx, case_id.0, SubmissionMode::Specified)?;
        let envelope = Submission { format: ENVELOPE_FORMAT.into(), sequence: self.next_sequence(),
            expected_revision: DecimalU64(doc.revision()), binding, resolved_case_config: resolved.config,
            request: FullRequest::Specified { input: resolved.input, config: resolved.config } };
        self.audit_envelope(&envelope)?; Ok(envelope)
    }
    pub fn quote_search(&self, doc: &Document, ctx: &HostContext, case_id: DecimalU64)
        -> Result<Submission, HostError> {
        let (request, config) = self.resolve_search_bounded(doc, ctx, case_id.0)?;
        let envelope = Submission { format: ENVELOPE_FORMAT.into(), sequence: self.next_sequence(),
            expected_revision: DecimalU64(doc.revision()), binding: self.binding(doc, ctx, case_id.0, SubmissionMode::Search)?,
            resolved_case_config: config, request: FullRequest::Search(request) };
        self.audit_envelope(&envelope)?; Ok(envelope)
    }
    fn audit_envelope(&self, envelope: &Submission) -> Result<String, HostError> {
        if envelope.format != ENVELOPE_FORMAT || envelope.sequence.0 == 0 { return Err(HostError::InvalidEnvelope); }
        finite::audit_finite(envelope, self.limits.jobs.max_input_bytes)?;
        let cap = self.limits.jobs.max_input_bytes.min(
            transport::MAX_REQUEST_LINE_BYTES - transport::WRAPPER_RESERVE_BYTES);
        hash(&(ENVELOPE_FORMAT, envelope), cap).map_err(|_| HostError::TransportBudget)
    }
    fn binding(&self, doc: &Document, ctx: &HostContext, case_id: u64, mode: SubmissionMode)
        -> Result<HostBinding, HostError> {
        ctx.validate()?;
        if doc.document_uuid.is_nil() { return Err(HostError::InvalidContext); }
        let case = doc.circular_case(AnalysisCaseId::new(case_id)).ok_or(HostError::UnknownCase)?;
        let ground = doc.ground_section(case.definition.ground_section_id).ok_or(HostError::NotReady)?;
        Ok(HostBinding { document_uuid: doc.document_uuid.to_string(), runtime_id: doc.runtime_instance_id(),
            session_id: ctx.session_id.clone(), open_generation: ctx.open_generation,
            case_id: DecimalU64(case_id), ground_id: DecimalU64(ground.ground_section_id.get()),
            ground_source_operation: DecimalU64(ground.source_operation_id.get()),
            case_source_operation: DecimalU64(case.source_operation_id.get()),
            solver_version: CIRCULAR_SOLVER_VERSION.into(), solver_source_fingerprint: source_fingerprint(), mode })
    }
    fn epoch(doc: &Document, ctx: &HostContext, binding: &HostBinding) -> Result<(), HostError> {
        ctx.validate()?;
        if doc.document_uuid.is_nil() { return Err(HostError::InvalidContext); }
        if binding.document_uuid != doc.document_uuid.to_string() || binding.runtime_id != doc.runtime_instance_id()
            || binding.session_id != ctx.session_id || binding.open_generation != ctx.open_generation {
            Err(HostError::ContextMismatch)
        } else { Ok(()) }
    }
    fn audit_source(&self, doc: &Document, ctx: &HostContext, case_id: u64) -> Result<(), HostError> {
        ctx.validate()?;
        let case = doc.circular_case(AnalysisCaseId::new(case_id)).ok_or(HostError::UnknownCase)?;
        let ground = doc.ground_section(case.definition.ground_section_id).ok_or(HostError::NotReady)?;
        // Audit BORROWED source before owner resolve clones geology vectors.
        finite::audit_finite(&(&ground.definition, &case.definition), self.limits.jobs.max_input_bytes)?;
        hash(&(&ground.definition, &case.definition), self.limits.jobs.max_input_bytes)?;
        Ok(())
    }
    fn resolve_bounded(&self, doc: &Document, ctx: &HostContext, case_id: u64)
        -> Result<ai_revit_core::ResolvedCircularAnalysis, HostError> {
        self.audit_source(doc, ctx, case_id)?;
        let case = doc.circular_case(AnalysisCaseId::new(case_id)).ok_or(HostError::UnknownCase)?;
        let ground = doc.ground_section(case.definition.ground_section_id).ok_or(HostError::NotReady)?;
        if !doc.ground_readiness(ground.ground_section_id).map_err(|_| HostError::NotReady)?.is_ready() {
            return Err(HostError::NotReady);
        }
        doc.resolve_circular_input(AnalysisCaseId::new(case_id)).map_err(|_| HostError::NotReady)
    }
    fn resolve_search_bounded(&self, doc: &Document, ctx: &HostContext, case_id: u64)
        -> Result<(SearchPublicRequestV1, CircularAnalysisConfig), HostError> {
        self.audit_source(doc, ctx, case_id)?;
        let case = doc.circular_case(AnalysisCaseId::new(case_id)).ok_or(HostError::UnknownCase)?;
        if case.definition.search_spec.is_none() { return Err(HostError::SearchSpecNotConfigured); }
        let resolved = doc.resolve_circular_search(AnalysisCaseId::new(case_id)).map_err(|_| HostError::NotReady)?;
        Ok((resolved.request, case.definition.config))
    }
    fn current_request(&self, doc: &Document, ctx: &HostContext, envelope: &Submission)
        -> Result<JobRequest, HostError> {
        Self::epoch(doc, ctx, &envelope.binding)?;
        if self.binding(doc, ctx, envelope.binding.case_id.0, envelope.binding.mode.clone())? != envelope.binding {
            return Err(HostError::SourceMismatch);
        }
        match &envelope.request {
            FullRequest::Specified { input, config } => {
                let resolved = self.resolve_bounded(doc, ctx, envelope.binding.case_id.0)?;
                if resolved.config != envelope.resolved_case_config { return Err(HostError::SourceMismatch); }
                if envelope.binding.mode != SubmissionMode::Specified || input != &resolved.input || config != &resolved.config {
                    return Err(HostError::SourceMismatch);
                }
                Ok(JobRequest::Specified { input: resolved.input, config: resolved.config })
            }
            FullRequest::Search(original) => {
                if envelope.binding.mode != SubmissionMode::Search { return Err(HostError::SourceMismatch); }
                let (current, config) = self.resolve_search_bounded(doc, ctx, envelope.binding.case_id.0)?;
                if config != envelope.resolved_case_config || &current != original { return Err(HostError::SourceMismatch); }
                Ok(JobRequest::Search(current))
            }
        }
    }
    fn fresh(&self, doc: &Document, ctx: &HostContext, entry: &Admission) -> bool {
        self.current_request(doc, ctx, &entry.envelope).and_then(|current|
            hash(&(jobs::REQUEST_FORMAT, &entry.binding, &current), self.limits.jobs.max_input_bytes))
            .is_ok_and(|digest| digest == entry.job_digest)
    }
    pub fn submit(&mut self, doc: &Document, ctx: &HostContext, envelope: &Submission)
        -> Result<HostStatus, HostError> {
        Self::epoch(doc, ctx, &envelope.binding)?;
        let envelope_digest = self.audit_envelope(envelope)?;
        let sequence = envelope.sequence.0;
        if sequence < self.manager.next_sequence() {
            // Retry validates CURRENT epoch but deliberately does not resolve a
            // changed source or require the old expected revision to equal now.
            self.manager.status(sequence)?; // retained receipt or Gone first
            let old = self.admissions.get(&sequence).ok_or(HostError::Job(jobs::JobError::Gone))?;
            if old.envelope_digest != envelope_digest { return Err(HostError::Job(jobs::JobError::SequenceReused)); }
            let status = self.manager.start(sequence, &old.binding, &old.request, envelope.expected_revision.0)?;
            return self.bounded_status(status);
        }
        if envelope.expected_revision.0 != doc.revision() { return Err(HostError::RevisionConflict); }
        let request = self.current_request(doc, ctx, envelope)?;
        let binding = envelope.binding.job();
        let status = self.manager.start(sequence, &binding, &request, doc.revision())?;
        self.admissions.retain(|id, _| self.manager.status(*id).is_ok());
        self.admissions.insert(sequence, Admission { envelope: envelope.clone(), envelope_digest,
            binding, request, job_digest: status.request_digest.clone() });
        self.prune_cursors();
        self.bounded_status(status)
    }
    /// MUST run after EVERY intermediate commit/undo/redo/open before next poll.
    /// A later undo can restore stamps; manager's permanent stale state must win.
    pub fn after_mutation(&mut self, doc: &Document, ctx: &HostContext) {
        let invalid: Vec<_> = self.admissions.values().filter(|e| !self.fresh(doc, ctx, e))
            .map(|e| e.binding.clone()).collect();
        self.manager.invalidate_where(|binding| invalid.iter().any(|old| old == binding));
        self.prune_cursors();
    }
    pub fn poll(&mut self, doc: &Document, ctx: &HostContext) {
        self.after_mutation(doc, ctx);
        // Split borrows: no Document clone and no worker owns a Document.
        let entries = &self.admissions;
        let cap = self.limits.jobs.max_input_bytes;
        let current: BTreeMap<_, _> = entries.iter().map(|(id, e)| (*id, self.fresh(doc, ctx, e))).collect();
        self.manager.poll(|binding, digest| entries.iter().any(|(id, e)|
            &e.binding == binding && e.job_digest == digest && current[id]
            && hash(&(jobs::REQUEST_FORMAT, binding, &e.request), cap).is_ok_and(|v| v == digest)));
        self.prune_cursors();
    }
    fn access(&self, doc: &Document, ctx: &HostContext, sequence: u64) -> Result<(), HostError> {
        self.manager.status(sequence)?;
        let entry = self.admissions.get(&sequence).ok_or(HostError::Job(jobs::JobError::Gone))?;
        Self::epoch(doc, ctx, &entry.envelope.binding)
    }
    fn bounded_status(&self, status: JobStatus) -> Result<HostStatus, HostError> {
        let status = HostStatus::from(status);
        let mut sink = BoundedBytes::new(self.limits.max_status_bytes);
        serde_json::to_writer(&mut sink, &status).map_err(|_| HostError::PageBudget)?;
        Ok(status)
    }
    pub fn status(&mut self, doc: &Document, ctx: &HostContext, sequence: DecimalU64) -> Result<HostStatus, HostError> {
        self.after_mutation(doc, ctx); self.access(doc, ctx, sequence.0)?;
        self.bounded_status(self.manager.status(sequence.0)?)
    }
    pub fn cancel(&mut self, doc: &Document, ctx: &HostContext, sequence: DecimalU64) -> Result<HostStatus, HostError> {
        self.access(doc, ctx, sequence.0)?;
        let status = self.manager.cancel(sequence.0)?; self.prune_cursors(); self.bounded_status(status)
    }
    /// Borrow the actual current typed saved result for native UI/report adoption.
    /// The borrow is tied to this controller AND the real Document/context. No
    /// Arc, clone, JSON cache, solver or report formatter is introduced. The
    /// caller owns limits on any subsequent work/copy; arbitrary caller work or
    /// heap use is NOT bounded by this borrowed-access function.
    ///
    /// A caller can inspect actual typed saved fields within this borrow:
    /// ```no_run
    /// use geotech_host::{CircularHostController, HostContext, DecimalU64};
    /// use ai_revit_core::Document;
    /// fn inspect(host: &mut CircularHostController, doc: &Document, ctx: &HostContext) {
    ///     let saved = host.current_result(doc, ctx, DecimalU64(1)).unwrap();
    ///     let _ = (saved.request(), saved.output(), &saved.binding, &saved.worker_arithmetic);
    /// }
    /// ```
    ///
    /// A result reference cannot survive owner-controller mutation:
    /// ```compile_fail
    /// use geotech_host::{CircularHostController, HostContext, DecimalU64};
    /// use ai_revit_core::Document;
    /// fn invalid(host: &mut CircularHostController, doc: &Document, ctx: &HostContext) {
    ///     let saved = host.current_result(doc, ctx, DecimalU64(1)).unwrap();
    ///     host.begin_shutdown();
    ///     let _ = saved.request();
    /// }
    /// ```
    /// Nor can it survive mutation/replacement of the borrowed actual Document:
    /// ```compile_fail
    /// use geotech_host::{CircularHostController, HostContext, DecimalU64};
    /// use ai_revit_core::Document;
    /// fn invalid(host: &mut CircularHostController, doc: &mut Document, ctx: &HostContext) {
    ///     let saved = host.current_result(doc, ctx, DecimalU64(1)).unwrap();
    ///     *doc = Document::new("replacement").unwrap();
    ///     let _ = saved.output();
    /// }
    /// ```
    pub fn current_result<'a>(&'a mut self, doc: &'a Document, ctx: &'a HostContext,
        sequence: DecimalU64) -> Result<&'a jobs::PublishedResult, HostError> {
        self.after_mutation(doc, ctx);
        self.access(doc, ctx, sequence.0)?;
        self.manager.result(sequence.0).map_err(HostError::from)
    }
    pub fn begin_shutdown(&mut self) { self.manager.begin_shutdown(); self.prune_cursors(); }
    pub fn is_quiescent(&self) -> bool { self.manager.is_quiescent() }
    fn prune_cursors(&mut self) {
        self.cursors.retain(|_, state| self.manager.status(state.sequence.0).is_ok_and(|s| s.result_available));
        self.cursor_order.retain(|token| self.cursors.contains_key(token));
    }
}
struct BoundedBytes { bytes: Vec<u8>, cap: usize }
impl BoundedBytes { fn new(cap: usize) -> Self { Self { bytes: Vec::new(), cap } } }
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.cap.saturating_sub(self.bytes.len()) { return Err(io::Error::other("host byte cap")); }
        self.bytes.extend_from_slice(bytes); Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
fn hash(value: &impl Serialize, cap: usize) -> Result<String, HostError> {
    struct Sink { hash: Sha256, left: usize }
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.left { return Err(io::Error::other("host digest cap")); }
            self.left -= bytes.len(); self.hash.update(bytes); Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }
    let mut sink = Sink { hash: Sha256::new(), left: cap };
    serde_json::to_writer(&mut sink, value).map_err(|_| HostError::InputBudget)?;
    Ok(format!("sha256:{:x}", sink.hash.finalize()))
}
