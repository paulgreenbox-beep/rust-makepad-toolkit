use crate::{
    execution::{LocalProcessEnvironment, verify_public_api_boundary},
    model::{
        FailureClassification, JobRecord, JobState, RunStatus, WINDOWS_INTERACTIVE_DESKTOP,
        WorkerCapabilities,
    },
    persistence::RunStore,
    pool::{WorkerPool, WorkerPoolConfig},
    report::RunSummary,
    scheduler::{Scheduler, SchedulerConfig},
    submission::{SubmissionOptions, TrustedApplicationCatalog, submit_source},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::Duration,
};

pub const WINDOWS_E2E_VERSION: u32 = 1;

const PASS_SPATIAL: &str = "scheduler-e2e-pass-spatial";
const PASS_ADVANCED: &str = "scheduler-e2e-pass-advanced";
const TEST_FAILURE: &str = "scheduler-e2e-test-failure";
const INFRA_FAILURE: &str = "scheduler-e2e-infrastructure-failure";
const WORKER_LOSS: &str = "scheduler-e2e-worker-loss-pass";
const PHYSICAL_SPATIAL: &str = "scheduler-e2e-physical-spatial";
const PHYSICAL_ADVANCED: &str = "scheduler-e2e-physical-advanced";

#[derive(Clone, Debug)]
pub struct WindowsE2eConfig {
    pub source: PathBuf,
    pub applications: PathBuf,
    pub runner: PathBuf,
    pub working_dir: PathBuf,
    pub output: PathBuf,
    pub run_id: String,
    pub workers: usize,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsE2eReport {
    pub version: u32,
    pub run_id: String,
    pub jobs: usize,
    pub workers_configured: usize,
    pub local_workers_used: Vec<String>,
    pub applications_passed: Vec<String>,
    pub passed: usize,
    pub test_failed: usize,
    pub infrastructure_failed: usize,
    pub attempts: usize,
    pub worker_lost_attempts: usize,
    pub public_runner_attempts: usize,
    pub script_reports_linked: usize,
    pub evidence_refs_linked: usize,
    pub physical_resource_acquisitions: usize,
    pub max_concurrent_physical_jobs: usize,
    pub semantic_parallelism_proven: bool,
    pub orphan_processes: Vec<String>,
    pub run_summary: PathBuf,
    pub markers: Vec<String>,
}

pub fn run_windows_e2e(config: WindowsE2eConfig) -> Result<WindowsE2eReport, String> {
    if !cfg!(windows) {
        return Err("Windows scheduler E2E is available only on Windows".into());
    }
    if config.workers < 2 {
        return Err("Windows scheduler E2E requires at least two local workers".into());
    }
    if config.output.join("scheduler-state.json").exists() {
        return Err(format!(
            "Windows E2E output already contains a scheduler run: {}",
            config.output.display()
        ));
    }
    verify_public_api_boundary()?;
    let applications = TrustedApplicationCatalog::load(&config.applications)?;
    let process_images = process_images(&applications, &config.runner)?;
    let process_baseline = snapshot_processes(&process_images)?;

    let mut options = SubmissionOptions::for_run(&config.run_id, &config.output);
    options.timeout_ms = config.timeout_ms;
    options.retry.max_attempts = 2;
    options.retry.retry_infrastructure_failures = true;
    options.retry.retry_worker_lost = true;
    options
        .metadata
        .insert("verification".into(), "windows-e2e-v1".into());
    let mut jobs = submit_source(&config.source, &applications, &options, 0)?;
    let mut names = BTreeSet::new();
    for job in &mut jobs {
        let name = script_name(job)?;
        if !names.insert(name.to_owned()) {
            return Err(format!("duplicate E2E script name: {name}"));
        }
        job.priority = match name {
            WORKER_LOSS => 100,
            PASS_SPATIAL | PASS_ADVANCED => 90,
            TEST_FAILURE | INFRA_FAILURE => 50,
            PHYSICAL_SPATIAL | PHYSICAL_ADVANCED => 10,
            other => return Err(format!("unexpected script in Windows E2E suite: {other}")),
        };
    }
    let expected_names = [
        PASS_SPATIAL,
        PASS_ADVANCED,
        TEST_FAILURE,
        INFRA_FAILURE,
        WORKER_LOSS,
        PHYSICAL_SPATIAL,
        PHYSICAL_ADVANCED,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    if names != expected_names {
        return Err(format!(
            "Windows E2E suite is incomplete: expected {expected_names:?}, found {names:?}"
        ));
    }

    let mut scheduler_config = SchedulerConfig::default();
    scheduler_config.worker_timeout_ms = 10;
    scheduler_config.lease_duration_ms = 1_000;
    let mut scheduler = Scheduler::new(scheduler_config);
    scheduler.submit_run(&config.run_id, jobs, 0)?;

    let lost_worker = "verification-lost-worker";
    scheduler.register_worker(
        &config.run_id,
        lost_worker,
        "verification-lost-worker:instance:1",
        WorkerCapabilities::default(),
        1,
        0,
    )?;
    let lost_grant = scheduler
        .lease_next(&config.run_id, lost_worker, 1)?
        .ok_or_else(|| "controlled worker-loss E2E could not lease a job".to_string())?;
    if script_name(&lost_grant.job)? != WORKER_LOSS {
        return Err(format!(
            "controlled loss leased {}, expected {WORKER_LOSS}",
            script_name(&lost_grant.job)?
        ));
    }
    scheduler.mark_starting(&lost_grant.lease.lease_id, 2)?;
    scheduler.mark_running(&lost_grant.lease.lease_id, 3)?;
    scheduler.tick(11)?;
    if scheduler.attempts[&lost_grant.attempt.attempt_id].status != JobState::WorkerLost
        || scheduler.jobs[&lost_grant.job.job_id].state != JobState::Ready
    {
        return Err("controlled worker loss did not requeue its job".into());
    }
    // The 10 ms threshold above belongs only to the injected manual clock.
    // Real local workers use a heartbeat/lease budget with normal OS jitter.
    scheduler.config.worker_timeout_ms = 5_000;
    scheduler.config.lease_duration_ms = 2_000;

    let store = RunStore::new(&config.output);
    store.write_all(&scheduler, &config.run_id)?;
    let environment = Arc::new(LocalProcessEnvironment::new(
        &config.runner,
        &config.working_dir,
    ));
    let mut pool = WorkerPool::new(
        &config.run_id,
        scheduler,
        store.clone(),
        WorkerPoolConfig {
            heartbeat_interval_ms: 100,
            idle_poll_ms: 10,
            no_progress_timeout_ms: 60_000,
        },
    );
    pool.add_local_workers(
        config.workers,
        WorkerCapabilities {
            max_parallel: 1,
            ..WorkerCapabilities::default()
        },
        environment,
    )?;
    let result = pool.run()?;
    let scheduler = result.scheduler;
    let summary = result.summary;
    scheduler
        .assert_consistent()
        .map_err(|errors| errors.join("; "))?;

    let jobs_by_name = jobs_by_name(&scheduler, &config.run_id)?;
    require_state(&jobs_by_name, PASS_SPATIAL, JobState::Passed)?;
    require_state(&jobs_by_name, PASS_ADVANCED, JobState::Passed)?;
    require_state(&jobs_by_name, PHYSICAL_SPATIAL, JobState::Passed)?;
    require_state(&jobs_by_name, PHYSICAL_ADVANCED, JobState::Passed)?;
    require_state(&jobs_by_name, TEST_FAILURE, JobState::Failed)?;
    require_state(&jobs_by_name, INFRA_FAILURE, JobState::InfrastructureFailed)?;
    require_state(&jobs_by_name, WORKER_LOSS, JobState::Passed)?;

    let test_job = jobs_by_name[TEST_FAILURE];
    if test_job.final_classification != Some(FailureClassification::TestFailure)
        || test_job.attempt_ids.len() != 1
    {
        return Err("real assertion failure was not classified as one TestFailure".into());
    }
    let infra_job = jobs_by_name[INFRA_FAILURE];
    if infra_job.final_classification != Some(FailureClassification::InfrastructureFailure)
        || infra_job.attempt_ids.len() != 2
    {
        return Err("invalid executable was not preserved as retried InfrastructureFailure".into());
    }
    let loss_job = jobs_by_name[WORKER_LOSS];
    if loss_job.attempt_ids.len() != 2
        || scheduler.attempts[&loss_job.attempt_ids[0]].status != JobState::WorkerLost
        || scheduler.attempts[&loss_job.attempt_ids[1]].status != JobState::Passed
    {
        return Err("WorkerLost -> requeue -> new passing attempt proof failed".into());
    }

    verify_summary(&summary)?;
    if summary.passed != 5
        || summary.failed != 1
        || summary.infrastructure_failed != 1
        || summary.worker_lost_attempts != 1
    {
        return Err(format!(
            "unexpected E2E aggregation: pass={}, test={}, infra={}, lost_attempts={}",
            summary.passed,
            summary.failed,
            summary.infrastructure_failed,
            summary.worker_lost_attempts
        ));
    }

    let mut local_workers_used = BTreeSet::new();
    let mut public_runner_attempts = 0;
    let mut script_reports_linked = 0;
    let mut evidence_refs = BTreeSet::new();
    for attempt in scheduler.attempts.values() {
        let belongs_to_run = scheduler
            .jobs
            .get(&attempt.job_id)
            .is_some_and(|job| job.job.run_id == config.run_id);
        if !belongs_to_run || attempt.status == JobState::WorkerLost {
            continue;
        }
        public_runner_attempts += 1;
        if attempt.worker_id.starts_with("local-worker:") {
            local_workers_used.insert(attempt.worker_id.clone());
        }
        if attempt
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.runner_pid)
            .is_none()
        {
            return Err(format!(
                "attempt {} has no owned public runner PID",
                attempt.attempt_id
            ));
        }
        let report = attempt.script_report.as_ref().ok_or_else(|| {
            format!(
                "attempt {} did not link a public script report",
                attempt.attempt_id
            )
        })?;
        if !report.is_file() {
            return Err(format!(
                "linked public script report does not exist: {}",
                report.display()
            ));
        }
        script_reports_linked += 1;
        evidence_refs.extend(attempt.evidence_refs.iter().cloned());
        let job = &scheduler.jobs[&attempt.job_id].job;
        let attempt_dir = job
            .artifacts
            .output_dir
            .join(format!("attempt-{:03}", attempt.number));
        for required in ["runner-process.json", "resolved-script.aiui.json"] {
            if !attempt_dir.join(required).is_file() {
                return Err(format!(
                    "attempt {} is missing public-boundary artifact {required}",
                    attempt.attempt_id
                ));
            }
        }
    }
    if local_workers_used.len() < 2 {
        return Err(format!(
            "E2E configured {} workers but only used {local_workers_used:?}",
            config.workers
        ));
    }
    if !evidence_refs.iter().any(|path| path.exists()) {
        return Err("real TEST_FAILED attempt did not link its evidence bundle".into());
    }

    let (physical_acquisitions, max_physical) = verify_physical_isolation(&scheduler)?;
    if physical_acquisitions != 2 || max_physical != 1 {
        return Err(format!(
            "physical isolation expected 2 sequential acquisitions, got {physical_acquisitions}, max={max_physical}"
        ));
    }
    let semantic_parallelism_proven = semantic_parallelism_proven(&scheduler);
    let applications_passed = [PASS_SPATIAL, PASS_ADVANCED]
        .into_iter()
        .map(|name| jobs_by_name[name].job.application.key.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if applications_passed.len() != 2 {
        return Err("multi-application proof did not pass two distinct application keys".into());
    }

    let orphan_processes =
        wait_for_new_processes_to_exit(&process_images, &process_baseline, 2_000)?;
    if !orphan_processes.is_empty() {
        return Err(format!(
            "owned runner/application processes remain after E2E: {orphan_processes:?}"
        ));
    }
    if !scheduler.queue.is_empty()
        || !scheduler.leases.is_empty()
        || scheduler.resources.active_lease_count() != 0
    {
        return Err("E2E completed with leaked queue/lease/resource state".into());
    }

    let mut markers = vec![
        "AI_UI_PUBLIC_SCRIPT_API_BOUNDARY_PASSED".into(),
        "AI_UI_WORKER_LOSS_RECOVERY_PASSED".into(),
        "AI_UI_RESOURCE_ISOLATION_PASSED".into(),
        "AI_UI_MULTI_APPLICATION_SCHEDULING_PASSED".into(),
        "AI_UI_RESULT_AGGREGATION_PASSED".into(),
        "AI_UI_SCHEDULER_E2E_PASSED".into(),
        "AI_UI_WORKER_POOL_PASSED".into(),
    ];
    if semantic_parallelism_proven {
        markers.insert(
            markers.len() - 2,
            "AI_UI_SEMANTIC_PARALLELISM_PASSED".into(),
        );
    }
    let report = WindowsE2eReport {
        version: WINDOWS_E2E_VERSION,
        run_id: config.run_id,
        jobs: summary.total_jobs,
        workers_configured: config.workers,
        local_workers_used: local_workers_used.into_iter().collect(),
        applications_passed,
        passed: summary.passed,
        test_failed: summary.failed,
        infrastructure_failed: summary.infrastructure_failed,
        attempts: summary.attempts,
        worker_lost_attempts: summary.worker_lost_attempts,
        public_runner_attempts,
        script_reports_linked,
        evidence_refs_linked: evidence_refs.len(),
        physical_resource_acquisitions: physical_acquisitions,
        max_concurrent_physical_jobs: max_physical,
        semantic_parallelism_proven,
        orphan_processes,
        run_summary: store.root().join("run-summary.json"),
        markers,
    };
    let path = store.root().join("windows-e2e-report.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write {}: {error}", path.display()))?;
    Ok(report)
}

fn script_name(job: &crate::model::TestJob) -> Result<&str, String> {
    job.metadata
        .get("script_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("job {} has no script_name metadata", job.job_id))
}

fn jobs_by_name<'a>(
    scheduler: &'a Scheduler,
    run_id: &str,
) -> Result<BTreeMap<String, &'a JobRecord>, String> {
    let run = scheduler
        .runs
        .get(run_id)
        .ok_or_else(|| format!("run not found: {run_id}"))?;
    let mut output = BTreeMap::new();
    for job_id in &run.job_ids {
        let job = scheduler
            .jobs
            .get(job_id)
            .ok_or_else(|| format!("job not found: {job_id}"))?;
        let name = script_name(&job.job)?.to_owned();
        if output.insert(name.clone(), job).is_some() {
            return Err(format!("duplicate job name in run: {name}"));
        }
    }
    Ok(output)
}

