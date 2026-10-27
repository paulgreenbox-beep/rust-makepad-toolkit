use super::*;
use geotech_core::circular::*;
use serde_json::Value;
use std::{
    sync::{Condvar, OnceLock, atomic::AtomicUsize},
    time::{Duration, Instant},
};

fn specified() -> (SourceBinding, CircularAnalysisInput, CircularAnalysisConfig) {
    let data: Value = serde_json::from_str(include_str!("../fixtures/family-inputs.json")).unwrap();
    let mut input: CircularAnalysisInput =
        serde_json::from_value(data["inputs"][0].clone()).unwrap();
    let binding = SourceBinding {
        document_uuid: "00000000-0000-4000-8000-000000000001".into(),
        session_id: "test-process-session".into(),
        runtime_id: "runtime-1".into(),
        open_generation: 1,
        analysis_id: CaseId(1),
        ground_id: GroundId(2),
        ground_source_operation: SourceOperationId(2),
        case_source_operation: SourceOperationId(3),
        solver_version: CIRCULAR_SOLVER_VERSION.into(),
        solver_source_fingerprint: source_fingerprint(),
        mode: JobMode::Specified,
    };
    input.analysis_id = binding.analysis_id.0.to_string();
    input.ground.section_id = GroundSectionId(binding.ground_id.0.to_string());
    input.ground.model_revision = binding.ground_source_operation.0;
    input.source_revision = binding.source_revision();
    let mut config = CircularAnalysisConfig::default();
    config.slices.target_slices = 64;
    config.bishop.step_abs = 1e-12;
    config.bishop.step_rel = 1e-12;
    config.bishop.residual_abs = 1e-12;
    config.bishop.residual_rel = 1e-12;
    (binding, input, config)
}
fn search() -> (SourceBinding, JobRequest) {
    let (mut binding, input, config) = specified();
    binding.mode = JobMode::Search;
    let axis = |v| InclusiveAxis {
        min: v,
        max: v,
        intervals: 0,
    };
    let q = SearchPublicRequestV1::from_specified(
        &input,
        &config,
        SearchDomain {
            center_x: axis(-0.125),
            center_z: axis(5.125),
            radius: axis(4.875),
            allowed_entry_x: None,
            allowed_exit_x: None,
        },
    );
    (binding, JobRequest::Search(q))
}
fn reference() -> &'static SearchResultV1 {
    static RESULT: OnceLock<SearchResultV1> = OnceLock::new();
    RESULT.get_or_init(|| {
        let (_, JobRequest::Search(q)) = search() else {
            unreachable!()
        };
        let r = search_critical_circle(&q).unwrap();
        assert_eq!(
            r.status,
            SearchRunStatus::CompletedMinimum,
            "{:?}",
            r.terminal_reason
        );
        r
    })
}
fn repeat_executor() -> Arc<Executor> {
    Arc::new(|_, _, _| Ok(JobOutput::Search(reference().clone())))
}
fn wait_returned(manager: &CircularJobManager) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !manager.worker.as_ref().unwrap().handle.is_finished() {
        assert!(Instant::now() < deadline, "worker did not return");
        thread::yield_now();
    }
}
fn finish(manager: &mut CircularJobManager) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !manager.is_quiescent() {
        manager.poll(|_, _| true);
        assert!(Instant::now() < deadline, "owner did not reap worker");
        thread::yield_now();
    }
}
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    wake: Condvar,
}
impl Gate {
    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.wake.wait(open).unwrap();
        }
    }
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.wake.notify_all();
    }
}
struct ReleaseOnDrop(Arc<Gate>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}
fn gated_executor(
    gate: Arc<Gate>,
    entered: mpsc::SyncSender<()>,
    progress_count: u64,
) -> Arc<Executor> {
    Arc::new(move |_, _, progress| {
        let result = reference().clone();
        for i in 0..progress_count {
            progress(SearchProgress {
                stage: "controlled_progress".into(),
                generated_proposals: i,
                unique_candidates: 1,
                completed_evaluations: 1,
                evaluated_candidates: 1,
                actual_slice_rows: 512,
                terminal_status: None,
            });
        }
        let _ = entered.try_send(());
        gate.wait();
        Ok(JobOutput::Search(result))
    })
}

