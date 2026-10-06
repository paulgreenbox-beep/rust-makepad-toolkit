use crate::{
    container::{ContainerEnvironment, ContainerReaperReport, container_worker_capabilities},
    container_runtime::{ContainerRuntime, DockerCliRuntime},
    container_verification::{ContainerE2eVerificationConfig, run_container_e2e_verification},
    container_worker_loss_e2e::{ContainerWorkerLossE2eConfig, run_container_worker_loss_e2e},
    e2e::{WindowsE2eConfig, run_windows_e2e},
    environment::{
        EnvironmentHealth, ExecutionEnvironmentKind, ExecutionEnvironmentRequirement,
        ExecutionEnvironmentSpec, TrustedExecutionEnvironmentCatalog,
    },
    execution::LocalProcessEnvironment,
    mixed_e2e::{MixedEnvironmentE2eConfig, run_mixed_environment_e2e},
    model::{RetryPolicy, WorkerCapabilities},
    persistence::{CancellationRequest, PERSISTENCE_VERSION, RunStore},
    pool::{WorkerPool, WorkerPoolConfig, now_unix_ms},
    report,
    scheduler::{Scheduler, SchedulerConfig},
    submission::{SubmissionOptions, TrustedApplicationCatalog, submit_source},
    verification::{run_scheduler_stress, write_stress_report},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, Default)]
struct CliArgs {
    positionals: Vec<String>,
    options: BTreeMap<String, String>,
    switches: BTreeSet<String>,
}

impl CliArgs {
    fn option(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(String::as_str)
    }

    fn path(&self, name: &str) -> Option<PathBuf> {
        self.option(name).map(PathBuf::from)
    }

    fn switch(&self, name: &str) -> bool {
        self.switches.contains(name)
    }

    fn positional_path(&self, index: usize) -> Option<PathBuf> {
        self.positionals.get(index).map(PathBuf::from)
    }
}

pub fn main_entry() {
    if let Err(error) = run_args(env::args().skip(1).collect()) {
        eprintln!("{error}");
        std::process::exit(
            if error.starts_with("invalid") || error.starts_with("missing") {
                2
            } else {
                3
            },
        );
    }
}

pub fn run_args(raw: Vec<String>) -> Result<(), String> {
    let mut raw = raw.into_iter();
    let command = raw.next().unwrap_or_else(|| "help".into());
    let args = parse_args(raw.collect());
    match command.as_str() {
        "submit" => submit_command(&args),
        "run" => run_command(&args),
        "status" => status_command(&args),
        "workers" => workers_command(&args),
        "environments" | "environment-capabilities" => environments_command(&args),
        "cancel" => cancel_command(&args),
        "verify-stress" => verify_stress_command(&args),
        "verify-windows" => verify_windows_command(&args),
        "verify-mixed-environments" => verify_mixed_environments_command(&args),
        "verify-container-worker-loss" => verify_container_worker_loss_command(&args),
        "verify-container" => verify_container_command(&args),
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        other => Err(format!("invalid scheduler command: {other}")),
    }
}

fn parse_args(raw: Vec<String>) -> CliArgs {
    let known_switches = [
        "retry-test-failures",
        "retry-timeouts",
        "retry-non-idempotent",
        "no-infrastructure-retry",
        "resume",
        "check-runtime",
        "help",
    ];
    let mut parsed = CliArgs::default();
    let mut index = 0;
    while index < raw.len() {
        let value = &raw[index];
        if let Some(flag) = value.strip_prefix("--") {
            if let Some((name, value)) = flag.split_once('=') {
                parsed.options.insert(name.into(), value.into());
            } else if known_switches.contains(&flag) {
                parsed.switches.insert(flag.into());
            } else if raw
                .get(index + 1)
                .is_some_and(|next| !next.starts_with("--"))
            {
                parsed.options.insert(flag.into(), raw[index + 1].clone());
                index += 1;
            } else {
                parsed.switches.insert(flag.into());
            }
        } else {
            parsed.positionals.push(value.clone());
        }
        index += 1;
    }
    parsed
}

