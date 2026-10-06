use crate::{
    container::{
        CONTAINER_REAPER_REPORT_VERSION, ContainerReaperReport,
        verify_container_public_api_boundary,
    },
    container_runtime::{ContainerRuntime, ContainerRuntimeInfo, DockerCliRuntime},
    container_worker_loss_e2e::{
        CONTAINER_WORKER_LOSS_E2E_REPORT_VERSION, ContainerWorkerLossE2eReport,
    },
    environment::{
        ENVIRONMENT_REPORT_VERSION, EXECUTION_ENVIRONMENT_CONTRACT_VERSION,
        EnvironmentCapabilities, EnvironmentIsolation, EnvironmentNetworkPolicy, EnvironmentReport,
        EnvironmentState, EnvironmentTimings, ExecutionEnvironmentKind, ExecutionEnvironmentSpec,
        GuiCapabilityLevel, TrustedExecutionEnvironmentCatalog, environment_transition_allowed,
    },
    execution::{verify_public_api_boundary, verify_scheduler_environment_boundary},
    mixed_e2e::{MIXED_ENVIRONMENT_E2E_REPORT_VERSION, MixedEnvironmentE2eReport},
    model::{AttemptRecord, FailureClassification, JobState, RunStatus},
    persistence::RunStore,
    report::{RUN_SUMMARY_VERSION, RunSummary},
    verification::{STRESS_VERIFICATION_VERSION, StressVerificationReport},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const CONTAINER_E2E_VERIFICATION_REPORT_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct ContainerE2eVerificationConfig {
    pub package_root: PathBuf,
    pub output: PathBuf,
    pub capability_report: PathBuf,
    pub environment_catalog: PathBuf,
    pub environment_key: String,
    pub container_runtime: PathBuf,
    pub pass_run: PathBuf,
    pub test_failed_run: PathBuf,
    pub infrastructure_failed_run: PathBuf,
    pub timeout_run: PathBuf,
    pub cancelled_run: PathBuf,
    pub isolation_run: PathBuf,
    pub mixed_report: PathBuf,
    pub worker_loss_report: PathBuf,
    pub stress_report: PathBuf,
    pub parent_tests_log: PathBuf,
    pub scheduler_tests_log: PathBuf,
    pub script_report: PathBuf,
    pub reliability_report: PathBuf,
    pub spatial_report: PathBuf,
    pub generator_report: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerVerificationCheck {
    pub name: String,
    pub status: String,
    pub evidence: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerRegressionVerification {
    pub parent_unit_tests_passed: usize,
    pub scheduler_unit_tests_passed: usize,
    pub script_api: String,
    pub reliability: String,
    pub spatial: String,
    pub generator: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerE2eVerificationReport {
    pub version: u32,
    pub generated_at_ms: u64,
    pub status: String,
    pub runtime: ContainerRuntimeInfo,
    pub environment_key: String,
    pub image: String,
    pub image_digest: String,
    pub gui_level: GuiCapabilityLevel,
    pub checks: Vec<ContainerVerificationCheck>,
    pub run_classifications: BTreeMap<String, String>,
    pub regression: ContainerRegressionVerification,
    pub owned_containers_remaining: usize,
    pub markers: Vec<String>,
    pub withheld_markers: Vec<String>,
    pub report_path: PathBuf,
}

#[derive(Clone, Copy)]
struct ExpectedRun {
    status: RunStatus,
    jobs: usize,
    passed: usize,
    failed: usize,
    infrastructure_failed: usize,
    timed_out: usize,
    cancelled: usize,
    attempts: usize,
    worker_lost_attempts: usize,
}

struct LoadedRun {
    scheduler: crate::scheduler::Scheduler,
    run_id: String,
    summary: RunSummary,
    attempts: Vec<AttemptRecord>,
}

pub fn run_container_e2e_verification(
    config: ContainerE2eVerificationConfig,
) -> Result<ContainerE2eVerificationReport, String> {
    let mut checks = Vec::new();

    verify_public_api_boundary()?;
    verify_container_public_api_boundary()?;
    verify_scheduler_environment_boundary()?;
    checks.push(check(
        "execution_environment_contract_and_public_api_boundary",
        vec![
            config.package_root.join("scheduler/src/execution.rs"),
            config.package_root.join("scheduler/src/container.rs"),
            config.package_root.join("scheduler/src/scheduler.rs"),
        ],
    ));

    let catalog = TrustedExecutionEnvironmentCatalog::load(&config.environment_catalog)?;
    let spec = catalog.resolve(&config.environment_key)?.clone();
    validate_container_spec(&spec)?;
    let image = spec
        .image
        .clone()
        .ok_or_else(|| "trusted container image is missing".to_string())?;
    let pinned_digest = spec
        .image_digest
        .clone()
        .ok_or_else(|| "trusted container image digest is missing".to_string())?;

    let capability = read_json_value(&config.capability_report)?;
    validate_capability_report(&capability, &config, &image, &pinned_digest)?;
    checks.push(check(
        "host_capability_report_level0",
        vec![config.capability_report.clone()],
    ));

    let runtime = DockerCliRuntime::new(&config.container_runtime);
    let health = runtime.health();
    require(health.healthy, || {
        format!(
            "container runtime is unhealthy: {}: {}",
            health.error_code.as_deref().unwrap_or("UNKNOWN"),
            health.message.as_deref().unwrap_or("no details")
        )
    })?;
    let runtime_info = runtime.info().map_err(|error| error.to_string())?;
    require(runtime_info.os == "linux", || {
        format!(
            "expected Linux container server, observed {}",
            runtime_info.os
        )
    })?;
    require(runtime_info.architecture == "amd64", || {
        format!(
            "expected amd64 container server, observed {}",
            runtime_info.architecture
        )
    })?;
    let runtime_digest = runtime
        .image_digest(&image)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("runtime image has no digest: {image}"))?;
    require(runtime_digest == pinned_digest, || {
        format!(
            "runtime image digest mismatch: expected {pinned_digest}, observed {runtime_digest}"
        )
    })?;
    checks.push(check(
        "current_runtime_and_pinned_image",
        vec![config.environment_catalog.clone()],
    ));

    let pass = load_run(
        &config.pass_run,
        ExpectedRun {
            status: RunStatus::Passed,
            jobs: 1,
            passed: 1,
            failed: 0,
            infrastructure_failed: 0,
            timed_out: 0,
            cancelled: 0,
            attempts: 1,
            worker_lost_attempts: 0,
        },
    )?;
    let pass_attempt = only_attempt(&pass)?;
    require_attempt_state(pass_attempt, JobState::Passed, None)?;
    let pass_environment = validate_environment_attempt(
        pass_attempt,
        &config,
        &runtime_info,
        &image,
        &pinned_digest,
        false,
    )?;
    let pass_script = validate_public_report(pass_attempt, &config, "PASSED", None, true)?;
    validate_environment_summary(&pass.summary, &[pass_environment])?;
    validate_clean_reapers(&config.pass_run, None)?;
    checks.push(check(
        "real_public_runner_pass_and_lifecycle",
        vec![config.pass_run.join("run-summary.json"), pass_script],
    ));

    let test_failed = validate_single_failure_run(
        &config.test_failed_run,
        &config,
        &runtime_info,
        &image,
        &pinned_digest,
        JobState::Failed,
        FailureClassification::TestFailure,
        "POSTCONDITION_FAILED",
    )?;
    let infrastructure_failed = validate_single_failure_run(
        &config.infrastructure_failed_run,
        &config,
        &runtime_info,
        &image,
        &pinned_digest,
        JobState::InfrastructureFailed,
        FailureClassification::InfrastructureFailure,
        "RUNNER_ENVIRONMENT_PROBE_FAILED",
    )?;
    let timed_out = validate_single_interrupted_run(
        &config.timeout_run,
        &config,
        &runtime_info,
        &image,
        &pinned_digest,
        RunStatus::Failed,
        JobState::TimedOut,
        FailureClassification::Timeout,
    )?;
    let cancelled = validate_single_interrupted_run(
        &config.cancelled_run,
        &config,
        &runtime_info,
        &image,
        &pinned_digest,
        RunStatus::Cancelled,
        JobState::Cancelled,
        FailureClassification::Cancelled,
    )?;
    checks.push(check(
        "failure_classification_and_cleanup_matrix",
        vec![
            config.test_failed_run.join("run-summary.json"),
            config.infrastructure_failed_run.join("run-summary.json"),
            config.timeout_run.join("run-summary.json"),
            config.cancelled_run.join("run-summary.json"),
            test_failed,
            infrastructure_failed,
            timed_out,
            cancelled,
        ],
    ));

    validate_isolation_run(&config, &runtime_info, &image, &pinned_digest)?;
    checks.push(check(
        "real_filesystem_and_environment_isolation",
        vec![config.isolation_run.join("run-summary.json")],
    ));

    validate_mixed_report(&config, &runtime_info, &image, &pinned_digest)?;
    checks.push(check(
        "mixed_local_and_container_scheduling",
        vec![config.mixed_report.clone()],
    ));

    validate_worker_loss_report(&config, &runtime_info, &image, &pinned_digest)?;
    checks.push(check(
        "worker_loss_startup_recovery_and_owner_scoped_reaper",
        vec![config.worker_loss_report.clone()],
    ));

    validate_stress_report(&config.stress_report)?;
    checks.push(check(
        "scheduler_mixed_environment_stress",
        vec![config.stress_report.clone()],
    ));

    let regression = validate_regressions(&config)?;
    checks.push(check(
        "critical_regressions",
        vec![
            config.parent_tests_log.clone(),
            config.scheduler_tests_log.clone(),
            config.script_report.clone(),
            config.reliability_report.clone(),
            config.spatial_report.clone(),
            config.generator_report.clone(),
        ],
    ));

    let owned = runtime
        .list_tool_owned()
        .map_err(|error| error.to_string())?;
    require(owned.is_empty(), || {
        format!(
            "final leak audit found ai-ui-owned containers: {:?}",
            owned
                .iter()
                .map(|container| (&container.id, &container.name))
                .collect::<Vec<_>>()
        )
    })?;
    checks.push(check("global_ai_ui_owned_container_leak_audit", Vec::new()));

    let markers = final_markers();
    let withheld_markers = withheld_gui_markers();
    let report_path = config.output.join("container-e2e-verification-report.json");
    let report = ContainerE2eVerificationReport {
        version: CONTAINER_E2E_VERIFICATION_REPORT_VERSION,
        generated_at_ms: now_unix_ms(),
        status: "PASSED".into(),
        runtime: runtime_info,
        environment_key: spec.key,
        image,
        image_digest: pinned_digest,
        gui_level: GuiCapabilityLevel::Level0,
        checks,
        run_classifications: BTreeMap::from([
            ("PASS".into(), "PASSED".into()),
            ("TEST_FAILED".into(), "TEST_FAILURE".into()),
            (
                "INFRASTRUCTURE_FAILED".into(),
                "INFRASTRUCTURE_FAILURE".into(),
            ),
            ("TIMEOUT".into(), "TIMEOUT".into()),
            ("CANCELLED".into(), "CANCELLED".into()),
            ("WORKER_LOST".into(), "WORKER_LOST_THEN_PASSED".into()),
        ]),
        regression,
        owned_containers_remaining: owned.len(),
        markers,
        withheld_markers,
        report_path: report_path.clone(),
    };
    fs::create_dir_all(&config.output)
        .map_err(|error| format!("create final verification output: {error}"))?;
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write final container verification report: {error}"))?;
    Ok(report)
}

fn validate_container_spec(spec: &ExecutionEnvironmentSpec) -> Result<(), String> {
    spec.validate().map_err(|errors| errors.join("; "))?;
    require(
        spec.version == EXECUTION_ENVIRONMENT_CONTRACT_VERSION,
        || "unexpected environment contract version".into(),
    )?;
    require(spec.kind == ExecutionEnvironmentKind::Container, || {
        "selected environment is not a container".into()
    })?;
    require(spec.platform == "linux", || {
        format!(
            "expected trusted Linux environment, observed {}",
            spec.platform
        )
    })?;
    require(spec.isolation == EnvironmentIsolation::Filesystem, || {
        "container v1 must use filesystem isolation".into()
    })?;
    require(spec.disposable && !spec.reusable, || {
        "container v1 must be disposable and non-reusable".into()
    })?;
    require(spec.mounts.is_empty(), || {
        "verification catalog must not grant static host mounts".into()
    })?;
    require(spec.network == EnvironmentNetworkPolicy::None, || {
        "verification container must have network disabled".into()
    })?;
    require(
        spec.limits.memory_mib.is_some()
            && spec.limits.cpu_millis.is_some()
            && spec.limits.pids.is_some(),
        || "verification container must have memory, CPU and PID limits".into(),
    )?;
    validate_level0_capabilities(&spec.capabilities)?;
    let reserved = [
        "AI_UI_ENVIRONMENT_ID",
        "AI_UI_ATTEMPT_ID",
        "AI_UI_JOB_ID",
        "AI_UI_RUN_ID",
        "AI_UI_WORKER_ID",
    ];
    require(
        !spec
            .environment
            .keys()
            .any(|key| reserved.contains(&key.as_str())),
        || "trusted catalog overrides a dynamic identity variable".into(),
    )?;
    Ok(())
}

fn validate_level0_capabilities(capabilities: &EnvironmentCapabilities) -> Result<(), String> {
    require(
        capabilities.process_isolation
            && capabilities.filesystem_isolation
            && capabilities.environment_isolation,
        || "Level 0 environment lacks proven process/filesystem/environment isolation".into(),
    )?;
    require(
        !capabilities.gpu
            && !capabilities.interactive_gui
            && !capabilities.semantic_ui
            && !capabilities.uia
            && !capabilities.physical_input
            && !capabilities.makepad
            && !capabilities.isolated_desktop
            && capabilities.gui_level == GuiCapabilityLevel::Level0,
        || "Level 0 environment claims an unproven GUI/UIA/input/desktop capability".into(),
    )
}

fn validate_capability_report(
    value: &Value,
    config: &ContainerE2eVerificationConfig,
    image: &str,
    digest: &str,
) -> Result<(), String> {
    require_u64(value, "/version", 2)?;
    for pointer in [
        "/probe_policy/host_configuration_changed",
        "/probe_policy/third_party_runtime_installed",
        "/probe_policy/existing_foreign_containers_modified",
    ] {
        require_bool(value, pointer, false)?;
    }
    require_string(value, "/runtime/status", "Supported")?;
    require_string(value, "/runtime/server_os", "linux")?;
    require_string(value, "/runtime/server_architecture", "amd64")?;
    for pointer in [
        "/capabilities/process_isolation/status",
        "/capabilities/filesystem_isolation/status",
        "/capabilities/environment_isolation/status",
        "/capabilities/network_isolation/status",
        "/capabilities/resource_limits/status",
        "/capabilities/public_ai_ui_runner_in_container/status",
    ] {
        require_string(value, pointer, "Supported")?;
    }
    for pointer in [
        "/capabilities/interactive_gui/status",
        "/capabilities/uia/status",
        "/capabilities/physical_input/status",
        "/capabilities/desktop_isolation/status",
    ] {
        require_string(value, pointer, "Unsupported")?;
    }
    require_string(value, "/isolation_layers/process_isolation", "Supported")?;
    require_string(
        value,
        "/isolation_layers/filesystem_environment_isolation",
        "Supported",
    )?;
    require_string(
        value,
        "/isolation_layers/interactive_desktop_isolation",
        "Unsupported",
    )?;
    require_string(
        value,
        "/gui_capability/highest_proven_level",
        "ContainerLevel0",
    )?;
    require_string(value, "/gui_capability/candidate_level_status", "Supported")?;
    let gui_markers = value_at(value, "/gui_capability/gui_markers_emitted")?
        .as_array()
        .ok_or_else(|| "gui_markers_emitted is not an array".to_string())?;
    require(gui_markers.is_empty(), || {
        "capability report contains unearned GUI markers".into()
    })?;
    require_string(value, "/real_container_e2e/status", "Supported")?;
    require_string(value, "/real_container_e2e/environment_type", "container")?;
    require_string(
        value,
        "/real_container_e2e/environment_key",
        &config.environment_key,
    )?;
    require_string(value, "/real_container_e2e/image", image)?;
    require_string(value, "/real_container_e2e/image_digest", digest)?;
    require_string(value, "/real_container_e2e/gui_level", "level0")?;
    for pointer in [
        "/real_container_e2e/public_runner",
        "/real_container_e2e/filesystem_isolation",
        "/real_container_e2e/environment_isolation",
        "/real_container_e2e/worker_loss_reaper",
        "/real_container_e2e/mixed_local_container",
    ] {
        require_bool(value, pointer, true)?;
    }
    require_u64(value, "/real_container_e2e/owned_containers_remaining", 0)?;
    let evidence = value_at(value, "/real_container_e2e/evidence")?
        .as_array()
        .ok_or_else(|| "real_container_e2e.evidence is not an array".to_string())?;
    require(evidence.len() >= 8, || {
        "real container capability report has incomplete evidence references".into()
    })?;
    for path in evidence.iter().filter_map(Value::as_str) {
        resolve_reference(config, Path::new(path))?;
    }
    Ok(())
}

fn validate_single_failure_run(
    root: &Path,
    config: &ContainerE2eVerificationConfig,
    runtime: &ContainerRuntimeInfo,
    image: &str,
    digest: &str,
    state: JobState,
    classification: FailureClassification,
    public_error_code: &str,
) -> Result<PathBuf, String> {
    let expected = ExpectedRun {
        status: RunStatus::Failed,
        jobs: 1,
        passed: 0,
        failed: usize::from(state == JobState::Failed),
        infrastructure_failed: usize::from(state == JobState::InfrastructureFailed),
        timed_out: 0,
        cancelled: 0,
        attempts: 1,
        worker_lost_attempts: 0,
    };
    let run = load_run(root, expected)?;
    let attempt = only_attempt(&run)?;
    require_attempt_state(attempt, state, Some(classification))?;
    let environment = validate_environment_attempt(attempt, config, runtime, image, digest, false)?;
    validate_environment_summary(&run.summary, &[environment])?;
    let script = validate_public_report(attempt, config, "FAILED", Some(public_error_code), false)?;
    validate_clean_reapers(root, None)?;
    Ok(script)
}

fn validate_single_interrupted_run(
    root: &Path,
    config: &ContainerE2eVerificationConfig,
    runtime: &ContainerRuntimeInfo,
    image: &str,
    digest: &str,
    run_status: RunStatus,
    state: JobState,
    classification: FailureClassification,
) -> Result<PathBuf, String> {
    let run = load_run(
        root,
        ExpectedRun {
            status: run_status,
            jobs: 1,
            passed: 0,
            failed: 0,
            infrastructure_failed: 0,
            timed_out: usize::from(state == JobState::TimedOut),
            cancelled: usize::from(state == JobState::Cancelled),
            attempts: 1,
            worker_lost_attempts: 0,
        },
    )?;
    let attempt = only_attempt(&run)?;
    require_attempt_state(attempt, state, Some(classification))?;
    require(attempt.script_report.is_none(), || {
        format!(
            "interrupted attempt unexpectedly has a script report: {}",
            attempt.attempt_id
        )
    })?;
    let environment = validate_environment_attempt(attempt, config, runtime, image, digest, true)?;
    let report_path = resolve_reference(
        config,
        attempt
            .environment_report
            .as_deref()
            .ok_or_else(|| "interrupted attempt has no environment report".to_string())?,
    )?;
    validate_environment_summary(&run.summary, &[environment])?;
    validate_clean_reapers(root, None)?;
    Ok(report_path)
}

fn validate_isolation_run(
    config: &ContainerE2eVerificationConfig,
    runtime: &ContainerRuntimeInfo,
    image: &str,
    digest: &str,
) -> Result<(), String> {
    let run = load_run(
        &config.isolation_run,
        ExpectedRun {
            status: RunStatus::Passed,
            jobs: 2,
            passed: 2,
            failed: 0,
            infrastructure_failed: 0,
            timed_out: 0,
            cancelled: 0,
            attempts: 2,
            worker_lost_attempts: 0,
        },
    )?;
    let mut environment_ids = BTreeSet::new();
    let mut container_ids = BTreeSet::new();
    let mut attempt_ids = BTreeSet::new();
    let mut state_paths = BTreeSet::new();
    let mut reports = Vec::new();
    for attempt in &run.attempts {
        require_attempt_state(attempt, JobState::Passed, None)?;
        let environment =
            validate_environment_attempt(attempt, config, runtime, image, digest, false)?;
        let script_path = validate_public_report(attempt, config, "PASSED", None, true)?;
        let script = read_json_value(&script_path)?;
        let result = value_at(&script, "/steps/0/response/result")?;
        require_string_value(result, "attempt_id", &attempt.attempt_id)?;
        require_string_value(
            result,
            "environment_id",
            &environment.metadata.environment_id,
        )?;
        require_string_value(result, "capability_level", "level0")?;
        let probe = result
            .get("filesystem_probe")
            .ok_or_else(|| "isolation report has no filesystem_probe".to_string())?;
        require(
            probe.get("preexisting_state").is_some_and(Value::is_null),
            || "isolation attempt observed pre-existing tmpfs state".into(),
        )?;
        require_bool_value(probe, "roundtrip_matches_environment", true)?;
        require_string_value(
            probe,
            "state_roundtrip",
            &environment.metadata.environment_id,
        )?;
        let state_path = require_string_field(probe, "state_path")?;
        require(
            state_path == "/tmp/ai-ui-environment-probe/state.json",
            || format!("unexpected isolation state path: {state_path}"),
        )?;
        let roots = probe
            .get("profile_roots")
            .ok_or_else(|| "isolation report has no profile_roots".to_string())?;
        for (key, expected) in [
            ("home", "/home/ai-ui"),
            ("tmp", "/tmp"),
            ("temp", "/tmp"),
            ("appdata", "/tmp/appdata"),
            ("localappdata", "/tmp/localappdata"),
            ("xdg_cache_home", "/tmp/cache"),
            ("xdg_config_home", "/tmp/config"),
            ("xdg_data_home", "/tmp/data"),
        ] {
            require_string_value(roots, key, expected)?;
        }
        environment_ids.insert(environment.metadata.environment_id.clone());
        container_ids.insert(
            environment
                .metadata
                .container_id
                .clone()
                .ok_or_else(|| "isolation metadata has no container id".to_string())?,
        );
        attempt_ids.insert(attempt.attempt_id.clone());
        state_paths.insert(state_path.to_string());
        reports.push(environment);
    }
    require(environment_ids.len() == 2, || {
        "isolation attempts reused an environment identity".into()
    })?;
    require(container_ids.len() == 2, || {
        "isolation attempts reused a container identity".into()
    })?;
    require(attempt_ids.len() == 2, || {
        "isolation attempts reused an attempt identity".into()
    })?;
    require(state_paths.len() == 1, || {
        "isolation attempts did not exercise the same sandbox path".into()
    })?;
    validate_environment_summary(&run.summary, &reports)?;
    validate_clean_reapers(&config.isolation_run, None)
}

fn validate_mixed_report(
    config: &ContainerE2eVerificationConfig,
    runtime: &ContainerRuntimeInfo,
    image: &str,
    digest: &str,
) -> Result<(), String> {
    let report: MixedEnvironmentE2eReport = read_json(&config.mixed_report)?;
    require(
        report.version == MIXED_ENVIRONMENT_E2E_REPORT_VERSION,
        || "unexpected mixed E2E report version".into(),
    )?;
    require(report.status == "PASSED" && report.jobs == 2, || {
        "mixed E2E report is not a two-job PASS".into()
    })?;
    require(
        report.local_worker_id.starts_with("local-worker:")
            && report.container_worker_id.starts_with("container-worker:"),
        || "mixed E2E report has incorrect worker environment assignment".into(),
    )?;
    require(report.image_digest.as_deref() == Some(digest), || {
        "mixed E2E report image digest mismatch".into()
    })?;
    require(
        report.preflight_reaper.remaining == 0
            && report.preflight_reaper.failures.is_empty()
            && report.reconciliation_reaper.remaining == 0
            && report.reconciliation_reaper.failures.is_empty()
            && report.leaked_owned_containers == 0,
        || "mixed E2E report does not prove cleanup".into(),
    )?;
    require_marker(&report.markers, "AI_UI_MIXED_ENVIRONMENT_SCHEDULING_PASSED")?;
    let required_fields = [
        "evidence_refs",
        "failures",
        "markers",
        "status",
        "steps",
        "summary",
        "test",
    ];
    require(
        report.canonical_report_fields == required_fields.map(str::to_string).to_vec(),
        || "mixed E2E canonical Script API field set changed".into(),
    )?;
    for path in [
        &report.local_script_report,
        &report.container_script_report,
        &report.container_environment_report,
        &report.run_summary,
    ] {
        require(path.is_file(), || {
            format!("mixed E2E evidence is missing: {}", path.display())
        })?;
    }

    let root = config
        .mixed_report
        .parent()
        .ok_or_else(|| "mixed report has no parent directory".to_string())?;
    let run = load_run(
        root,
        ExpectedRun {
            status: RunStatus::Passed,
            jobs: 2,
            passed: 2,
            failed: 0,
            infrastructure_failed: 0,
            timed_out: 0,
            cancelled: 0,
            attempts: 2,
            worker_lost_attempts: 0,
        },
    )?;
    let mut local = Vec::new();
    let mut container = Vec::new();
    for attempt in &run.attempts {
        if attempt.execution_environment.is_some() {
            container.push(attempt);
        } else {
            local.push(attempt);
        }
    }
    require(local.len() == 1 && container.len() == 1, || {
        "mixed run did not contain exactly one local and one container attempt".into()
    })?;
    require_attempt_state(local[0], JobState::Passed, None)?;
    require(
        local[0].worker_id.starts_with("local-worker:") && local[0].environment_report.is_none(),
        || "mixed local attempt crossed the environment boundary".into(),
    )?;
    let local_script = validate_public_report(local[0], config, "PASSED", None, true)?;
    require_attempt_state(container[0], JobState::Passed, None)?;
    let environment =
        validate_environment_attempt(container[0], config, runtime, image, digest, false)?;
    let container_script = validate_public_report(container[0], config, "PASSED", None, true)?;
    require(
        canonical_fields(&local_script)? == canonical_fields(&container_script)?,
        || "local and container public report fields differ".into(),
    )?;
    require(
        report.environment_id == environment.metadata.environment_id,
        || "mixed report environment identity mismatch".into(),
    )?;
    require(
        report.container_id
            == environment
                .metadata
                .container_id
                .clone()
                .unwrap_or_default(),
        || "mixed report container identity mismatch".into(),
    )?;
    validate_environment_summary(&run.summary, &[environment])?;
    validate_clean_reapers(root, None)
}

fn validate_worker_loss_report(
    config: &ContainerE2eVerificationConfig,
    runtime: &ContainerRuntimeInfo,
    image: &str,
    digest: &str,
) -> Result<(), String> {
    let report: ContainerWorkerLossE2eReport = read_json(&config.worker_loss_report)?;
    require(
        report.version == CONTAINER_WORKER_LOSS_E2E_REPORT_VERSION
            && report.status == "PASSED"
            && report.stale_container_was_running,
        || "worker-loss E2E report is incomplete".into(),
    )?;
    require(
        report.old_attempt_id != report.new_attempt_id
            && report.old_worker_instance_id != report.new_worker_instance_id
            && report.old_container_id != report.new_container_id,
        || "worker-loss recovery reused stale identity".into(),
    )?;
    require(
        report.old_attempt_status == JobState::WorkerLost
            && report.new_attempt_status == JobState::Passed,
        || "worker-loss report has incorrect attempt states".into(),
    )?;
    require(
        report.preflight_reaper.version == CONTAINER_REAPER_REPORT_VERSION
            && report.preflight_reaper.discovered == 1
            && report.preflight_reaper.removed == 1
            && report.preflight_reaper.remaining == 0
            && report.preflight_reaper.failures.is_empty(),
        || "startup reaper did not remove exactly one stale owned container".into(),
    )?;
    require(
        report.reconciliation_reaper.remaining == 0
            && report.reconciliation_reaper.failures.is_empty()
            && report.owned_containers_remaining == 0,
        || "worker-loss reconciliation is not clean".into(),
    )?;
    require_marker(&report.markers, "AI_UI_CONTAINER_WORKER_LOSS_REAPER_PASSED")?;
    require_marker(&report.markers, "AI_UI_CONTAINER_STARTUP_RECOVERY_PASSED")?;
    for path in [
        &report.environment_report,
        &report.run_summary,
        &report.first_process_stdout,
        &report.first_process_stderr,
        &report.recovery_process_stdout,
        &report.recovery_process_stderr,
    ] {
        require(path.is_file(), || {
            format!("worker-loss evidence is missing: {}", path.display())
        })?;
    }
    let root = config
        .worker_loss_report
        .parent()
        .ok_or_else(|| "worker-loss report has no parent directory".to_string())?;
    let run = load_run(
        root,
        ExpectedRun {
            status: RunStatus::Passed,
            jobs: 1,
            passed: 1,
            failed: 0,
            infrastructure_failed: 0,
            timed_out: 0,
            cancelled: 0,
            attempts: 2,
            worker_lost_attempts: 1,
        },
    )?;
    let old = run
        .attempts
        .iter()
        .find(|attempt| attempt.attempt_id == report.old_attempt_id)
        .ok_or_else(|| "persisted old WorkerLost attempt is missing".to_string())?;
    let new = run
        .attempts
        .iter()
        .find(|attempt| attempt.attempt_id == report.new_attempt_id)
        .ok_or_else(|| "persisted replacement attempt is missing".to_string())?;
    require_attempt_state(
        old,
        JobState::WorkerLost,
        Some(FailureClassification::WorkerLost),
    )?;
    require(
        old.execution_environment.is_none() && old.environment_report.is_none(),
        || "crash snapshot incorrectly claims a completed old environment report".into(),
    )?;
    require(
        old.worker_instance_id == report.old_worker_instance_id,
        || "old worker instance identity changed".into(),
    )?;
    require_attempt_state(new, JobState::Passed, None)?;
    require(
        new.worker_instance_id == report.new_worker_instance_id,
        || "replacement worker instance identity mismatch".into(),
    )?;
    let environment = validate_environment_attempt(new, config, runtime, image, digest, false)?;
    require(
        environment.metadata.container_id.as_deref() == Some(report.new_container_id.as_str()),
        || "replacement container identity mismatch".into(),
    )?;
    validate_public_report(new, config, "PASSED", None, true)?;
    validate_environment_summary(&run.summary, &[environment])?;
    require(
        run.scheduler.events.iter().any(|event| {
            event.run_id == run.run_id
                && event.event == "worker_lost"
                && event.worker_id.as_deref() == Some(old.worker_id.as_str())
        }),
        || "worker-loss event is missing".into(),
    )?;
    require(
        run.scheduler.events.iter().any(|event| {
            event.run_id == run.run_id
                && event.event == "retry_scheduled"
                && event.attempt_id.as_deref() == Some(old.attempt_id.as_str())
        }),
        || "worker-loss retry event is missing".into(),
    )?;
    validate_clean_reapers(root, Some((1, 1, 0)))
}

fn validate_stress_report(path: &Path) -> Result<(), String> {
    let report: StressVerificationReport = read_json(path)?;
    require(report.version == STRESS_VERIFICATION_VERSION, || {
        "unexpected stress report version".into()
    })?;
    require(
        report.jobs == 500
            && report.workers == 4
            && report.local_jobs == 250
            && report.container_jobs == 250
            && report.attempts == 516
            && report.passed == 490
            && report.test_failed == 5
            && report.cancelled == 5
            && report.worker_lost_attempts == 1
            && report.transient_infrastructure_attempts == 12
            && report.transient_timeout_attempts == 5
            && report.transient_cleanup_attempts == 3,
        || "mixed environment stress counts differ from the deterministic contract".into(),
    )?;
    require(
        report.queue_empty
            && report.leases_empty
            && report.resources_released
            && report.duplicate_attempts == 0
            && report.local_assignments > 0
            && report.container_assignments > 0
            && report.mixed_environment_assignments_verified,
        || "mixed environment stress did not release or match all resources".into(),
    )?;
    require_marker(&report.markers, "AI_UI_SCHEDULER_STRESS_PASSED")?;
    require_marker(
        &report.markers,
        "AI_UI_SCHEDULER_MIXED_ENVIRONMENT_STRESS_PASSED",
    )
}

fn validate_regressions(
    config: &ContainerE2eVerificationConfig,
) -> Result<ContainerRegressionVerification, String> {
    let parent_log = read_text(&config.parent_tests_log)?;
    let scheduler_log = read_text(&config.scheduler_tests_log)?;
    require(
        parent_log.contains("test result: ok. 48 passed; 0 failed;"),
        || "parent library regression log does not prove 48/48".into(),
    )?;
    require(
        scheduler_log.contains("test result: ok. 47 passed; 0 failed;"),
        || "scheduler regression log does not prove 47/47".into(),
    )?;

    let script = read_json_value(&config.script_report)?;
    require_string(&script, "/status", "PASSED")?;
    require_u64(&script, "/summary/total", 3)?;
    require_u64(&script, "/summary/passed", 3)?;
    require_report_markers(
        &script,
        &[
            "AI_UI_SCRIPT_API_PARITY_PASSED",
            "AI_UI_SCRIPT_API_PASSED",
            "AI_UI_SCRIPT_RUNNER_PARITY_PASSED",
            "AI_UI_SCRIPT_SELF_TEST_PASSED",
        ],
    )?;

    let reliability = read_json_value(&config.reliability_report)?;
    require_string(&reliability, "/status", "AI_UI_RELIABILITY_PASSED")?;
    require_report_markers(
        &reliability,
        &[
            "AI_UI_RELIABILITY_PASSED",
            "AI_UI_MULTI_WINDOW_PASSED",
            "AI_UI_RUNTIME_FAILURE_DETECTED",
            "AI_UI_EVIDENCE_EXPORT_PASSED",
        ],
    )?;

    let spatial = read_json_value(&config.spatial_report)?;
    require_string(&spatial, "/status", "AI_UI_SPATIAL_PASSED")?;
    require(
        spatial
            .get("missing_markers")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty),
        || "spatial regression reports missing markers".into(),
    )?;
    require_report_markers(
        &spatial,
        &[
            "AI_UI_SPATIAL_RESIZE_PASSED",
            "AI_UI_SPATIAL_SELECTION_PASSED",
            "AI_UI_RELATIVE_DRAG_PASSED",
            "AI_UI_SPATIAL_CONTEXT_MENU_PASSED",
            "AI_UI_SPATIAL_HOVER_PASSED",
            "AI_UI_POINTER_CLEANUP_PASSED",
            "AI_UI_SPATIAL_MULTI_WINDOW_PASSED",
            "AI_UI_SPATIAL_RUNTIME_FAILURE_DETECTED",
        ],
    )?;

    let generator = read_json_value(&config.generator_report)?;
    require_string(&generator, "/status", "PASSED")?;
    require_report_markers(
        &generator,
        &[
            "AI_UI_AUDIT_PIPELINE_PASSED",
            "AI_UI_COVERAGE_ANALYSIS_PASSED",
            "AI_UI_COVERAGE_IMPROVEMENT_PASSED",
            "AI_UI_EVIDENCE_CONSISTENCY_PASSED",
            "AI_UI_GENERATED_SCRIPT_EXECUTION_PASSED",
            "AI_UI_GENERATED_SCRIPT_VALIDATION_PASSED",
            "AI_UI_GENERATOR_SAFETY_PASSED",
            "AI_UI_INVENTORY_PASSED",
            "AI_UI_TEST_GENERATOR_PASSED",
            "AI_UI_TEST_PLAN_PASSED",
        ],
    )?;

    Ok(ContainerRegressionVerification {
        parent_unit_tests_passed: 48,
        scheduler_unit_tests_passed: 47,
        script_api: "PASSED".into(),
        reliability: "PASSED".into(),
        spatial: "PASSED".into(),
        generator: "PASSED".into(),
    })
}

fn load_run(root: &Path, expected: ExpectedRun) -> Result<LoadedRun, String> {
    let store = RunStore::new(root);
    let scheduler = store
        .load_scheduler()
        .map_err(|error| format!("load scheduler snapshot {}: {error}", root.display()))?;
    require(scheduler.runs.len() == 1, || {
        format!("{} must contain exactly one run", root.display())
    })?;
    let run_id = scheduler
        .runs
        .keys()
        .next()
        .cloned()
        .ok_or_else(|| "scheduler snapshot has no run".to_string())?;
    let run = scheduler
        .runs
        .get(&run_id)
        .ok_or_else(|| "scheduler run disappeared".to_string())?;
    let summary: RunSummary = read_json(&root.join("run-summary.json"))?;
    require(summary.version == RUN_SUMMARY_VERSION, || {
        format!("unexpected run summary version in {}", root.display())
    })?;
    require(
        summary.run_id == run_id && summary.status == run.status,
        || format!("run summary identity/status mismatch in {}", root.display()),
    )?;
    let mut counts = [0_usize; 7];
    let mut attempts = Vec::new();
    for job_id in &run.job_ids {
        let job = scheduler
            .jobs
            .get(job_id)
            .ok_or_else(|| format!("run references missing job {job_id}"))?;
        match job.state {
            JobState::Passed => counts[0] += 1,
            JobState::Failed => counts[1] += 1,
            JobState::InfrastructureFailed => counts[2] += 1,
            JobState::TimedOut => counts[3] += 1,
            JobState::Cancelled => counts[4] += 1,
            JobState::WorkerLost => counts[5] += 1,
            JobState::Skipped => counts[6] += 1,
            state => {
                return Err(format!("run contains non-terminal job {job_id}: {state:?}"));
            }
        }
        for attempt_id in &job.attempt_ids {
            attempts.push(
                scheduler
                    .attempts
                    .get(attempt_id)
                    .ok_or_else(|| format!("job references missing attempt {attempt_id}"))?
                    .clone(),
            );
        }
    }
    let worker_lost_attempts = attempts
        .iter()
        .filter(|attempt| attempt.status == JobState::WorkerLost)
        .count();
    require(
        run.status == expected.status
            && run.job_ids.len() == expected.jobs
            && counts[0] == expected.passed
            && counts[1] == expected.failed
            && counts[2] == expected.infrastructure_failed
            && counts[3] == expected.timed_out
            && counts[4] == expected.cancelled
            && attempts.len() == expected.attempts
            && worker_lost_attempts == expected.worker_lost_attempts,
        || {
            format!(
                "persisted run does not match expected outcome: {}",
                root.display()
            )
        },
    )?;
    require(
        summary.total_jobs == run.job_ids.len()
            && summary.passed == counts[0]
            && summary.failed == counts[1]
            && summary.infrastructure_failed == counts[2]
            && summary.timed_out == counts[3]
            && summary.cancelled == counts[4]
            && summary.worker_lost == counts[5]
            && summary.skipped == counts[6]
            && summary.attempts == attempts.len()
            && summary.worker_lost_attempts == worker_lost_attempts,
        || format!("run summary aggregation mismatch: {}", root.display()),
    )?;
    require_marker(&summary.markers, "AI_UI_RESULT_AGGREGATION_PASSED")?;
    Ok(LoadedRun {
        scheduler,
        run_id,
        summary,
        attempts,
    })
}

fn validate_environment_attempt(
    attempt: &AttemptRecord,
    config: &ContainerE2eVerificationConfig,
    runtime: &ContainerRuntimeInfo,
    image: &str,
    digest: &str,
    require_stopping: bool,
) -> Result<EnvironmentReport, String> {
    let attempt_metadata = attempt.execution_environment.as_ref().ok_or_else(|| {
        format!(
            "attempt has no environment metadata: {}",
            attempt.attempt_id
        )
    })?;
    let report_ref = attempt
        .environment_report
        .as_deref()
        .ok_or_else(|| format!("attempt has no environment report: {}", attempt.attempt_id))?;
    let report_path = resolve_reference(config, report_ref)?;
    let report: EnvironmentReport = read_json(&report_path)?;
    require(report.version == ENVIRONMENT_REPORT_VERSION, || {
        "unexpected environment report version".into()
    })?;
    require(report.metadata == *attempt_metadata, || {
        "attempt and environment report metadata differ".into()
    })?;
    require(
        report.identity.attempt_id == attempt.attempt_id
            && report.identity.worker_id == attempt.worker_id
            && report.identity.environment_id == report.metadata.environment_id
            && report.identity.container_id == report.metadata.container_id
            && report.identity.capabilities == report.metadata.capabilities,
        || "environment identity and attempt metadata differ".into(),
    )?;
    require(
        report.identity.state == EnvironmentState::Destroyed
            && report.cleanup_succeeded
            && report.owned_resources_remaining == 0,
        || "environment report does not prove destruction and cleanup".into(),
    )?;
    require(
        report.metadata.version == EXECUTION_ENVIRONMENT_CONTRACT_VERSION
            && report.metadata.environment_type == ExecutionEnvironmentKind::Container
            && report
                .metadata
                .environment_key
                .starts_with("docker-linux-level0")
            && report.metadata.isolation_mode == EnvironmentIsolation::Filesystem
            && report.metadata.image.as_deref() == Some(image)
            && report.metadata.image_digest.as_deref() == Some(digest)
            && report
                .metadata
                .container_id
                .as_deref()
                .is_some_and(|id| !id.is_empty()),
        || "environment metadata does not match the trusted Level 0 backend".into(),
    )?;
    validate_level0_capabilities(&report.metadata.capabilities)?;
    let expected_os = format!("{}/{}", runtime.os, runtime.architecture);
    require(
        report.metadata.fingerprint.os == expected_os
            && report.metadata.fingerprint.environment_type == ExecutionEnvironmentKind::Container
            && report.metadata.fingerprint.image_digest.as_deref() == Some(digest)
            && report.metadata.fingerprint.gui_level == GuiCapabilityLevel::Level0
            && !report.metadata.fingerprint.gpu_available
            && !report.metadata.fingerprint.tool_version.is_empty()
            && report.metadata.fingerprint.runner_version == "public-script-api-v1",
        || "environment fingerprint does not match current runtime/image/public runner".into(),
    )?;
    let timings = &report.metadata.timings;
    require(
        timings.startup_ms > 0
            && timings.execution_ms > 0
            && timings.artifact_collection_ms > 0
            && timings.cleanup_ms > 0,
        || "environment timings do not prove all lifecycle phases".into(),
    )?;
    require(!report.lifecycle.is_empty(), || {
        "environment lifecycle is empty".into()
    })?;
    require(
        report.lifecycle.first().map(|event| event.state) == Some(EnvironmentState::Created)
            && report.lifecycle.last().map(|event| event.state)
                == Some(EnvironmentState::Destroyed),
        || "environment lifecycle does not span CREATED to DESTROYED".into(),
    )?;
    for pair in report.lifecycle.windows(2) {
        require(
            pair[0].timestamp_ms <= pair[1].timestamp_ms
                && environment_transition_allowed(pair[0].state, pair[1].state),
            || {
                format!(
                    "invalid environment lifecycle transition: {:?} -> {:?}",
                    pair[0].state, pair[1].state
                )
            },
        )?;
    }
    for state in [
        EnvironmentState::Preparing,
        EnvironmentState::Ready,
        EnvironmentState::Running,
        EnvironmentState::Stopped,
    ] {
        require(
            report.lifecycle.iter().any(|event| event.state == state),
            || format!("environment lifecycle is missing {state:?}"),
        )?;
    }
    if require_stopping {
        require(
            report
                .lifecycle
                .iter()
                .any(|event| event.state == EnvironmentState::Stopping),
            || "interrupted environment lifecycle is missing STOPPING".into(),
        )?;
    }
    let expected_report = fs::canonicalize(&report_path)
        .map_err(|error| format!("canonicalize {}: {error}", report_path.display()))?;
    let mut report_referenced = false;
    for artifact in &report.artifact_refs {
        let resolved = resolve_reference(config, artifact)?;
        if fs::canonicalize(&resolved).ok().as_ref() == Some(&expected_report) {
            report_referenced = true;
        }
    }
    require(report_referenced, || {
        "environment report does not reference itself as an artifact".into()
    })?;
    Ok(report)
}

fn validate_environment_summary(
    summary: &RunSummary,
    reports: &[EnvironmentReport],
) -> Result<(), String> {
    require_marker(
        &summary.markers,
        "AI_UI_ENVIRONMENT_REPORT_AGGREGATION_PASSED",
    )?;
    require(
        summary.environment.attempts == reports.len()
            && summary.environment.reports_observed == reports.len()
            && summary.environment.report_refs.len() == reports.len()
            && summary.environment.cleanup_succeeded
            && summary.environment.cleanup_failures == 0
            && summary.environment.owned_resources_remaining == 0
            && summary.environment.report_errors.is_empty()
            && summary.environment.failure_codes.is_empty(),
        || "run environment summary does not prove complete clean aggregation".into(),
    )?;
    let keys = reports
        .iter()
        .map(|report| report.metadata.environment_key.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let ids = reports
        .iter()
        .map(|report| report.metadata.environment_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    require(
        summary.environment.environment_types == [ExecutionEnvironmentKind::Container]
            && summary.environment.isolation_modes == [EnvironmentIsolation::Filesystem]
            && summary.environment.environment_keys == keys
            && summary.environment.environment_ids == ids,
        || "run environment summary identity/isolation aggregation mismatch".into(),
    )?;
    let total = reports
        .iter()
        .fold(EnvironmentTimings::default(), |mut total, report| {
            let value = &report.metadata.timings;
            total.environment_prepare_ms = total
                .environment_prepare_ms
                .saturating_add(value.environment_prepare_ms);
            total.startup_ms = total.startup_ms.saturating_add(value.startup_ms);
            total.execution_ms = total.execution_ms.saturating_add(value.execution_ms);
            total.artifact_collection_ms = total
                .artifact_collection_ms
                .saturating_add(value.artifact_collection_ms);
            total.cleanup_ms = total.cleanup_ms.saturating_add(value.cleanup_ms);
            total
        });
    require(summary.environment.timings == total, || {
        "run environment timing aggregation mismatch".into()
    })?;
    let unique_fingerprints = reports
        .iter()
        .map(|report| &report.metadata.fingerprint)
        .fold(Vec::new(), |mut values, value| {
            if !values.contains(&value) {
                values.push(value);
            }
            values
        });
    require(
        summary.environment.fingerprints.len() == unique_fingerprints.len()
            && unique_fingerprints
                .iter()
                .all(|fingerprint| summary.environment.fingerprints.contains(fingerprint)),
        || "run environment fingerprint aggregation mismatch".into(),
    )
}

fn validate_public_report(
    attempt: &AttemptRecord,
    config: &ContainerE2eVerificationConfig,
    status: &str,
    error_code: Option<&str>,
    require_parity_markers: bool,
) -> Result<PathBuf, String> {
    let path = resolve_reference(
        config,
        attempt.script_report.as_deref().ok_or_else(|| {
            format!(
                "attempt has no public script report: {}",
                attempt.attempt_id
            )
        })?,
    )?;
    let report = read_json_value(&path)?;
    require_string(&report, "/status", status)?;
    for field in [
        "status",
        "test",
        "summary",
        "steps",
        "failures",
        "evidence_refs",
        "markers",
    ] {
        require(report.get(field).is_some(), || {
            format!(
                "public report has no canonical field {field}: {}",
                path.display()
            )
        })?;
    }
    if let Some(error_code) = error_code {
        require_string(&report, "/failures/0/error_code", error_code)?;
    }
    if require_parity_markers {
        require_report_markers(
            &report,
            &[
                "AI_UI_SCRIPT_API_PARITY_PASSED",
                "AI_UI_SCRIPT_RUNNER_PARITY_PASSED",
            ],
        )?;
    }
    Ok(path)
}

fn validate_clean_reapers(
    root: &Path,
    exact_preflight: Option<(usize, usize, usize)>,
) -> Result<(), String> {
    let preflight: ContainerReaperReport =
        read_json(&root.join("container-reaper-preflight.json"))?;
    let reconciliation: ContainerReaperReport =
        read_json(&root.join("container-reaper-reconciliation.json"))?;
    require(
        preflight.version == CONTAINER_REAPER_REPORT_VERSION
            && reconciliation.version == CONTAINER_REAPER_REPORT_VERSION
            && preflight.failures.is_empty()
            && reconciliation.failures.is_empty()
            && preflight.remaining == 0
            && reconciliation.remaining == 0
            && reconciliation.discovered == 0
            && reconciliation.removed == 0,
        || format!("container reaper reports are not clean: {}", root.display()),
    )?;
    if let Some((discovered, removed, remaining)) = exact_preflight {
        require(
            preflight.discovered == discovered
                && preflight.removed == removed
                && preflight.remaining == remaining,
            || {
                format!(
                    "container preflight reaper count mismatch: {}",
                    root.display()
                )
            },
        )?;
    } else {
        require(preflight.discovered == 0 && preflight.removed == 0, || {
            format!(
                "unexpected stale owned container before run: {}",
                root.display()
            )
        })?;
    }
    Ok(())
}

fn require_attempt_state(
    attempt: &AttemptRecord,
    state: JobState,
    classification: Option<FailureClassification>,
) -> Result<(), String> {
    require(
        attempt.status == state && attempt.classification == classification,
        || {
            format!(
                "attempt outcome mismatch {}: {:?}/{:?}",
                attempt.attempt_id, attempt.status, attempt.classification
            )
        },
    )
}

fn only_attempt(run: &LoadedRun) -> Result<&AttemptRecord, String> {
    match run.attempts.as_slice() {
        [attempt] => Ok(attempt),
        attempts => Err(format!(
            "run {} expected one attempt, observed {}",
            run.run_id,
            attempts.len()
        )),
    }
}

fn canonical_fields(path: &Path) -> Result<Vec<String>, String> {
    let value = read_json_value(path)?;
    Ok(value
        .as_object()
        .map(|object| object.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default()
        .into_iter()
        .collect())
}

fn resolve_reference(
    config: &ContainerE2eVerificationConfig,
    path: &Path,
) -> Result<PathBuf, String> {
    if path.is_absolute() && path.is_file() {
        return Ok(path.to_path_buf());
    }
    if let Ok(current) = env::current_dir() {
        let candidate = current.join(path);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    for anchor in config.package_root.ancestors() {
        let candidate = anchor.join(path);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!("evidence reference is missing: {}", path.display()))
}

fn final_markers() -> Vec<String> {
    [
        "AI_UI_EXECUTION_ENVIRONMENT_CONTRACT_PASSED",
        "AI_UI_CONTAINER_PUBLIC_API_BOUNDARY_PASSED",
        "AI_UI_CONTAINER_CAPABILITY_PROBE_PASSED",
        "AI_UI_CONTAINER_LIFECYCLE_PASSED",
        "AI_UI_CONTAINER_FILESYSTEM_ISOLATION_PASSED",
        "AI_UI_CONTAINER_ENVIRONMENT_ISOLATION_PASSED",
        "AI_UI_CONTAINER_CLEANUP_PASSED",
        "AI_UI_CONTAINER_FAILURE_CLASSIFICATION_PASSED",
        "AI_UI_MIXED_ENVIRONMENT_SCHEDULING_PASSED",
        "AI_UI_CONTAINER_WORKER_LOSS_REAPER_PASSED",
        "AI_UI_CONTAINER_STARTUP_RECOVERY_PASSED",
        "AI_UI_SCHEDULER_MIXED_ENVIRONMENT_STRESS_PASSED",
        "AI_UI_CONTAINER_REGRESSION_PASSED",
        "AI_UI_REAL_CONTAINER_E2E_PASSED",
        "AI_UI_CONTAINER_WORKER_BACKEND_PASSED",
        "AI_UI_CONTAINER_WORKER_REAL_E2E_PASSED",
        "AI_UI_CONTAINER_WORKER_PASSED",
    ]
    .map(str::to_string)
    .to_vec()
}

fn withheld_gui_markers() -> Vec<String> {
    [
        "AI_UI_CONTAINER_GUI_LAUNCH_PASSED",
        "AI_UI_CONTAINER_UIA_PASSED",
        "AI_UI_CONTAINER_PHYSICAL_INPUT_PASSED",
        "AI_UI_CONTAINER_DESKTOP_ISOLATION_PASSED",
        "AI_UI_CONTAINER_GUI_PASSED",
    ]
    .map(str::to_string)
    .to_vec()
}

fn check(name: &str, evidence: Vec<PathBuf>) -> ContainerVerificationCheck {
    ContainerVerificationCheck {
        name: name.into(),
        status: "PASSED".into(),
        evidence,
    }
}

fn require_report_markers(value: &Value, required: &[&str]) -> Result<(), String> {
    let markers = value
        .get("markers")
        .and_then(Value::as_array)
        .ok_or_else(|| "report markers are missing or invalid".to_string())?;
    for required in required {
        require(
            markers
                .iter()
                .any(|marker| marker.as_str() == Some(required)),
            || format!("required report marker is missing: {required}"),
        )?;
    }
    Ok(())
}

fn require_marker(markers: &[String], required: &str) -> Result<(), String> {
    require(markers.iter().any(|marker| marker == required), || {
        format!("required marker is missing: {required}")
    })
}

fn require_string(value: &Value, pointer: &str, expected: &str) -> Result<(), String> {
    let observed = value_at(value, pointer)?
        .as_str()
        .ok_or_else(|| format!("{pointer} is not a string"))?;
    require(observed == expected, || {
        format!("{pointer}: expected {expected}, observed {observed}")
    })
}

fn require_u64(value: &Value, pointer: &str, expected: u64) -> Result<(), String> {
    let observed = value_at(value, pointer)?
        .as_u64()
        .ok_or_else(|| format!("{pointer} is not an unsigned integer"))?;
    require(observed == expected, || {
        format!("{pointer}: expected {expected}, observed {observed}")
    })
}

fn require_bool(value: &Value, pointer: &str, expected: bool) -> Result<(), String> {
    let observed = value_at(value, pointer)?
        .as_bool()
        .ok_or_else(|| format!("{pointer} is not a boolean"))?;
    require(observed == expected, || {
        format!("{pointer}: expected {expected}, observed {observed}")
    })
}

fn require_string_value<'a>(value: &'a Value, key: &str, expected: &str) -> Result<(), String> {
    let observed = require_string_field(value, key)?;
    require(observed == expected, || {
        format!("{key}: expected {expected}, observed {observed}")
    })
}

fn require_string_field<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing or invalid string field: {key}"))
}

fn require_bool_value(value: &Value, key: &str, expected: bool) -> Result<(), String> {
    let observed = value
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("missing or invalid boolean field: {key}"))?;
    require(observed == expected, || {
        format!("{key}: expected {expected}, observed {observed}")
    })
}

fn value_at<'a>(value: &'a Value, pointer: &str) -> Result<&'a Value, String> {
    value
        .pointer(pointer)
        .ok_or_else(|| format!("missing JSON value: {pointer}"))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("parse {}: {error}", path.display()))
}

fn read_json_value(path: &Path) -> Result<Value, String> {
    read_json(path)
}

fn read_text(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))
}

fn require(condition: bool, message: impl FnOnce() -> String) -> Result<(), String> {
    if condition { Ok(()) } else { Err(message()) }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