#[test]
fn lifecycle_actual_public_search_worker_fp_and_typed_archive() {
    let (binding, request) = search();
    let mut manager = CircularJobManager::new(JobLimits::default()).unwrap();
    let admitted = manager.start(1, &binding, &request, 41).unwrap();
    assert!(matches!(
        admitted.phase,
        JobPhase::Pending | JobPhase::Running
    ));
    finish(&mut manager);
    let status = manager.status(1).unwrap();
    assert_eq!(status.phase, JobPhase::Completed, "{:?}", status.failure);
    assert_eq!(status.captured_project_revision, 41);
    assert!(status.worker_arithmetic.as_ref().unwrap().accepted);
    eprintln!(
        "ACTUAL_TEST_WORKER_FP {}",
        serde_json::to_string(status.worker_arithmetic.as_ref().unwrap()).unwrap()
    );
    assert!(!status.worker_owned);
    let archive = manager.result(1).unwrap();
    assert_eq!(archive.binding, binding);
    assert!(archive.accepted_minimum());
    assert_eq!(
        archive.content_digest,
        format!("sha256:{:x}", Sha256::digest(archive.canonical_bytes()))
    );
    let decoded: Value = serde_json::from_slice(archive.canonical_bytes()).unwrap();
    assert_eq!(decoded[0], ARCHIVE_FORMAT);
    let JobOutput::Search(result) = archive.output() else {
        panic!("mode changed")
    };
    assert_eq!(result.status, SearchRunStatus::CompletedMinimum);
    assert!(result.warnings.iter().any(|s| s == "SEARCH_DOMAIN_LIMITED"));
    assert!(
        result
            .warnings
            .iter()
            .any(|s| s == "ENGINEERING_REVIEW_REQUIRED")
    );
    assert_eq!(result.execution_policy, "SerialOnly");
    assert_eq!(manager.cancel(1).unwrap().phase, JobPhase::Completed);
    manager.begin_shutdown();
    assert!(manager.is_quiescent());
}

#[test]
fn lifecycle_cancel_after_return_before_publication_retains_slot_until_reaped() {
    let (binding, request) = search();
    let mut manager =
        CircularJobManager::with_executor(JobLimits::default(), repeat_executor()).unwrap();
    manager.start(1, &binding, &request, 0).unwrap();
    wait_returned(&manager);
    assert!(manager.cancel(1).unwrap().worker_owned);
    assert!(!manager.is_quiescent());
    assert!(matches!(
        manager.start(2, &binding, &request, 0),
        Err(JobError::Busy)
    ));
    assert!(matches!(manager.result(1), Err(JobError::NotCompleted)));
    finish(&mut manager);
    assert_eq!(manager.status(1).unwrap().phase, JobPhase::Cancelled);
    assert!(
        manager
            .status(1)
            .unwrap()
            .worker_arithmetic
            .unwrap()
            .accepted
    );
    assert!(matches!(manager.result(1), Err(JobError::NotCompleted)));
    manager.start(2, &binding, &request, 0).unwrap();
    finish(&mut manager);
    assert!(manager.result(2).unwrap().accepted_minimum());
}

#[test]
fn lifecycle_ground_edit_undo_open_and_rebind_are_permanently_stale() {
    for change in 0..3 {
        let (binding, request) = search();
        let gate = Arc::new(Gate::default());
        let (entered, observed) = mpsc::sync_channel(1);
        let mut manager = CircularJobManager::with_executor(
            JobLimits::default(),
            gated_executor(Arc::clone(&gate), entered, 0),
        )
        .unwrap();
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        manager.start(1, &binding, &request, 0).unwrap();
        observed.recv_timeout(Duration::from_secs(30)).unwrap();
        let mut current = binding.clone();
        match change {
            0 => current.ground_source_operation = SourceOperationId(40),
            1 => current.open_generation += 1,
            _ => current.ground_id = GroundId(99),
        }
        manager.poll(|old, _| old == &current);
        assert_eq!(manager.status(1).unwrap().phase, JobPhase::Stale);
        current = binding.clone(); // ABA: edit undone, same stamps return
        manager.poll(|old, _| old == &current);
        gate.release();
        finish(&mut manager);
        assert_eq!(manager.status(1).unwrap().phase, JobPhase::Stale);
        assert!(matches!(manager.result(1), Err(JobError::NotCompleted)));
    }
}