fn print_usage() {
    println!(
        "ai-ui-scheduler submit <script|suite|directory> --applications trusted-apps.json \
         [--run-id ID] [--output reports/scheduler/ID]\n\
         ai-ui-scheduler run <script|suite|directory> --applications trusted-apps.json \
         [--runner ai-ui-test.exe] [--workers N] [--output DIR] [--fixtures FILE] \
         [--environment local|ALIAS] [--environments trusted-environments.json] \
         [--container-runtime docker]\n\
         ai-ui-scheduler environments --environments trusted-environments.json \
         [--check-runtime] [--container-runtime docker]\n\
         ai-ui-scheduler environment-capabilities --environments trusted-environments.json \
         [--check-runtime] [--container-runtime docker]\n\
         ai-ui-scheduler status <run-directory> [--run-id ID]\n\
         ai-ui-scheduler workers <run-directory>\n\
         ai-ui-scheduler cancel <run-directory> [--job JOB_ID]\n\
         ai-ui-scheduler verify-stress [--jobs 500] [--workers 4] [--output DIR]\n\
         ai-ui-scheduler verify-windows [--workers 2] [--output DIR]\n\
         ai-ui-scheduler verify-mixed-environments [--output DIR] \
         [--container-runtime docker]\n\
         ai-ui-scheduler verify-container-worker-loss [--output DIR] \
         [--container-runtime docker]\n\
         ai-ui-scheduler verify-container [--output DIR] \
         [--container-runtime docker]"
    );
}

fn submit_command(args: &CliArgs) -> Result<(), String> {
    let (scheduler, store, run_id, jobs) = prepare_submission(args)?;
    store.write_all(&scheduler, &run_id)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "SUBMITTED",
            "run_id": run_id,
            "jobs": jobs,
            "output": store.root()
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn run_command(args: &CliArgs) -> Result<(), String> {
    let (scheduler, store, run_id) = if args.switch("resume") {
        let output = required_run_directory(args)?;
        let store = RunStore::new(output);
        let scheduler = store.load_scheduler()?;
        let run_id = selected_run_id(&scheduler, args.option("run-id"))?;
        (scheduler, store, run_id)
    } else {
        let (scheduler, store, run_id, _) = prepare_submission(args)?;
        (scheduler, store, run_id)
    };

    let workers = parse_usize(args.option("workers"), 1, "workers")?;
    let selected_environment = resolve_run_environment(args, &scheduler, &run_id)?;

    let mut pool_config = WorkerPoolConfig::default();
    pool_config.heartbeat_interval_ms = parse_u64(
        args.option("heartbeat-ms"),
        pool_config.heartbeat_interval_ms,
        "heartbeat-ms",
    )?;
    pool_config.no_progress_timeout_ms = parse_u64(
        args.option("no-progress-ms"),
        pool_config.no_progress_timeout_ms,
        "no-progress-ms",
    )?;

    let mut pool = WorkerPool::new(&run_id, scheduler, store.clone(), pool_config);
    let mut container_environment = None;
    if let Some(spec) = selected_environment {
        if spec.kind != ExecutionEnvironmentKind::Container {
            return Err("only container trusted environments are implemented in CLI v1".into());
        }
        let runtime_executable = args
            .path("container-runtime")
            .unwrap_or_else(|| PathBuf::from("docker"));
        let runtime: Arc<dyn ContainerRuntime> =
            Arc::new(DockerCliRuntime::new(runtime_executable));
        let host_root = absolute_path(&store.root().join("environment-runtime"))?;
        let environment = Arc::new(ContainerEnvironment::new(
            spec.clone(),
            runtime,
            host_root,
            container_owner_scope(&run_id),
        )?);
        let preflight = environment.reap_owned();
        write_reaper_report(&store, "preflight", &preflight)?;
        ensure_reaper_clean("preflight", &preflight)?;
        pool.add_workers(
            "container-worker",
            workers,
            container_worker_capabilities(&spec, 1)?,
            environment.clone(),
        )?;
        container_environment = Some(environment);
    } else {
        let runner = args.path("runner").unwrap_or(default_public_runner()?);
        let working_dir = args
            .path("working-dir")
            .unwrap_or(env::current_dir().map_err(|error| error.to_string())?);
        let environment = Arc::new(LocalProcessEnvironment::new(runner, working_dir));
        pool.add_local_workers(
            workers,
            WorkerCapabilities {
                max_parallel: 1,
                ..WorkerCapabilities::default()
            },
            environment,
        )?;
    }
    let result = pool.run()?;
    if let Some(environment) = container_environment {
        let reconciliation = environment.reap_owned();
        write_reaper_report(&store, "reconciliation", &reconciliation)?;
        ensure_reaper_clean("reconciliation", &reconciliation)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&result.summary).map_err(|error| error.to_string())?
    );
    for marker in &result.summary.markers {
        println!("{marker}");
    }
    if result.summary.status == crate::model::RunStatus::Passed {
        Ok(())
    } else {
        Err(format!(
            "scheduler run completed with status {:?}",
            result.summary.status
        ))
    }
}

