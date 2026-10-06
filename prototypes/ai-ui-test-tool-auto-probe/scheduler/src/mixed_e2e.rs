use crate::{
    container::{ContainerEnvironment, ContainerReaperReport, container_worker_capabilities},
    container_runtime::{ContainerRuntime, DockerCliRuntime},
    environment::{
        ExecutionEnvironmentKind, ExecutionEnvironmentRequirement,
        TrustedExecutionEnvironmentCatalog,
    },
    execution::LocalProcessEnvironment,
    model::{RetryPolicy, RunStatus, WorkerCapabilities},
    persistence::RunStore,
    pool::{WorkerPool, WorkerPoolConfig, now_unix_ms},
    scheduler::{Scheduler, SchedulerConfig},
    submission::{SubmissionOptions, TrustedApplicationCatalog, submit_source},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

pub const MIXED_ENVIRONMENT_E2E_REPORT_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct MixedEnvironmentE2eConfig {
    pub run_id: String,
    pub output: PathBuf,
    pub local_source: PathBuf,
    pub container_source: PathBuf,
    pub applications: PathBuf,
    pub environments: PathBuf,
    pub container_environment_key: String,
    pub public_runner: PathBuf,
    pub working_dir: PathBuf,
    pub container_runtime: PathBuf,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MixedEnvironmentE2eReport {
    pub version: u32,
    pub run_id: String,
    pub status: String,
    pub jobs: usize,
    pub local_job_id: String,
    pub container_job_id: String,
    pub local_worker_id: String,
    pub container_worker_id: String,
    pub local_script_report: PathBuf,
    pub container_script_report: PathBuf,
    pub container_environment_report: PathBuf,
    pub run_summary: PathBuf,
    pub report_path: PathBuf,
    pub canonical_report_fields: Vec<String>,
    pub environment_id: String,
    pub container_id: String,
    pub image_digest: Option<String>,
    pub preflight_reaper: ContainerReaperReport,
    pub reconciliation_reaper: ContainerReaperReport,
    pub leaked_owned_containers: usize,
    pub markers: Vec<String>,
}

pub fn run_mixed_environment_e2e(
    config: MixedEnvironmentE2eConfig,
) -> Result<MixedEnvironmentE2eReport, String> {
    if config.run_id.trim().is_empty() {
        return Err("mixed E2E run_id must not be empty".into());
    }
    if config.timeout_ms == 0 {
        return Err("mixed E2E timeout must be greater than zero".into());
    }
    let output = absolute_path(&config.output)?;
    if output.join("scheduler-state.json").exists() {
        return Err(format!(
            "mixed E2E output already contains scheduler state: {}",
            output.display()
        ));
    }
    let public_runner = absolute_path(&config.public_runner)?;
    let working_dir = absolute_path(&config.working_dir)?;
    let applications = TrustedApplicationCatalog::load(&config.applications)?;
    let environment_catalog = TrustedExecutionEnvironmentCatalog::load(&config.environments)?;
    let container_spec = environment_catalog
        .resolve(&config.container_environment_key)?
        .clone();
    let container_capabilities = container_worker_capabilities(&container_spec, 1)?;

    let mut retry = RetryPolicy::default();
    retry.max_attempts = 1;
    retry.retry_infrastructure_failures = false;
    let now = now_unix_ms();
    let mut local_options = SubmissionOptions::for_run(&config.run_id, &output);
    local_options.timeout_ms = config.timeout_ms;
    local_options.retry = retry.clone();
    local_options.application_key = Some("local-environment-probe".into());
    local_options.metadata.insert(
        "mixed_environment_role".into(),
        Value::String("local".into()),
    );
    let local_jobs = submit_source(&config.local_source, &applications, &local_options, now)?;

    let mut container_options = SubmissionOptions::for_run(&config.run_id, &output);
    container_options.timeout_ms = config.timeout_ms;
    container_options.retry = retry;
    container_options.application_key = Some("environment-probe".into());
    container_options.execution_environment = ExecutionEnvironmentRequirement {
        environment_key: container_spec.key.clone(),
        kind: container_spec.kind,
        minimum_isolation: container_spec.isolation,
    };
    container_options.selected_environment_capabilities = Some(container_capabilities.clone());
    container_options.metadata.insert(
        "mixed_environment_role".into(),
        Value::String("container".into()),
    );
    let container_jobs = submit_source(
        &config.container_source,
        &applications,
        &container_options,
        now,
    )?;
    if local_jobs.len() != 1 || container_jobs.len() != 1 {
        return Err("mixed E2E requires exactly one local and one container script".into());
    }
    let local_job_id = local_jobs[0].job_id.clone();
    let container_job_id = container_jobs[0].job_id.clone();
    if local_job_id == container_job_id {
        return Err("mixed E2E generated duplicate job IDs".into());
    }
    let mut jobs = local_jobs;
    jobs.extend(container_jobs);

    let mut scheduler = Scheduler::new(SchedulerConfig::default());
    scheduler.submit_run(&config.run_id, jobs, now)?;
    let store = RunStore::new(&output);
    store.write_all(&scheduler, &config.run_id)?;

    let runtime: Arc<dyn ContainerRuntime> =
        Arc::new(DockerCliRuntime::new(config.container_runtime));
    let container_environment = Arc::new(ContainerEnvironment::new(
        container_spec,
        runtime,
        output.join("environment-runtime"),
        owner_scope(&config.run_id),
    )?);
    let preflight_reaper = container_environment.reap_owned();
    write_reaper_report(&output, "preflight", &preflight_reaper)?;
    require_clean_reaper("preflight", &preflight_reaper)?;

    let mut pool_config = WorkerPoolConfig::default();
    pool_config.no_progress_timeout_ms = config.timeout_ms.saturating_add(30_000);
    let mut pool = WorkerPool::new(&config.run_id, scheduler, store.clone(), pool_config);
    pool.add_local_workers(
        1,
        WorkerCapabilities::default(),
        Arc::new(LocalProcessEnvironment::new(public_runner, working_dir)),
    )?;
    pool.add_workers(
        "container-worker",
        1,
        container_capabilities,
        container_environment.clone(),
    )?;
    let result = pool.run()?;

    let reconciliation_reaper = container_environment.reap_owned();
    write_reaper_report(&output, "reconciliation", &reconciliation_reaper)?;
    require_clean_reaper("reconciliation", &reconciliation_reaper)?;
    if result.summary.status != RunStatus::Passed
        || result.summary.total_jobs != 2
        || result.summary.passed != 2
        || result.summary.environment.attempts != 1
        || !result.summary.environment.cleanup_succeeded
        || result.summary.environment.owned_resources_remaining != 0
    {
        return Err(format!(
            "mixed environment run did not satisfy result/cleanup contract: {:?}",
            result.summary.status
        ));
    }
    if !result
        .summary
        .markers
        .iter()
        .any(|marker| marker == "AI_UI_ENVIRONMENT_REPORT_AGGREGATION_PASSED")
    {
        return Err("mixed run has no validated environment aggregation marker".into());
    }

    let local_attempt = terminal_attempt(&result.scheduler, &local_job_id)?;
    let container_attempt = terminal_attempt(&result.scheduler, &container_job_id)?;
    if !local_attempt.worker_id.starts_with("local-worker:")
        || local_attempt.execution_environment.is_some()
        || local_attempt.environment_report.is_some()
    {
        return Err("local job was not executed by LocalProcessEnvironment".into());
    }
    if !container_attempt.worker_id.starts_with("container-worker:") {
        return Err("container job was not executed by ContainerEnvironment".into());
    }
    let metadata = container_attempt
        .execution_environment
        .as_ref()
        .ok_or_else(|| "container attempt has no environment metadata".to_string())?;
    if metadata.environment_type != ExecutionEnvironmentKind::Container
        || metadata.environment_key != config.container_environment_key
    {
        return Err("container attempt metadata does not match trusted environment".into());
    }
    let environment_report = container_attempt
        .environment_report
        .clone()
        .ok_or_else(|| "container attempt has no environment report".to_string())?;
    if !environment_report.is_file() {
        return Err(format!(
            "container environment report is missing: {}",
            environment_report.display()
        ));
    }
    let local_script_report = local_attempt
        .script_report
        .clone()
        .ok_or_else(|| "local attempt has no public script report".to_string())?;
    let container_script_report = container_attempt
        .script_report
        .clone()
        .ok_or_else(|| "container attempt has no public script report".to_string())?;
    let local_fields = canonical_report_fields(&local_script_report)?;
    let container_fields = canonical_report_fields(&container_script_report)?;
    if local_fields != container_fields {
        return Err(format!(
            "local/container public report contracts differ: local={local_fields:?}, container={container_fields:?}"
        ));
    }

    let report_path = output.join("mixed-environment-e2e-report.json");
    let report = MixedEnvironmentE2eReport {
        version: MIXED_ENVIRONMENT_E2E_REPORT_VERSION,
        run_id: config.run_id,
        status: "PASSED".into(),
        jobs: 2,
        local_job_id,
        container_job_id,
        local_worker_id: local_attempt.worker_id.clone(),
        container_worker_id: container_attempt.worker_id.clone(),
        local_script_report,
        container_script_report,
        container_environment_report: environment_report,
        run_summary: output.join("run-summary.json"),
        report_path: report_path.clone(),
        canonical_report_fields: local_fields,
        environment_id: metadata.environment_id.clone(),
        container_id: metadata.container_id.clone().unwrap_or_default(),
        image_digest: metadata.image_digest.clone(),
        preflight_reaper,
        reconciliation_reaper: reconciliation_reaper.clone(),
        leaked_owned_containers: reconciliation_reaper.remaining,
        markers: vec!["AI_UI_MIXED_ENVIRONMENT_SCHEDULING_PASSED".into()],
    };
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write mixed E2E report: {error}"))?;
    Ok(report)
}

fn terminal_attempt<'a>(
    scheduler: &'a Scheduler,
    job_id: &str,
) -> Result<&'a crate::model::AttemptRecord, String> {
    let job = scheduler
        .jobs
        .get(job_id)
        .ok_or_else(|| format!("mixed E2E job not found: {job_id}"))?;
    if !job.state.is_terminal() || job.attempt_ids.len() != 1 {
        return Err(format!(
            "mixed E2E job does not have exactly one terminal attempt: {job_id}"
        ));
    }
    scheduler
        .attempts
        .get(&job.attempt_ids[0])
        .ok_or_else(|| format!("mixed E2E attempt not found: {}", job.attempt_ids[0]))
}