#[test]
fn lifecycle_unknown_reply_retry_changed_payload_and_evicted_receipt_are_safe() {
    let (binding, request) = search();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let executor: Arc<Executor> = Arc::new(move |_, _, _| {
        counter.fetch_add(1, Ordering::Relaxed);
        Ok(JobOutput::Search(reference().clone()))
    });
    let mut manager = CircularJobManager::with_executor(
        JobLimits {
            max_receipts: 1,
            ..Default::default()
        },
        executor,
    )
    .unwrap();
    let lost_reply = manager.start(1, &binding, &request, 9).unwrap();
    let retry = manager.start(1, &binding, &request, 99).unwrap();
    assert_eq!(lost_reply.request_digest, retry.request_digest);
    assert_eq!(retry.captured_project_revision, 9);
    let mut changed = request.clone();
    let JobRequest::Search(q) = &mut changed else {
        unreachable!()
    };
    q.near_tie_abs *= 2.;
    assert!(matches!(
        manager.start(1, &binding, &changed, 9),
        Err(JobError::SequenceReused)
    ));
    finish(&mut manager);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    manager.start(2, &binding, &request, 9).unwrap();
    finish(&mut manager);
    assert_eq!(manager.retained_receipts(), 1);
    assert!(matches!(manager.status(1), Err(JobError::Gone)));
    assert!(matches!(
        manager.start(1, &binding, &request, 9),
        Err(JobError::Gone)
    ));
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(manager.next_sequence(), 3);
}

#[test]
fn lifecycle_admission_output_and_cache_budgets_are_distinct_and_bounded() {
    let (binding, mut request) = search();
    let mut small = CircularJobManager::with_executor(
        JobLimits {
            max_input_bytes: 128,
            ..Default::default()
        },
        repeat_executor(),
    )
    .unwrap();
    assert!(matches!(
        small.start(1, &binding, &request, 0),
        Err(JobError::InputBudget)
    ));
    assert_eq!(small.next_sequence(), 1);
    assert!(small.is_quiescent());
    let JobRequest::Search(q) = &mut request else {
        unreachable!()
    };
    q.domain.center_x.min = f64::NAN;
    assert!(matches!(
        small.start(1, &binding, &request, 0),
        Err(JobError::InputBudget | JobError::InvalidInput)
    ));
    let mut normal =
        CircularJobManager::with_executor(JobLimits::default(), repeat_executor()).unwrap();
    assert!(matches!(
        normal.start(1, &binding, &request, 0),
        Err(JobError::InvalidInput)
    ));
    let (_, request) = search();
    let mut tiny_output = CircularJobManager::with_executor(
        JobLimits {
            max_output_bytes: 64,
            ..Default::default()
        },
        repeat_executor(),
    )
    .unwrap();
    tiny_output.start(1, &binding, &request, 0).unwrap();
    finish(&mut tiny_output);
    assert_eq!(
        tiny_output.status(1).unwrap().failure.unwrap().kind,
        FailureKind::OutputBudget
    );
    assert_eq!(tiny_output.cached_serialized_bytes(), 0);
    normal.start(1, &binding, &request, 0).unwrap();
    finish(&mut normal);
    let one_size = normal.result(1).unwrap().canonical_bytes().len();
    let mut cache = CircularJobManager::with_executor(
        JobLimits {
            max_output_bytes: one_size + 16,
            max_cached_bytes: one_size + 16,
            max_receipts: 3,
            ..Default::default()
        },
        repeat_executor(),
    )
    .unwrap();
    for sequence in 1..=2 {
        cache.start(sequence, &binding, &request, 0).unwrap();
        finish(&mut cache);
    }
    assert_eq!(cache.status(1).unwrap().phase, JobPhase::Completed);
    assert!(!cache.status(1).unwrap().result_available);
    assert!(matches!(cache.result(1), Err(JobError::Gone)));
    assert!(cache.result(2).unwrap().accepted_minimum());
    assert!(cache.cached_serialized_bytes() <= one_size + 16);
}