fn require_state(
    jobs: &BTreeMap<String, &JobRecord>,
    name: &str,
    expected: JobState,
) -> Result<(), String> {
    let job = jobs
        .get(name)
        .ok_or_else(|| format!("E2E job not found: {name}"))?;
    if job.state != expected {
        return Err(format!(
            "E2E job {name} has state {:?}, expected {expected:?}",
            job.state
        ));
    }
    Ok(())
}

fn verify_summary(summary: &RunSummary) -> Result<(), String> {
    if summary.status != RunStatus::Failed
        || summary.total_jobs != 7
        || summary.cancelled != 0
        || summary.timed_out != 0
        || summary.worker_lost != 0
        || !summary
            .markers
            .iter()
            .any(|marker| marker == "AI_UI_RESULT_AGGREGATION_PASSED")
    {
        return Err(format!(
            "Windows E2E run summary is inconsistent: {summary:?}"
        ));
    }
    Ok(())
}

fn verify_physical_isolation(scheduler: &Scheduler) -> Result<(usize, usize), String> {
    let mut active = 0_usize;
    let mut max_active = 0_usize;
    let mut acquisitions = 0_usize;
    for event in &scheduler.events {
        if event
            .metadata
            .get("resource")
            .and_then(serde_json::Value::as_str)
            != Some(WINDOWS_INTERACTIVE_DESKTOP)
        {
            continue;
        }
        match event.event.as_str() {
            "resource_acquired" => {
                active += 1;
                acquisitions += 1;
                max_active = max_active.max(active);
                if active > 1 {
                    return Err("overlapping interactive desktop leases detected".into());
                }
            }
            "resource_released" => {
                if active == 0 {
                    return Err("interactive desktop resource released without owner".into());
                }
                active -= 1;
            }
            _ => {}
        }
    }
    if active != 0 {
        return Err("interactive desktop resource remains acquired".into());
    }
    Ok((acquisitions, max_active))
}