fn status_command(args: &CliArgs) -> Result<(), String> {
    let store = RunStore::new(required_run_directory(args)?);
    let scheduler = store.load_scheduler()?;
    let run_id = selected_run_id(&scheduler, args.option("run-id"))?;
    let summary = report::aggregate(&scheduler, &run_id)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&summary).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn workers_command(args: &CliArgs) -> Result<(), String> {
    let store = RunStore::new(required_run_directory(args)?);
    let scheduler = store.load_scheduler()?;
    let workers = scheduler.workers.list().collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string_pretty(&workers).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn environments_command(args: &CliArgs) -> Result<(), String> {
    let catalog_path = args
        .path("environments")
        .ok_or_else(|| "missing --environments trusted-environments.json".to_string())?;
    let catalog = TrustedExecutionEnvironmentCatalog::load(catalog_path)?;
    let check_runtime = args.switch("check-runtime");
    let runtime_executable = args
        .path("container-runtime")
        .unwrap_or_else(|| PathBuf::from("docker"));
    let runtime = DockerCliRuntime::new(runtime_executable);
    let environments = catalog
        .environments
        .iter()
        .map(|spec| {
            let runtime_health = if !check_runtime {
                None
            } else {
                Some(match spec.kind {
                    ExecutionEnvironmentKind::Container => runtime.health(),
                    ExecutionEnvironmentKind::LocalProcess => EnvironmentHealth::healthy(),
                    ExecutionEnvironmentKind::Remote => EnvironmentHealth::unhealthy(
                        "ENVIRONMENT_BACKEND_NOT_IMPLEMENTED",
                        "remote execution environment backend is not implemented",
                    ),
                })
            };
            json!({
                "spec": spec,
                "runtime_health": runtime_health,
            })
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": catalog.version,
            "environments": environments,
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn cancel_command(args: &CliArgs) -> Result<(), String> {
    let store = RunStore::new(required_run_directory(args)?);
    let scheduler = store.load_scheduler()?;
    let run_id = selected_run_id(&scheduler, args.option("run-id"))?;
    if scheduler.run_complete(&run_id) {
        return Err(format!("run is already terminal: {run_id}"));
    }
    let job_id = args.option("job").map(str::to_owned);
    if let Some(job_id) = &job_id {
        if !scheduler.jobs.contains_key(job_id) {
            return Err(format!("job not found: {job_id}"));
        }
        if scheduler.jobs[job_id].job.run_id != run_id {
            return Err(format!("job {job_id} does not belong to {run_id}"));
        }
    }
    let request = CancellationRequest {
        version: PERSISTENCE_VERSION,
        run_id: run_id.clone(),
        job_id: job_id.clone(),
        requested_at_ms: now_unix_ms(),
    };
    store.request_cancel(&request)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "CANCELLATION_REQUESTED",
            "run_id": run_id,
            "job_id": job_id
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn verify_stress_command(args: &CliArgs) -> Result<(), String> {
    let jobs = parse_usize(args.option("jobs"), 500, "jobs")?;
    let workers = parse_usize(args.option("workers"), 4, "workers")?;
    let output = args
        .path("output")
        .unwrap_or_else(|| PathBuf::from("reports/scheduler/stress"));
    let report = run_scheduler_stress(jobs, workers)?;
    let path = write_stress_report(output, &report)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "PASSED",
            "jobs": report.jobs,
            "workers": report.workers,
            "attempts": report.attempts,
            "passed": report.passed,
            "test_failed": report.test_failed,
            "cancelled": report.cancelled,
            "worker_lost_attempts": report.worker_lost_attempts,
            "report": path,
        }))
        .map_err(|error| error.to_string())?
    );
    for marker in &report.markers {
        println!("{marker}");
    }
    Ok(())
}

fn verify_mixed_environments_command(args: &CliArgs) -> Result<(), String> {
    let package_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "scheduler crate has no package parent".to_string())?
        .to_path_buf();
    let fixtures = package_root.join("scheduler/container");
    let run_id = args
        .option("run-id")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("run:mixed-environments:{}", now_unix_ms()));
    let output = args.path("output").unwrap_or_else(|| {
        package_root
            .join("reports/scheduler")
            .join(safe_path_segment(&run_id))
    });
    let report = run_mixed_environment_e2e(MixedEnvironmentE2eConfig {
        run_id,
        output,
        local_source: args
            .path("local-source")
            .unwrap_or_else(|| fixtures.join("local-environment-probe.aiui.json")),
        container_source: args
            .path("container-source")
            .unwrap_or_else(|| fixtures.join("environment-probe.aiui.json")),
        applications: args
            .path("applications")
            .unwrap_or_else(|| fixtures.join("verification-trusted-apps.json")),
        environments: args
            .path("environments")
            .unwrap_or_else(|| fixtures.join("environment-e2e-catalog.json")),
        container_environment_key: args
            .option("environment")
            .unwrap_or("docker-linux-level0")
            .into(),
        public_runner: args.path("runner").unwrap_or(default_public_runner()?),
        working_dir: args
            .path("working-dir")
            .unwrap_or_else(|| package_root.clone()),
        container_runtime: args
            .path("container-runtime")
            .unwrap_or_else(|| PathBuf::from("docker")),
        timeout_ms: parse_u64(args.option("timeout-ms"), 30_000, "timeout-ms")?,
    })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    for marker in &report.markers {
        println!("{marker}");
    }
    Ok(())
}