#[test]
fn lifecycle_core_failure_empty_family_and_mode_mismatch_never_invent_minima() {
    let (binding, mut request) = search();
    let JobRequest::Search(q) = &mut request else {
        unreachable!()
    };
    q.max_candidates = 0; // actual public core rejects admission
    let mut manager = CircularJobManager::new(JobLimits::default()).unwrap();
    manager.start(1, &binding, &request, 0).unwrap();
    finish(&mut manager);
    assert_eq!(manager.status(1).unwrap().phase, JobPhase::Failed);
    assert!(
        manager
            .status(1)
            .unwrap()
            .failure
            .unwrap()
            .core_code
            .is_some()
    );
    assert!(matches!(manager.result(1), Err(JobError::NotCompleted)));
    let (_, mut empty) = search();
    let JobRequest::Search(q) = &mut empty else {
        unreachable!()
    };
    q.domain.center_x = InclusiveAxis {
        min: 100.,
        max: 100.,
        intervals: 0,
    };
    manager.start(2, &binding, &empty, 0).unwrap();
    finish(&mut manager);
    let status = manager.status(2).unwrap();
    assert_eq!(
        status.core_status,
        Some(SearchRunStatus::CompletedNoEligibleCandidates)
    );
    assert_eq!(status.phase, JobPhase::Completed, "{:?}", status.failure);
    assert!(!manager.result(2).unwrap().accepted_minimum());
    let (specified_binding, input, config) = specified();
    let specified_request = JobRequest::Specified { input, config };
    let mut wrong =
        CircularJobManager::with_executor(JobLimits::default(), repeat_executor()).unwrap();
    wrong
        .start(1, &specified_binding, &specified_request, 0)
        .unwrap();
    finish(&mut wrong);
    assert_eq!(
        wrong.status(1).unwrap().failure.unwrap().kind,
        FailureKind::OutputBinding
    );
    assert!(matches!(wrong.result(1), Err(JobError::NotCompleted)));
    let search_digest =
        canonical_digest(&(REQUEST_FORMAT, &binding, search().1), usize::MAX).unwrap();
    let specified_digest = canonical_digest(
        &(REQUEST_FORMAT, &specified_binding, specified_request),
        usize::MAX,
    )
    .unwrap();
    assert_ne!(search_digest, specified_digest);
}

#[test]
fn lifecycle_actual_specified_request_keeps_full_config_and_review_flags() {
    let (binding, input, mut config) = specified();
    config.slices.verify_refinement = false; // prescribed finite-table lifecycle fixture
    let request = JobRequest::Specified { input, config };
    let mut manager = CircularJobManager::new(JobLimits::default()).unwrap();
    manager.start(1, &binding, &request, 5).unwrap();
    finish(&mut manager);
    let status = manager.status(1).unwrap();
    assert_eq!(status.phase, JobPhase::Completed, "{:?}", status.failure);
    let archive = manager.result(1).unwrap();
    assert_eq!(archive.request(), &request);
    let JobOutput::Specified(r) = archive.output() else {
        panic!("wrong output mode")
    };
    assert!(!r.numerically_verified && r.engineering_review_required);
    assert!(archive.worker_arithmetic.accepted);
}

#[test]
fn lifecycle_actual_core_deadline_and_callback_cancellation_publish_no_minima() {
    let (binding, mut request) = search();
    let JobRequest::Search(q) = &mut request else {
        unreachable!()
    };
    q.max_wall_time_ms = 1;
    let mut manager = CircularJobManager::new(JobLimits::default()).unwrap();
    manager.start(1, &binding, &request, 0).unwrap();
    finish(&mut manager);
    let status = manager.status(1).unwrap();
    assert_eq!(status.phase, JobPhase::Failed, "{:?}", status.core_status);
    assert_eq!(status.core_status, Some(SearchRunStatus::DeadlineExhausted));
    assert!(matches!(manager.result(1), Err(JobError::NotCompleted)));
    let (_, request) = search();
    let executor: Arc<Executor> = Arc::new(|request, cancel, progress| {
        let JobRequest::Search(q) = request else {
            unreachable!()
        };
        search_critical_circle_cancellable(q, cancel, &mut |p| {
            if p.stage != "terminal" {
                cancel.store(true, Ordering::Release);
            }
            progress(p);
        })
        .map(JobOutput::Search)
    });
    let mut cancelled = CircularJobManager::with_executor(JobLimits::default(), executor).unwrap();
    cancelled.start(1, &binding, &request, 0).unwrap();
    finish(&mut cancelled);
    assert_eq!(cancelled.status(1).unwrap().phase, JobPhase::Cancelled);
    assert!(matches!(cancelled.result(1), Err(JobError::NotCompleted)));
}

#[test]
fn lifecycle_actual_diagnostic_search_is_not_published_as_a_minimum() {
    let (binding, mut request) = search();
    let JobRequest::Search(q) = &mut request else {
        unreachable!()
    };
    q.output_mode = SearchOutputMode::Diagnostic;
    let mut manager = CircularJobManager::new(JobLimits::default()).unwrap();
    manager.start(1, &binding, &request, 0).unwrap();
    finish(&mut manager);
    let status = manager.status(1).unwrap();
    assert_eq!(status.phase, JobPhase::Failed);
    assert_eq!(status.failure.unwrap().kind, FailureKind::OutputBinding);
    assert!(matches!(manager.result(1), Err(JobError::NotCompleted)));
}