fn semantic_parallelism_proven(scheduler: &Scheduler) -> bool {
    let attempts = scheduler
        .attempts
        .values()
        .filter(|attempt| attempt.status == JobState::Passed)
        .filter_map(|attempt| {
            let job = scheduler.jobs.get(&attempt.job_id)?;
            if !job.job.requirements.semantic_only {
                return None;
            }
            Some((
                attempt.worker_id.as_str(),
                job.job.application.key.as_str(),
                attempt.started_at_ms?,
                attempt.finished_at_ms?,
            ))
        })
        .collect::<Vec<_>>();
    attempts.iter().enumerate().any(|(index, left)| {
        attempts.iter().skip(index + 1).any(|right| {
            left.0 != right.0 && left.1 != right.1 && left.2 < right.3 && right.2 < left.3
        })
    })
}

fn process_images(
    applications: &TrustedApplicationCatalog,
    runner: &Path,
) -> Result<BTreeSet<String>, String> {
    let mut images = applications
        .applications
        .iter()
        .filter_map(|application| application.executable.file_name())
        .filter_map(|name| name.to_str())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let runner = runner
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("runner has no file name: {}", runner.display()))?;
    images.insert(runner.into());
    Ok(images)
}

fn snapshot_processes(
    images: &BTreeSet<String>,
) -> Result<BTreeMap<String, BTreeSet<u32>>, String> {
    let mut snapshot = BTreeMap::new();
    for image in images {
        snapshot.insert(image.clone(), process_ids_for_image(image)?);
    }
    Ok(snapshot)
}

