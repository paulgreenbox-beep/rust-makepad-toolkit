use crate::{
    container_runtime::{
        CONTAINER_RUNTIME_CONTRACT_VERSION, ContainerCreatePlan, ContainerRuntime,
        ContainerRuntimeError, ContainerRuntimeInfo, RuntimeContainerStatus, RuntimeMount,
    },
    environment::{
        ContainerFailureCode, ENVIRONMENT_REPORT_VERSION, EXECUTION_ENVIRONMENT_CONTRACT_VERSION,
        EnvironmentCapabilities, EnvironmentFingerprint, EnvironmentIdentity, EnvironmentReport,
        EnvironmentState, EnvironmentTimings, ExecutionEnvironmentKind,
        ExecutionEnvironmentMetadata, ExecutionEnvironmentSpec, LifecycleEvent, MountAccess,
        OwnershipLabels, WorkerEnvironmentCapabilities, environment_transition_allowed,
    },
    execution::{
        ExecutionControl, ExecutionEnvironment, ExecutionRequest, classify_public_report,
        collect_report_evidence, find_script_report,
    },
    model::{FailureClassification, RuntimeDiagnostics, WorkerCapabilities},
    scheduler::AttemptOutcome,
};
use ai_ui_test_tool_auto_probe::script;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const CONTAINER_ENVIRONMENT_IMPLEMENTATION_VERSION: u32 = 1;
pub const CONTAINER_REAPER_REPORT_VERSION: u32 = 1;

pub fn verify_container_public_api_boundary() -> Result<(), String> {
    let source = include_str!("container.rs");
    let forbidden = [
        concat!("Reliability", "Manager"),
        concat!("Windows", "Automation"),
        concat!("generator", "::"),
        concat!("windows", "_uia"),
        concat!("makepad", "_widgets"),
        concat!("Command", "::new"),
    ];
    for forbidden in forbidden {
        if source.contains(forbidden) {
            return Err(format!(
                "ContainerEnvironment contains forbidden public API bypass: {forbidden}"
            ));
        }
    }
    for required in [
        "ExecutionRequest",
        "AttemptOutcome",
        "script::load",
        "runner_arguments",
        "\"run\".into()",
        "/work/tests/resolved-script.aiui.json",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "ContainerEnvironment public runner boundary is missing: {required}"
            ));
        }
    }
    Ok(())
}

const CONTAINER_RESERVED_ENVIRONMENT_KEYS: &[&str] = &[
    "AI_UI_ENVIRONMENT_ID",
    "AI_UI_ATTEMPT_ID",
    "AI_UI_JOB_ID",
    "AI_UI_RUN_ID",
    "AI_UI_WORKER_ID",
];

pub fn container_worker_capabilities(
    spec: &ExecutionEnvironmentSpec,
    max_parallel: usize,
) -> Result<WorkerCapabilities, String> {
    spec.validate().map_err(|errors| errors.join("; "))?;
    if spec.kind != ExecutionEnvironmentKind::Container {
        return Err("selected environment is not a container backend".into());
    }
    if max_parallel == 0 {
        return Err("worker max_parallel must be greater than zero".into());
    }
    Ok(WorkerCapabilities {
        platform: spec.platform.clone(),
        interactive_desktop: spec.capabilities.interactive_gui,
        gpu: spec.capabilities.gpu,
        max_parallel,
        semantic_ui: spec.capabilities.semantic_ui && spec.capabilities.uia,
        physical_input: spec.capabilities.physical_input,
        makepad: spec.capabilities.makepad,
        execution_environment: WorkerEnvironmentCapabilities {
            environment_key: spec.key.clone(),
            kind: spec.kind,
            isolation: spec.isolation,
            gui_level: spec.capabilities.gui_level,
            reusable: spec.reusable,
        },
    })
}