fn verify_container_worker_loss_command(args: &CliArgs) -> Result<(), String> {
    let package_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "scheduler crate has no package parent".to_string())?
        .to_path_buf();
    let fixtures = package_root.join("scheduler/container");
    let run_id = args
        .option("run-id")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("run:container-worker-loss:{}", now_unix_ms()));
    let output = args.path("output").unwrap_or_else(|| {
        package_root
            .join("reports/scheduler")
            .join(safe_path_segment(&run_id))
    });
    let report = run_container_worker_loss_e2e(ContainerWorkerLossE2eConfig {
        run_id,
        output,
        source: args
            .path("source")
            .unwrap_or_else(|| fixtures.join("environment-probe.aiui.json")),
        applications: args
            .path("applications")
            .unwrap_or_else(|| fixtures.join("verification-trusted-apps.json")),
        environments: args
            .path("environments")
            .unwrap_or_else(|| fixtures.join("environment-e2e-catalog.json")),
        wait_environment_key: args
            .option("wait-environment")
            .unwrap_or("docker-linux-level0-wait")
            .into(),
        pass_environment_key: args
            .option("pass-environment")
            .unwrap_or("docker-linux-level0")
            .into(),
        scheduler_executable: args
            .path("scheduler")
            .unwrap_or(env::current_exe().map_err(|error| error.to_string())?),
        container_runtime: args
            .path("container-runtime")
            .unwrap_or_else(|| PathBuf::from("docker")),
        startup_timeout_ms: parse_u64(args.option("timeout-ms"), 30_000, "timeout-ms")?,
        worker_timeout_ms: parse_u64(args.option("worker-timeout-ms"), 500, "worker-timeout-ms")?,
    })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    for marker in &report.markers {
        println!("{marker}");
    }
    Ok(())
}

