//! One owned background formatter. It consumes saved typed evidence, never a solver.
//! Serialized limits bound admission/output, not total allocations or process RSS.
use crate::{circular_host::HostContext, circular_jobs as jobs};
use crate::geotech_circular_archive_view::{self as archive_view, ArchiveInspection, ArchiveKey};
use ai_revit_core::{
    Document,
    engineering::{AnalysisCaseId, GroundSectionId},
};
use geotech_report::{ReportArchive, ReportBundle, ReportContext, ReportEvidence, ReportLimits};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const COPY_BYTES: usize = 8 << 20;
const DEADLINE: Duration = Duration::from_secs(120);
pub(super) fn native_report_limits() -> ReportLimits {
    ReportLimits {
        input_bytes: 9 << 20,
        output_bytes: 8 << 20,
        archive_bytes: 16 << 20,
        ..ReportLimits::default()
    }
}

// Source metadata is derived only from the same owned saved input as the report.
// It records entered material provenance without claiming a verified publication.
fn saved_material_source_references(evidence: &ReportEvidence) -> Vec<geotech_report::SourceReference> {
    let ground = match evidence {
        ReportEvidence::Specified { input, .. } => &input.ground,
        ReportEvidence::Search { request, .. } => &request.ground,
    };
    ground.materials.iter().map(|material| geotech_report::SourceReference {
        title: format!(
            "Saved material-property provenance: {} / {}",
            material.material_id.0, material.properties.name,
        ),
        locator: None,
        publication_date: None,
        section_or_page: None,
        availability: if material.properties.provenance.trim().is_empty() {
            geotech_report::ReferenceAvailability::Missing
        } else {
            geotech_report::ReferenceAvailability::ProvidedUnverified
        },
        note: Some(material.properties.provenance.clone()),
    }).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReportKey {
    pub compute_sequence: u64,
    pub binding: jobs::SourceBinding,
    pub request_digest: String,
    pub content_digest: String,
}
impl ReportKey {
    fn matches_document(&self, doc: &Document, ctx: &HostContext) -> bool {
        let b = &self.binding;
        b.document_uuid == doc.document_uuid.to_string()
            && b.runtime_id == doc.runtime_instance_id()
            && b.session_id == ctx.session_id
            && b.open_generation == ctx.open_generation.0
            && doc
                .circular_cases()
                .get(&AnalysisCaseId::new(b.analysis_id.0))
                .is_some_and(|case| {
                    case.source_operation_id.get() == b.case_source_operation.0
                        && case.definition.ground_section_id.get() == b.ground_id.0
                })
            && doc
                .ground_sections()
                .get(&GroundSectionId::new(b.ground_id.0))
                .is_some_and(|ground| {
                    ground.source_operation_id.get() == b.ground_source_operation.0
                })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReportPhase {
    Idle,
    Running,
    Completed,
    Cancelled,
    Stale,
    Deadline,
    Failed,
    ShuttingDown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReportWorkError {
    Busy,
    ShuttingDown,
    InputBudget,
    ModeMismatch,
    SpawnFailed,
    SequenceExhausted,
}
struct Slot {
    handle: JoinHandle<()>,
    receiver: Receiver<Result<WorkerOutput, String>>,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
}
enum WorkerOutput {
    Report(ReportArchive),
    Archive(Arc<ArchiveInspection>),
}
pub(super) struct CircularReportWorker {
    slot: Option<Slot>,
    key: Option<ReportKey>,
    ready: Option<ReportArchive>,
    archive_key: Option<ArchiveKey>,
    archive_ready: Option<Arc<ArchiveInspection>>,
    phase: ReportPhase,
    next_sequence: u64,
    sequence: u64,
    shutting_down: bool,
}
impl Default for CircularReportWorker {
    fn default() -> Self {
        Self {
            slot: None,
            key: None,
            ready: None,
            archive_key: None,
            archive_ready: None,
            phase: ReportPhase::Idle,
            next_sequence: 1,
            sequence: 0,
            shutting_down: false,
        }
    }
}
impl CircularReportWorker {
    pub(super) fn phase(&self) -> ReportPhase {
        self.phase
    }
    pub(super) fn sequence(&self) -> u64 {
        self.sequence
    }
    pub(super) fn key(&self) -> Option<&ReportKey> {
        self.key.as_ref()
    }
    pub(super) fn archive_key(&self) -> Option<&ArchiveKey> {
        self.archive_key.as_ref()
    }
    pub(super) fn is_quiescent(&self) -> bool {
        self.slot.is_none()
    }

    /// Explicit saved-result load, using this formatter's existing single slot,
    /// cancellation flag, deadline and lifecycle. Same-key polls reuse the
    /// inspection; they never clone/revalidate its full source or report.
    pub(super) fn start_archive(
        &mut self,
        doc: &Document,
        ctx: &HostContext,
        id: AnalysisCaseId,
    ) -> Result<u64, ReportWorkError> {
        self.start_archive_with(doc, ctx, id, archive_view::inspect)
    }

    fn start_archive_with<F>(
        &mut self,
        doc: &Document,
        ctx: &HostContext,
        id: AnalysisCaseId,
        inspect: F,
    ) -> Result<u64, ReportWorkError>
    where
        F: FnOnce(AnalysisCaseId, Option<ai_revit_core::CircularSnapshotInspection>)
            -> Result<ArchiveInspection, String> + Send + 'static,
    {
        if !self.shutting_down
            && self.archive_key.as_ref().is_some_and(|key| {
                key.analysis_id == id && key.matches_document(doc, ctx)
            })
            && matches!(self.phase, ReportPhase::Running | ReportPhase::Completed)
        {
            return Ok(self.sequence);
        }
        self.check_admission()?;
        let key = ArchiveKey::capture(doc, ctx, id).map_err(|_| ReportWorkError::ModeMismatch)?;
        let captured = doc.capture_circular_snapshot_inspection(id, archive_view::ARCHIVE_COPY_BYTES)
            .map_err(|_| ReportWorkError::InputBudget)?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let handle = thread::Builder::new().name("circular-report".into()).spawn(move || {
            if worker_cancel.load(Ordering::Acquire) { return; }
            let result = inspect(id, captured)
                .map(|value| WorkerOutput::Archive(Arc::new(value)));
            if !worker_cancel.load(Ordering::Acquire) { let _ = sender.send(result); }
        }).map_err(|_| ReportWorkError::SpawnFailed)?;
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.sequence = sequence;
        self.key = None;
        self.ready = None;
        self.archive_key = Some(key);
        self.archive_ready = None;
        self.phase = ReportPhase::Running;
        self.slot = Some(Slot { handle, receiver, cancel, deadline: Instant::now() + DEADLINE });
        Ok(sequence)
    }

    /// Call ONLY while borrowing an actual current Host published result.
    /// One bounded typed copy is moved into the worker; no canonical byte clone.
    pub(super) fn start_from_saved(
        &mut self,
        compute_sequence: u64,
        saved: &jobs::PublishedResult,
        captured_revision: u64,
    ) -> Result<u64, ReportWorkError> {
        self.check_admission()?;
        if saved.canonical_bytes().len() > COPY_BYTES {
            return Err(ReportWorkError::InputBudget);
        }
        let evidence = match (saved.request(), saved.output()) {
            (jobs::JobRequest::Specified { input, config }, jobs::JobOutput::Specified(result)) => {
                ReportEvidence::Specified {
                    input: input.clone(),
                    config: config.clone(),
                    result: result.clone(),
                }
            }
            (jobs::JobRequest::Search(request), jobs::JobOutput::Search(result)) => {
                ReportEvidence::Search {
                    request: request.clone(),
                    result: result.clone(),
                }
            }
            _ => return Err(ReportWorkError::ModeMismatch),
        };
        let source_references = saved_material_source_references(&evidence);
        let b = &saved.binding;
        let bundle = ReportBundle {
            context: ReportContext {
                project_id: geotech_report::ProjectId(b.document_uuid.clone()),
                case_id: geotech_report::CaseId(b.analysis_id.0),
                ground_id: geotech_report::GroundId(b.ground_id.0),
                captured_global_revision: captured_revision,
                ground_source_operation: geotech_report::SourceOperationId(
                    b.ground_source_operation.0,
                ),
                case_source_operation: geotech_report::SourceOperationId(b.case_source_operation.0),
                archived_solver_version: b.solver_version.clone(),
                archived_solver_source_fingerprint: b.solver_source_fingerprint.clone(),
                saved_calculation_worker_arithmetic: Some(saved.worker_arithmetic.clone()),
            },
            evidence,
            source_references,
        };
        let key = ReportKey {
            compute_sequence,
            binding: b.clone(),
            request_digest: saved.request_digest.clone(),
            content_digest: saved.content_digest.clone(),
        };
        self.start_with(key, bundle, geotech_report::create_archive)
    }

    fn check_admission(&self) -> Result<(), ReportWorkError> {
        if self.shutting_down {
            return Err(ReportWorkError::ShuttingDown);
        }
        if self.slot.is_some() {
            return Err(ReportWorkError::Busy);
        }
        if self.next_sequence == u64::MAX {
            return Err(ReportWorkError::SequenceExhausted);
        }
        Ok(())
    }
    fn start_with<F>(
        &mut self,
        key: ReportKey,
        bundle: ReportBundle,
        format: F,
    ) -> Result<u64, ReportWorkError>
    where
        F: FnOnce(ReportBundle, ReportLimits) -> Result<ReportArchive, geotech_report::ReportError>
            + Send
            + 'static,
    {
        self.check_admission()?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let handle = thread::Builder::new()
            .name("circular-report".into())
            .spawn(move || {
                if worker_cancel.load(Ordering::Acquire) {
                    return;
                }
                let result = format(bundle, native_report_limits())
                    .map(WorkerOutput::Report).map_err(|error| format!("{error:?}"));
                if !worker_cancel.load(Ordering::Acquire) {
                    let _ = sender.send(result);
                }
            })
            .map_err(|_| ReportWorkError::SpawnFailed)?;
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.sequence = sequence;
        self.ready = None;
        self.key = Some(key);
        self.archive_key = None;
        self.archive_ready = None;
        self.phase = ReportPhase::Running;
        self.slot = Some(Slot {
            handle,
            receiver,
            cancel,
            deadline: Instant::now() + DEADLINE,
        });
        Ok(sequence)
    }

    fn invalidate(&mut self, phase: ReportPhase) {
        self.ready = None;
        self.archive_ready = None;
        if let Some(slot) = &self.slot {
            slot.cancel.store(true, Ordering::Release);
        }
        self.phase = phase;
    }
    pub(super) fn after_mutation(&mut self, doc: &Document, ctx: &HostContext) {
        if (self
            .key
            .as_ref()
            .is_some_and(|key| !key.matches_document(doc, ctx))
            || self.archive_key.as_ref().is_some_and(|key| !key.matches_document(doc, ctx)))
            && matches!(self.phase, ReportPhase::Running | ReportPhase::Completed)
        {
            self.invalidate(ReportPhase::Stale);
        }
    }
    pub(super) fn cancel(&mut self) {
        if matches!(self.phase, ReportPhase::Running | ReportPhase::Completed) {
            self.invalidate(ReportPhase::Cancelled);
        }
    }
    pub(super) fn invalidate_compute(&mut self, sequence: u64) {
        if self
            .key
            .as_ref()
            .is_some_and(|key| key.compute_sequence == sequence)
            && matches!(self.phase, ReportPhase::Running | ReportPhase::Completed)
        {
            self.invalidate(ReportPhase::Stale);
        }
    }
    pub(super) fn begin_shutdown(&mut self) {
        self.shutting_down = true;
        self.invalidate(ReportPhase::ShuttingDown);
    }
    pub(super) fn poll(&mut self, doc: &Document, ctx: &HostContext) {
        self.after_mutation(doc, ctx);
        self.poll_finished();
    }
    fn poll_finished(&mut self) {
        if self
            .slot
            .as_ref()
            .is_some_and(|slot| Instant::now() >= slot.deadline)
            && self.phase == ReportPhase::Running
        {
            self.invalidate(ReportPhase::Deadline);
        }
        if self
            .slot
            .as_ref()
            .is_none_or(|slot| !slot.handle.is_finished())
        {
            return;
        }
        let slot = self.slot.take().expect("finished slot");
        let joined = slot.handle.join(); // is_finished was true: never wait for live formatting.
        if self.phase != ReportPhase::Running {
            return;
        }
        if Instant::now() >= slot.deadline {
            self.invalidate(ReportPhase::Deadline);
            return;
        }
        match (joined, slot.receiver.try_recv()) {
            (Ok(()), Ok(Ok(WorkerOutput::Report(archive)))) if self.key.is_some() => {
                self.ready = Some(archive);
                self.phase = ReportPhase::Completed;
            }
            (Ok(()), Ok(Ok(WorkerOutput::Archive(inspection)))) if self.archive_key.is_some() => {
                self.archive_ready = Some(inspection);
                self.phase = ReportPhase::Completed;
            }
            _ => self.invalidate(ReportPhase::Failed),
        }
    }
    pub(super) fn current<'a>(
        &'a mut self,
        doc: &'a Document,
        ctx: &'a HostContext,
    ) -> Option<(&'a ReportKey, &'a ReportArchive)> {
        self.after_mutation(doc, ctx);
        if self.phase != ReportPhase::Completed {
            return None;
        }
        Some((self.key.as_ref()?, self.ready.as_ref()?))
    }

    pub(super) fn archive_current(
        &mut self,
        doc: &Document,
        ctx: &HostContext,
    ) -> Option<&Arc<ArchiveInspection>> {
        self.after_mutation(doc, ctx);
        if self.phase != ReportPhase::Completed { return None; }
        self.archive_key.as_ref()?;
        self.archive_ready.as_ref()
    }
}
impl Drop for CircularReportWorker {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            slot.cancel.store(true, Ordering::Release);
            if !slot.handle.is_finished() {
                eprintln!("CIRCULAR_LIFECYCLE report_drop_defensively_waiting_for_owned_worker");
            }
            let _ = slot.handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, Mutex};
    fn archived_document() -> (Document, HostContext, AnalysisCaseId) {
        let fixtures = ai_revit_core::test_fixtures::ensure_circular_archive_fixtures();
        let document = Document::open(fixtures.join("specified.native4.json")).unwrap();
        let id = *document.circular_cases().keys().next().unwrap();
        let context = HostContext { session_id: "archive-worker-test".into(), open_generation: crate::circular_host::DecimalU64(4) };
        (document, context, id)
    }

    #[test]
    fn archive_inspection_uses_owned_worker_and_reuses_immutable_cache_without_a_job() {
        let (document, context, id) = archived_document();
        let owner = thread::current().id();
        let observed = Arc::new(Mutex::new(None));
        let out = observed.clone();
        let mut worker = CircularReportWorker::default();
        let sequence = worker.start_archive_with(&document, &context, id, move |id, captured| {
            *out.lock().unwrap() = Some(thread::current().id());
            archive_view::inspect(id, captured)
        }).unwrap();
        reap(&mut worker);
        assert_ne!(observed.lock().unwrap().unwrap(), owner);
        let saved = worker.archive_current(&document, &context).unwrap().clone();
        assert_eq!(saved.analysis_id, id);
        assert!(!saved.presentation.current_job_result);
        assert!(worker.key().is_none());
        assert!(worker.current(&document, &context).is_none());
        assert_eq!(worker.start_archive(&document, &context, id).unwrap(), sequence);
        assert!(Arc::ptr_eq(worker.archive_current(&document, &context).unwrap(), &saved));
        assert!(worker.is_quiescent());
    }

    #[test]
    fn archive_inspection_cancel_deadline_epoch_change_and_shutdown_never_publish() {
        for outcome in 0..4 {
            let (document, mut context, id) = archived_document();
            let entered = Arc::new(Barrier::new(2));
            let release = Arc::new(Barrier::new(2));
            let (entered2, release2) = (entered.clone(), release.clone());
            let mut worker = CircularReportWorker::default();
            worker.start_archive_with(&document, &context, id, move |id, captured| {
                entered2.wait();
                release2.wait();
                archive_view::inspect(id, captured)
            }).unwrap();
            entered.wait();
            assert!(!worker.is_quiescent());
            match outcome {
                0 => worker.cancel(),
                1 => worker.slot.as_mut().unwrap().deadline = Instant::now(),
                2 => { context.open_generation.0 += 1; },
                _ => worker.begin_shutdown(),
            }
            worker.poll(&document, &context);
            assert!(!worker.is_quiescent());
            assert!(worker.archive_current(&document, &context).is_none());
            // The occupied slot stays owned until the worker actually returns.
            assert!(worker.start_archive(&document, &context, id).is_err());
            release.wait();
            reap(&mut worker);
            assert_eq!(worker.phase(), [ReportPhase::Cancelled, ReportPhase::Deadline, ReportPhase::Stale, ReportPhase::ShuttingDown][outcome]);
            assert!(worker.archive_current(&document, &context).is_none());
            assert!(worker.current(&document, &context).is_none());
        }
    }

    #[test]
    fn explicit_record_load_releases_completed_report_before_saved_archive_inspection() {
        let (report_a, bundle_a) = fixture();
        let (document_b, context_b, case_b) = archived_document();
        let mut worker = CircularReportWorker::default();
        worker.start_with(report_a, bundle_a, geotech_report::create_archive).unwrap();
        reap(&mut worker);
        assert_eq!(worker.phase(), ReportPhase::Completed);
        assert!(worker.key().is_some());
        assert!(worker.ready.is_some());
        // The native explicit-load handoff cancels only the old report
        // presentation and resets its UI capability, never the host job.
        worker.cancel();
        assert_eq!(worker.phase(), ReportPhase::Cancelled);
        assert!(worker.ready.is_none());
        worker.start_archive(&document_b, &context_b, case_b).unwrap();
        reap(&mut worker);
        assert!(worker.key().is_none());
        let saved_b = worker.archive_current(&document_b, &context_b).unwrap();
        assert_eq!(saved_b.analysis_id, case_b);
        assert!(saved_b.snapshot.is_some());
        assert!(!saved_b.presentation.current_job_result);
        assert!(!saved_b.presentation.calculated_in_this_runtime);
    }

    #[test]
    fn explicit_record_load_keeps_running_report_owned_until_reaped_then_inspects_archive() {
        let (report_a, bundle_a) = fixture();
        let (document_b, context_b, case_b) = archived_document();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (entered_worker, release_worker) = (entered.clone(), release.clone());
        let mut worker = CircularReportWorker::default();
        worker.start_with(report_a, bundle_a, move |bundle, limits| {
            entered_worker.wait();
            release_worker.wait();
            geotech_report::create_archive(bundle, limits)
        }).unwrap();
        entered.wait();
        worker.cancel();
        assert_eq!(worker.phase(), ReportPhase::Cancelled);
        assert!(!worker.is_quiescent());
        assert_eq!(worker.start_archive(&document_b, &context_b, case_b), Err(ReportWorkError::Busy));
        assert!(worker.archive_current(&document_b, &context_b).is_none());
        release.wait();
        reap(&mut worker);
        assert!(worker.is_quiescent());
        assert_eq!(worker.phase(), ReportPhase::Cancelled);
        assert!(worker.ready.is_none());
        // This is the native pending-sequence-zero retry after owned reap.
        worker.start_archive(&document_b, &context_b, case_b).unwrap();
        reap(&mut worker);
        let saved_b = worker.archive_current(&document_b, &context_b).unwrap();
        assert_eq!(saved_b.analysis_id, case_b);
        assert!(saved_b.snapshot.is_some());
        assert!(!saved_b.presentation.current_job_result);
        assert!(worker.key().is_none());
    }

    fn fixture() -> (ReportKey, ReportBundle) {
        let path = ai_revit_core::test_fixtures::ensure_circular_archive_fixtures()
            .join("specified.archive.json");
        let archive = geotech_report::decode_archive(
            &std::fs::read(path).expect("explicitly generated specified archive"),
            geotech_report::ReportLimits::default(),
        )
        .unwrap();
        let c = &archive.bundle.context;
        let key = ReportKey {
            compute_sequence: 1,
            binding: jobs::SourceBinding {
                document_uuid: c.project_id.0.clone(),
                session_id: "test-session".into(),
                runtime_id: "test-runtime".into(),
                open_generation: 1,
                analysis_id: jobs::CaseId(c.case_id.0),
                ground_id: jobs::GroundId(c.ground_id.0),
                ground_source_operation: jobs::SourceOperationId(c.ground_source_operation.0),
                case_source_operation: jobs::SourceOperationId(c.case_source_operation.0),
                solver_version: c.archived_solver_version.clone(),
                solver_source_fingerprint: c.archived_solver_source_fingerprint.clone(),
                mode: jobs::JobMode::Specified,
            },
            request_digest: "saved-test-request".into(),
            content_digest: "saved-test-content".into(),
        };
        (key, archive.bundle)
    }
    fn reap(worker: &mut CircularReportWorker) {
        let end = Instant::now() + Duration::from_secs(5);
        while !worker.is_quiescent() {
            worker.poll_finished();
            assert!(Instant::now() < end, "owned worker failed to return");
            thread::yield_now();
        }
    }
    #[test]
    fn geotech_circular_reportworker_saved_archive_runs_off_owner_and_preserves_provenance() {
        let (key, bundle) = fixture();
        let owner = thread::current().id();
        let observed = Arc::new(Mutex::new(None));
        let out = observed.clone();
        let mut worker = CircularReportWorker::default();
        worker
            .start_with(key.clone(), bundle, move |bundle, limits| {
                *out.lock().unwrap() = Some(thread::current().id());
                geotech_report::create_archive(bundle, limits)
            })
            .unwrap();
        reap(&mut worker);
        assert_eq!(worker.phase, ReportPhase::Completed);
        assert_ne!(observed.lock().unwrap().unwrap(), owner);
        let saved = worker.ready.as_ref().unwrap();
        assert_eq!(
            saved.bundle.context.archived_solver_version,
            key.binding.solver_version
        );
        assert!(saved.markdown.contains("ENGINEERING_REVIEW_REQUIRED"));
        geotech_report::verify_archive(saved, native_report_limits()).unwrap();
    }
    #[test]
    fn geotech_circular_reportworker_shutdown_keeps_live_slot_until_return_then_reaps() {
        let (key, bundle) = fixture();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (entered2, release2) = (entered.clone(), release.clone());
        let mut worker = CircularReportWorker::default();
        worker
            .start_with(key.clone(), bundle.clone(), move |b, l| {
                entered2.wait();
                release2.wait();
                geotech_report::create_archive(b, l)
            })
            .unwrap();
        entered.wait();
        worker.begin_shutdown();
        assert!(!worker.is_quiescent());
        assert_eq!(
            worker.start_with(key, bundle, geotech_report::create_archive),
            Err(ReportWorkError::ShuttingDown)
        );
        worker.poll_finished();
        assert!(!worker.is_quiescent());
        release.wait();
        reap(&mut worker);
        assert_eq!(worker.phase, ReportPhase::ShuttingDown);
        assert!(worker.ready.is_none());
    }
    #[test]
    fn geotech_circular_reportworker_cancel_stale_deadline_and_panic_never_publish() {
        for outcome in 0..4 {
            let (key, bundle) = fixture();
            let mut worker = CircularReportWorker::default();
            worker
                .start_with(key, bundle, move |b, l| {
                    if outcome == 3 {
                        panic!("intentional worker panic control");
                    }
                    geotech_report::create_archive(b, l)
                })
                .unwrap();
            match outcome {
                0 => worker.cancel(),
                1 => worker.invalidate(ReportPhase::Stale),
                2 => worker.slot.as_mut().unwrap().deadline = Instant::now(),
                _ => (),
            }
            reap(&mut worker);
            assert_eq!(
                worker.phase,
                [
                    ReportPhase::Cancelled,
                    ReportPhase::Stale,
                    ReportPhase::Deadline,
                    ReportPhase::Failed
                ][outcome]
            );
            assert!(worker.ready.is_none());
            worker.poll_finished();
            assert!(worker.ready.is_none());
        }
    }

    fn assert_saved_material_source_references(evidence: &mut ReportEvidence) {
        let materials = match evidence {
            ReportEvidence::Specified { input, .. } => &mut input.ground.materials,
            ReportEvidence::Search { request, .. } => &mut request.ground.materials,
        };
        let template = materials
            .first()
            .expect("saved archive fixture contains a ground material")
            .to_owned();
        let expected = [
            ("soil-a", "Upper fill", "entered prototype material note"),
            ("soil-b", "Blank source", ""),
            ("soil-c", "Whitespace source", " \t "),
        ];
        *materials = expected
            .iter()
            .map(|(material_id, name, provenance)| {
                let mut material = template.clone();
                material.material_id = (*material_id).into();
                material.properties.name = (*name).into();
                material.properties.provenance = (*provenance).into();
                material
            })
            .collect();

        let references = saved_material_source_references(evidence);
        assert_eq!(references.len(), expected.len());
        for (reference, &(material_id, name, provenance)) in
            references.iter().zip(expected.iter())
        {
            assert_eq!(
                reference.title,
                format!("Saved material-property provenance: {material_id} / {name}")
            );
            assert_eq!(reference.note.as_deref(), Some(provenance));
            assert_eq!(
                reference.availability,
                if provenance.trim().is_empty() {
                    geotech_report::ReferenceAvailability::Missing
                } else {
                    geotech_report::ReferenceAvailability::ProvidedUnverified
                }
            );
            assert_eq!(reference.locator, None);
            assert_eq!(reference.publication_date, None);
            assert_eq!(reference.section_or_page, None);
        }
    }

    fn saved_evidence_from_archive(name: &str) -> ReportEvidence {
        let fixtures = ai_revit_core::test_fixtures::ensure_circular_archive_fixtures();
        let bytes = std::fs::read(fixtures.join(name)).expect("explicitly generated archive");
        geotech_report::decode_archive(&bytes, geotech_report::ReportLimits::default())
            .expect("saved report archive fixture decodes")
            .bundle
            .evidence
    }

    #[test]
    fn saved_specified_material_provenance_preserves_order_and_raw_notes() {
        let mut evidence = saved_evidence_from_archive("specified.archive.json");
        assert!(matches!(&evidence, ReportEvidence::Specified { .. }));
        assert_saved_material_source_references(&mut evidence);
    }

    #[test]
    fn saved_search_material_provenance_preserves_order_and_raw_notes() {
        let mut evidence = saved_evidence_from_archive("search.archive.json");
        assert!(matches!(&evidence, ReportEvidence::Search { .. }));
        assert_saved_material_source_references(&mut evidence);
    }

}