#[cfg(windows)]
fn process_ids_for_image(image: &str) -> Result<BTreeSet<u32>, String> {
    let output = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH", "/FI", &format!("IMAGENAME eq {image}")])
        .output()
        .map_err(|error| format!("query process image {image}: {error}"))?;
    if !output.status.success() {
        return Err(format!("tasklist failed for image {image}"));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut pids = BTreeSet::new();
    for line in text.lines().filter(|line| line.starts_with('"')) {
        let columns = line.split("\",\"").collect::<Vec<_>>();
        if let Some(pid) = columns.get(1).and_then(|value| value.parse::<u32>().ok()) {
            pids.insert(pid);
        }
    }
    Ok(pids)
}

#[cfg(not(windows))]
fn process_ids_for_image(_image: &str) -> Result<BTreeSet<u32>, String> {
    Err("process snapshot is implemented only for Windows".into())
}

fn wait_for_new_processes_to_exit(
    images: &BTreeSet<String>,
    baseline: &BTreeMap<String, BTreeSet<u32>>,
    timeout_ms: u64,
) -> Result<Vec<String>, String> {
    let attempts = (timeout_ms / 50).max(1);
    let mut remaining = Vec::new();
    for _ in 0..=attempts {
        remaining.clear();
        for image in images {
            let current = process_ids_for_image(image)?;
            let previous = baseline.get(image).cloned().unwrap_or_default();
            remaining.extend(
                current
                    .difference(&previous)
                    .map(|pid| format!("{image}:{pid}")),
            );
        }
        if remaining.is_empty() {
            return Ok(remaining);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(remaining)
}