fn verify_container_command(args: &CliArgs) -> Result<(), String> {
    let package_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "scheduler crate has no package parent".to_string())?
        .to_path_buf();
    let scheduler_reports = package_root.join("reports/scheduler");
    let regression = package_root.join("reports/regression-container-worker-v1");
    let report = run_container_e2e_verification(ContainerE2eVerificationConfig {
        package_root: package_root.clone(),
        output: args
            .path("output")
            .unwrap_or_else(|| scheduler_reports.join("container-e2e-verification-v1")),
        capability_report: args
            .path("capability-report")
            .unwrap_or_else(|| scheduler_reports.join("container-capability-report.json")),
        environment_catalog: args
            .path("environments")
            .unwrap_or_else(|| package_root.join("scheduler/container/environment-catalog.json")),
        environment_key: args
            .option("environment")
            .unwrap_or("docker-linux-level0")
            .into(),
        container_runtime: args
            .path("container-runtime")
            .unwrap_or_else(|| PathBuf::from("docker")),
        pass_run: scheduler_reports.join("real-container-environment-v3"),
        test_failed_run: scheduler_reports.join("real-container-test-failed-v1"),
        infrastructure_failed_run: scheduler_reports
            .join("real-container-infrastructure-failed-v1"),
        timeout_run: scheduler_reports.join("real-container-timeout-v1"),
        cancelled_run: scheduler_reports.join("real-container-cancelled-v2"),
        isolation_run: scheduler_reports.join("real-container-isolation-v1"),
        mixed_report: scheduler_reports
            .join("real-mixed-environments-v3/mixed-environment-e2e-report.json"),
        worker_loss_report: scheduler_reports
            .join("real-container-worker-loss-v1/container-worker-loss-e2e-report.json"),
        stress_report: scheduler_reports.join("container-worker-stress-v2/stress-report.json"),
        parent_tests_log: regression.join("parent-lib-tests.log"),
        scheduler_tests_log: regression.join("scheduler-tests.log"),
        script_report: regression.join("script-api/script-self-test/final-report.json"),
        reliability_report: regression.join("reliability/summary.json"),
        spatial_report: regression.join("spatial/summary.json"),
        generator_report: regression.join("generator-audit/final-report.json"),
    })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": report.status,
            "runtime": report.runtime,
            "image": report.image,
            "image_digest": report.image_digest,
            "gui_level": report.gui_level,
            "checks": report.checks.len(),
            "owned_containers_remaining": report.owned_containers_remaining,
            "report": report.report_path,
            "withheld_markers": report.withheld_markers,
        }))
        .map_err(|error| error.to_string())?
    );
    for marker in &report.markers {
        println!("{marker}");
    }
    Ok(())
}

fn verify_windows_command(args: &CliArgs) -> Result<(), String> {
    let package_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "scheduler crate has no package parent".to_string())?
        .to_path_buf();
    let fixtures = package_root.join("scheduler/examples/windows-e2e");
    let source = args
        .positional_path(0)
        .or_else(|| args.path("source"))
        .unwrap_or_else(|| fixtures.join("suite.json"));
    let runner = args.path("runner").unwrap_or(default_public_runner()?);
    let working_dir = args
        .path("working-dir")
        .unwrap_or_else(|| package_root.clone());
    let run_id = args
        .option("run-id")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("run:windows-e2e:{}", now_unix_ms()));
    let output = args.path("output").unwrap_or_else(|| {
        package_root
            .join("reports/scheduler")
            .join(safe_path_segment(&run_id))
    });
    let applications = if let Some(path) = args.path("applications") {
        path
    } else {
        let scheduler_executable = env::current_exe().map_err(|error| error.to_string())?;
        let binary_dir = scheduler_executable.parent().ok_or_else(|| {
            format!(
                "scheduler executable has no binary directory: {}",
                scheduler_executable.display()
            )
        })?;
        resolve_windows_fixture_catalog(&fixtures.join("trusted-apps.json"), binary_dir, &output)?
    };
    let report = run_windows_e2e(WindowsE2eConfig {
        source,
        applications: applications.clone(),
        runner,
        working_dir,
        output,
        run_id,
        workers: parse_usize(args.option("workers"), 2, "workers")?,
        timeout_ms: parse_u64(args.option("timeout-ms"), 60_000, "timeout-ms")?,
    })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "PASSED",
            "run_id": report.run_id,
            "jobs": report.jobs,
            "workers_used": report.local_workers_used,
            "passed": report.passed,
            "test_failed": report.test_failed,
            "infrastructure_failed": report.infrastructure_failed,
            "worker_lost_attempts": report.worker_lost_attempts,
            "physical_resource_acquisitions": report.physical_resource_acquisitions,
            "semantic_parallelism_proven": report.semantic_parallelism_proven,
            "orphan_processes": report.orphan_processes,
            "resolved_fixture_catalog": applications,
            "run_summary": report.run_summary,
        }))
        .map_err(|error| error.to_string())?
    );
    for marker in &report.markers {
        println!("{marker}");
    }
    Ok(())
}