#[test]
fn lifecycle_second_freshness_crosses_deadline_and_cannot_publish() {
    let (binding, request) = search();
    let mut manager =
        CircularJobManager::with_executor(JobLimits::default(), repeat_executor()).unwrap();
    manager.start(1, &binding, &request, 0).unwrap();
    wait_returned(&manager);
    // Set a known owner deadline only after the controlled worker has returned.
    // Poll enters before it; specifically its SECOND host callback crosses it.
    let deadline = Instant::now() + Duration::from_millis(100);
    manager.records.get_mut(&1).unwrap().deadline = deadline;
    let mut callbacks = 0;
    manager.poll(|_, _| {
        callbacks += 1;
        if callbacks == 1 {
            assert!(Instant::now() < deadline);
        } else {
            assert_eq!(callbacks, 2);
            while Instant::now() <= deadline {
                thread::yield_now();
            }
        }
        true
    });
    assert_eq!(callbacks, 2, "must exercise the final freshness callback");
    let status = manager.status(1).unwrap();
    assert_eq!(status.phase, JobPhase::Failed);
    assert_eq!(status.failure.unwrap().kind, FailureKind::OwnerDeadline);
    assert!(manager.records[&1].token.load(Ordering::Acquire));
    assert!(!status.result_available && !status.worker_owned);
    assert_eq!(manager.cached_serialized_bytes(), 0);
    assert!(matches!(manager.result(1), Err(JobError::NotCompleted)));
    assert!(manager.is_quiescent());
}

#[test]
fn lifecycle_invalidation_hook_before_edit_undo_poll_is_permanent_and_scoped() {
    for affected_ground in [GroundId(2), GroundId(99)] {
        let (binding, request) = search();
        let gate = Arc::new(Gate::default());
        let (entered, observed) = mpsc::sync_channel(1);
        let mut manager = CircularJobManager::with_executor(
            JobLimits::default(),
            gated_executor(Arc::clone(&gate), entered, 0),
        )
        .unwrap();
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        manager.start(1, &binding, &request, 0).unwrap();
        observed.recv_timeout(Duration::from_secs(30)).unwrap();
        // Shared host mutation hook runs on edit. Undo then restores the old
        // binding BEFORE any owner poll; freshness alone cannot detect this ABA.
        let mut current = binding.clone();
        current.ground_source_operation = SourceOperationId(40);
        assert_ne!(current, binding);
        manager.invalidate_where(|old| old.ground_id == affected_ground);
        current = binding.clone();
        gate.release();
        wait_returned(&manager);
        manager.poll(|old, _| old == &current);
        assert!(manager.is_quiescent());
        if affected_ground == binding.ground_id {
            assert_eq!(manager.status(1).unwrap().phase, JobPhase::Stale);
            assert!(matches!(manager.result(1), Err(JobError::NotCompleted)));
        } else {
            assert_eq!(manager.status(1).unwrap().phase, JobPhase::Completed);
            assert!(manager.result(1).unwrap().accepted_minimum());
        }
    }
}

#[test]
fn lifecycle_owner_deadline_closes_publication_without_releasing_a_busy_worker() {
    let (binding, request) = search();
    let gate = Arc::new(Gate::default());
    let (entered, observed) = mpsc::sync_channel(1);
    let mut manager = CircularJobManager::with_executor(
        JobLimits {
            max_elapsed_ms: 1,
            ..Default::default()
        },
        gated_executor(Arc::clone(&gate), entered, 0),
    )
    .unwrap();
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    manager.start(1, &binding, &request, 0).unwrap();
    observed.recv_timeout(Duration::from_secs(30)).unwrap();
    thread::sleep(Duration::from_millis(5));
    manager.poll(|_, _| true);
    let status = manager.status(1).unwrap();
    assert_eq!(status.phase, JobPhase::Failed);
    assert_eq!(status.failure.unwrap().kind, FailureKind::OwnerDeadline);
    assert!(status.worker_owned && !manager.is_quiescent());
    assert!(matches!(
        manager.start(2, &binding, &request, 0),
        Err(JobError::Busy)
    ));
    gate.release();
    finish(&mut manager);
    assert!(matches!(manager.result(1), Err(JobError::NotCompleted)));
}

