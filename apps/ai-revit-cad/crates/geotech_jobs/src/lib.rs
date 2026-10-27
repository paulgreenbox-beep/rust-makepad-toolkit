//! Isolated Gate8 preparation; NOT production integration or native acceptance.
//! CHEAPEST_VERIFICATION_PATH: offline tests of this isolated crate only.
//!
//! One owner thread runs start/status/result/cancel/invalidate/poll. One owned
//! compute thread receives an immutable admitted request, never a Document/Cx.
//! Host adapters must bind real standalone ground/case IDs and source operation
//! stamps. Both modes inspect the actual worker arithmetic environment read-only.
//!
//! Shutdown: begin_shutdown; keep polling in the host UI lifecycle until
//! is_quiescent; then drop. poll joins ONLY a thread whose is_finished is true.
//! A premature Drop logs a contract violation, cancels and joins defensively:
//! it never silently detaches or reports quiescence. That fallback can block and
//! MUST NOT replace the normal nonblocking UI shutdown protocol.
use geotech_core::circular::{
    ArithmeticEnvironmentReport, CIRCULAR_SOLVER_VERSION, CircularAnalysisConfig,
    CircularAnalysisInput, CircularError, CircularErrorCode, CircularSlopeResult, SearchProgress,
    SearchPublicRequestV1, SearchResultV1, SearchRunStatus, analyze_specified_circle_cancellable,
    inspect_arithmetic_environment, search_critical_circle_cancellable, source_fingerprint,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const REQUEST_FORMAT: &str = "circular-job-request-binding-v1";
pub const ARCHIVE_FORMAT: &str = "circular-job-result-archive-v1";

macro_rules! numeric_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);
    };
}
numeric_id!(CaseId);
numeric_id!(GroundId);
numeric_id!(SourceOperationId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobMode {
    Specified,
    Search,
}

/// Adapter types, NOT a substitute Document or allocation authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceBinding {
    pub document_uuid: String,
    pub session_id: String,
    pub runtime_id: String,
    pub open_generation: u64,
    pub analysis_id: CaseId,
    pub ground_id: GroundId,
    pub ground_source_operation: SourceOperationId,
    pub case_source_operation: SourceOperationId,
    pub solver_version: String,
    pub solver_source_fingerprint: String,
    pub mode: JobMode,
}
impl SourceBinding {
    pub fn source_revision(&self) -> String {
        format!(
            "ground-operation:{}/case-operation:{}",
            self.ground_source_operation.0, self.case_source_operation.0
        )
    }
    fn validate(&self) -> Result<(), JobError> {
        if [
            self.analysis_id.0,
            self.ground_id.0,
            self.ground_source_operation.0,
            self.case_source_operation.0,
        ]
        .contains(&0)
            || [&self.document_uuid, &self.session_id, &self.runtime_id]
                .iter()
                .any(|s| s.trim().is_empty() || s.len() > 256)
            || self.solver_version != CIRCULAR_SOLVER_VERSION
            || self.solver_source_fingerprint != source_fingerprint()
        {
            return Err(JobError::InvalidBinding);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "mode", content = "request", rename_all = "snake_case")]
pub enum JobRequest {
    Specified {
        input: CircularAnalysisInput,
        config: CircularAnalysisConfig,
    },
    Search(SearchPublicRequestV1),
}
impl JobRequest {
    pub fn mode(&self) -> JobMode {
        match self {
            Self::Specified { .. } => JobMode::Specified,
            Self::Search(_) => JobMode::Search,
        }
    }
    fn validate_binding(&self, binding: &SourceBinding) -> Result<(), JobError> {
        binding.validate()?;
        let (ground, revision) = match self {
            Self::Specified { input, .. } => {
                if input.analysis_id != binding.analysis_id.0.to_string() {
                    return Err(JobError::InvalidBinding);
                }
                (&input.ground, &input.source_revision)
            }
            Self::Search(request) => (&request.ground, &request.source_revision),
        };
        if self.mode() != binding.mode
            || ground.section_id.0 != binding.ground_id.0.to_string()
            || ground.model_revision != binding.ground_source_operation.0
            || revision != &binding.source_revision()
        {
            return Err(JobError::InvalidBinding);
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "mode", content = "result", rename_all = "snake_case")]
pub enum JobOutput {
    Specified(CircularSlopeResult),
    Search(SearchResultV1),
}

#[derive(Clone, Debug)]
pub struct JobLimits {
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
    pub max_cached_bytes: usize,
    pub max_receipts: usize,
    pub max_elapsed_ms: u64,
}
impl Default for JobLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 1024 * 1024,
            max_output_bytes: 8 * 1024 * 1024,
            max_cached_bytes: 64 * 1024 * 1024,
            max_receipts: 8,
            max_elapsed_ms: 120_000,
        }
    }
}
impl JobLimits {
    fn validate(&self) -> Result<(), JobError> {
        if self.max_input_bytes == 0
            || self.max_output_bytes == 0
            || self.max_receipts == 0
            || self.max_elapsed_ms == 0
            || self.max_elapsed_ms > 3_600_000
            || self.max_output_bytes > self.max_cached_bytes
        {
            return Err(JobError::InvalidLimits);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobPhase {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    Stale,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobError {
    InvalidLimits,
    InvalidBinding,
    InvalidInput,
    InputBudget,
    Busy,
    ShuttingDown,
    SequenceExpected(u64),
    SequenceReused,
    Gone,
    NotCompleted,
    SpawnFailed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    ArithmeticEnvironment,
    Core,
    OutputBudget,
    OutputBinding,
    OwnerDeadline,
    Panic,
    WorkerLost,
}
#[derive(Clone, Debug, Serialize)]
pub struct JobFailure {
    pub kind: FailureKind,
    pub core_code: Option<CircularErrorCode>,
    pub message: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct JobStatus {
    pub sequence: u64,
    pub phase: JobPhase,
    pub request_digest: String,
    pub captured_project_revision: u64,
    pub result_available: bool,
    pub worker_owned: bool,
    pub progress: Option<SearchProgress>,
    pub progress_updates: u64,
    pub failure: Option<JobFailure>,
    pub worker_arithmetic: Option<ArithmeticEnvironmentReport>,
    pub core_status: Option<SearchRunStatus>,
}

/// Typed archive view. Borrowed access deliberately avoids handing callers
/// untracked Arc clones of large cache entries. Host copies require own budgets.
pub struct PublishedResult {
    pub binding: SourceBinding,
    pub request_digest: String,
    pub content_digest: String,
    pub worker_arithmetic: ArithmeticEnvironmentReport,
    request: JobRequest,
    output: JobOutput,
    canonical_bytes: Vec<u8>,
}
impl PublishedResult {
    pub fn request(&self) -> &JobRequest {
        &self.request
    }
    pub fn output(&self) -> &JobOutput {
        &self.output
    }
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }
    pub fn accepted_minimum(&self) -> bool {
        match &self.output {
            JobOutput::Specified(r) => {
                r.accepted
                    && !r.diagnostic_only
                    && !r.numerically_verified
                    && r.engineering_review_required
            }
            JobOutput::Search(r) => {
                r.status == SearchRunStatus::CompletedMinimum && r.accepted && !r.diagnostic_only
            }
        }
    }
}

struct ProgressSlot {
    latest: Mutex<Option<SearchProgress>>,
    updates: AtomicU64,
    started: AtomicBool,
}
impl ProgressSlot {
    fn new() -> Self {
        Self {
            latest: Mutex::new(None),
            updates: AtomicU64::new(0),
            started: AtomicBool::new(false),
        }
    }
    fn emit(&self, mut value: SearchProgress) {
        self.updates.fetch_add(1, Ordering::Relaxed);
        value.stage = value.stage.chars().take(64).collect();
        if let Ok(mut slot) = self.latest.try_lock() {
            *slot = Some(value);
        }
    }
}
struct Record {
    binding: SourceBinding,
    digest: String,
    revision: u64,
    phase: JobPhase,
    token: Arc<AtomicBool>,
    progress: Arc<ProgressSlot>,
    failure: Option<JobFailure>,
    arithmetic: Option<ArithmeticEnvironmentReport>,
    core_status: Option<SearchRunStatus>,
    result: Option<PublishedResult>,
    deadline: Instant,
}
struct WorkerSlot {
    sequence: u64,
    handle: JoinHandle<()>,
    receiver: Receiver<WorkerTerminal>,
}
struct WorkerTerminal {
    arithmetic: ArithmeticEnvironmentReport,
    core_status: Option<SearchRunStatus>,
    outcome: Result<PublishedResult, JobFailure>,
}
type Executor = dyn Fn(&JobRequest, &AtomicBool, &mut dyn FnMut(SearchProgress)) -> Result<JobOutput, CircularError>
    + Send
    + Sync;

pub struct CircularJobManager {
    limits: JobLimits,
    next_sequence: u64,
    records: BTreeMap<u64, Record>,
    receipt_order: VecDeque<u64>,
    cache_order: VecDeque<u64>,
    cached_bytes: usize,
    worker: Option<WorkerSlot>,
    shutting_down: bool,
    executor: Arc<Executor>,
    #[cfg(test)]
    worker_setup: Option<Arc<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>>,
}
impl CircularJobManager {
    pub fn new(limits: JobLimits) -> Result<Self, JobError> {
        Self::with_executor(limits, Arc::new(core_execute))
    }
    fn with_executor(limits: JobLimits, executor: Arc<Executor>) -> Result<Self, JobError> {
        limits.validate()?;
        Ok(Self {
            limits,
            next_sequence: 1,
            records: BTreeMap::new(),
            receipt_order: VecDeque::new(),
            cache_order: VecDeque::new(),
            cached_bytes: 0,
            worker: None,
            shutting_down: false,
            executor,
            #[cfg(test)]
            worker_setup: None,
        })
    }
    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }
    pub fn is_quiescent(&self) -> bool {
        self.worker.is_none()
    }
    pub fn cached_serialized_bytes(&self) -> usize {
        self.cached_bytes
    }
    pub fn retained_receipts(&self) -> usize {
        self.records.len()
    }

    /// Borrow/measure/hash before cloning any input. An old exact sequence is
    /// looked up before checking current worker occupancy or host freshness.
    pub fn start(
        &mut self,
        sequence: u64,
        binding: &SourceBinding,
        request: &JobRequest,
        captured_project_revision: u64,
    ) -> Result<JobStatus, JobError> {
        request.validate_binding(binding)?;
        audit_finite(&(binding, request), self.limits.max_input_bytes)?;
        let digest = canonical_digest(
            &(REQUEST_FORMAT, binding, request),
            self.limits.max_input_bytes,
        )
        .map_err(|_| JobError::InputBudget)?;
        if let Some(old) = self.records.get(&sequence) {
            return if old.digest == digest {
                self.status(sequence)
            } else {
                Err(JobError::SequenceReused)
            };
        }
        if sequence < self.next_sequence {
            return Err(JobError::Gone);
        }
        if sequence != self.next_sequence {
            return Err(JobError::SequenceExpected(self.next_sequence));
        }
        if self.shutting_down {
            return Err(JobError::ShuttingDown);
        }
        if self.worker.is_some() {
            return Err(JobError::Busy);
        }
        let next = sequence
            .checked_add(1)
            .ok_or(JobError::SequenceExpected(self.next_sequence))?;
        let owned_request = request.clone();
        let owned_binding = binding.clone();
        let worker_binding = owned_binding.clone();
        let worker_digest = digest.clone();
        let token = Arc::new(AtomicBool::new(false));
        let worker_token = Arc::clone(&token);
        let progress = Arc::new(ProgressSlot::new());
        let worker_progress = Arc::clone(&progress);
        let executor = Arc::clone(&self.executor);
        let max_output_bytes = self.limits.max_output_bytes;
        #[cfg(test)]
        let worker_setup = self.worker_setup.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let handle = thread::Builder::new()
            .name("circular-job-draft".into())
            .spawn(move || {
                #[cfg(test)]
                let _restore = worker_setup.as_ref().map(|setup| setup());
                worker_progress.started.store(true, Ordering::Release);
                let arithmetic = inspect_arithmetic_environment();
                let terminal = if !arithmetic.accepted {
                    WorkerTerminal {
                        arithmetic,
                        core_status: None,
                        outcome: Err(failure(
                            FailureKind::ArithmeticEnvironment,
                            None,
                            "Actual compute worker arithmetic environment is unsupported",
                        )),
                    }
                } else {
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        executor(&owned_request, &worker_token, &mut |p| {
                            worker_progress.emit(p)
                        })
                    }));
                    let mut core_status = None;
                    let outcome = match result {
                        Ok(Ok(output)) => {
                            if let JobOutput::Search(r) = &output {
                                core_status = Some(r.status.clone());
                            }
                            prepare_archive(
                                worker_binding,
                                worker_digest,
                                &owned_request,
                                output,
                                arithmetic.clone(),
                                max_output_bytes,
                            )
                        }
                        Ok(Err(error)) => {
                            Err(failure(FailureKind::Core, Some(error.code), &error.message))
                        }
                        Err(_) => Err(failure(
                            FailureKind::Panic,
                            None,
                            "Compute executor panicked",
                        )),
                    };
                    WorkerTerminal {
                        arithmetic,
                        core_status,
                        outcome,
                    }
                };
                // Capacity is one and no replacement worker is admitted until this
                // owned handle is finished and polled. Never block on owner delivery.
                let _ = sender.try_send(terminal);
            })
            .map_err(|_| JobError::SpawnFailed)?;
        self.next_sequence = next;
        while self.records.len() >= self.limits.max_receipts {
            let old = self
                .receipt_order
                .pop_front()
                .expect("bounded receipt order");
            self.remove_result(old);
            self.records.remove(&old);
        }
        self.records.insert(
            sequence,
            Record {
                binding: owned_binding,
                digest,
                revision: captured_project_revision,
                phase: JobPhase::Pending,
                token,
                progress,
                failure: None,
                arithmetic: None,
                core_status: None,
                result: None,
                deadline: Instant::now()
                    .checked_add(Duration::from_millis(self.limits.max_elapsed_ms))
                    .unwrap_or_else(Instant::now),
            },
        );
        self.receipt_order.push_back(sequence);
        self.worker = Some(WorkerSlot {
            sequence,
            handle,
            receiver,
        });
        self.status(sequence)
    }
    pub fn status(&self, sequence: u64) -> Result<JobStatus, JobError> {
        let r = self.records.get(&sequence).ok_or(JobError::Gone)?;
        let phase = if r.phase == JobPhase::Pending && r.progress.started.load(Ordering::Acquire) {
            JobPhase::Running
        } else {
            r.phase
        };
        Ok(JobStatus {
            sequence,
            phase,
            request_digest: r.digest.clone(),
            captured_project_revision: r.revision,
            result_available: r.result.is_some(),
            worker_owned: self.worker.as_ref().is_some_and(|w| w.sequence == sequence),
            progress: r.progress.latest.try_lock().ok().and_then(|p| p.clone()),
            progress_updates: r.progress.updates.load(Ordering::Relaxed),
            failure: r.failure.clone(),
            worker_arithmetic: r.arithmetic.clone(),
            core_status: r.core_status.clone(),
        })
    }
    pub fn result(&self, sequence: u64) -> Result<&PublishedResult, JobError> {
        let r = self.records.get(&sequence).ok_or(JobError::Gone)?;
        if r.phase != JobPhase::Completed {
            return Err(JobError::NotCompleted);
        }
        r.result.as_ref().ok_or(JobError::Gone)
    }
    /// Owner publication is closed before the cooperative worker token is set.
    pub fn cancel(&mut self, sequence: u64) -> Result<JobStatus, JobError> {
        let r = self.records.get_mut(&sequence).ok_or(JobError::Gone)?;
        if matches!(r.phase, JobPhase::Pending | JobPhase::Running) {
            r.phase = JobPhase::Cancelled;
            r.token.store(true, Ordering::Release);
        }
        self.status(sequence)
    }
    /// Host calls this after source mutation/rebind/undo/open, including when a
    /// later undo would restore the original stamps. Stale is irreversible.
    pub fn invalidate_where(&mut self, mut invalid: impl FnMut(&SourceBinding) -> bool) {
        let affected: Vec<_> = self
            .records
            .iter()
            .filter_map(|(id, r)| {
                (invalid(&r.binding) && !matches!(r.phase, JobPhase::Cancelled | JobPhase::Stale))
                    .then_some(*id)
            })
            .collect();
        for id in affected {
            self.remove_result(id);
            if let Some(r) = self.records.get_mut(&id) {
                r.phase = JobPhase::Stale;
                r.token.store(true, Ordering::Release);
            }
        }
    }
    /// Nonblocking owner tick. Freshness must re-resolve the real Document's
    /// ground/case references/stamps, config/spec and runtime/open generation.
    /// Global revision alone is deliberately not the freshness key.
    pub fn poll(&mut self, mut fresh: impl FnMut(&SourceBinding, &str) -> bool) {
        let Some(slot) = self.worker.as_ref() else {
            return;
        };
        let sequence = slot.sequence;
        let r = self.records.get_mut(&sequence).expect("owned job receipt");
        if matches!(r.phase, JobPhase::Pending | JobPhase::Running) && Instant::now() >= r.deadline
        {
            r.phase = JobPhase::Failed;
            r.failure = Some(failure(
                FailureKind::OwnerDeadline,
                None,
                "Owner publication deadline expired",
            ));
            r.token.store(true, Ordering::Release);
        }
        if matches!(r.phase, JobPhase::Pending | JobPhase::Running) && !fresh(&r.binding, &r.digest)
        {
            r.phase = JobPhase::Stale;
            r.token.store(true, Ordering::Release);
        }
        if !slot.handle.is_finished() {
            return;
        }
        let slot = self.worker.take().expect("finished worker");
        let received = slot.receiver.try_recv();
        let joined = slot.handle.join(); // is_finished was checked above
        let r = self.records.get_mut(&sequence).expect("owned job receipt");
        let Ok(terminal) = received else {
            if matches!(r.phase, JobPhase::Pending | JobPhase::Running) {
                r.phase = JobPhase::Failed;
                r.failure = Some(failure(
                    FailureKind::WorkerLost,
                    None,
                    "Worker returned no outcome",
                ));
            }
            return;
        };
        r.arithmetic = Some(terminal.arithmetic);
        r.core_status = terminal.core_status;
        if joined.is_err() {
            r.failure = Some(failure(
                FailureKind::Panic,
                None,
                "Worker delivery panicked",
            ));
            if matches!(r.phase, JobPhase::Pending | JobPhase::Running) {
                r.phase = JobPhase::Failed;
            }
            return;
        }
        if !matches!(r.phase, JobPhase::Pending | JobPhase::Running) {
            return;
        }
        // Explicit owner commit barrier, after core return. Cancellation/open/
        // source invalidation processed before this tick always wins publication.
        if self.shutting_down || r.token.load(Ordering::Acquire) {
            r.phase = JobPhase::Cancelled;
            return;
        }
        if !fresh(&r.binding, &r.digest) {
            r.phase = JobPhase::Stale;
            return;
        }
        // Freshness is host code and may cross the owner deadline. Recheck
        // cancellation after that callback as well as at the final commit.
        if self.shutting_down || r.token.load(Ordering::Acquire) {
            r.phase = JobPhase::Cancelled;
            return;
        }
        if Instant::now() >= r.deadline {
            r.phase = JobPhase::Failed;
            r.failure = Some(failure(
                FailureKind::OwnerDeadline,
                None,
                "Owner publication deadline expired",
            ));
            r.token.store(true, Ordering::Release);
            return;
        }
        match terminal.outcome {
            Ok(archive) => {
                let size = archive.canonical_bytes.len();
                while self.cached_bytes.saturating_add(size) > self.limits.max_cached_bytes {
                    let old = self
                        .cache_order
                        .front()
                        .copied()
                        .expect("bounded positive cache");
                    self.remove_result(old);
                }
                let r = self.records.get_mut(&sequence).expect("owned job receipt");
                // Final publication barrier: no callback or cache work after
                // this check may consume the remaining publication deadline.
                if self.shutting_down || r.token.load(Ordering::Acquire) {
                    r.phase = JobPhase::Cancelled;
                    return;
                }
                if Instant::now() >= r.deadline {
                    r.phase = JobPhase::Failed;
                    r.failure = Some(failure(
                        FailureKind::OwnerDeadline,
                        None,
                        "Owner publication deadline expired",
                    ));
                    r.token.store(true, Ordering::Release);
                    return;
                }
                self.cached_bytes += size;
                self.cache_order.push_back(sequence);
                r.result = Some(archive);
                r.phase = JobPhase::Completed;
            }
            Err(error) => {
                r.phase = if error.core_code == Some(CircularErrorCode::Cancelled) {
                    JobPhase::Cancelled
                } else {
                    JobPhase::Failed
                };
                r.failure = Some(error);
            }
        }
    }
    pub fn begin_shutdown(&mut self) {
        self.shutting_down = true;
        if let Some(sequence) = self.worker.as_ref().map(|w| w.sequence) {
            let _ = self.cancel(sequence);
        }
    }
    fn remove_result(&mut self, id: u64) {
        if let Some(archive) = self.records.get_mut(&id).and_then(|r| r.result.take()) {
            self.cached_bytes -= archive.canonical_bytes.len();
        }
        self.cache_order.retain(|value| *value != id);
    }
}
impl Drop for CircularJobManager {
    fn drop(&mut self) {
        if let Some(slot) = self.worker.take() {
            if let Some(r) = self.records.get_mut(&slot.sequence) {
                r.phase = JobPhase::Cancelled;
                r.token.store(true, Ordering::Release);
            }
            if !slot.handle.is_finished() {
                eprintln!(
                    "CircularJobManager protocol violation: premature Drop; joining owned worker, never detaching"
                );
            }
            let _ = slot.handle.join();
        }
    }
}