fn prepare_submission(args: &CliArgs) -> Result<(Scheduler, RunStore, String, usize), String> {
    let source = args
        .positional_path(0)
        .or_else(|| args.path("source"))
        .ok_or_else(|| "missing script, suite, or directory source".to_string())?;
    let applications_path = args
        .path("applications")
        .ok_or_else(|| "missing --applications trusted-apps.json".to_string())?;
    let applications = TrustedApplicationCatalog::load(applications_path)?;
    let run_id = args
        .option("run-id")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("run:{}", now_unix_ms()));
    let output = args.path("output").unwrap_or_else(|| {
        PathBuf::from("reports")
            .join("scheduler")
            .join(safe_path_segment(&run_id))
    });
    let store = RunStore::new(&output);
    if output.join("scheduler-state.json").exists() && !args.switch("resume") {
        return Err(format!(
            "scheduler output already contains a run; use --resume or another --output: {}",
            output.display()
        ));
    }

    let mut retry = RetryPolicy::default();
    retry.max_attempts = parse_u32(
        args.option("max-attempts"),
        retry.max_attempts,
        "max-attempts",
    )?;
    retry.retry_test_failures = args.switch("retry-test-failures");
    retry.retry_timeouts = args.switch("retry-timeouts");
    retry.retry_non_idempotent = args.switch("retry-non-idempotent");
    retry.retry_infrastructure_failures = !args.switch("no-infrastructure-retry");

    let mut options = SubmissionOptions::for_run(&run_id, &output);
    options.priority = parse_i32(args.option("priority"), 0, "priority")?;
    options.timeout_ms = parse_u64(args.option("timeout-ms"), 120_000, "timeout-ms")?;
    options.retry = retry;
    options.fixtures = args.path("fixtures");
    options.application_key = args.option("application-key").map(str::to_owned);
    if let Some(spec) = selected_environment_from_args(args)? {
        options.execution_environment = ExecutionEnvironmentRequirement {
            environment_key: spec.key.clone(),
            kind: spec.kind,
            minimum_isolation: spec.isolation,
        };
        options.selected_environment_capabilities = Some(container_worker_capabilities(&spec, 1)?);
        options
            .metadata
            .insert("environment_key".into(), Value::String(spec.key.clone()));
        options.metadata.insert(
            "environment_type".into(),
            serde_json::to_value(spec.kind).map_err(|error| error.to_string())?,
        );
    }
    options.metadata.insert(
        "submitted_by".into(),
        Value::String("ai-ui-scheduler".into()),
    );
    let now = now_unix_ms();
    let jobs = submit_source(source, &applications, &options, now)?;

    let mut config = SchedulerConfig::default();
    config.lease_duration_ms = parse_u64(
        args.option("lease-ms"),
        config.lease_duration_ms,
        "lease-ms",
    )?;
    config.worker_timeout_ms = parse_u64(
        args.option("worker-timeout-ms"),
        config.worker_timeout_ms,
        "worker-timeout-ms",
    )?;
    let mut scheduler = Scheduler::new(config);
    let count = jobs.len();
    scheduler.submit_run(&run_id, jobs, now)?;
    Ok((scheduler, store, run_id, count))
}

fn selected_environment_from_args(
    args: &CliArgs,
) -> Result<Option<ExecutionEnvironmentSpec>, String> {
    match args.option("environment") {
        None | Some("local") => Ok(None),
        Some(key) => load_environment_spec(args, key).map(Some),
    }
}

fn load_environment_spec(args: &CliArgs, key: &str) -> Result<ExecutionEnvironmentSpec, String> {
    let catalog_path = args
        .path("environments")
        .ok_or_else(|| "missing --environments trusted-environments.json".to_string())?;
    let catalog = TrustedExecutionEnvironmentCatalog::load(catalog_path)?;
    catalog.resolve(key).cloned()
}