#[derive(Clone)]
pub struct ContainerEnvironment {
    spec: ExecutionEnvironmentSpec,
    runtime: Arc<dyn ContainerRuntime>,
    host_root: PathBuf,
    owner_scope: String,
    poll_interval_ms: u64,
    stop_grace_ms: u64,
    runner_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerReaperFailure {
    pub container_id: String,
    pub error_code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerReaperReport {
    pub version: u32,
    pub owner_scope: String,
    pub discovered: usize,
    pub removed: usize,
    pub remaining: usize,
    pub failures: Vec<ContainerReaperFailure>,
}

impl ContainerEnvironment {
    pub fn new(
        spec: ExecutionEnvironmentSpec,
        runtime: Arc<dyn ContainerRuntime>,
        host_root: impl Into<PathBuf>,
        owner_scope: impl Into<String>,
    ) -> Result<Self, String> {
        spec.validate().map_err(|errors| errors.join("; "))?;
        if spec.kind != ExecutionEnvironmentKind::Container {
            return Err("ContainerEnvironment requires a container environment spec".into());
        }
        if !spec.disposable || spec.reusable {
            return Err("ContainerEnvironment v1 requires disposable one-attempt mode".into());
        }
        if let Some(key) = spec
            .environment
            .keys()
            .find(|key| CONTAINER_RESERVED_ENVIRONMENT_KEYS.contains(&key.as_str()))
        {
            return Err(format!(
                "trusted container environment cannot override reserved variable {key}"
            ));
        }
        if !spec.capabilities.process_isolation {
            return Err("container spec must prove process isolation".into());
        }
        if spec.isolation >= crate::environment::EnvironmentIsolation::Filesystem
            && (!spec.capabilities.filesystem_isolation || !spec.capabilities.environment_isolation)
        {
            return Err(
                "filesystem isolation requires proven filesystem and environment isolation".into(),
            );
        }
        let host_root = host_root.into();
        if !host_root.is_absolute() || host_root.parent().is_none() {
            return Err("container host_root must be an absolute non-root path".into());
        }
        let owner_scope = owner_scope.into();
        OwnershipLabels {
            owner_scope: owner_scope.clone(),
            run_id: "validation".into(),
            job_id: "validation".into(),
            attempt_id: "validation".into(),
            worker_id: "validation".into(),
        }
        .validate()?;
        for mount in &spec.mounts {
            validate_static_mount(mount, &host_root)?;
        }
        Ok(Self {
            spec,
            runtime,
            host_root,
            owner_scope,
            poll_interval_ms: 25,
            stop_grace_ms: 2_000,
            runner_version: "public-script-api-v1".into(),
        })
    }

    pub fn with_poll_interval_ms(mut self, value: u64) -> Self {
        self.poll_interval_ms = value.max(1);
        self
    }

    pub fn with_stop_grace_ms(mut self, value: u64) -> Self {
        self.stop_grace_ms = value;
        self
    }

    pub fn with_runner_version(mut self, value: impl Into<String>) -> Self {
        self.runner_version = value.into();
        self
    }

    pub fn worker_environment_capabilities(&self) -> WorkerEnvironmentCapabilities {
        WorkerEnvironmentCapabilities {
            environment_key: self.spec.key.clone(),
            kind: ExecutionEnvironmentKind::Container,
            isolation: self.spec.isolation,
            gui_level: self.spec.capabilities.gui_level,
            reusable: self.spec.reusable,
        }
    }

    pub fn spec(&self) -> &ExecutionEnvironmentSpec {
        &self.spec
    }

    pub fn owner_scope(&self) -> &str {
        &self.owner_scope
    }

    pub fn reap_owned(&self) -> ContainerReaperReport {
        let mut report = ContainerReaperReport {
            version: CONTAINER_REAPER_REPORT_VERSION,
            owner_scope: self.owner_scope.clone(),
            discovered: 0,
            removed: 0,
            remaining: 0,
            failures: Vec::new(),
        };
        let containers = match self.runtime.list_owned(&self.owner_scope) {
            Ok(containers) => containers,
            Err(error) => {
                report.failures.push(ContainerReaperFailure {
                    container_id: String::new(),
                    error_code: error.code.as_str().into(),
                    message: error.message,
                });
                return report;
            }
        };
        report.discovered = containers.len();
        for container in containers {
            match self.runtime.remove(&container.id, true) {
                Ok(()) => report.removed += 1,
                Err(error) => report.failures.push(ContainerReaperFailure {
                    container_id: container.id,
                    error_code: error.code.as_str().into(),
                    message: error.message,
                }),
            }
        }
        match self.runtime.list_owned(&self.owner_scope) {
            Ok(containers) => report.remaining = containers.len(),
            Err(error) => report.failures.push(ContainerReaperFailure {
                container_id: String::new(),
                error_code: error.code.as_str().into(),
                message: error.message,
            }),
        }
        report
    }

    fn execute_inner(
        &self,
        request: &ExecutionRequest,
        control: &ExecutionControl,
    ) -> AttemptOutcome {
        let artifact_dir = request
            .job
            .artifacts
            .output_dir
            .join(format!("attempt-{:03}", request.attempt.number));
        if let Err(error) = fs::create_dir_all(&artifact_dir) {
            return infrastructure_outcome(
                ContainerFailureCode::ContainerArtifactCollectionFailed,
                format!("create attempt artifact directory: {error}"),
                None,
                None,
            );
        }

        let created_at_ms = now_unix_ms();
        let environment_id = environment_id(request, &self.owner_scope);
        let mut session = AttemptSession::new(
            environment_id,
            artifact_dir.clone(),
            created_at_ms,
            self.spec.capabilities.clone(),
        );
        let mut guard = OwnedContainerGuard::new(self.runtime.clone());
        let run_result = self.run_attempt(request, control, &mut session, &mut guard);
        let mut result = match run_result {
            Ok(result) => result,
            Err(error) => {
                session.record_failure(&error.message);
                ContainerRunResult::infrastructure(error.code, error.message)
            }
        };
        let cleanup_started = Instant::now();
        let cleanup_result = self.cleanup_attempt(&mut session, &mut guard);
        session.timings.cleanup_ms = elapsed_ms(cleanup_started);

        let cleanup_succeeded = cleanup_result.is_ok();
        if let Err(error) = cleanup_result {
            session.record_failure(&error.message);
            result = ContainerRunResult::infrastructure(error.code, error.message);
        }

        let runtime_info = session
            .runtime_info
            .clone()
            .unwrap_or(ContainerRuntimeInfo {
                os: "unknown".into(),
                architecture: "unknown".into(),
                version: "unknown".into(),
            });
        let metadata = ExecutionEnvironmentMetadata {
            version: EXECUTION_ENVIRONMENT_CONTRACT_VERSION,
            environment_type: ExecutionEnvironmentKind::Container,
            environment_key: self.spec.key.clone(),
            environment_id: session.environment_id.clone(),
            container_id: session.container_id.clone(),
            image: self.spec.image.clone(),
            image_digest: session
                .resolved_image_digest
                .clone()
                .or_else(|| self.spec.image_digest.clone()),
            isolation_mode: self.spec.isolation,
            capabilities: self.spec.capabilities.clone(),
            fingerprint: EnvironmentFingerprint {
                os: format!("{}/{}", runtime_info.os, runtime_info.architecture),
                environment_type: ExecutionEnvironmentKind::Container,
                image_digest: session
                    .resolved_image_digest
                    .clone()
                    .or_else(|| self.spec.image_digest.clone()),
                tool_version: env!("CARGO_PKG_VERSION").into(),
                runner_version: self.runner_version.clone(),
                application_version: request
                    .job
                    .metadata
                    .get("application_version")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                gpu_available: self.spec.capabilities.gpu,
                gui_level: self.spec.capabilities.gui_level,
            },
            timings: session.timings.clone(),
        };
        let environment_report_path = artifact_dir.join("environment-report.json");
        let mut artifact_refs = collect_files(&artifact_dir).unwrap_or_default();
        artifact_refs.push(environment_report_path.clone());
        artifact_refs.sort();
        artifact_refs.dedup();
        let report = EnvironmentReport {
            version: ENVIRONMENT_REPORT_VERSION,
            identity: EnvironmentIdentity {
                environment_id: session.environment_id.clone(),
                container_id: session.container_id.clone(),
                worker_id: request.attempt.worker_id.clone(),
                attempt_id: request.attempt.attempt_id.clone(),
                created_at_ms,
                state: session.state,
                capabilities: self.spec.capabilities.clone(),
            },
            metadata: metadata.clone(),
            lifecycle: session.lifecycle.clone(),
            cleanup_succeeded,
            owned_resources_remaining: session.owned_resources_remaining,
            failure_code: result.error_code.clone(),
            artifact_refs,
        };
        if let Err(error) = write_json(&environment_report_path, &report) {
            return infrastructure_outcome(
                ContainerFailureCode::ContainerArtifactCollectionFailed,
                format!("write environment report: {error}"),
                Some(metadata),
                None,
            );
        }

        AttemptOutcome {
            classification: result.classification,
            script_report: result.script_report,
            evidence_refs: result.evidence_refs,
            runtime: Some(RuntimeDiagnostics {
                runner_pid: None,
                exit_code: result.exit_code,
                stdout_path: result.stdout_path,
                stderr_path: result.stderr_path,
                error_code: result.error_code,
                message: result.message,
            }),
            execution_environment: Some(metadata),
            environment_report: Some(environment_report_path),
        }
    }

    fn run_attempt(
        &self,
        request: &ExecutionRequest,
        control: &ExecutionControl,
        session: &mut AttemptSession,
        guard: &mut OwnedContainerGuard,
    ) -> Result<ContainerRunResult, RunFailure> {
        self.validate_request(request)?;
        session.transition(EnvironmentState::Preparing, "prepare isolated attempt")?;
        let prepare_started = Instant::now();
        let paths = self.prepare_staging(request, session)?;
        session.timings.environment_prepare_ms = elapsed_ms(prepare_started);

        let runtime_info = self.runtime.info().map_err(RunFailure::from_runtime)?;
        session.runtime_info = Some(runtime_info);
        let image = self.spec.image.as_deref().ok_or_else(|| {
            RunFailure::new(
                ContainerFailureCode::ContainerInvalidConfiguration,
                "trusted container image is missing",
            )
        })?;
        let resolved_digest = self
            .runtime
            .image_digest(image)
            .map_err(RunFailure::from_runtime)?;
        if let Some(expected) = &self.spec.image_digest {
            if resolved_digest.as_deref() != Some(expected.as_str()) {
                return Err(RunFailure::new(
                    ContainerFailureCode::ContainerImageMissing,
                    format!(
                        "trusted image digest mismatch: expected {expected}, observed {}",
                        resolved_digest.as_deref().unwrap_or("none")
                    ),
                ));
            }
        }
        session.resolved_image_digest = resolved_digest;
        let plan = self.create_plan(request, session, &paths)?;
        let container = self
            .runtime
            .create(&plan)
            .map_err(RunFailure::from_runtime)?;
        session.container_id = Some(container.id.clone());
        guard.arm(container.id.clone());
        session.transition(EnvironmentState::Ready, "container created")?;

        let startup_started = Instant::now();
        self.runtime
            .start(&container.id)
            .map_err(RunFailure::from_runtime)?;
        session.timings.startup_ms = elapsed_ms(startup_started);
        session.transition(EnvironmentState::Running, "public runner started")?;

        let execution_started = Instant::now();
        let mut terminal_override = None;
        let exit_code = loop {
            if control.is_cancelled() {
                terminal_override = Some(FailureClassification::Cancelled);
                self.stop_running(session, &container.id, "cancellation requested")?;
                break Some(130);
            }
            if execution_started.elapsed().as_millis() >= u128::from(request.job.timeout_ms) {
                terminal_override = Some(FailureClassification::Timeout);
                self.stop_running(session, &container.id, "job timeout reached")?;
                break Some(124);
            }
            let state = self
                .runtime
                .state(&container.id)
                .map_err(RunFailure::from_runtime)?;
            match state.status {
                RuntimeContainerStatus::Created | RuntimeContainerStatus::Running => {
                    thread::sleep(Duration::from_millis(self.poll_interval_ms.max(1)));
                }
                RuntimeContainerStatus::Exited => {
                    session.transition(EnvironmentState::Stopped, "container exited")?;
                    break state.exit_code;
                }
                RuntimeContainerStatus::Dead | RuntimeContainerStatus::Unknown => {
                    return Err(RunFailure::new(
                        ContainerFailureCode::ContainerExitedUnexpectedly,
                        format!("container entered unexpected state: {:?}", state.status),
                    ));
                }
            }
        };
        session.timings.execution_ms = elapsed_ms(execution_started);

        let collection_started = Instant::now();
        copy_tree_contents(&paths.output, &session.artifact_dir).map_err(|error| {
            RunFailure::new(
                ContainerFailureCode::ContainerArtifactCollectionFailed,
                format!("collect container output: {error}"),
            )
        })?;
        let logs = self
            .runtime
            .logs(&container.id)
            .map_err(RunFailure::from_runtime)?;
        let stdout_path = session.artifact_dir.join("container-stdout.log");
        let stderr_path = session.artifact_dir.join("container-stderr.log");
        fs::write(&stdout_path, logs.stdout).map_err(|error| {
            RunFailure::new(
                ContainerFailureCode::ContainerArtifactCollectionFailed,
                format!("write container stdout: {error}"),
            )
        })?;
        fs::write(&stderr_path, logs.stderr).map_err(|error| {
            RunFailure::new(
                ContainerFailureCode::ContainerArtifactCollectionFailed,
                format!("write container stderr: {error}"),
            )
        })?;
        session.timings.artifact_collection_ms = elapsed_ms(collection_started);

        if let Some(classification) = terminal_override {
            return Ok(ContainerRunResult {
                classification: Some(classification),
                script_report: find_script_report(&session.artifact_dir),
                evidence_refs: Vec::new(),
                exit_code,
                stdout_path: Some(stdout_path),
                stderr_path: Some(stderr_path),
                error_code: None,
                message: Some(match classification {
                    FailureClassification::Cancelled => "container attempt cancelled".into(),
                    FailureClassification::Timeout => "container attempt timed out".into(),
                    _ => "container attempt stopped".into(),
                }),
            });
        }
        self.public_result(&session.artifact_dir, exit_code, stdout_path, stderr_path)
    }

    fn validate_request(&self, request: &ExecutionRequest) -> Result<(), RunFailure> {
        let requirement = &request.job.requirements.execution_environment;
        if requirement.environment_key != self.spec.key
            || requirement.kind != ExecutionEnvironmentKind::Container
            || !self.spec.isolation.satisfies(requirement.minimum_isolation)
        {
            return Err(RunFailure::new(
                ContainerFailureCode::ContainerInvalidConfiguration,
                "job execution environment requirement does not match this container backend",
            ));
        }
        let requirements = &request.job.requirements;
        let capabilities = &self.spec.capabilities;
        if !self
            .spec
            .platform
            .eq_ignore_ascii_case(&requirements.platform)
        {
            return Err(capability_failure("platform"));
        }
        if requirements.gpu && !capabilities.gpu {
            return Err(capability_failure("gpu"));
        }
        if requirements.interactive_desktop && !capabilities.interactive_gui {
            return Err(capability_failure("interactive_gui"));
        }
        if requirements.physical_input && !capabilities.physical_input {
            return Err(capability_failure("physical_input"));
        }
        if requirements.semantic_ui && !(capabilities.semantic_ui && capabilities.uia) {
            return Err(capability_failure("semantic_ui/uia"));
        }
        if requirements.makepad && !capabilities.makepad {
            return Err(capability_failure("makepad"));
        }
        if !request.job.application.args.is_empty() {
            return Err(RunFailure::new(
                ContainerFailureCode::ContainerInvalidConfiguration,
                "container runner cannot pass application args until the public Script API supports them",
            ));
        }
        Ok(())
    }

    fn prepare_staging(
        &self,
        request: &ExecutionRequest,
        session: &mut AttemptSession,
    ) -> Result<StagingPaths, RunFailure> {
        let attempts_root = self.host_root.join("attempts");
        fs::create_dir_all(&attempts_root).map_err(|error| {
            RunFailure::new(
                ContainerFailureCode::ContainerMountFailed,
                format!("create trusted staging root: {error}"),
            )
        })?;
        let root = attempts_root.join(short_hash(&format!(
            "{}|{}|{}|{}",
            self.owner_scope, request.job.run_id, request.job.job_id, request.attempt.attempt_id
        )));
        if root.exists() {
            return Err(RunFailure::new(
                ContainerFailureCode::ContainerInvalidConfiguration,
                "attempt staging directory already exists",
            ));
        }
        fs::create_dir(&root).map_err(|error| {
            RunFailure::new(
                ContainerFailureCode::ContainerMountFailed,
                format!("create attempt staging directory: {error}"),
            )
        })?;
        session.staging_root = Some(root.clone());
        let tests = root.join("tests");
        let fixtures = root.join("fixtures");
        let output = root.join("output");
        for path in [&tests, &fixtures, &output] {
            fs::create_dir(path).map_err(|error| {
                RunFailure::new(
                    ContainerFailureCode::ContainerMountFailed,
                    format!("create isolated staging directory: {error}"),
                )
            })?;
        }
        let mut resolved_script =
            script::load(request.job.source.script_path()).map_err(|errors| {
                RunFailure::new(
                    ContainerFailureCode::ContainerInvalidConfiguration,
                    format!("invalid source script: {}", errors.join("; ")),
                )
            })?;
        resolved_script.application.command = Some(
            request
                .job
                .application
                .executable
                .to_string_lossy()
                .into_owned(),
        );
        let script_path = tests.join("resolved-script.aiui.json");
        write_json(&script_path, &resolved_script).map_err(|error| {
            RunFailure::new(
                ContainerFailureCode::ContainerMountFailed,
                format!("stage resolved public script: {error}"),
            )
        })?;
        let fixtures_present = if let Some(source) = &request.job.fixtures {
            copy_source(source, &fixtures).map_err(|error| {
                RunFailure::new(
                    ContainerFailureCode::ContainerMountFailed,
                    format!("stage trusted fixtures: {error}"),
                )
            })?;
            true
        } else {
            false
        };
        Ok(StagingPaths {
            root,
            tests,
            fixtures,
            output,
            fixtures_present,
        })
    }

    fn create_plan(
        &self,
        request: &ExecutionRequest,
        session: &AttemptSession,
        paths: &StagingPaths,
    ) -> Result<ContainerCreatePlan, RunFailure> {
        let mut labels = OwnershipLabels {
            owner_scope: self.owner_scope.clone(),
            run_id: request.job.run_id.clone(),
            job_id: request.job.job_id.clone(),
            attempt_id: request.attempt.attempt_id.clone(),
            worker_id: request.attempt.worker_id.clone(),
        }
        .as_map()
        .map_err(|error| {
            RunFailure::new(ContainerFailureCode::ContainerInvalidConfiguration, error)
        })?;
        labels.insert("ai-ui.environment_key".into(), self.spec.key.clone());
        labels.insert(
            "ai-ui.contract_version".into(),
            EXECUTION_ENVIRONMENT_CONTRACT_VERSION.to_string(),
        );
        let mut mounts = self
            .spec
            .mounts
            .iter()
            .map(|mount| RuntimeMount {
                source: mount.source.clone(),
                target: mount.target.clone(),
                access: mount.access,
            })
            .collect::<Vec<_>>();
        mounts.push(RuntimeMount {
            source: paths.tests.clone(),
            target: "/work/tests".into(),
            access: MountAccess::ReadOnly,
        });
        if paths.fixtures_present {
            mounts.push(RuntimeMount {
                source: paths.fixtures.clone(),
                target: "/work/fixtures".into(),
                access: MountAccess::ReadOnly,
            });
        }
        mounts.push(RuntimeMount {
            source: paths.output.clone(),
            target: "/output".into(),
            access: MountAccess::ArtifactOutput,
        });
        let mut environment = self.spec.environment.clone();
        environment.extend(BTreeMap::from([
            ("HOME".into(), "/home/ai-ui".into()),
            ("TMP".into(), "/tmp".into()),
            ("TEMP".into(), "/tmp".into()),
            ("APPDATA".into(), "/tmp/appdata".into()),
            ("LOCALAPPDATA".into(), "/tmp/localappdata".into()),
            ("XDG_CACHE_HOME".into(), "/tmp/cache".into()),
            ("XDG_CONFIG_HOME".into(), "/tmp/config".into()),
            ("XDG_DATA_HOME".into(), "/tmp/data".into()),
            (
                "AI_UI_ENVIRONMENT_ID".into(),
                session.environment_id.clone(),
            ),
            (
                "AI_UI_ATTEMPT_ID".into(),
                request.attempt.attempt_id.clone(),
            ),
            ("AI_UI_JOB_ID".into(), request.job.job_id.clone()),
            ("AI_UI_RUN_ID".into(), request.job.run_id.clone()),
            ("AI_UI_WORKER_ID".into(), request.attempt.worker_id.clone()),
        ]));
        let runner = self.spec.runner_path.clone().ok_or_else(|| {
            RunFailure::new(
                ContainerFailureCode::ContainerInvalidConfiguration,
                "trusted runner_path is missing",
            )
        })?;
        let mut runner_arguments = vec![
            "run".into(),
            "/work/tests/resolved-script.aiui.json".into(),
            "--output".into(),
            "/output".into(),
        ];
        if paths.fixtures_present {
            runner_arguments.push("--fixtures".into());
            runner_arguments.push("/work/fixtures".into());
        }
        let plan = ContainerCreatePlan {
            version: CONTAINER_RUNTIME_CONTRACT_VERSION,
            name: container_name(&session.environment_id),
            image: self.spec.image.clone().unwrap_or_default(),
            image_digest: session.resolved_image_digest.clone(),
            labels,
            mounts,
            environment,
            network: self.spec.network,
            limits: self.spec.limits.clone(),
            working_dir: "/work".into(),
            runner_executable: runner,
            runner_arguments,
            read_only_root: true,
            tmpfs: vec![
                "/tmp:rw,noexec,nosuid,nodev,size=128m,mode=1777".into(),
                "/run:rw,noexec,nosuid,nodev,size=16m,mode=0755".into(),
            ],
        };
        plan.validate().map_err(|errors| {
            RunFailure::new(
                ContainerFailureCode::ContainerInvalidConfiguration,
                errors.join("; "),
            )
        })?;
        Ok(plan)
    }

    fn stop_running(
        &self,
        session: &mut AttemptSession,
        container_id: &str,
        message: &str,
    ) -> Result<(), RunFailure> {
        session.transition(EnvironmentState::Stopping, message)?;
        self.runtime
            .stop(container_id, self.stop_grace_ms)
            .map_err(RunFailure::from_runtime)?;
        session.transition(EnvironmentState::Stopped, "container stopped")?;
        Ok(())
    }

    fn public_result(
        &self,
        artifact_dir: &Path,
        exit_code: Option<i32>,
        stdout_path: PathBuf,
        stderr_path: PathBuf,
    ) -> Result<ContainerRunResult, RunFailure> {
        let script_report = find_script_report(artifact_dir).ok_or_else(|| {
            RunFailure::new(
                ContainerFailureCode::ContainerExitedUnexpectedly,
                "public ai-ui-test runner exited without report.json",
            )
        })?;
        let report: Value = serde_json::from_slice(&fs::read(&script_report).map_err(|error| {
            RunFailure::new(
                ContainerFailureCode::ContainerArtifactCollectionFailed,
                format!("read public runner report: {error}"),
            )
        })?)
        .map_err(|error| {
            RunFailure::new(
                ContainerFailureCode::ContainerArtifactCollectionFailed,
                format!("parse public runner report: {error}"),
            )
        })?;
        let passed = report.get("status").and_then(Value::as_str) == Some("PASSED");
        let classification = if passed && exit_code == Some(0) {
            None
        } else if passed {
            return Err(RunFailure::new(
                ContainerFailureCode::ContainerExitedUnexpectedly,
                format!("public runner reported PASSED but exited with {exit_code:?}"),
            ));
        } else {
            Some(classify_public_report(&report))
        };
        let evidence_refs = collect_report_evidence(&report)
            .into_iter()
            .filter_map(|path| map_container_artifact_ref(artifact_dir, &path))
            .collect();
        Ok(ContainerRunResult {
            classification,
            script_report: Some(script_report),
            evidence_refs,
            exit_code,
            stdout_path: Some(stdout_path),
            stderr_path: Some(stderr_path),
            error_code: None,
            message: if classification.is_some() {
                Some("public runner reported a test or infrastructure failure".into())
            } else {
                None
            },
        })
    }

    fn cleanup_attempt(
        &self,
        session: &mut AttemptSession,
        guard: &mut OwnedContainerGuard,
    ) -> Result<(), RunFailure> {
        let mut errors = Vec::new();
        if let Some(container_id) = session.container_id.clone() {
            if session.state == EnvironmentState::Running {
                if let Err(error) = self.runtime.stop(&container_id, self.stop_grace_ms) {
                    errors.push(error.to_string());
                } else {
                    let _ =
                        session.transition(EnvironmentState::Stopping, "cleanup requested stop");
                    let _ = session.transition(EnvironmentState::Stopped, "cleanup stopped");
                }
            }
            match self.runtime.remove(&container_id, true) {
                Ok(()) => guard.disarm(),
                Err(error) => errors.push(error.to_string()),
            }
            match self.runtime.list_owned(&self.owner_scope) {
                Ok(containers) => {
                    session.owned_resources_remaining = usize::from(
                        containers
                            .iter()
                            .any(|container| container.id == container_id),
                    );
                    if session.owned_resources_remaining > 0 {
                        errors.push("owned container remains after cleanup".into());
                    }
                }
                Err(error) => errors.push(format!("verify owned cleanup: {error}")),
            }
        }
        if let Some(root) = &session.staging_root {
            if !is_direct_descendant_of(root, &self.host_root.join("attempts")) {
                errors.push("refused to remove staging path outside trusted root".into());
            } else if root.exists() {
                if let Err(error) = fs::remove_dir_all(root) {
                    errors.push(format!("remove isolated staging directory: {error}"));
                }
            }
        }
        if errors.is_empty() {
            if matches!(
                session.state,
                EnvironmentState::Stopped | EnvironmentState::Failed
            ) {
                session.transition(EnvironmentState::Destroyed, "owned resources destroyed")?;
            } else if session.state == EnvironmentState::Preparing {
                session.transition(EnvironmentState::Failed, "attempt preparation failed")?;
                session.transition(EnvironmentState::Destroyed, "staging destroyed")?;
            }
            Ok(())
        } else {
            session.record_failure("container cleanup failed");
            Err(RunFailure::new(
                ContainerFailureCode::ContainerCleanupFailed,
                errors.join("; "),
            ))
        }
    }
}

impl ExecutionEnvironment for ContainerEnvironment {
    fn kind(&self) -> ExecutionEnvironmentKind {
        ExecutionEnvironmentKind::Container
    }

    fn health(&self) -> crate::environment::EnvironmentHealth {
        if let Err(errors) = self.spec.validate() {
            return crate::environment::EnvironmentHealth::unhealthy(
                ContainerFailureCode::ContainerInvalidConfiguration.as_str(),
                errors.join("; "),
            );
        }
        self.runtime.health()
    }

    fn execute(&self, request: &ExecutionRequest, control: &ExecutionControl) -> AttemptOutcome {
        self.execute_inner(request, control)
    }
}

#[derive(Debug)]
struct StagingPaths {
    #[allow(dead_code)]
    root: PathBuf,
    tests: PathBuf,
    fixtures: PathBuf,
    output: PathBuf,
    fixtures_present: bool,
}

#[derive(Debug)]
struct AttemptSession {
    environment_id: String,
    artifact_dir: PathBuf,
    created_at_ms: u64,
    state: EnvironmentState,
    lifecycle: Vec<LifecycleEvent>,
    timings: EnvironmentTimings,
    staging_root: Option<PathBuf>,
    container_id: Option<String>,
    runtime_info: Option<ContainerRuntimeInfo>,
    resolved_image_digest: Option<String>,
    owned_resources_remaining: usize,
}

impl AttemptSession {
    fn new(
        environment_id: String,
        artifact_dir: PathBuf,
        created_at_ms: u64,
        _capabilities: EnvironmentCapabilities,
    ) -> Self {
        Self {
            environment_id,
            artifact_dir,
            created_at_ms,
            state: EnvironmentState::Created,
            lifecycle: vec![LifecycleEvent {
                state: EnvironmentState::Created,
                timestamp_ms: created_at_ms,
                message: Some("environment identity created".into()),
            }],
            timings: EnvironmentTimings::default(),
            staging_root: None,
            container_id: None,
            runtime_info: None,
            resolved_image_digest: None,
            owned_resources_remaining: 0,
        }
    }

    fn transition(
        &mut self,
        next: EnvironmentState,
        message: impl Into<String>,
    ) -> Result<(), RunFailure> {
        if !environment_transition_allowed(self.state, next) {
            return Err(RunFailure::new(
                ContainerFailureCode::ContainerInvalidConfiguration,
                format!(
                    "illegal environment transition: {:?} -> {next:?}",
                    self.state
                ),
            ));
        }
        self.state = next;
        self.lifecycle.push(LifecycleEvent {
            state: next,
            timestamp_ms: now_unix_ms().max(self.created_at_ms),
            message: Some(message.into()),
        });
        Ok(())
    }

    fn record_failure(&mut self, message: &str) {
        if self.state != EnvironmentState::Failed
            && environment_transition_allowed(self.state, EnvironmentState::Failed)
        {
            let _ = self.transition(EnvironmentState::Failed, message);
        }
    }
}

struct OwnedContainerGuard {
    runtime: Arc<dyn ContainerRuntime>,
    container_id: Option<String>,
}

impl OwnedContainerGuard {
    fn new(runtime: Arc<dyn ContainerRuntime>) -> Self {
        Self {
            runtime,
            container_id: None,
        }
    }

    fn arm(&mut self, container_id: String) {
        self.container_id = Some(container_id);
    }

    fn disarm(&mut self) {
        self.container_id = None;
    }
}

impl Drop for OwnedContainerGuard {
    fn drop(&mut self) {
        if let Some(container_id) = self.container_id.take() {
            let _ = self.runtime.remove(&container_id, true);
        }
    }
}

#[derive(Debug)]
struct RunFailure {
    code: ContainerFailureCode,
    message: String,
}

impl RunFailure {
    fn new(code: ContainerFailureCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn from_runtime(error: ContainerRuntimeError) -> Self {
        Self::new(error.code, error.message)
    }
}

#[derive(Debug)]
struct ContainerRunResult {
    classification: Option<FailureClassification>,
    script_report: Option<PathBuf>,
    evidence_refs: Vec<PathBuf>,
    exit_code: Option<i32>,
    stdout_path: Option<PathBuf>,
    stderr_path: Option<PathBuf>,
    error_code: Option<String>,
    message: Option<String>,
}

impl ContainerRunResult {
    fn infrastructure(code: ContainerFailureCode, message: impl Into<String>) -> Self {
        Self {
            classification: Some(FailureClassification::InfrastructureFailure),
            script_report: None,
            evidence_refs: Vec::new(),
            exit_code: None,
            stdout_path: None,
            stderr_path: None,
            error_code: Some(code.as_str().into()),
            message: Some(message.into()),
        }
    }
}

fn capability_failure(capability: &str) -> RunFailure {
    RunFailure::new(
        ContainerFailureCode::ContainerInvalidConfiguration,
        format!("container environment has no proven {capability} capability"),
    )
}

fn infrastructure_outcome(
    code: ContainerFailureCode,
    message: impl Into<String>,
    metadata: Option<ExecutionEnvironmentMetadata>,
    environment_report: Option<PathBuf>,
) -> AttemptOutcome {
    AttemptOutcome {
        classification: Some(FailureClassification::InfrastructureFailure),
        script_report: None,
        evidence_refs: Vec::new(),
        runtime: Some(RuntimeDiagnostics {
            runner_pid: None,
            exit_code: None,
            stdout_path: None,
            stderr_path: None,
            error_code: Some(code.as_str().into()),
            message: Some(message.into()),
        }),
        execution_environment: metadata,
        environment_report,
    }
}

fn validate_static_mount(
    mount: &crate::environment::TrustedMount,
    host_root: &Path,
) -> Result<(), String> {
    mount.validate()?;
    let normalized_target = mount.target.replace('\\', "/").to_ascii_lowercase();
    if ["/work", "/output", "/tmp", "/run", "/home"]
        .iter()
        .any(|reserved| {
            normalized_target == *reserved || normalized_target.starts_with(&format!("{reserved}/"))
        })
    {
        return Err(format!(
            "trusted static mount targets reserved container path: {}",
            mount.target
        ));
    }
    let source = fs::canonicalize(&mount.source).map_err(|error| {
        format!(
            "trusted static mount source cannot be resolved {}: {error}",
            mount.source.display()
        )
    })?;
    let workspace = std::env::current_dir()
        .ok()
        .and_then(|path| fs::canonicalize(path).ok());
    let profile = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .and_then(|path| fs::canonicalize(path).ok());
    for forbidden in [workspace, profile].into_iter().flatten() {
        if source.starts_with(&forbidden)
            || forbidden.starts_with(&source)
            || host_root.starts_with(&source)
        {
            return Err(format!(
                "trusted static mount exposes a forbidden broad host path: {}",
                source.display()
            ));
        }
    }
    Ok(())
}

fn environment_id(request: &ExecutionRequest, owner_scope: &str) -> String {
    format!(
        "env-{}",
        short_hash(&format!(
            "{owner_scope}|{}|{}|{}",
            request.job.run_id, request.job.job_id, request.attempt.attempt_id
        ))
    )
}

fn container_name(environment_id: &str) -> String {
    format!("ai-ui-{environment_id}").chars().take(63).collect()
}

fn short_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn map_container_artifact_ref(root: &Path, path: &Path) -> Option<PathBuf> {
    if path.is_relative() {
        return Some(root.join(path));
    }
    path.strip_prefix("/output")
        .ok()
        .map(|relative| root.join(relative))
}

fn copy_source(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("fixture symlinks are not allowed".into());
    }
    if metadata.is_file() {
        let name = source
            .file_name()
            .ok_or_else(|| "fixture file has no name".to_string())?;
        fs::copy(source, destination.join(name)).map_err(|error| error.to_string())?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err("fixtures must be a regular file or directory".into());
    }
    copy_tree_contents(source, destination)
}

fn copy_tree_contents(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let mut entries = fs::read_dir(source)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "symlink is not allowed in isolated copy: {}",
                source_path.display()
            ));
        }
        if metadata.is_dir() {
            copy_tree_contents(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &destination_path).map_err(|error| error.to_string())?;
        } else {
            return Err(format!(
                "unsupported file type in isolated copy: {}",
                source_path.display()
            ));
        }
    }
    Ok(())
}

