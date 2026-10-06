use crate::{
    container::{ContainerReaperReport, container_worker_capabilities},
    container_runtime::{ContainerRuntime, DockerCliRuntime, RuntimeContainerStatus},
    environment::{
        EnvironmentReport, ExecutionEnvironmentKind, ExecutionEnvironmentRequirement,
        ExecutionEnvironmentSpec, TrustedExecutionEnvironmentCatalog,
    },
    model::{FailureClassification, JobState, RetryPolicy, RunStatus},
    persistence::RunStore,
    pool::now_unix_ms,
    scheduler::{Scheduler, SchedulerConfig},
    submission::{SubmissionOptions, TrustedApplicationCatalog, submit_source},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

pub const CONTAINER_WORKER_LOSS_E2E_REPORT_VERSION: u32 = 1;
const RECOVERY_ENVIRONMENT_KEY: &str = "docker-linux-level0-worker-loss";

#[derive(Clone, Debug)]
pub struct ContainerWorkerLossE2eConfig {
    pub run_id: String,
    pub output: PathBuf,
    pub source: PathBuf,
    pub applications: PathBuf,
    pub environments: PathBuf,
    pub wait_environment_key: String,
    pub pass_environment_key: String,
    pub scheduler_executable: PathBuf,
    pub container_runtime: PathBuf,
    pub startup_timeout_ms: u64,
    pub worker_timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerWorkerLossE2eReport {
    pub version: u32,
    pub run_id: String,
    pub status: String,
    pub crashed_scheduler_pid: u32,
    pub old_attempt_id: String,
    pub new_attempt_id: String,
    pub old_worker_instance_id: String,
    pub new_worker_instance_id: String,
    pub old_container_id: String,
    pub new_container_id: String,
    pub stale_container_was_running: bool,
    pub old_attempt_status: JobState,
    pub new_attempt_status: JobState,
    pub preflight_reaper: ContainerReaperReport,
    pub reconciliation_reaper: ContainerReaperReport,
    pub environment_report: PathBuf,
    pub run_summary: PathBuf,
    pub first_process_stdout: PathBuf,
    pub first_process_stderr: PathBuf,
    pub recovery_process_stdout: PathBuf,
    pub recovery_process_stderr: PathBuf,
    pub owned_containers_remaining: usize,
    pub markers: Vec<String>,
}

pub fn run_container_worker_loss_e2e(
    config: ContainerWorkerLossE2eConfig,
) -> Result<ContainerWorkerLossE2eReport, String> {
    validate_config(&config)?;
    let output = absolute_path(&config.output)?;
    if output.join("scheduler-state.json").exists() {
        return Err(format!(
            "worker-loss E2E output already contains scheduler state: {}",
            output.display()
        ));
    }
    fs::create_dir_all(&output)
        .map_err(|error| format!("create worker-loss E2E output: {error}"))?;

    let source_catalog = TrustedExecutionEnvironmentCatalog::load(&config.environments)?;
    let wait_spec = recovery_spec(
        source_catalog.resolve(&config.wait_environment_key)?,
        RECOVERY_ENVIRONMENT_KEY,
    )?;
    let pass_spec = recovery_spec(
        source_catalog.resolve(&config.pass_environment_key)?,
        RECOVERY_ENVIRONMENT_KEY,
    )?;
    if wait_spec.image_digest != pass_spec.image_digest || wait_spec.image != pass_spec.image {
        return Err("worker-loss wait/pass environments must use the same trusted image".into());
    }

    let runtime: Arc<dyn ContainerRuntime> =
        Arc::new(DockerCliRuntime::new(&config.container_runtime));
    let health = runtime.health();
    if !health.healthy {
        return Err(format!(
            "container runtime is unhealthy: {}: {}",
            health
                .error_code
                .as_deref()
                .unwrap_or("CONTAINER_RUNTIME_UNAVAILABLE"),
            health.message.as_deref().unwrap_or("no details")
        ));
    }
    let owner_scope = owner_scope(&config.run_id);
    let mut scope_cleanup = OwnedScopeCleanup::new(runtime.clone(), owner_scope.clone());
    require_no_owned(runtime.as_ref(), &owner_scope, "initial preflight")?;

    let recovery_catalog_path = output.join("worker-loss-environments.json");
    write_single_environment_catalog(&recovery_catalog_path, wait_spec.clone())?;
    let applications = TrustedApplicationCatalog::load(&config.applications)?;
    let capabilities = container_worker_capabilities(&wait_spec, 1)?;
    let mut retry = RetryPolicy::default();
    retry.max_attempts = 2;
    retry.retry_test_failures = false;
    retry.retry_infrastructure_failures = false;
    retry.retry_timeouts = false;
    retry.retry_worker_lost = true;
    let mut options = SubmissionOptions::for_run(&config.run_id, &output);
    options.timeout_ms = config.startup_timeout_ms.saturating_mul(2).max(5_000);
    options.retry = retry;
    options.application_key = Some("environment-probe".into());
    options.execution_environment = ExecutionEnvironmentRequirement {
        environment_key: RECOVERY_ENVIRONMENT_KEY.into(),
        kind: ExecutionEnvironmentKind::Container,
        minimum_isolation: wait_spec.isolation,
    };
    options.selected_environment_capabilities = Some(capabilities);
    options.metadata.insert(
        "verification_role".into(),
        Value::String("container_worker_loss".into()),
    );
    let jobs = submit_source(&config.source, &applications, &options, now_unix_ms())?;
    if jobs.len() != 1 {
        return Err("worker-loss E2E requires exactly one probe job".into());
    }
    let job_id = jobs[0].job_id.clone();
    let mut scheduler_config = SchedulerConfig::default();
    scheduler_config.worker_timeout_ms = config.worker_timeout_ms;
    scheduler_config.lease_duration_ms = config.worker_timeout_ms.saturating_mul(2).max(200);
    let mut scheduler = Scheduler::new(scheduler_config);
    scheduler.submit_run(&config.run_id, jobs, now_unix_ms())?;
    let store = RunStore::new(&output);
    store.write_all(&scheduler, &config.run_id)?;

    let first_stdout = output.join("crashed-scheduler-stdout.log");
    let first_stderr = output.join("crashed-scheduler-stderr.log");
    let first_child = spawn_resume_scheduler(
        &config,
        &output,
        &recovery_catalog_path,
        &first_stdout,
        &first_stderr,
    )?;
    let crashed_scheduler_pid = first_child.id();
    let mut first_child = ChildGuard::new(first_child);
    let (old_attempt_id, old_worker_instance_id, old_container_id) = wait_for_running_container(
        &store,
        &config.run_id,
        &job_id,
        runtime.as_ref(),
        &owner_scope,
        first_child.child_mut()?,
        Duration::from_millis(config.startup_timeout_ms),
        &first_stdout,
        &first_stderr,
    )?;
    let stale_container_was_running = matches!(
        runtime
            .state(&old_container_id)
            .map_err(|error| error.to_string())?
            .status,
        RuntimeContainerStatus::Running
    );
    if !stale_container_was_running {
        return Err("worker-loss probe container was not running before process loss".into());
    }
    first_child.terminate_expected()?;

    let stale = runtime
        .list_owned(&owner_scope)
        .map_err(|error| error.to_string())?;
    if stale.len() != 1 || stale[0].id != old_container_id {
        return Err(format!(
            "scheduler process loss did not leave exactly the expected owned container: {stale:?}"
        ));
    }

    write_single_environment_catalog(&recovery_catalog_path, pass_spec)?;
    wait_until_worker_is_stale(&store, &config.run_id, config.worker_timeout_ms)?;

    let recovery_stdout = output.join("recovery-scheduler-stdout.log");
    let recovery_stderr = output.join("recovery-scheduler-stderr.log");
    let recovery_child = spawn_resume_scheduler(
        &config,
        &output,
        &recovery_catalog_path,
        &recovery_stdout,
        &recovery_stderr,
    )?;
    let mut recovery_child = ChildGuard::new(recovery_child);
    recovery_child.wait_success(
        Duration::from_millis(config.startup_timeout_ms.saturating_mul(2)),
        &recovery_stdout,
        &recovery_stderr,
    )?;

    let final_scheduler = store.load_scheduler()?;
    let run = final_scheduler
        .runs
        .get(&config.run_id)
        .ok_or_else(|| "worker-loss E2E run disappeared".to_string())?;
    if run.status != RunStatus::Passed {
        return Err(format!(
            "worker-loss recovery run is not PASSED: {:?}",
            run.status
        ));
    }
    let job = final_scheduler
        .jobs
        .get(&job_id)
        .ok_or_else(|| "worker-loss E2E job disappeared".to_string())?;
    if job.attempt_ids.len() != 2 {
        return Err(format!(
            "worker-loss recovery expected two attempts, observed {}",
            job.attempt_ids.len()
        ));
    }
    if job.attempt_ids[0] != old_attempt_id {
        return Err("worker-loss recovery replaced the historical attempt identity".into());
    }
    let old_attempt = &final_scheduler.attempts[&job.attempt_ids[0]];
    let new_attempt = &final_scheduler.attempts[&job.attempt_ids[1]];
    if old_attempt.status != JobState::WorkerLost
        || old_attempt.classification != Some(FailureClassification::WorkerLost)
    {
        return Err("old attempt was not classified as WorkerLost".into());
    }
    if new_attempt.status != JobState::Passed || new_attempt.classification.is_some() {
        return Err("replacement attempt did not pass cleanly".into());
    }
    if old_attempt.attempt_id == new_attempt.attempt_id
        || old_attempt.worker_instance_id == new_attempt.worker_instance_id
    {
        return Err("replacement attempt reused stale attempt/worker instance identity".into());
    }
    if old_attempt.worker_instance_id != old_worker_instance_id {
        return Err("persisted old worker instance changed during recovery".into());
    }
    let metadata = new_attempt
        .execution_environment
        .as_ref()
        .ok_or_else(|| "replacement attempt has no environment metadata".to_string())?;
    let new_container_id = metadata
        .container_id
        .clone()
        .ok_or_else(|| "replacement attempt metadata has no container id".to_string())?;
    if new_container_id == old_container_id {
        return Err("replacement attempt reused stale container identity".into());
    }
    let environment_report_path = new_attempt
        .environment_report
        .clone()
        .ok_or_else(|| "replacement attempt has no environment report".to_string())?;
    let environment_report: EnvironmentReport = read_json(&environment_report_path)?;
    if !environment_report.cleanup_succeeded || environment_report.owned_resources_remaining != 0 {
        return Err("replacement environment report does not prove clean cleanup".into());
    }
    let preflight_reaper: ContainerReaperReport =
        read_json(&output.join("container-reaper-preflight.json"))?;
    let reconciliation_reaper: ContainerReaperReport =
        read_json(&output.join("container-reaper-reconciliation.json"))?;
    if preflight_reaper.discovered != 1
        || preflight_reaper.removed != 1
        || preflight_reaper.remaining != 0
        || !preflight_reaper.failures.is_empty()
    {
        return Err(format!(
            "startup reaper did not remove exactly one stale owned container: {preflight_reaper:?}"
        ));
    }
    if reconciliation_reaper.remaining != 0 || !reconciliation_reaper.failures.is_empty() {
        return Err(format!(
            "post-recovery reconciliation is not clean: {reconciliation_reaper:?}"
        ));
    }
    if !final_scheduler.events.iter().any(|event| {
        event.run_id == config.run_id
            && event.event == "worker_lost"
            && event.worker_id.as_deref() == Some(old_attempt.worker_id.as_str())
    }) || !final_scheduler.events.iter().any(|event| {
        event.run_id == config.run_id
            && event.event == "retry_scheduled"
            && event.attempt_id.as_deref() == Some(old_attempt.attempt_id.as_str())
    }) {
        return Err("worker-loss recovery events are incomplete".into());
    }
    let remaining = runtime
        .list_owned(&owner_scope)
        .map_err(|error| error.to_string())?
        .len();
    if remaining != 0 {
        return Err(format!(
            "worker-loss recovery leaked {remaining} owned container(s)"
        ));
    }

    let report_path = output.join("container-worker-loss-e2e-report.json");
    let report = ContainerWorkerLossE2eReport {
        version: CONTAINER_WORKER_LOSS_E2E_REPORT_VERSION,
        run_id: config.run_id,
        status: "PASSED".into(),
        crashed_scheduler_pid,
        old_attempt_id: old_attempt.attempt_id.clone(),
        new_attempt_id: new_attempt.attempt_id.clone(),
        old_worker_instance_id: old_attempt.worker_instance_id.clone(),
        new_worker_instance_id: new_attempt.worker_instance_id.clone(),
        old_container_id,
        new_container_id,
        stale_container_was_running,
        old_attempt_status: old_attempt.status,
        new_attempt_status: new_attempt.status,
        preflight_reaper,
        reconciliation_reaper,
        environment_report: environment_report_path,
        run_summary: output.join("run-summary.json"),
        first_process_stdout: first_stdout,
        first_process_stderr: first_stderr,
        recovery_process_stdout: recovery_stdout,
        recovery_process_stderr: recovery_stderr,
        owned_containers_remaining: remaining,
        markers: vec![
            "AI_UI_CONTAINER_WORKER_LOSS_REAPER_PASSED".into(),
            "AI_UI_CONTAINER_STARTUP_RECOVERY_PASSED".into(),
        ],
    };
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write worker-loss E2E report: {error}"))?;
    scope_cleanup.disarm();
    Ok(report)
}

fn validate_config(config: &ContainerWorkerLossE2eConfig) -> Result<(), String> {
    if config.run_id.trim().is_empty() {
        return Err("worker-loss E2E run_id must not be empty".into());
    }
    if config.startup_timeout_ms < 1_000 {
        return Err("worker-loss E2E startup timeout must be at least 1000 ms".into());
    }
    if config.worker_timeout_ms < 50 || config.worker_timeout_ms >= config.startup_timeout_ms {
        return Err("worker timeout must be >= 50 ms and lower than startup timeout".into());
    }
    for (name, path) in [
        ("source", &config.source),
        ("applications", &config.applications),
        ("environments", &config.environments),
        ("scheduler executable", &config.scheduler_executable),
    ] {
        if !path.is_file() {
            return Err(format!(
                "worker-loss E2E {name} does not exist: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn recovery_spec(
    source: &ExecutionEnvironmentSpec,
    key: &str,
) -> Result<ExecutionEnvironmentSpec, String> {
    let mut spec = source.clone();
    spec.key = key.into();
    spec.validate().map_err(|errors| errors.join("; "))?;
    Ok(spec)
}

fn write_single_environment_catalog(
    path: &Path,
    spec: ExecutionEnvironmentSpec,
) -> Result<(), String> {
    let catalog = TrustedExecutionEnvironmentCatalog {
        version: crate::environment::EXECUTION_ENVIRONMENT_CONTRACT_VERSION,
        environments: vec![spec],
    };
    catalog.validate().map_err(|errors| errors.join("; "))?;
    fs::write(
        path,
        serde_json::to_vec_pretty(&catalog).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write recovery environment catalog: {error}"))
}

fn spawn_resume_scheduler(
    config: &ContainerWorkerLossE2eConfig,
    output: &Path,
    environments: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<Child, String> {
    let stdout = fs::File::create(stdout_path)
        .map_err(|error| format!("create {}: {error}", stdout_path.display()))?;
    let stderr = fs::File::create(stderr_path)
        .map_err(|error| format!("create {}: {error}", stderr_path.display()))?;
    let mut command = Command::new(&config.scheduler_executable);
    command
        .arg("run")
        .arg(output)
        .arg("--resume")
        .arg("--workers")
        .arg("1")
        .arg("--environment")
        .arg(RECOVERY_ENVIRONMENT_KEY)
        .arg("--environments")
        .arg(environments)
        .arg("--container-runtime")
        .arg(&config.container_runtime)
        .arg("--heartbeat-ms")
        .arg("50")
        .arg("--no-progress-ms")
        .arg(config.startup_timeout_ms.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
        .spawn()
        .map_err(|error| format!("start child scheduler: {error}"))
}

#[allow(clippy::too_many_arguments)]
fn wait_for_running_container(
    store: &RunStore,
    run_id: &str,
    job_id: &str,
    runtime: &dyn ContainerRuntime,
    owner_scope: &str,
    child: &mut Child,
    timeout: Duration,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<(String, String, String), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(child_failure(
                "crash-target scheduler exited before container was running",
                status,
                stdout_path,
                stderr_path,
            ));
        }
        if let Ok(scheduler) = store.load_scheduler()
            && let Some(run) = scheduler.runs.get(run_id)
            && let Some(job) = scheduler.jobs.get(job_id)
            && run.status == RunStatus::Running
            && let Some(attempt_id) = job.attempt_ids.last()
            && let Some(attempt) = scheduler.attempts.get(attempt_id)
            && attempt.status == JobState::Running
        {
            let containers = runtime
                .list_owned(owner_scope)
                .map_err(|error| error.to_string())?;
            if containers.len() == 1
                && matches!(
                    runtime
                        .state(&containers[0].id)
                        .map_err(|error| error.to_string())?
                        .status,
                    RuntimeContainerStatus::Running
                )
            {
                return Ok((
                    attempt.attempt_id.clone(),
                    attempt.worker_instance_id.clone(),
                    containers[0].id.clone(),
                ));
            }
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for crash-target container to run".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_until_worker_is_stale(
    store: &RunStore,
    run_id: &str,
    worker_timeout_ms: u64,
) -> Result<(), String> {
    let scheduler = store.load_scheduler()?;
    let worker = scheduler
        .workers
        .list()
        .find(|worker| !worker.current_jobs.is_empty())
        .ok_or_else(|| "persisted crash snapshot has no busy worker".to_string())?;
    let stale_at = worker
        .last_heartbeat_ms
        .saturating_add(worker_timeout_ms)
        .saturating_add(50);
    let now = now_unix_ms();
    if stale_at > now {
        thread::sleep(Duration::from_millis(stale_at - now));
    }
    if !scheduler.runs.contains_key(run_id) {
        return Err("persisted crash snapshot has no target run".into());
    }
    Ok(())
}

fn require_no_owned(
    runtime: &dyn ContainerRuntime,
    owner_scope: &str,
    phase: &str,
) -> Result<(), String> {
    let owned = runtime
        .list_owned(owner_scope)
        .map_err(|error| error.to_string())?;
    if owned.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{phase} found {} pre-existing owned container(s) in scope {owner_scope}",
            owned.len()
        ))
    }
}

fn child_failure(prefix: &str, status: ExitStatus, stdout: &Path, stderr: &Path) -> String {
    format!(
        "{prefix}: status={status}, stdout={}, stderr={}",
        bounded_file(stdout),
        bounded_file(stderr)
    )
}

fn bounded_file(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| format!("<unreadable: {error}>"))
        .chars()
        .take(4_096)
        .collect()
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("parse {}: {error}", path.display()))
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
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

struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn child_mut(&mut self) -> Result<&mut Child, String> {
        self.child
            .as_mut()
            .ok_or_else(|| "child scheduler is no longer active".into())
    }

    fn terminate_expected(&mut self) -> Result<(), String> {
        let mut child = self
            .child
            .take()
            .ok_or_else(|| "child scheduler is no longer active".to_string())?;
        child
            .kill()
            .map_err(|error| format!("terminate crash-target scheduler: {error}"))?;
        child
            .wait()
            .map_err(|error| format!("wait for terminated crash-target scheduler: {error}"))?;
        Ok(())
    }

    fn wait_success(
        &mut self,
        timeout: Duration,
        stdout_path: &Path,
        stderr_path: &Path,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            let status = self
                .child_mut()?
                .try_wait()
                .map_err(|error| format!("poll recovery scheduler: {error}"))?;
            if let Some(status) = status {
                self.child.take();
                return if status.success() {
                    Ok(())
                } else {
                    Err(child_failure(
                        "recovery scheduler failed",
                        status,
                        stdout_path,
                        stderr_path,
                    ))
                };
            }
            if Instant::now() >= deadline {
                return Err("timed out waiting for recovery scheduler".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct OwnedScopeCleanup {
    runtime: Arc<dyn ContainerRuntime>,
    owner_scope: String,
    armed: bool,
}

impl OwnedScopeCleanup {
    fn new(runtime: Arc<dyn ContainerRuntime>, owner_scope: String) -> Self {
        Self {
            runtime,
            owner_scope,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for OwnedScopeCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(containers) = self.runtime.list_owned(&self.owner_scope) {
            for container in containers {
                let _ = self.runtime.remove(&container.id, true);
            }
        }
    }
}