fn core_execute(
    request: &JobRequest,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(SearchProgress),
) -> Result<JobOutput, CircularError> {
    match request {
        JobRequest::Specified { input, config } => {
            analyze_specified_circle_cancellable(input, config, cancel).map(JobOutput::Specified)
        }
        JobRequest::Search(request) => {
            search_critical_circle_cancellable(request, cancel, progress).map(JobOutput::Search)
        }
    }
}
fn failure(kind: FailureKind, code: Option<CircularErrorCode>, message: &str) -> JobFailure {
    JobFailure {
        kind,
        core_code: code,
        message: message.chars().take(512).collect(),
    }
}
fn prepare_archive(
    binding: SourceBinding,
    request_digest: String,
    request: &JobRequest,
    output: JobOutput,
    arithmetic: ArithmeticEnvironmentReport,
    limit: usize,
) -> Result<PublishedResult, JobFailure> {
    let valid = match (request, &output) {
        (JobRequest::Specified { input, config }, JobOutput::Specified(r)) => {
            r.analysis_id == input.analysis_id
                && r.source_revision == input.source_revision
                && geotech_core::circular::input_fingerprint(input, config)
                    .ok()
                    .as_ref()
                    == Some(&r.input_fingerprint)
                && r.source_fingerprint == binding.solver_source_fingerprint
                && r.solver_version == binding.solver_version
                && r.accepted
                && !r.diagnostic_only
                && !r.numerically_verified
                && r.engineering_review_required
        }
        (JobRequest::Search(q), JobOutput::Search(r)) => {
            r.request.public == *q
                && r.source_fingerprint == binding.solver_source_fingerprint
                && r.solver_version == binding.solver_version
                && r.arithmetic_environment.accepted
                && ((r.status == SearchRunStatus::CompletedMinimum
                    && r.accepted
                    && !r.diagnostic_only)
                    || (r.status == SearchRunStatus::CompletedNoEligibleCandidates
                        && !r.accepted
                        && r.minima.fellenius.is_none()
                        && r.minima.bishop.is_none()))
        }
        _ => false,
    };
    if !valid {
        return Err(failure(
            FailureKind::OutputBinding,
            None,
            "Core output is mismatched, incomplete, diagnostic or has no publishable terminal decision",
        ));
    }
    let mut bytes = LimitedBytes {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(
        &mut bytes,
        &(
            ARCHIVE_FORMAT,
            &binding,
            &request_digest,
            request,
            &arithmetic,
            &output,
        ),
    )
    .map_err(|_| {
        failure(
            FailureKind::OutputBudget,
            None,
            "Serialized archive exceeds host output budget",
        )
    })?;
    let content_digest = format!("sha256:{:x}", Sha256::digest(&bytes.bytes));
    Ok(PublishedResult {
        binding,
        request_digest,
        content_digest,
        worker_arithmetic: arithmetic,
        request: request.clone(),
        output,
        canonical_bytes: bytes.bytes,
    })
}
struct LimitedBytes {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for LimitedBytes {
    fn write(&mut self, value: &[u8]) -> io::Result<usize> {
        if value.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("output byte limit"));
        }
        self.bytes.extend_from_slice(value);
        Ok(value.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct DigestSink {
    hash: Sha256,
    used: usize,
    limit: usize,
}
impl Write for DigestSink {
    fn write(&mut self, value: &[u8]) -> io::Result<usize> {
        if value.len() > self.limit.saturating_sub(self.used) {
            return Err(io::Error::other("input byte limit"));
        }
        self.used += value.len();
        self.hash.update(value);
        Ok(value.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn canonical_digest(value: &impl Serialize, limit: usize) -> Result<String, serde_json::Error> {
    let mut sink = DigestSink {
        hash: Sha256::new(),
        used: 0,
        limit,
    };
    serde_json::to_writer(&mut sink, value)?;
    Ok(format!("sha256:{:x}", sink.hash.finalize()))
}

mod finite;
use finite::audit_finite;
#[cfg(test)]
mod tests;