fn collect_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut output = Vec::new();
    collect_files_inner(root, &mut output)?;
    output.sort();
    Ok(output)
}

fn collect_files_inner(root: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "artifact symlink is not allowed: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_files_inner(&path, output)?;
        } else if metadata.is_file() {
            output.push(path);
        }
    }
    Ok(())
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn is_direct_descendant_of(path: &Path, root: &Path) -> bool {
    path.parent() == Some(root)
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir | Component::CurDir))
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        container_runtime::{
            CONTAINER_RUNTIME_CONTRACT_VERSION, ContainerCreatePlan, MockContainerRuntime,
            MockExecution, MockFailurePoint, RuntimeMount,
        },
        environment::{
            EnvironmentIsolation, EnvironmentNetworkPolicy, EnvironmentResourceLimits,
            ExecutionEnvironmentRequirement,
        },
        model::{
            ApplicationSpec, ArtifactSpec, AttemptRecord, ExecutionRequirements, JobSource,
            JobState, RetryPolicy, TEST_JOB_VERSION, TestJob,
        },
    };
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ROOT: AtomicU64 = AtomicU64::new(1);

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ai-ui-container-{name}-{}-{}",
            std::process::id(),
            NEXT_TEST_ROOT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn spec() -> ExecutionEnvironmentSpec {
        ExecutionEnvironmentSpec {
            version: EXECUTION_ENVIRONMENT_CONTRACT_VERSION,
            key: "mock-container".into(),
            kind: ExecutionEnvironmentKind::Container,
            platform: "linux".into(),
            image: Some("ai-ui/mock-runner:v1".into()),
            image_digest: Some("sha256:mock-container-image-v1".into()),
            runner_path: Some("/opt/ai-ui/ai-ui-test".into()),
            isolation: EnvironmentIsolation::Filesystem,
            disposable: true,
            reusable: false,
            mounts: Vec::new(),
            environment: BTreeMap::from([("AI_UI_TEST_MODE".into(), "mock".into())]),
            network: EnvironmentNetworkPolicy::None,
            limits: EnvironmentResourceLimits {
                memory_mib: Some(128),
                cpu_millis: Some(500),
                pids: Some(32),
            },
            capabilities: EnvironmentCapabilities {
                process_isolation: true,
                filesystem_isolation: true,
                environment_isolation: true,
                ..EnvironmentCapabilities::default()
            },
        }
    }

    fn request(root: &Path, timeout_ms: u64) -> ExecutionRequest {
        let source = root.join("source.aiui.json");
        fs::create_dir_all(root).unwrap();
        fs::write(
            &source,
            serde_json::to_vec_pretty(&json!({
                "version": 1,
                "name": "container mock proof",
                "application": {"command": "untrusted-placeholder"},
                "options": {"semantic_only": true},
                "steps": [{
                    "op": "session.open",
                    "save_session_as": "app"
                }, {
                    "op": "assert_visible",
                    "session": "app",
                    "id": "button.create"
                }],
                "teardown": [{
                    "op": "session.close",
                    "session": "app"
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let attempt = AttemptRecord {
            attempt_id: "attempt:container:1".into(),
            job_id: "job:container:1".into(),
            number: 1,
            worker_id: "container-worker:000".into(),
            worker_instance_id: "container-worker:000:instance".into(),
            lease_id: "lease:container:1".into(),
            status: JobState::Running,
            leased_at_ms: 1,
            started_at_ms: Some(2),
            finished_at_ms: None,
            classification: None,
            script_report: None,
            evidence_refs: Vec::new(),
            runtime: None,
            execution_environment: None,
            environment_report: None,
        };
        ExecutionRequest {
            job: TestJob {
                version: TEST_JOB_VERSION,
                job_id: attempt.job_id.clone(),
                run_id: "run:container:1".into(),
                application: ApplicationSpec {
                    key: "mock-app".into(),
                    executable: "/opt/ai-ui/mock-app".into(),
                    args: Vec::new(),
                    max_parallel: 1,
                },
                source: JobSource::Script { path: source },
                fixtures: None,
                priority: 0,
                timeout_ms,
                retry: RetryPolicy::default(),
                requirements: ExecutionRequirements {
                    platform: "linux".into(),
                    interactive_desktop: false,
                    gpu: false,
                    semantic_ui: false,
                    physical_input: false,
                    makepad: false,
                    semantic_only: true,
                    execution_environment: ExecutionEnvironmentRequirement {
                        environment_key: "mock-container".into(),
                        kind: ExecutionEnvironmentKind::Container,
                        minimum_isolation: EnvironmentIsolation::Filesystem,
                    },
                },
                artifacts: ArtifactSpec {
                    output_dir: root.join("artifacts"),
                },
                metadata: BTreeMap::new(),
                created_at_ms: 0,
                retry_safe: true,
            },
            attempt,
        }
    }

    #[test]
    fn disposable_attempt_uses_public_runner_collects_report_and_leaks_nothing() {
        let test_root = root("pass");
        let runtime = Arc::new(MockContainerRuntime::default());
        let environment = ContainerEnvironment::new(
            spec(),
            runtime.clone(),
            test_root.join("host-root"),
            "scope-pass",
        )
        .unwrap()
        .with_poll_interval_ms(1);
        let request = request(&test_root, 1_000);
        let outcome = environment.execute(&request, &ExecutionControl::default());
        assert_eq!(outcome.classification, None);
        assert_eq!(runtime.remaining(), 0);
        assert!(
            outcome
                .script_report
                .as_ref()
                .is_some_and(|path| path.is_file())
        );
        let report_path = outcome.environment_report.unwrap();
        let report: EnvironmentReport =
            serde_json::from_slice(&fs::read(report_path).unwrap()).unwrap();
        assert!(report.cleanup_succeeded);
        assert_eq!(report.owned_resources_remaining, 0);
        assert_eq!(report.identity.state, EnvironmentState::Destroyed);
        assert_eq!(
            report.metadata.environment_type,
            ExecutionEnvironmentKind::Container
        );
        let plans = runtime.observed_plans();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].runner_arguments[0], "run");
        assert_eq!(
            plans[0].runner_arguments[1],
            "/work/tests/resolved-script.aiui.json"
        );
        assert!(plans[0].read_only_root);
        assert_eq!(plans[0].network, EnvironmentNetworkPolicy::None);
        assert_eq!(plans[0].environment["TMP"], "/tmp");
        assert_eq!(
            plans[0].environment["AI_UI_ATTEMPT_ID"],
            request.attempt.attempt_id
        );
        assert!(plans[0]
            .mounts
            .iter()
            .any(|mount| mount.target == "/work/tests" && mount.access == MountAccess::ReadOnly));
        assert!(
            plans[0]
                .mounts
                .iter()
                .all(|mount| mount.source != test_root)
        );
        let _ = fs::remove_dir_all(test_root);
    }

    #[test]
    fn timeout_and_cancellation_stop_and_remove_owned_container() {
        for (name, cancel, expected) in [
            ("timeout", false, FailureClassification::Timeout),
            ("cancel", true, FailureClassification::Cancelled),
        ] {
            let test_root = root(name);
            let runtime = Arc::new(MockContainerRuntime::default());
            runtime.enqueue(MockExecution {
                polls_before_exit: 10_000,
                ..MockExecution::default()
            });
            let environment = ContainerEnvironment::new(
                spec(),
                runtime.clone(),
                test_root.join("host-root"),
                format!("scope-{name}"),
            )
            .unwrap()
            .with_poll_interval_ms(1);
            let control = ExecutionControl::default();
            if cancel {
                control.cancel();
            }
            let outcome = environment.execute(&request(&test_root, 3), &control);
            assert_eq!(outcome.classification, Some(expected));
            assert_eq!(runtime.remaining(), 0);
            let _ = fs::remove_dir_all(test_root);
        }
    }

    #[test]
    fn canonical_create_failure_is_infrastructure_and_still_writes_report() {
        let test_root = root("create-failure");
        let runtime = Arc::new(MockContainerRuntime::default());
        runtime.enqueue(MockExecution {
            failure: Some(crate::container_runtime::MockFailurePoint::Create),
            ..MockExecution::default()
        });
        let environment = ContainerEnvironment::new(
            spec(),
            runtime.clone(),
            test_root.join("host-root"),
            "scope-create-failure",
        )
        .unwrap();
        let outcome = environment.execute(&request(&test_root, 100), &ExecutionControl::default());
        assert_eq!(
            outcome.classification,
            Some(FailureClassification::InfrastructureFailure)
        );
        assert_eq!(
            outcome.runtime.as_ref().unwrap().error_code.as_deref(),
            Some(ContainerFailureCode::ContainerCreateFailed.as_str())
        );
        assert!(
            outcome
                .environment_report
                .as_ref()
                .is_some_and(|path| path.is_file())
        );
        assert_eq!(runtime.remaining(), 0);
        let _ = fs::remove_dir_all(test_root);
    }

    #[test]
    fn explicit_reaper_removes_only_its_owner_scope() {
        let test_root = root("reaper");
        let runtime = Arc::new(MockContainerRuntime::default());
        let environment = ContainerEnvironment::new(
            spec(),
            runtime.clone(),
            test_root.join("host-root"),
            "owned-scope",
        )
        .unwrap();
        let make_plan = |scope: &str, name: &str| ContainerCreatePlan {
            version: CONTAINER_RUNTIME_CONTRACT_VERSION,
            name: name.into(),
            image: "ai-ui/mock-runner:v1".into(),
            image_digest: None,
            labels: BTreeMap::from([
                ("ai-ui.tool".into(), "true".into()),
                ("ai-ui.owner_scope".into(), scope.into()),
            ]),
            mounts: vec![RuntimeMount {
                source: test_root.join(format!("output-{name}")),
                target: "/output".into(),
                access: MountAccess::ArtifactOutput,
            }],
            environment: BTreeMap::from([("TMP".into(), "/tmp".into())]),
            network: EnvironmentNetworkPolicy::None,
            limits: EnvironmentResourceLimits::default(),
            working_dir: "/work".into(),
            runner_executable: "/opt/ai-ui/ai-ui-test".into(),
            runner_arguments: vec!["run".into(), "/work/test.json".into()],
            read_only_root: true,
            tmpfs: vec!["/tmp:rw,size=16m".into()],
        };
        let owned = runtime.create(&make_plan("owned-scope", "owned")).unwrap();
        let foreign = runtime
            .create(&make_plan("foreign-scope", "foreign"))
            .unwrap();
        let report = environment.reap_owned();
        assert_eq!(report.discovered, 1);
        assert_eq!(report.removed, 1);
        assert_eq!(report.remaining, 0);
        assert!(report.failures.is_empty());
        assert!(runtime.removed_ids().contains(&owned.id));
        assert!(!runtime.removed_ids().contains(&foreign.id));
        assert_eq!(runtime.remaining(), 1);
        runtime.remove(&foreign.id, true).unwrap();
        let _ = fs::remove_dir_all(test_root);
    }

    #[test]
    fn runtime_failure_points_keep_canonical_infrastructure_codes() {
        let cases = [
            (
                MockFailurePoint::Info,
                ContainerFailureCode::ContainerRuntimeUnavailable,
            ),
            (
                MockFailurePoint::ImageInspect,
                ContainerFailureCode::ContainerImageMissing,
            ),
            (
                MockFailurePoint::Create,
                ContainerFailureCode::ContainerCreateFailed,
            ),
            (
                MockFailurePoint::Start,
                ContainerFailureCode::ContainerStartFailed,
            ),
            (
                MockFailurePoint::Inspect,
                ContainerFailureCode::ContainerExitedUnexpectedly,
            ),
            (
                MockFailurePoint::Logs,
                ContainerFailureCode::ContainerArtifactCollectionFailed,
            ),
        ];
        for (point, expected_code) in cases {
            let test_root = root(&format!("failure-{point:?}"));
            let runtime = Arc::new(MockContainerRuntime::default());
            runtime.enqueue(MockExecution {
                failure: Some(point),
                ..MockExecution::default()
            });
            let environment = ContainerEnvironment::new(
                spec(),
                runtime.clone(),
                test_root.join("host-root"),
                format!("scope-failure-{point:?}"),
            )
            .unwrap()
            .with_poll_interval_ms(1);
            let outcome =
                environment.execute(&request(&test_root, 100), &ExecutionControl::default());
            assert_eq!(
                outcome.classification,
                Some(FailureClassification::InfrastructureFailure),
                "failure point {point:?}"
            );
            assert_eq!(
                outcome.runtime.as_ref().unwrap().error_code.as_deref(),
                Some(expected_code.as_str()),
                "failure point {point:?}"
            );
            let report: EnvironmentReport = serde_json::from_slice(
                &fs::read(outcome.environment_report.as_ref().unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(report.failure_code.as_deref(), Some(expected_code.as_str()));
            assert!(report.cleanup_succeeded);
            assert_eq!(report.owned_resources_remaining, 0);
            assert_eq!(runtime.remaining(), 0);
            let _ = fs::remove_dir_all(test_root);
        }
    }

    #[test]
    fn public_report_test_and_infrastructure_failures_remain_distinct() {
        for (name, error_code, expected) in [
            (
                "test",
                "POSTCONDITION_FAILED",
                FailureClassification::TestFailure,
            ),
            (
                "infrastructure",
                "APP_NOT_FOUND",
                FailureClassification::InfrastructureFailure,
            ),
        ] {
            let test_root = root(&format!("public-{name}"));
            let runtime = Arc::new(MockContainerRuntime::default());
            runtime.enqueue(MockExecution {
                exit_code: 3,
                report: Some(json!({
                    "version": 1,
                    "status": "FAILED",
                    "failures": [{
                        "category": "ASSERTION_FAILED",
                        "error_code": error_code
                    }],
                    "evidence_refs": []
                })),
                ..MockExecution::default()
            });
            let environment = ContainerEnvironment::new(
                spec(),
                runtime.clone(),
                test_root.join("host-root"),
                format!("scope-public-{name}"),
            )
            .unwrap()
            .with_poll_interval_ms(1);
            let outcome =
                environment.execute(&request(&test_root, 100), &ExecutionControl::default());
            assert_eq!(outcome.classification, Some(expected));
            assert!(
                outcome
                    .script_report
                    .as_ref()
                    .is_some_and(|path| path.is_file())
            );
            assert_eq!(runtime.remaining(), 0);
            let _ = fs::remove_dir_all(test_root);
        }
    }

    #[test]
    fn cleanup_failure_is_infrastructure_and_reaper_reports_the_owned_leak() {
        let test_root = root("cleanup-failure");
        let runtime = Arc::new(MockContainerRuntime::default());
        runtime.enqueue(MockExecution {
            failure: Some(MockFailurePoint::Remove),
            ..MockExecution::default()
        });
        let environment = ContainerEnvironment::new(
            spec(),
            runtime.clone(),
            test_root.join("host-root"),
            "scope-cleanup-failure",
        )
        .unwrap()
        .with_poll_interval_ms(1);
        let outcome = environment.execute(&request(&test_root, 100), &ExecutionControl::default());
        assert_eq!(
            outcome.classification,
            Some(FailureClassification::InfrastructureFailure)
        );
        assert_eq!(
            outcome.runtime.as_ref().unwrap().error_code.as_deref(),
            Some(ContainerFailureCode::ContainerCleanupFailed.as_str())
        );
        let report: EnvironmentReport = serde_json::from_slice(
            &fs::read(outcome.environment_report.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        assert!(!report.cleanup_succeeded);
        assert_eq!(
            report.failure_code.as_deref(),
            Some("CONTAINER_CLEANUP_FAILED")
        );
        assert_eq!(report.owned_resources_remaining, 1);
        assert_eq!(runtime.remaining(), 1);
        let reaper = environment.reap_owned();
        assert_eq!(reaper.discovered, 1);
        assert_eq!(reaper.remaining, 1);
        assert!(
            reaper
                .failures
                .iter()
                .all(|failure| failure.error_code == "CONTAINER_CLEANUP_FAILED")
        );
        drop(runtime);
        let _ = fs::remove_dir_all(test_root);
    }

    #[test]
    fn reserved_dynamic_environment_identity_cannot_be_overridden() {
        let test_root = root("reserved-environment");
        let mut invalid = spec();
        invalid
            .environment
            .insert("AI_UI_ATTEMPT_ID".into(), "untrusted-override".into());
        let error = ContainerEnvironment::new(
            invalid,
            Arc::new(MockContainerRuntime::default()),
            test_root.join("host-root"),
            "scope-reserved-environment",
        )
        .err()
        .expect("reserved variable must be rejected");
        assert!(error.contains("cannot override reserved variable AI_UI_ATTEMPT_ID"));
        let _ = fs::remove_dir_all(test_root);
    }
}
