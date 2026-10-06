use crate::{
    environment::{
        ENVIRONMENT_REPORT_VERSION, EXECUTION_ENVIRONMENT_CONTRACT_VERSION, EnvironmentFingerprint,
        EnvironmentIsolation, EnvironmentReport, EnvironmentState, EnvironmentTimings,
        ExecutionEnvironmentKind, ExecutionEnvironmentMetadata,
    },
    model::{AttemptRecord, JobState, RunStatus},
    scheduler::Scheduler,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

pub const RUN_SUMMARY_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub version: u32,
    pub run_id: String,
    pub status: RunStatus,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub total_jobs: usize,
    pub passed: usize,
    pub failed: usize,
    pub infrastructure_failed: usize,
    pub timed_out: usize,
    pub cancelled: usize,
    pub worker_lost: usize,
    pub skipped: usize,
    pub attempts: usize,
    pub worker_lost_attempts: usize,
    pub workers_used: Vec<String>,
    pub job_report_refs: Vec<JobReportRef>,
    pub markers: Vec<String>,
    #[serde(default)]
    pub environment: RunEnvironmentSummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEnvironmentSummary {
    pub attempts: usize,
    pub environment_types: Vec<ExecutionEnvironmentKind>,
    pub environment_keys: Vec<String>,
    pub environment_ids: Vec<String>,
    pub isolation_modes: Vec<EnvironmentIsolation>,
    pub report_refs: Vec<PathBuf>,
    pub failure_codes: Vec<String>,
    pub fingerprints: Vec<EnvironmentFingerprint>,
    pub timings: EnvironmentTimings,
    pub reports_observed: usize,
    pub cleanup_succeeded: bool,
    pub cleanup_failures: usize,
    pub owned_resources_remaining: usize,
    pub report_errors: Vec<String>,
}

impl Default for RunEnvironmentSummary {
    fn default() -> Self {
        Self {
            attempts: 0,
            environment_types: Vec::new(),
            environment_keys: Vec::new(),
            environment_ids: Vec::new(),
            isolation_modes: Vec::new(),
            report_refs: Vec::new(),
            failure_codes: Vec::new(),
            fingerprints: Vec::new(),
            timings: EnvironmentTimings::default(),
            reports_observed: 0,
            cleanup_succeeded: true,
            cleanup_failures: 0,
            owned_resources_remaining: 0,
            report_errors: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobReportRef {
    pub job_id: String,
    pub state: JobState,
    pub attempts: Vec<AttemptReportRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptReportRef {
    pub attempt_id: String,
    pub worker_id: String,
    pub status: JobState,
    pub script_report: Option<PathBuf>,
    pub evidence_refs: Vec<PathBuf>,
    #[serde(default)]
    pub execution_environment: Option<ExecutionEnvironmentMetadata>,
    #[serde(default)]
    pub environment_report: Option<PathBuf>,
    #[serde(default)]
    pub error_code: Option<String>,
}

pub fn aggregate(scheduler: &Scheduler, run_id: &str) -> Result<RunSummary, String> {
    let run = scheduler
        .runs
        .get(run_id)
        .ok_or_else(|| format!("run not found: {run_id}"))?;
    let mut passed = 0;
    let mut failed = 0;
    let mut infrastructure_failed = 0;
    let mut timed_out = 0;
    let mut cancelled = 0;
    let mut worker_lost = 0;
    let mut skipped = 0;
    let mut attempts = 0;
    let mut worker_lost_attempts = 0;
    let mut workers_used = BTreeSet::new();
    let mut job_report_refs = Vec::new();
    let mut environment = RunEnvironmentSummary::default();

    for job_id in &run.job_ids {
        let job = scheduler
            .jobs
            .get(job_id)
            .ok_or_else(|| format!("run references missing job: {job_id}"))?;
        match job.state {
            JobState::Passed => passed += 1,
            JobState::Failed => failed += 1,
            JobState::InfrastructureFailed => infrastructure_failed += 1,
            JobState::TimedOut => timed_out += 1,
            JobState::Cancelled => cancelled += 1,
            JobState::WorkerLost => worker_lost += 1,
            JobState::Skipped => skipped += 1,
            _ => {}
        }
        let mut attempt_refs = Vec::new();
        for attempt_id in &job.attempt_ids {
            let attempt = scheduler
                .attempts
                .get(attempt_id)
                .ok_or_else(|| format!("job references missing attempt: {attempt_id}"))?;
            attempts += 1;
            workers_used.insert(attempt.worker_id.clone());
            if attempt.status == JobState::WorkerLost {
                worker_lost_attempts += 1;
            }
            observe_environment_attempt(&mut environment, attempt);
            attempt_refs.push(AttemptReportRef {
                attempt_id: attempt.attempt_id.clone(),
                worker_id: attempt.worker_id.clone(),
                status: attempt.status,
                script_report: attempt.script_report.clone(),
                evidence_refs: attempt.evidence_refs.clone(),
                execution_environment: attempt.execution_environment.clone(),
                environment_report: attempt.environment_report.clone(),
                error_code: attempt
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.error_code.clone()),
            });
        }
        job_report_refs.push(JobReportRef {
            job_id: job_id.clone(),
            state: job.state,
            attempts: attempt_refs,
        });
    }

    let mut markers = Vec::new();
    if run.status.is_terminal()
        && scheduler.assert_consistent().is_ok()
        && passed + failed + infrastructure_failed + timed_out + cancelled + worker_lost + skipped
            == run.job_ids.len()
    {
        markers.push("AI_UI_RESULT_AGGREGATION_PASSED".into());
    }
    if environment.attempts > 0
        && environment.reports_observed == environment.attempts
        && environment.cleanup_succeeded
        && environment.cleanup_failures == 0
        && environment.owned_resources_remaining == 0
        && environment.report_errors.is_empty()
    {
        markers.push("AI_UI_ENVIRONMENT_REPORT_AGGREGATION_PASSED".into());
    }

    Ok(RunSummary {
        version: RUN_SUMMARY_VERSION,
        run_id: run_id.into(),
        status: run.status,
        started_at_ms: run.started_at_ms,
        finished_at_ms: run.finished_at_ms,
        total_jobs: run.job_ids.len(),
        passed,
        failed,
        infrastructure_failed,
        timed_out,
        cancelled,
        worker_lost,
        skipped,
        attempts,
        worker_lost_attempts,
        workers_used: workers_used.into_iter().collect(),
        job_report_refs,
        markers,
        environment,
    })
}

fn observe_environment_attempt(summary: &mut RunEnvironmentSummary, attempt: &AttemptRecord) {
    if attempt.execution_environment.is_none() && attempt.environment_report.is_none() {
        return;
    }

    summary.attempts += 1;
    if let Some(code) = attempt
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.error_code.as_ref())
    {
        push_sorted_unique(&mut summary.failure_codes, code.clone());
    }

    let mut cleanup_ok = true;
    let mut parsed_report = None;
    match &attempt.environment_report {
        Some(path) => {
            push_sorted_unique(&mut summary.report_refs, path.clone());
            match load_environment_report(path) {
                Ok(report) => {
                    summary.reports_observed += 1;
                    summary.owned_resources_remaining = summary
                        .owned_resources_remaining
                        .saturating_add(report.owned_resources_remaining);
                    if let Some(code) = &report.failure_code {
                        push_sorted_unique(&mut summary.failure_codes, code.clone());
                    }
                    if report.version != ENVIRONMENT_REPORT_VERSION {
                        record_report_error(
                            summary,
                            attempt,
                            format!(
                                "environment report version {} != {}",
                                report.version, ENVIRONMENT_REPORT_VERSION
                            ),
                        );
                        cleanup_ok = false;
                    }
                    if report.metadata.version != EXECUTION_ENVIRONMENT_CONTRACT_VERSION {
                        record_report_error(
                            summary,
                            attempt,
                            format!(
                                "environment metadata version {} != {}",
                                report.metadata.version, EXECUTION_ENVIRONMENT_CONTRACT_VERSION
                            ),
                        );
                        cleanup_ok = false;
                    }
                    if report.identity.attempt_id != attempt.attempt_id {
                        record_report_error(
                            summary,
                            attempt,
                            "environment report attempt identity mismatch",
                        );
                        cleanup_ok = false;
                    }
                    if report.identity.worker_id != attempt.worker_id {
                        record_report_error(
                            summary,
                            attempt,
                            "environment report worker identity mismatch",
                        );
                        cleanup_ok = false;
                    }
                    if report.identity.environment_id != report.metadata.environment_id
                        || report.identity.container_id != report.metadata.container_id
                        || report.identity.capabilities != report.metadata.capabilities
                        || report.metadata.fingerprint.environment_type
                            != report.metadata.environment_type
                    {
                        record_report_error(
                            summary,
                            attempt,
                            "environment report identity/metadata mismatch",
                        );
                        cleanup_ok = false;
                    }
                    if attempt
                        .execution_environment
                        .as_ref()
                        .is_some_and(|metadata| metadata != &report.metadata)
                    {
                        record_report_error(
                            summary,
                            attempt,
                            "attempt and environment report metadata differ",
                        );
                        cleanup_ok = false;
                    }
                    if report.identity.state != EnvironmentState::Destroyed
                        || report
                            .lifecycle
                            .last()
                            .is_none_or(|event| event.state != EnvironmentState::Destroyed)
                        || !report.cleanup_succeeded
                        || report.owned_resources_remaining != 0
                    {
                        cleanup_ok = false;
                    }
                    if !report.artifact_refs.contains(path) {
                        record_report_error(
                            summary,
                            attempt,
                            "environment report does not reference itself",
                        );
                        cleanup_ok = false;
                    }
                    parsed_report = Some(report);
                }
                Err(error) => {
                    record_report_error(summary, attempt, error);
                    cleanup_ok = false;
                }
            }
        }
        None => {
            record_report_error(
                summary,
                attempt,
                "attempt has environment metadata but no environment report",
            );
            cleanup_ok = false;
        }
    }

    let metadata = attempt
        .execution_environment
        .as_ref()
        .or_else(|| parsed_report.as_ref().map(|report| &report.metadata));
    match metadata {
        Some(metadata) => observe_environment_metadata(summary, metadata),
        None => {
            record_report_error(summary, attempt, "environment metadata is missing");
            cleanup_ok = false;
        }
    }
    if attempt.execution_environment.is_none() {
        record_report_error(summary, attempt, "attempt environment metadata is missing");
        cleanup_ok = false;
    }

    if !cleanup_ok {
        summary.cleanup_succeeded = false;
        summary.cleanup_failures += 1;
    }
}

fn observe_environment_metadata(
    summary: &mut RunEnvironmentSummary,
    metadata: &ExecutionEnvironmentMetadata,
) {
    push_sorted_unique(&mut summary.environment_types, metadata.environment_type);
    push_sorted_unique(
        &mut summary.environment_keys,
        metadata.environment_key.clone(),
    );
    push_sorted_unique(
        &mut summary.environment_ids,
        metadata.environment_id.clone(),
    );
    push_sorted_unique(&mut summary.isolation_modes, metadata.isolation_mode);
    if !summary.fingerprints.contains(&metadata.fingerprint) {
        summary.fingerprints.push(metadata.fingerprint.clone());
        summary.fingerprints.sort_by(|left, right| {
            left.os
                .cmp(&right.os)
                .then(left.environment_type.cmp(&right.environment_type))
                .then(left.image_digest.cmp(&right.image_digest))
                .then(left.tool_version.cmp(&right.tool_version))
                .then(left.runner_version.cmp(&right.runner_version))
                .then(left.application_version.cmp(&right.application_version))
                .then(left.gpu_available.cmp(&right.gpu_available))
                .then(left.gui_level.cmp(&right.gui_level))
        });
    }
    add_timings(&mut summary.timings, &metadata.timings);
}

fn add_timings(total: &mut EnvironmentTimings, value: &EnvironmentTimings) {
    total.environment_prepare_ms = total
        .environment_prepare_ms
        .saturating_add(value.environment_prepare_ms);
    total.startup_ms = total.startup_ms.saturating_add(value.startup_ms);
    total.execution_ms = total.execution_ms.saturating_add(value.execution_ms);
    total.artifact_collection_ms = total
        .artifact_collection_ms
        .saturating_add(value.artifact_collection_ms);
    total.cleanup_ms = total.cleanup_ms.saturating_add(value.cleanup_ms);
}

fn load_environment_report(path: &Path) -> Result<EnvironmentReport, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("read environment report {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse environment report {}: {error}", path.display()))
}

fn record_report_error(
    summary: &mut RunEnvironmentSummary,
    attempt: &AttemptRecord,
    message: impl AsRef<str>,
) {
    summary
        .report_errors
        .push(format!("{}: {}", attempt.attempt_id, message.as_ref()));
}

fn push_sorted_unique<T: Ord>(values: &mut Vec<T>, value: T) {
    match values.binary_search(&value) {
        Ok(_) => {}
        Err(index) => values.insert(index, value),
    }
}

pub fn write(output: impl AsRef<Path>, summary: &RunSummary) -> Result<PathBuf, String> {
    let output = output.as_ref();
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    let path = output.join("run-summary.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(summary).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        environment::{
            EnvironmentCapabilities, EnvironmentIdentity, ExecutionEnvironmentRequirement,
            LifecycleEvent, WorkerEnvironmentCapabilities,
        },
        model::{
            ApplicationSpec, ArtifactSpec, ExecutionRequirements, JobSource, RetryPolicy,
            TEST_JOB_VERSION, TestJob, WorkerCapabilities,
        },
        scheduler::{AttemptOutcome, SchedulerConfig},
    };
    use std::{
        collections::BTreeMap,
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn job() -> TestJob {
        TestJob {
            version: TEST_JOB_VERSION,
            job_id: "job:summary".into(),
            run_id: "run:summary".into(),
            application: ApplicationSpec {
                key: "app".into(),
                executable: "app.exe".into(),
                args: Vec::new(),
                max_parallel: 1,
            },
            source: JobSource::Script {
                path: "summary.aiui.json".into(),
            },
            fixtures: None,
            priority: 0,
            timeout_ms: 1_000,
            retry: RetryPolicy::default(),
            requirements: ExecutionRequirements {
                semantic_only: true,
                ..ExecutionRequirements::default()
            },
            artifacts: ArtifactSpec {
                output_dir: "reports/summary".into(),
            },
            metadata: BTreeMap::new(),
            created_at_ms: 0,
            retry_safe: true,
        }
    }

    #[test]
    fn terminal_run_aggregates_attempt_refs_without_embedding_reports() {
        let mut scheduler = Scheduler::new(SchedulerConfig::default());
        scheduler.submit_run("run:summary", vec![job()], 0).unwrap();
        scheduler
            .register_worker(
                "run:summary",
                "worker:1",
                "instance:1",
                WorkerCapabilities::default(),
                1,
                0,
            )
            .unwrap();
        let grant = scheduler
            .lease_next("run:summary", "worker:1", 1)
            .unwrap()
            .unwrap();
        scheduler.mark_starting(&grant.lease.lease_id, 2).unwrap();
        scheduler.mark_running(&grant.lease.lease_id, 3).unwrap();
        let mut outcome = AttemptOutcome::passed();
        outcome.script_report = Some("reports/script/report.json".into());
        outcome.evidence_refs = vec!["reports/script/evidence".into()];
        scheduler
            .complete_attempt(&grant.lease.lease_id, outcome, 4)
            .unwrap();

        let summary = aggregate(&scheduler, "run:summary").unwrap();
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.attempts, 1);
        assert_eq!(
            summary.job_report_refs[0].attempts[0].script_report,
            Some(PathBuf::from("reports/script/report.json"))
        );
        assert!(
            summary
                .markers
                .contains(&"AI_UI_RESULT_AGGREGATION_PASSED".into())
        );
    }

    #[test]
    fn environment_report_is_validated_and_aggregated() {
        let mut container_job = job();
        container_job.requirements = ExecutionRequirements {
            platform: "linux".into(),
            interactive_desktop: false,
            gpu: false,
            semantic_ui: false,
            physical_input: false,
            makepad: false,
            semantic_only: true,
            execution_environment: ExecutionEnvironmentRequirement {
                environment_key: "container-test".into(),
                kind: ExecutionEnvironmentKind::Container,
                minimum_isolation: EnvironmentIsolation::Filesystem,
            },
        };
        let capabilities = WorkerCapabilities {
            platform: "linux".into(),
            interactive_desktop: false,
            gpu: false,
            max_parallel: 1,
            semantic_ui: false,
            physical_input: false,
            makepad: false,
            execution_environment: WorkerEnvironmentCapabilities {
                environment_key: "container-test".into(),
                kind: ExecutionEnvironmentKind::Container,
                isolation: EnvironmentIsolation::Filesystem,
                gui_level: Default::default(),
                reusable: false,
            },
        };
        let mut scheduler = Scheduler::new(SchedulerConfig::default());
        scheduler
            .submit_run("run:summary", vec![container_job], 0)
            .unwrap();
        scheduler
            .register_worker(
                "run:summary",
                "container-worker:1",
                "instance:1",
                capabilities,
                1,
                0,
            )
            .unwrap();
        let grant = scheduler
            .lease_next("run:summary", "container-worker:1", 1)
            .unwrap()
            .unwrap();
        scheduler.mark_starting(&grant.lease.lease_id, 2).unwrap();
        scheduler.mark_running(&grant.lease.lease_id, 3).unwrap();

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ai-ui-environment-summary-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let report_path = root.join("environment-report.json");
        let timings = EnvironmentTimings {
            environment_prepare_ms: 1,
            startup_ms: 2,
            execution_ms: 3,
            artifact_collection_ms: 4,
            cleanup_ms: 5,
        };
        let environment_capabilities = EnvironmentCapabilities {
            process_isolation: true,
            filesystem_isolation: true,
            environment_isolation: true,
            ..EnvironmentCapabilities::default()
        };
        let metadata = ExecutionEnvironmentMetadata {
            version: EXECUTION_ENVIRONMENT_CONTRACT_VERSION,
            environment_type: ExecutionEnvironmentKind::Container,
            environment_key: "container-test".into(),
            environment_id: "environment:test".into(),
            container_id: Some("container:test".into()),
            image: Some("trusted:test".into()),
            image_digest: Some("sha256:test".into()),
            isolation_mode: EnvironmentIsolation::Filesystem,
            capabilities: environment_capabilities.clone(),
            fingerprint: EnvironmentFingerprint {
                os: "linux/amd64".into(),
                environment_type: ExecutionEnvironmentKind::Container,
                image_digest: Some("sha256:test".into()),
                tool_version: "test".into(),
                runner_version: "public-script-api-v1".into(),
                application_version: None,
                gpu_available: false,
                gui_level: Default::default(),
            },
            timings,
        };
        let report = EnvironmentReport {
            version: ENVIRONMENT_REPORT_VERSION,
            identity: EnvironmentIdentity {
                environment_id: metadata.environment_id.clone(),
                container_id: metadata.container_id.clone(),
                worker_id: grant.attempt.worker_id.clone(),
                attempt_id: grant.attempt.attempt_id.clone(),
                created_at_ms: 1,
                state: EnvironmentState::Destroyed,
                capabilities: environment_capabilities,
            },
            metadata: metadata.clone(),
            lifecycle: vec![
                LifecycleEvent {
                    state: EnvironmentState::Created,
                    timestamp_ms: 1,
                    message: None,
                },
                LifecycleEvent {
                    state: EnvironmentState::Destroyed,
                    timestamp_ms: 2,
                    message: None,
                },
            ],
            cleanup_succeeded: true,
            owned_resources_remaining: 0,
            failure_code: None,
            artifact_refs: vec![report_path.clone()],
        };
        fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        let mut outcome = AttemptOutcome::passed();
        outcome.execution_environment = Some(metadata);
        outcome.environment_report = Some(report_path);
        scheduler
            .complete_attempt(&grant.lease.lease_id, outcome, 4)
            .unwrap();

        let summary = aggregate(&scheduler, "run:summary").unwrap();
        assert_eq!(summary.version, RUN_SUMMARY_VERSION);
        assert_eq!(summary.environment.attempts, 1);
        assert_eq!(summary.environment.reports_observed, 1);
        assert!(summary.environment.cleanup_succeeded);
        assert_eq!(summary.environment.timings.execution_ms, 3);
        assert!(summary.environment.report_errors.is_empty());
        assert!(
            summary
                .markers
                .contains(&"AI_UI_ENVIRONMENT_REPORT_AGGREGATION_PASSED".into())
        );
        fs::remove_dir_all(root).unwrap();
    }
}