fn resolve_run_environment(
    args: &CliArgs,
    scheduler: &Scheduler,
    run_id: &str,
) -> Result<Option<ExecutionEnvironmentSpec>, String> {
    let run = scheduler
        .runs
        .get(run_id)
        .ok_or_else(|| format!("run not found: {run_id}"))?;
    let mut selected_requirement: Option<ExecutionEnvironmentRequirement> = None;
    for job_id in &run.job_ids {
        let requirement = scheduler
            .jobs
            .get(job_id)
            .ok_or_else(|| format!("run references missing job: {job_id}"))?
            .job
            .requirements
            .execution_environment
            .clone();
        if selected_requirement
            .as_ref()
            .is_some_and(|selected| selected != &requirement)
        {
            return Err(
                "run contains mixed execution-environment requirements; use a mixed pool API"
                    .into(),
            );
        }
        selected_requirement = Some(requirement);
    }
    let requirement = selected_requirement.unwrap_or_default();
    if requirement.kind == ExecutionEnvironmentKind::LocalProcess {
        if args.option("environment").is_some_and(|key| key != "local") {
            return Err("requested environment does not match persisted local jobs".into());
        }
        return Ok(None);
    }
    let key = args
        .option("environment")
        .unwrap_or(&requirement.environment_key);
    if key != requirement.environment_key {
        return Err(format!(
            "requested environment {key} does not match persisted requirement {}",
            requirement.environment_key
        ));
    }
    let spec = load_environment_spec(args, key)?;
    if spec.kind != requirement.kind || !spec.isolation.satisfies(requirement.minimum_isolation) {
        return Err("trusted environment does not satisfy persisted requirement".into());
    }
    Ok(Some(spec))
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

fn container_owner_scope(run_id: &str) -> String {
    let mut segment = safe_path_segment(run_id);
    if segment.is_empty() {
        segment = "run".into();
    }
    segment.truncate(160);
    format!("ai-ui-scheduler-{segment}")
}

fn write_reaper_report(
    store: &RunStore,
    phase: &str,
    report: &ContainerReaperReport,
) -> Result<PathBuf, String> {
    if !matches!(phase, "preflight" | "reconciliation") {
        return Err("invalid container reaper report phase".into());
    }
    fs::create_dir_all(store.root()).map_err(|error| error.to_string())?;
    let path = store.root().join(format!("container-reaper-{phase}.json"));
    fs::write(
        &path,
        serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(path)
}

fn ensure_reaper_clean(phase: &str, report: &ContainerReaperReport) -> Result<(), String> {
    if report.remaining == 0 && report.failures.is_empty() {
        return Ok(());
    }
    let failures = report
        .failures
        .iter()
        .map(|failure| format!("{}:{}", failure.error_code, failure.message))
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!(
        "CONTAINER_CLEANUP_FAILED during {phase}: remaining={}, failures={failures}",
        report.remaining
    ))
}

fn selected_run_id(scheduler: &Scheduler, requested: Option<&str>) -> Result<String, String> {
    if let Some(requested) = requested {
        if scheduler.runs.contains_key(requested) {
            return Ok(requested.into());
        }
        return Err(format!("run not found: {requested}"));
    }
    match scheduler.runs.keys().collect::<Vec<_>>().as_slice() {
        [run_id] => Ok((*run_id).clone()),
        [] => Err("scheduler snapshot has no runs".into()),
        _ => Err("snapshot contains multiple runs; pass --run-id".into()),
    }
}

fn required_run_directory(args: &CliArgs) -> Result<PathBuf, String> {
    args.positional_path(0)
        .or_else(|| args.path("output"))
        .ok_or_else(|| "missing run directory".to_string())
}

fn default_public_runner() -> Result<PathBuf, String> {
    let current = env::current_exe().map_err(|error| error.to_string())?;
    let name = if cfg!(windows) {
        "ai-ui-test.exe"
    } else {
        "ai-ui-test"
    };
    Ok(current
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(name))
}

fn resolve_windows_fixture_catalog(
    template: &Path,
    binary_dir: &Path,
    output: &Path,
) -> Result<PathBuf, String> {
    let mut catalog = TrustedApplicationCatalog::load(template)?;
    for application in &mut catalog.applications {
        if application.executable.is_relative() {
            let name = application.executable.file_name().ok_or_else(|| {
                format!(
                    "fixture executable has no file name: {}",
                    application.executable.display()
                )
            })?;
            application.executable = binary_dir.join(name);
        }
    }
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    let resolved = output.join("resolved-trusted-apps.json");
    fs::write(
        &resolved,
        serde_json::to_vec_pretty(&catalog).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write {}: {error}", resolved.display()))?;
    Ok(resolved)
}

fn parse_u64(value: Option<&str>, default: u64, name: &str) -> Result<u64, String> {
    value
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| format!("invalid --{name}: {value}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_u32(value: Option<&str>, default: u32, name: &str) -> Result<u32, String> {
    value
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| format!("invalid --{name}: {value}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_i32(value: Option<&str>, default: i32, name: &str) -> Result<i32, String> {
    value
        .map(|value| {
            value
                .parse::<i32>()
                .map_err(|_| format!("invalid --{name}: {value}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_usize(value: Option<&str>, default: usize, name: &str) -> Result<usize, String> {
    let value = value
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| format!("invalid --{name}: {value}"))
        })
        .transpose()?
        .unwrap_or(default);
    if value == 0 {
        Err(format!("invalid --{name}: must be greater than zero"))
    } else {
        Ok(value)
    }
}

fn safe_path_segment(value: &str) -> String {
    value
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
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parser_accepts_values_switches_and_positionals() {
        let args = parse_args(vec![
            "suite.json".into(),
            "--workers".into(),
            "4".into(),
            "--retry-timeouts".into(),
            "--priority=50".into(),
        ]);
        assert_eq!(args.positionals, ["suite.json"]);
        assert_eq!(args.option("workers"), Some("4"));
        assert_eq!(args.option("priority"), Some("50"));
        assert!(args.switch("retry-timeouts"));
    }

    #[test]
    fn windows_fixture_catalog_resolves_relative_executables_from_scheduler_binary_dir() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "ai-ui-scheduler-portable-fixtures-{}-{stamp}",
            std::process::id()
        ));
        let template = root.join("trusted-apps.json");
        let output = root.join("report");
        let binary_dir = root.join("isolated-target/debug");
        fs::create_dir_all(&binary_dir).unwrap();
        fs::write(
            &template,
            r#"{
                "version": 1,
                "applications": [
                    {"key":"spatial","executable":"target/debug/spatial-demo.exe","max_parallel":1},
                    {"key":"external","executable":"C:/trusted/external.exe","max_parallel":1}
                ]
            }"#,
        )
        .unwrap();

        let resolved = resolve_windows_fixture_catalog(&template, &binary_dir, &output).unwrap();
        let catalog = TrustedApplicationCatalog::load(&resolved).unwrap();
        assert_eq!(
            catalog.applications[0].executable,
            binary_dir.join("spatial-demo.exe")
        );
        assert_eq!(
            catalog.applications[1].executable,
            PathBuf::from("C:/trusted/external.exe")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn submit_cli_expands_public_script_into_persisted_job() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "ai-ui-scheduler-cli-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let script_path = root.join("smoke.aiui.json");
        fs::write(
            &script_path,
            serde_json::to_vec_pretty(&json!({
                "version": 1,
                "name": "CLI smoke",
                "application": {"command": "fixture.exe"},
                "options": {"semantic_only": true},
                "steps": [{"op": "assert_visible", "id": "button.ok"}]
            }))
            .unwrap(),
        )
        .unwrap();
        let applications = root.join("trusted-apps.json");
        fs::write(
            &applications,
            serde_json::to_vec_pretty(&json!({
                "version": 1,
                "applications": [{
                    "key": "fixture",
                    "executable": "C:/trusted/fixture.exe",
                    "script_command_aliases": ["fixture.exe"],
                    "max_parallel": 1
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let output = root.join("run");
        run_args(vec![
            "submit".into(),
            script_path.to_string_lossy().into_owned(),
            "--applications".into(),
            applications.to_string_lossy().into_owned(),
            "--run-id".into(),
            "run:cli".into(),
            "--output".into(),
            output.to_string_lossy().into_owned(),
        ])
        .unwrap();
        let scheduler = RunStore::new(&output).load_scheduler().unwrap();
        assert_eq!(scheduler.jobs.len(), 1);
        assert_eq!(scheduler.queue.len(), 1);
        assert_eq!(
            scheduler.jobs.values().next().unwrap().job.application.key,
            "fixture"
        );
        let _ = fs::remove_dir_all(root);
    }
}