#[test]
fn lifecycle_progress_is_coalesced_and_shutdown_waits_for_actual_worker_exit() {
    let (binding, request) = search();
    let gate = Arc::new(Gate::default());
    let (entered, observed) = mpsc::sync_channel(1);
    let mut manager = CircularJobManager::with_executor(
        JobLimits::default(),
        gated_executor(Arc::clone(&gate), entered, 20_000),
    )
    .unwrap();
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    manager.start(1, &binding, &request, 0).unwrap();
    observed.recv_timeout(Duration::from_secs(30)).unwrap();
    let status = manager.status(1).unwrap();
    assert_eq!(status.progress_updates, 20_000);
    assert!(status.progress.is_some());
    manager.begin_shutdown();
    manager.poll(|_, _| true); // must return while controlled executor is blocked
    assert!(!manager.is_quiescent());
    assert!(manager.status(1).unwrap().worker_owned);
    assert!(matches!(
        manager.start(2, &binding, &request, 0),
        Err(JobError::ShuttingDown)
    ));
    gate.release();
    finish(&mut manager);
    assert_eq!(manager.status(1).unwrap().phase, JobPhase::Cancelled);
    assert!(!manager.status(1).unwrap().worker_owned);
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[test]
fn lifecycle_both_modes_fail_closed_on_actual_worker_ftz_with_thread_local_restore() {
    struct Restore(u32);
    impl Drop for Restore {
        fn drop(&mut self) {
            // TEST ONLY: restores exactly the disposable worker's old controls.
            unsafe {
                core::arch::asm!("ldmxcsr [{p}]", p = in(reg) &self.0, options(nostack, preserves_flags));
            }
        }
    }
    let owner_before = inspect_arithmetic_environment();
    for mode in [JobMode::Search, JobMode::Specified] {
        let (binding, request) = if mode == JobMode::Search {
            search()
        } else {
            let (b, input, config) = specified();
            (b, JobRequest::Specified { input, config })
        };
        let called = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&called);
        let mut manager = CircularJobManager::with_executor(
            JobLimits::default(),
            Arc::new(move |_, _, _| {
                counter.fetch_add(1, Ordering::Relaxed);
                Ok(JobOutput::Search(reference().clone()))
            }),
        )
        .unwrap();
        manager.worker_setup = Some(Arc::new(|| {
            let mut old = 0_u32;
            // TEST ONLY, disposable worker; production never writes FP controls.
            unsafe {
                core::arch::asm!("stmxcsr [{p}]", p = in(reg) &mut old, options(nostack, preserves_flags));
            }
            let ftz = old | 0x8000;
            unsafe {
                core::arch::asm!("ldmxcsr [{p}]", p = in(reg) &ftz, options(nostack, preserves_flags));
            }
            Box::new(Restore(old))
        }));
        manager.start(1, &binding, &request, 0).unwrap();
        finish(&mut manager);
        let status = manager.status(1).unwrap();
        assert_eq!(status.phase, JobPhase::Failed);
        assert_eq!(
            status.failure.unwrap().kind,
            FailureKind::ArithmeticEnvironment
        );
        let report = status.worker_arithmetic.unwrap();
        eprintln!(
            "REJECTED_TEST_WORKER_FP {}",
            serde_json::to_string(&report).unwrap()
        );
        assert!(!report.accepted && report.flush_to_zero);
        assert_eq!(called.load(Ordering::Relaxed), 0);
        assert!(matches!(manager.result(1), Err(JobError::NotCompleted)));
    }
    let owner_after = inspect_arithmetic_environment();
    assert_eq!(
        owner_before.mxcsr.map(|v| v & 0xffc0),
        owner_after.mxcsr.map(|v| v & 0xffc0)
    );
}

#[test]
fn lifecycle_premature_drop_defensively_joins_instead_of_detaching() {
    let (binding, request) = search();
    let gate = Arc::new(Gate::default());
    let (entered, observed) = mpsc::sync_channel(1);
    let mut manager = CircularJobManager::with_executor(
        JobLimits::default(),
        gated_executor(Arc::clone(&gate), entered, 0),
    )
    .unwrap();
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    manager.start(1, &binding, &request, 0).unwrap();
    observed.recv_timeout(Duration::from_secs(30)).unwrap();
    let (done, received) = mpsc::sync_channel(1);
    let dropping = thread::spawn(move || {
        drop(manager);
        done.send(()).unwrap();
    });
    assert!(received.recv_timeout(Duration::from_millis(20)).is_err());
    gate.release();
    received.recv_timeout(Duration::from_secs(30)).unwrap();
    dropping.join().unwrap();
}