fn canonical_report_fields(path: &Path) -> Result<Vec<String>, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("read public script report {}: {error}", path.display()))?;
    let report: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse public script report {}: {error}", path.display()))?;
    if report.get("status").and_then(Value::as_str) != Some("PASSED") {
        return Err(format!(
            "public script report is not PASSED: {}",
            path.display()
        ));
    }
    let required = [
        "status",
        "test",
        "summary",
        "steps",
        "failures",
        "evidence_refs",
        "markers",
    ];
    for field in required {
        if report.get(field).is_none() {
            return Err(format!(
                "public script report {} has no canonical field {field}",
                path.display()
            ));
        }
    }
    Ok(report
        .as_object()
        .map(|object| object.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default()
        .into_iter()
        .collect())
}

fn write_reaper_report(
    output: &Path,
    phase: &str,
    report: &ContainerReaperReport,
) -> Result<(), String> {
    let path = output.join(format!("container-reaper-{phase}.json"));
    fs::write(
        &path,
        serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write {}: {error}", path.display()))
}

fn require_clean_reaper(phase: &str, report: &ContainerReaperReport) -> Result<(), String> {
    if report.remaining == 0 && report.failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "CONTAINER_CLEANUP_FAILED during mixed {phase}: remaining={}, failures={}",
            report.remaining,
            serde_json::to_string(&report.failures).unwrap_or_else(|_| "unavailable".into())
        ))
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path))
    }
}

fn owner_scope(run_id: &str) -> String {
    let mut safe = run_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if safe.is_empty() {
        safe = "run".into();
    }
    safe.truncate(160);
    format!("ai-ui-scheduler-{safe}")
}
