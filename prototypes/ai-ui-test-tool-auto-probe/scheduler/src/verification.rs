use crate::{
    environment::{
        EnvironmentIsolation, ExecutionEnvironmentKind, ExecutionEnvironmentRequirement,
        WorkerEnvironmentCapabilities,
    },
    model::{
        ApplicationSpec, ArtifactSpec, ExecutionRequirements, FailureClassification, JobSource,
        JobState, RetryPolicy, TEST_JOB_VERSION, TestJob, WINDOWS_INTERACTIVE_DESKTOP,
        WorkerCapabilities,
    },
    queue::JobQueue,
    report,
    scheduler::{AttemptOutcome, LeaseGrant, Scheduler, SchedulerConfig},
    state::can_transition,
    worker::capabilities_match,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

pub const STRESS_VERIFICATION_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StressVerificationReport {
    pub version: u32,
    pub run_id: String,
    pub jobs: usize,
    pub workers: usize,
    pub applications: usize,
    pub local_jobs: usize,
    pub container_jobs: usize,
    pub attempts: usize,
    pub passed: usize,
    pub test_failed: usize,
    pub cancelled: usize,
    pub worker_lost_attempts: usize,
    pub transient_infrastructure_attempts: usize,
    pub transient_timeout_attempts: usize,
    pub transient_cleanup_attempts: usize,
    pub max_active_leases: usize,
    pub max_interactive_desktop_used: usize,
    pub queue_empty: bool,
    pub leases_empty: bool,
    pub resources_released: bool,
    pub duplicate_attempts: usize,
    pub local_assignments: usize,
    pub container_assignments: usize,
    pub mixed_environment_assignments_verified: bool,
    pub markers: Vec<String>,
}

pub fn run_scheduler_stress(
    job_count: usize,
    worker_count: usize,
) -> Result<StressVerificationReport, String> {
    if job_count < 1 {
        return Err("stress job count must be greater than zero".into());
    }
    if worker_count < 2 {
        return Err("stress worker count must be at least two".into());
    }
    verify_queue_contract()?;
    if can_transition(JobState::Passed, JobState::Running) {
        return Err("state machine permits Passed -> Running".into());
    }
    let mut physical_worker = WorkerCapabilities::default();
    let physical_requirement = ExecutionRequirements {
        interactive_desktop: true,
        physical_input: true,
        semantic_only: false,
        ..ExecutionRequirements::default()
    };
    if !capabilities_match(&physical_worker, &physical_requirement) {
        return Err("matching worker rejected a physical job".into());
    }
    physical_worker.physical_input = false;
    if capabilities_match(&physical_worker, &physical_requirement) {
        return Err("capability matcher accepted a worker without physical_input".into());
    }

    let run_id = format!("run:stress:{job_count}:{worker_count}");
    let mut config = SchedulerConfig::default();
    config.worker_timeout_ms = 10;
    config.lease_duration_ms = 100;
    let mut scheduler = Scheduler::new(config);
    let jobs = (0..job_count)
        .map(|index| stress_job(&run_id, index))
        .collect::<Vec<_>>();
    scheduler.submit_run(&run_id, jobs, 0)?;

    let worker_ids = (0..worker_count)
        .map(|index| format!("stress-worker:{index:03}"))
        .collect::<Vec<_>>();
    let worker_capabilities = (0..worker_count)
        .map(stress_worker_capabilities)
        .collect::<Vec<_>>();
    for (worker_id, capabilities) in worker_ids.iter().zip(&worker_capabilities) {
        scheduler.register_worker(
            &run_id,
            worker_id,
            format!("{worker_id}:instance:1"),
            capabilities.clone(),
            1,
            0,
        )?;
    }
    if scheduler.workers.list().count() != worker_count {
        return Err("worker registry count differs from configured stress workers".into());
    }

    let cancelled_indices = (0..job_count)
        .filter(|index| index % 113 == 0)
        .collect::<BTreeSet<_>>();
    for index in &cancelled_indices {
        scheduler.cancel_job(&stress_job_id(*index), 1)?;
    }

    // Lease one real job to worker 0, stop its heartbeat, and advance a manual
    // clock. The same stable worker id then registers a fresh instance.
    let lost = scheduler
        .lease_next(&run_id, &worker_ids[0], 2)?
        .ok_or_else(|| "stress worker-loss setup could not lease a job".to_string())?;
    scheduler.mark_starting(&lost.lease.lease_id, 3)?;
    scheduler.mark_running(&lost.lease.lease_id, 4)?;
    for worker_id in worker_ids.iter().skip(1) {
        scheduler.heartbeat_and_renew(worker_id, &format!("{worker_id}:instance:1"), None, 12)?;
    }
    scheduler.tick(12)?;
    if scheduler.attempts[&lost.attempt.attempt_id].status != JobState::WorkerLost
        || scheduler.jobs[&lost.job.job_id].state != JobState::Ready
    {
        return Err("worker loss did not preserve the attempt and requeue the job".into());
    }
    scheduler.register_worker(
        &run_id,
        &worker_ids[0],
        format!("{}:instance:2", worker_ids[0]),
        worker_capabilities[0].clone(),
        1,
        12,
    )?;

    let mut now_ms = 13_u64;
    let mut loop_guard = 0_usize;
    let mut seen_attempts = BTreeSet::from([lost.attempt.attempt_id.clone()]);
    let mut duplicate_attempts = 0_usize;
    let mut transient_infrastructure_attempts = 0_usize;
    let mut transient_timeout_attempts = 0_usize;
    let mut transient_cleanup_attempts = 0_usize;
    let mut max_active_leases = 0_usize;
    let mut max_interactive_desktop_used = 0_usize;
    let mut local_assignments = 0_usize;
    let mut container_assignments = 0_usize;

    while !scheduler.run_complete(&run_id) {
        loop_guard += 1;
        if loop_guard > job_count.saturating_mul(8).saturating_add(100) {
            return Err("stress scheduler made no bounded progress".into());
        }
        for (index, worker_id) in worker_ids.iter().enumerate() {
            let instance = if index == 0 {
                format!("{worker_id}:instance:2")
            } else {
                format!("{worker_id}:instance:1")
            };
            scheduler.heartbeat_and_renew(worker_id, &instance, None, now_ms)?;
        }
        scheduler.tick(now_ms)?;

        let mut grants = Vec::new();
        for worker_id in &worker_ids {
            if let Some(grant) = scheduler.lease_next(&run_id, worker_id, now_ms)? {
                if !seen_attempts.insert(grant.attempt.attempt_id.clone()) {
                    duplicate_attempts += 1;
                }
                let required_kind = grant.job.requirements.execution_environment.kind;
                let worker_kind = scheduler
                    .workers
                    .get(worker_id)
                    .ok_or_else(|| format!("stress worker disappeared: {worker_id}"))?
                    .capabilities
                    .execution_environment
                    .kind;
                if required_kind != worker_kind {
                    return Err(format!(
                        "mixed stress assigned {required_kind:?} job to {worker_kind:?} worker"
                    ));
                }
                match required_kind {
                    ExecutionEnvironmentKind::LocalProcess => local_assignments += 1,
                    ExecutionEnvironmentKind::Container => container_assignments += 1,
                    ExecutionEnvironmentKind::Remote => {}
                }
                scheduler.mark_starting(&grant.lease.lease_id, now_ms)?;
                scheduler.mark_running(&grant.lease.lease_id, now_ms)?;
                grants.push(grant);
            }
        }
        if grants.is_empty() && !scheduler.run_complete(&run_id) {
            return Err("stress queue is non-terminal but no worker can make progress".into());
        }
        max_active_leases = max_active_leases.max(scheduler.leases.len());
        max_interactive_desktop_used =
            max_interactive_desktop_used.max(scheduler.resources.used(WINDOWS_INTERACTIVE_DESKTOP));
        if scheduler.resources.used(WINDOWS_INTERACTIVE_DESKTOP) > 1 {
            return Err("interactive desktop capacity exceeded one".into());
        }
        for application in 0..8.min(job_count.max(1)) {
            if scheduler
                .resources
                .application_used(&format!("stress-app:{application}"))
                > 1
            {
                return Err("per-application parallelism exceeded configured limit".into());
            }
        }

        for grant in grants {
            let index = stress_index(&grant)?;
            let outcome = if grant.attempt.number == 1 && index % 137 == 0 {
                transient_cleanup_attempts += 1;
                AttemptOutcome::failed(FailureClassification::InfrastructureFailure)
            } else if grant.attempt.number == 1 && index % 41 == 0 {
                transient_infrastructure_attempts += 1;
                AttemptOutcome::failed(FailureClassification::InfrastructureFailure)
            } else if grant.attempt.number == 1 && index % 89 == 0 {
                transient_timeout_attempts += 1;
                AttemptOutcome::failed(FailureClassification::Timeout)
            } else if index % 97 == 0 {
                AttemptOutcome::failed(FailureClassification::TestFailure)
            } else {
                AttemptOutcome::passed()
            };
            scheduler.complete_attempt(&grant.lease.lease_id, outcome, now_ms)?;
        }
        scheduler
            .assert_consistent()
            .map_err(|errors| errors.join("; "))?;
        now_ms = now_ms.saturating_add(1);
    }

    scheduler
        .assert_consistent()
        .map_err(|errors| errors.join("; "))?;
    let serialized = serde_json::to_vec(&scheduler).map_err(|error| error.to_string())?;
    let restored: Scheduler =
        serde_json::from_slice(&serialized).map_err(|error| error.to_string())?;
    restored
        .assert_consistent()
        .map_err(|errors| errors.join("; "))?;
    let summary = report::aggregate(&scheduler, &run_id)?;
    let terminal_total = summary.passed
        + summary.failed
        + summary.infrastructure_failed
        + summary.timed_out
        + summary.cancelled
        + summary.worker_lost
        + summary.skipped;
    if terminal_total != job_count
        || summary.cancelled != cancelled_indices.len()
        || summary.worker_lost_attempts != 1
        || duplicate_attempts != 0
        || !scheduler.queue.is_empty()
        || !scheduler.leases.is_empty()
        || scheduler.resources.active_lease_count() != 0
        || scheduler.queue.completed_history().len() != job_count
    {
        return Err("stress terminal/attempt/queue/resource accounting is inconsistent".into());
    }
    if !summary
        .markers
        .iter()
        .any(|marker| marker == "AI_UI_RESULT_AGGREGATION_PASSED")
    {
        return Err("result aggregation proof marker was not earned".into());
    }
    let required_events = [
        "job_enqueued",
        "job_ready",
        "job_leased",
        "attempt_started",
        "attempt_finished",
        "retry_scheduled",
        "worker_registered",
        "worker_lost",
        "resource_acquired",
        "resource_released",
        "run_completed",
    ];
    for event in required_events {
        if !scheduler.events.iter().any(|item| item.event == event) {
            return Err(format!("stress event log is missing {event}"));
        }
    }
    if !scheduler
        .events
        .windows(2)
        .all(|pair| pair[0].sequence < pair[1].sequence)
    {
        return Err("scheduler event sequence is not strictly increasing".into());
    }

    let mut markers = vec![
        "AI_UI_QUEUE_PASSED".into(),
        "AI_UI_JOB_STATE_MACHINE_PASSED".into(),
        "AI_UI_WORKER_REGISTRATION_PASSED".into(),
        "AI_UI_WORKER_LEASE_PASSED".into(),
        "AI_UI_WORKER_LOSS_RECOVERY_PASSED".into(),
        "AI_UI_RESOURCE_ISOLATION_PASSED".into(),
        "AI_UI_CAPABILITY_MATCHING_PASSED".into(),
        "AI_UI_RESULT_AGGREGATION_PASSED".into(),
    ];
    if job_count >= 500 && worker_count >= 4 {
        markers.push("AI_UI_SCHEDULER_STRESS_PASSED".into());
        if local_assignments > 0 && container_assignments > 0 {
            markers.push("AI_UI_SCHEDULER_MIXED_ENVIRONMENT_STRESS_PASSED".into());
        }
    }

    Ok(StressVerificationReport {
        version: STRESS_VERIFICATION_VERSION,
        run_id,
        jobs: job_count,
        workers: worker_count,
        applications: 8.min(job_count.max(1)),
        local_jobs: job_count.div_ceil(2),
        container_jobs: job_count / 2,
        attempts: summary.attempts,
        passed: summary.passed,
        test_failed: summary.failed,
        cancelled: summary.cancelled,
        worker_lost_attempts: summary.worker_lost_attempts,
        transient_infrastructure_attempts,
        transient_timeout_attempts,
        transient_cleanup_attempts,
        max_active_leases,
        max_interactive_desktop_used,
        queue_empty: scheduler.queue.is_empty(),
        leases_empty: scheduler.leases.is_empty(),
        resources_released: scheduler.resources.active_lease_count() == 0,
        duplicate_attempts,
        local_assignments,
        container_assignments,
        mixed_environment_assignments_verified: local_assignments > 0 && container_assignments > 0,
        markers,
    })
}

pub fn write_stress_report(
    output: impl AsRef<Path>,
    report: &StressVerificationReport,
) -> Result<PathBuf, String> {
    fs::create_dir_all(output.as_ref()).map_err(|error| error.to_string())?;
    let path = output.as_ref().join("stress-report.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(path)
}

fn stress_job(run_id: &str, index: usize) -> TestJob {
    let container = index % 2 == 1;
    let physical = !container && index % 58 == 0;
    let mut requirements = ExecutionRequirements {
        interactive_desktop: physical,
        physical_input: physical,
        semantic_only: !physical,
        ..ExecutionRequirements::default()
    };
    if container {
        requirements.gpu = false;
        requirements.semantic_ui = false;
        requirements.makepad = false;
        requirements.execution_environment = ExecutionEnvironmentRequirement {
            environment_key: "stress-container".into(),
            kind: ExecutionEnvironmentKind::Container,
            minimum_isolation: EnvironmentIsolation::Filesystem,
        };
    }
    TestJob {
        version: TEST_JOB_VERSION,
        job_id: stress_job_id(index),
        run_id: run_id.into(),
        application: ApplicationSpec {
            key: format!("stress-app:{}", index % 8),
            executable: "stress-fixture.exe".into(),
            args: Vec::new(),
            max_parallel: 1,
        },
        source: JobSource::Script {
            path: "stress-fixture.aiui.json".into(),
        },
        fixtures: None,
        priority: (index % 7) as i32,
        timeout_ms: 1_000,
        retry: RetryPolicy {
            max_attempts: 3,
            retry_test_failures: false,
            retry_infrastructure_failures: true,
            retry_timeouts: true,
            retry_worker_lost: true,
            retry_non_idempotent: false,
        },
        requirements,
        artifacts: ArtifactSpec {
            output_dir: PathBuf::from("stress-artifacts").join(format!("job-{index:04}")),
        },
        metadata: [("stress_index".into(), json!(index))]
            .into_iter()
            .collect(),
        created_at_ms: index as u64,
        retry_safe: true,
    }
}

fn stress_worker_capabilities(index: usize) -> WorkerCapabilities {
    if index % 2 == 0 {
        WorkerCapabilities::default()
    } else {
        WorkerCapabilities {
            platform: "windows".into(),
            interactive_desktop: false,
            gpu: false,
            max_parallel: 1,
            semantic_ui: false,
            physical_input: false,
            makepad: false,
            execution_environment: WorkerEnvironmentCapabilities {
                environment_key: "stress-container".into(),
                kind: ExecutionEnvironmentKind::Container,
                isolation: EnvironmentIsolation::Filesystem,
                gui_level: Default::default(),
                reusable: false,
            },
        }
    }
}

fn stress_job_id(index: usize) -> String {
    format!("job:stress:{index:04}")
}

fn stress_index(grant: &LeaseGrant) -> Result<usize, String> {
    grant
        .job
        .metadata
        .get("stress_index")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("stress job {} has no valid index", grant.job.job_id))
}

fn verify_queue_contract() -> Result<(), String> {
    let mut queue = JobQueue::default();
    queue.enqueue("fifo:first", 10)?;
    queue.enqueue("priority:high", 50)?;
    queue.enqueue("fifo:second", 10)?;
    let order = (0..3)
        .filter_map(|_| queue.pop_matching(|_| true).map(|entry| entry.job_id))
        .collect::<Vec<_>>();
    if order != ["priority:high", "fifo:first", "fifo:second"] {
        return Err(format!(
            "priority/FIFO queue order is not deterministic: {order:?}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_stress_500_jobs_four_workers() {
        let report = run_scheduler_stress(500, 4).unwrap();
        assert_eq!(report.jobs, 500);
        assert_eq!(report.workers, 4);
        assert_eq!(report.duplicate_attempts, 0);
        assert_eq!(report.max_interactive_desktop_used, 1);
        assert_eq!(report.local_jobs, 250);
        assert_eq!(report.container_jobs, 250);
        assert!(report.local_assignments > 0);
        assert!(report.container_assignments > 0);
        assert!(report.mixed_environment_assignments_verified);
        assert!(report.queue_empty && report.leases_empty && report.resources_released);
        assert!(
            report
                .markers
                .contains(&"AI_UI_SCHEDULER_STRESS_PASSED".into())
        );
        assert!(
            report
                .markers
                .contains(&"AI_UI_SCHEDULER_MIXED_ENVIRONMENT_STRESS_PASSED".into())
        );
    }
}
