use crate::environment::{
    ContainerFailureCode, EnvironmentHealth, EnvironmentNetworkPolicy, EnvironmentResourceLimits,
    MountAccess, TrustedMount,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::OsString,
    fmt, fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::Mutex,
};

pub const CONTAINER_RUNTIME_CONTRACT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeMount {
    pub source: PathBuf,
    pub target: String,
    pub access: MountAccess,
}

impl RuntimeMount {
    fn validate(&self) -> Result<(), String> {
        TrustedMount {
            source: self.source.clone(),
            target: self.target.clone(),
            access: self.access,
        }
        .validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerCreatePlan {
    pub version: u32,
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub image_digest: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub mounts: Vec<RuntimeMount>,
    pub environment: BTreeMap<String, String>,
    pub network: EnvironmentNetworkPolicy,
    pub limits: EnvironmentResourceLimits,
    pub working_dir: String,
    pub runner_executable: String,
    pub runner_arguments: Vec<String>,
    pub read_only_root: bool,
    pub tmpfs: Vec<String>,
}

impl ContainerCreatePlan {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.version != CONTAINER_RUNTIME_CONTRACT_VERSION {
            errors.push(format!(
                "container create plan version must be {CONTAINER_RUNTIME_CONTRACT_VERSION}"
            ));
        }
        if !safe_runtime_name(&self.name) {
            errors.push("container name contains unsupported characters".into());
        }
        if self.image.is_empty() || unsafe_text(&self.image) {
            errors.push("container image must be non-empty and control-character safe".into());
        }
        if !absolute_container_path(&self.working_dir)
            || !absolute_container_path(&self.runner_executable)
        {
            errors.push("container working directory and runner must be absolute".into());
        }
        if unsafe_text(&self.runner_executable)
            || self.runner_arguments.iter().any(|value| unsafe_text(value))
        {
            errors.push("container runner values contain unsafe control characters".into());
        }
        if self.labels.get("ai-ui.tool").map(String::as_str) != Some("true")
            || self
                .labels
                .get("ai-ui.owner_scope")
                .is_none_or(String::is_empty)
        {
            errors.push("container ownership labels are incomplete".into());
        }
        for (key, value) in &self.labels {
            if key.is_empty() || unsafe_text(key) || value.is_empty() || unsafe_text(value) {
                errors.push(format!("invalid container label: {key}"));
            }
        }
        for (key, value) in &self.environment {
            if !safe_environment_key(key) || unsafe_text(value) {
                errors.push(format!("invalid container environment entry: {key}"));
            }
        }
        let mut mount_targets = BTreeSet::new();
        let mut artifact_outputs = 0;
        for mount in &self.mounts {
            if let Err(error) = mount.validate() {
                errors.push(error);
            }
            if !mount_targets.insert(mount.target.replace('\\', "/").to_ascii_lowercase()) {
                errors.push(format!(
                    "duplicate container mount target: {}",
                    mount.target
                ));
            }
            if mount.access == MountAccess::ArtifactOutput {
                artifact_outputs += 1;
            }
        }
        if artifact_outputs != 1 {
            errors.push("container create plan requires exactly one artifact output mount".into());
        }
        if let Err(error) = self.limits.validate() {
            errors.push(error);
        }
        if self.tmpfs.iter().any(|entry| {
            let target = entry.split(':').next().unwrap_or_default();
            !absolute_container_path(target) || unsafe_text(entry)
        }) {
            errors.push("tmpfs entries must use absolute, control-character-safe paths".into());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn output_mount(&self) -> Option<&RuntimeMount> {
        self.mounts
            .iter()
            .find(|mount| mount.access == MountAccess::ArtifactOutput)
    }
}

fn safe_runtime_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn safe_environment_key(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn absolute_container_path(value: &str) -> bool {
    value.starts_with('/')
        || (value.len() >= 3
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'/' | b'\\'))
}

fn unsafe_text(value: &str) -> bool {
    value.contains('\0') || value.contains('\r') || value.contains('\n')
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerRuntimeError {
    pub code: ContainerFailureCode,
    pub message: String,
}

impl ContainerRuntimeError {
    pub fn new(code: ContainerFailureCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ContainerRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ContainerRuntimeError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerRuntimeInfo {
    pub os: String,
    pub architecture: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeContainer {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeContainerStatus {
    Created,
    Running,
    Exited,
    Dead,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeContainerState {
    pub status: RuntimeContainerStatus,
    pub exit_code: Option<i32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeLogs {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub trait ContainerRuntime: Send + Sync {
    fn health(&self) -> EnvironmentHealth;
    fn info(&self) -> Result<ContainerRuntimeInfo, ContainerRuntimeError>;
    fn image_digest(&self, image: &str) -> Result<Option<String>, ContainerRuntimeError>;
    fn create(&self, plan: &ContainerCreatePlan)
    -> Result<RuntimeContainer, ContainerRuntimeError>;
    fn start(&self, container_id: &str) -> Result<(), ContainerRuntimeError>;
    fn state(&self, container_id: &str) -> Result<RuntimeContainerState, ContainerRuntimeError>;
    fn logs(&self, container_id: &str) -> Result<RuntimeLogs, ContainerRuntimeError>;
    fn stop(&self, container_id: &str, grace_ms: u64) -> Result<(), ContainerRuntimeError>;
    fn remove(&self, container_id: &str, force: bool) -> Result<(), ContainerRuntimeError>;
    fn list_tool_owned(&self) -> Result<Vec<RuntimeContainer>, ContainerRuntimeError>;
    fn list_owned(&self, owner_scope: &str)
    -> Result<Vec<RuntimeContainer>, ContainerRuntimeError>;
}

#[derive(Clone, Debug)]
pub struct DockerCliRuntime {
    pub executable: PathBuf,
}

impl DockerCliRuntime {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    fn invoke(
        &self,
        arguments: &[OsString],
        code: ContainerFailureCode,
    ) -> Result<Output, ContainerRuntimeError> {
        let output = Command::new(&self.executable)
            .args(arguments)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| {
                ContainerRuntimeError::new(
                    ContainerFailureCode::ContainerRuntimeUnavailable,
                    format!("start Docker-compatible CLI: {error}"),
                )
            })?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(ContainerRuntimeError::new(
                code,
                bounded_stderr(&output.stderr),
            ))
        }
    }
}

impl ContainerRuntime for DockerCliRuntime {
    fn health(&self) -> EnvironmentHealth {
        match self.info() {
            Ok(_) => EnvironmentHealth::healthy(),
            Err(error) => EnvironmentHealth::unhealthy(error.code.as_str(), error.message),
        }
    }

    fn info(&self) -> Result<ContainerRuntimeInfo, ContainerRuntimeError> {
        let output = self.invoke(
            &[
                "version".into(),
                "--format".into(),
                "{{.Server.Os}}|{{.Server.Arch}}|{{.Server.Version}}".into(),
            ],
            ContainerFailureCode::ContainerRuntimeUnavailable,
        )?;
        let text = String::from_utf8_lossy(&output.stdout);
        let mut values = text.trim().split('|');
        let os = values.next().unwrap_or_default().to_string();
        let architecture = values.next().unwrap_or_default().to_string();
        let version = values.next().unwrap_or_default().to_string();
        if os.is_empty() || architecture.is_empty() || version.is_empty() {
            return Err(ContainerRuntimeError::new(
                ContainerFailureCode::ContainerRuntimeUnavailable,
                "container runtime returned incomplete server information",
            ));
        }
        Ok(ContainerRuntimeInfo {
            os,
            architecture,
            version,
        })
    }

    fn image_digest(&self, image: &str) -> Result<Option<String>, ContainerRuntimeError> {
        let output = self.invoke(
            &[
                "image".into(),
                "inspect".into(),
                "--format".into(),
                "{{json .RepoDigests}}|{{.Id}}".into(),
                image.into(),
            ],
            ContainerFailureCode::ContainerImageMissing,
        )?;
        let text = String::from_utf8_lossy(&output.stdout);
        let (repo_digests, image_id) = text.trim().split_once('|').unwrap_or(("[]", ""));
        let values: Vec<String> = serde_json::from_str(repo_digests).unwrap_or_default();
        Ok(values
            .first()
            .and_then(|value| value.rsplit_once('@').map(|(_, digest)| digest.to_string()))
            .or_else(|| (!image_id.is_empty()).then(|| image_id.to_string())))
    }

    fn create(
        &self,
        plan: &ContainerCreatePlan,
    ) -> Result<RuntimeContainer, ContainerRuntimeError> {
        plan.validate().map_err(|errors| {
            ContainerRuntimeError::new(
                ContainerFailureCode::ContainerInvalidConfiguration,
                errors.join("; "),
            )
        })?;
        let mut arguments = vec![
            OsString::from("create"),
            OsString::from("--name"),
            OsString::from(&plan.name),
        ];
        for (key, value) in &plan.labels {
            arguments.push("--label".into());
            arguments.push(format!("{key}={value}").into());
        }
        if plan.network == EnvironmentNetworkPolicy::None {
            arguments.push("--network".into());
            arguments.push("none".into());
        }
        if plan.read_only_root {
            arguments.push("--read-only".into());
        }
        for tmpfs in &plan.tmpfs {
            arguments.push("--tmpfs".into());
            arguments.push(tmpfs.into());
        }
        arguments.push("--workdir".into());
        arguments.push(plan.working_dir.clone().into());
        for (key, value) in &plan.environment {
            arguments.push("--env".into());
            arguments.push(format!("{key}={value}").into());
        }
        if let Some(memory_mib) = plan.limits.memory_mib {
            arguments.push("--memory".into());
            arguments.push(format!("{memory_mib}m").into());
        }
        if let Some(cpu_millis) = plan.limits.cpu_millis {
            arguments.push("--cpus".into());
            arguments.push(format!("{:.3}", f64::from(cpu_millis) / 1_000.0).into());
        }
        if let Some(pids) = plan.limits.pids {
            arguments.push("--pids-limit".into());
            arguments.push(pids.to_string().into());
        }
        for mount in &plan.mounts {
            let mut rendered = format!(
                "type=bind,source={},target={}",
                mount.source.to_string_lossy(),
                mount.target
            );
            if mount.access == MountAccess::ReadOnly {
                rendered.push_str(",readonly");
            }
            arguments.push("--mount".into());
            arguments.push(rendered.into());
        }
        arguments.push("--entrypoint".into());
        arguments.push(plan.runner_executable.clone().into());
        arguments.push(plan.image.clone().into());
        arguments.extend(plan.runner_arguments.iter().cloned().map(OsString::from));
        let output = self.invoke(&arguments, ContainerFailureCode::ContainerCreateFailed)?;
        let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if id.is_empty() {
            return Err(ContainerRuntimeError::new(
                ContainerFailureCode::ContainerCreateFailed,
                "container runtime returned an empty container id",
            ));
        }
        Ok(RuntimeContainer {
            id,
            name: plan.name.clone(),
        })
    }

    fn start(&self, container_id: &str) -> Result<(), ContainerRuntimeError> {
        self.invoke(
            &["start".into(), container_id.into()],
            ContainerFailureCode::ContainerStartFailed,
        )?;
        Ok(())
    }

    fn state(&self, container_id: &str) -> Result<RuntimeContainerState, ContainerRuntimeError> {
        let output = self.invoke(
            &[
                "inspect".into(),
                "--format".into(),
                "{{.State.Status}}|{{.State.ExitCode}}".into(),
                container_id.into(),
            ],
            ContainerFailureCode::ContainerExitedUnexpectedly,
        )?;
        let text = String::from_utf8_lossy(&output.stdout);
        let mut values = text.trim().split('|');
        let status = match values.next().unwrap_or_default() {
            "created" => RuntimeContainerStatus::Created,
            "running" | "restarting" => RuntimeContainerStatus::Running,
            "exited" => RuntimeContainerStatus::Exited,
            "dead" | "removing" => RuntimeContainerStatus::Dead,
            _ => RuntimeContainerStatus::Unknown,
        };
        let exit_code = values.next().and_then(|value| value.parse::<i32>().ok());
        Ok(RuntimeContainerState { status, exit_code })
    }

    fn logs(&self, container_id: &str) -> Result<RuntimeLogs, ContainerRuntimeError> {
        let output = self.invoke(
            &["logs".into(), container_id.into()],
            ContainerFailureCode::ContainerArtifactCollectionFailed,
        )?;
        Ok(RuntimeLogs {
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn stop(&self, container_id: &str, grace_ms: u64) -> Result<(), ContainerRuntimeError> {
        let seconds = grace_ms.saturating_add(999) / 1_000;
        self.invoke(
            &[
                "stop".into(),
                "--time".into(),
                seconds.to_string().into(),
                container_id.into(),
            ],
            ContainerFailureCode::ContainerCleanupFailed,
        )?;
        Ok(())
    }

    fn remove(&self, container_id: &str, force: bool) -> Result<(), ContainerRuntimeError> {
        let mut arguments = vec![OsString::from("rm")];
        if force {
            arguments.push("--force".into());
        }
        arguments.push(container_id.into());
        self.invoke(&arguments, ContainerFailureCode::ContainerCleanupFailed)?;
        Ok(())
    }

    fn list_tool_owned(&self) -> Result<Vec<RuntimeContainer>, ContainerRuntimeError> {
        let output = self.invoke(
            &[
                "ps".into(),
                "--all".into(),
                "--filter".into(),
                "label=ai-ui.tool=true".into(),
                "--format".into(),
                "{{.ID}}|{{.Names}}".into(),
            ],
            ContainerFailureCode::ContainerCleanupFailed,
        )?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let (id, name) = line.split_once('|')?;
                Some(RuntimeContainer {
                    id: id.to_string(),
                    name: name.to_string(),
                })
            })
            .collect())
    }

    fn list_owned(
        &self,
        owner_scope: &str,
    ) -> Result<Vec<RuntimeContainer>, ContainerRuntimeError> {
        if owner_scope.is_empty() || unsafe_text(owner_scope) {
            return Err(ContainerRuntimeError::new(
                ContainerFailureCode::ContainerInvalidConfiguration,
                "invalid owner scope",
            ));
        }
        let output = self.invoke(
            &[
                "ps".into(),
                "--all".into(),
                "--filter".into(),
                "label=ai-ui.tool=true".into(),
                "--filter".into(),
                format!("label=ai-ui.owner_scope={owner_scope}").into(),
                "--format".into(),
                "{{.ID}}|{{.Names}}".into(),
            ],
            ContainerFailureCode::ContainerCleanupFailed,
        )?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let (id, name) = line.split_once('|')?;
                Some(RuntimeContainer {
                    id: id.to_string(),
                    name: name.to_string(),
                })
            })
            .collect())
    }
}

fn bounded_stderr(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "container runtime command failed without diagnostics".into()
    } else {
        trimmed.chars().take(2_048).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MockFailurePoint {
    Info,
    ImageInspect,
    Create,
    Start,
    Inspect,
    Logs,
    Stop,
    Remove,
    List,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockExecution {
    pub polls_before_exit: usize,
    pub exit_code: i32,
    pub report: Option<Value>,
    pub artifacts: BTreeMap<String, Vec<u8>>,
    pub failure: Option<MockFailurePoint>,
}

impl Default for MockExecution {
    fn default() -> Self {
        Self {
            polls_before_exit: 0,
            exit_code: 0,
            report: Some(json!({
                "version": 1,
                "status": "PASSED",
                "evidence_refs": []
            })),
            artifacts: BTreeMap::new(),
            failure: None,
        }
    }
}

#[derive(Debug)]
struct MockContainer {
    plan: ContainerCreatePlan,
    behavior: MockExecution,
    started: bool,
    stopped: bool,
    polls: usize,
}

#[derive(Debug)]
struct MockState {
    healthy: bool,
    next_id: u64,
    queued: VecDeque<MockExecution>,
    containers: BTreeMap<String, MockContainer>,
    observed_plans: Vec<ContainerCreatePlan>,
    removed_ids: Vec<String>,
}

#[derive(Debug)]
pub struct MockContainerRuntime {
    state: Mutex<MockState>,
}

impl Default for MockContainerRuntime {
    fn default() -> Self {
        Self {
            state: Mutex::new(MockState {
                healthy: true,
                next_id: 1,
                queued: VecDeque::new(),
                containers: BTreeMap::new(),
                observed_plans: Vec::new(),
                removed_ids: Vec::new(),
            }),
        }
    }
}

impl MockContainerRuntime {
    pub fn enqueue(&self, behavior: MockExecution) {
        self.state.lock().unwrap().queued.push_back(behavior);
    }

    pub fn set_healthy(&self, healthy: bool) {
        self.state.lock().unwrap().healthy = healthy;
    }

    pub fn observed_plans(&self) -> Vec<ContainerCreatePlan> {
        self.state.lock().unwrap().observed_plans.clone()
    }

    pub fn remaining(&self) -> usize {
        self.state.lock().unwrap().containers.len()
    }

    pub fn removed_ids(&self) -> Vec<String> {
        self.state.lock().unwrap().removed_ids.clone()
    }

    fn failure(point: MockFailurePoint) -> ContainerRuntimeError {
        let code = match point {
            MockFailurePoint::Info => ContainerFailureCode::ContainerRuntimeUnavailable,
            MockFailurePoint::ImageInspect => ContainerFailureCode::ContainerImageMissing,
            MockFailurePoint::Create => ContainerFailureCode::ContainerCreateFailed,
            MockFailurePoint::Start => ContainerFailureCode::ContainerStartFailed,
            MockFailurePoint::Inspect => ContainerFailureCode::ContainerExitedUnexpectedly,
            MockFailurePoint::Logs => ContainerFailureCode::ContainerArtifactCollectionFailed,
            MockFailurePoint::Stop | MockFailurePoint::Remove | MockFailurePoint::List => {
                ContainerFailureCode::ContainerCleanupFailed
            }
        };
        ContainerRuntimeError::new(code, format!("mock failure at {point:?}"))
    }
}

impl ContainerRuntime for MockContainerRuntime {
    fn health(&self) -> EnvironmentHealth {
        if self.state.lock().unwrap().healthy {
            EnvironmentHealth::healthy()
        } else {
            EnvironmentHealth::unhealthy(
                ContainerFailureCode::ContainerRuntimeUnavailable.as_str(),
                "mock runtime is unhealthy",
            )
        }
    }

    fn info(&self) -> Result<ContainerRuntimeInfo, ContainerRuntimeError> {
        let state = self.state.lock().unwrap();
        if !state.healthy
            || state
                .queued
                .front()
                .is_some_and(|behavior| behavior.failure == Some(MockFailurePoint::Info))
        {
            return Err(Self::failure(MockFailurePoint::Info));
        }
        Ok(ContainerRuntimeInfo {
            os: "mock-linux".into(),
            architecture: "mock-amd64".into(),
            version: "mock-runtime-v1".into(),
        })
    }

    fn image_digest(&self, _image: &str) -> Result<Option<String>, ContainerRuntimeError> {
        let state = self.state.lock().unwrap();
        if state
            .queued
            .front()
            .is_some_and(|behavior| behavior.failure == Some(MockFailurePoint::ImageInspect))
        {
            return Err(Self::failure(MockFailurePoint::ImageInspect));
        }
        Ok(Some("sha256:mock-container-image-v1".into()))
    }

    fn create(
        &self,
        plan: &ContainerCreatePlan,
    ) -> Result<RuntimeContainer, ContainerRuntimeError> {
        plan.validate().map_err(|errors| {
            ContainerRuntimeError::new(
                ContainerFailureCode::ContainerInvalidConfiguration,
                errors.join("; "),
            )
        })?;
        let mut state = self.state.lock().unwrap();
        let behavior = state.queued.pop_front().unwrap_or_default();
        if behavior.failure == Some(MockFailurePoint::Create) {
            return Err(Self::failure(MockFailurePoint::Create));
        }
        let id = format!("mock-container-{:08}", state.next_id);
        state.next_id += 1;
        state.observed_plans.push(plan.clone());
        state.containers.insert(
            id.clone(),
            MockContainer {
                plan: plan.clone(),
                behavior,
                started: false,
                stopped: false,
                polls: 0,
            },
        );
        Ok(RuntimeContainer {
            id,
            name: plan.name.clone(),
        })
    }

    fn start(&self, container_id: &str) -> Result<(), ContainerRuntimeError> {
        let mut state = self.state.lock().unwrap();
        let container = state.containers.get_mut(container_id).ok_or_else(|| {
            ContainerRuntimeError::new(
                ContainerFailureCode::ContainerStartFailed,
                "mock container not found",
            )
        })?;
        if container.behavior.failure == Some(MockFailurePoint::Start) {
            return Err(Self::failure(MockFailurePoint::Start));
        }
        container.started = true;
        let output = container.plan.output_mount().ok_or_else(|| {
            ContainerRuntimeError::new(
                ContainerFailureCode::ContainerMountFailed,
                "mock plan has no artifact output mount",
            )
        })?;
        fs::create_dir_all(&output.source).map_err(|error| {
            ContainerRuntimeError::new(
                ContainerFailureCode::ContainerMountFailed,
                format!("create mock output: {error}"),
            )
        })?;
        if let Some(report) = &container.behavior.report {
            let report_dir = output.source.join("public-run");
            fs::create_dir_all(&report_dir).map_err(|error| {
                ContainerRuntimeError::new(
                    ContainerFailureCode::ContainerArtifactCollectionFailed,
                    error.to_string(),
                )
            })?;
            fs::write(
                report_dir.join("report.json"),
                serde_json::to_vec_pretty(report).unwrap(),
            )
            .map_err(|error| {
                ContainerRuntimeError::new(
                    ContainerFailureCode::ContainerArtifactCollectionFailed,
                    error.to_string(),
                )
            })?;
        }
        for (relative, bytes) in &container.behavior.artifacts {
            let path = safe_mock_artifact_path(&output.source, relative)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    ContainerRuntimeError::new(
                        ContainerFailureCode::ContainerArtifactCollectionFailed,
                        error.to_string(),
                    )
                })?;
            }
            fs::write(path, bytes).map_err(|error| {
                ContainerRuntimeError::new(
                    ContainerFailureCode::ContainerArtifactCollectionFailed,
                    error.to_string(),
                )
            })?;
        }
        Ok(())
    }

    fn state(&self, container_id: &str) -> Result<RuntimeContainerState, ContainerRuntimeError> {
        let mut state = self.state.lock().unwrap();
        let container = state.containers.get_mut(container_id).ok_or_else(|| {
            ContainerRuntimeError::new(
                ContainerFailureCode::ContainerExitedUnexpectedly,
                "mock container not found",
            )
        })?;
        if container.behavior.failure == Some(MockFailurePoint::Inspect) {
            return Err(Self::failure(MockFailurePoint::Inspect));
        }
        if !container.started {
            return Ok(RuntimeContainerState {
                status: RuntimeContainerStatus::Created,
                exit_code: None,
            });
        }
        if container.stopped {
            return Ok(RuntimeContainerState {
                status: RuntimeContainerStatus::Exited,
                exit_code: Some(130),
            });
        }
        if container.polls < container.behavior.polls_before_exit {
            container.polls += 1;
            return Ok(RuntimeContainerState {
                status: RuntimeContainerStatus::Running,
                exit_code: None,
            });
        }
        Ok(RuntimeContainerState {
            status: RuntimeContainerStatus::Exited,
            exit_code: Some(container.behavior.exit_code),
        })
    }

    fn logs(&self, container_id: &str) -> Result<RuntimeLogs, ContainerRuntimeError> {
        let state = self.state.lock().unwrap();
        let container = state.containers.get(container_id).ok_or_else(|| {
            ContainerRuntimeError::new(
                ContainerFailureCode::ContainerArtifactCollectionFailed,
                "mock container not found",
            )
        })?;
        if container.behavior.failure == Some(MockFailurePoint::Logs) {
            return Err(Self::failure(MockFailurePoint::Logs));
        }
        Ok(RuntimeLogs {
            stdout: b"mock public runner stdout\n".to_vec(),
            stderr: Vec::new(),
        })
    }

    fn stop(&self, container_id: &str, _grace_ms: u64) -> Result<(), ContainerRuntimeError> {
        let mut state = self.state.lock().unwrap();
        let container = state.containers.get_mut(container_id).ok_or_else(|| {
            ContainerRuntimeError::new(
                ContainerFailureCode::ContainerCleanupFailed,
                "mock container not found",
            )
        })?;
        if container.behavior.failure == Some(MockFailurePoint::Stop) {
            return Err(Self::failure(MockFailurePoint::Stop));
        }
        container.stopped = true;
        Ok(())
    }

    fn remove(&self, container_id: &str, _force: bool) -> Result<(), ContainerRuntimeError> {
        let mut state = self.state.lock().unwrap();
        if state
            .containers
            .get(container_id)
            .is_some_and(|container| container.behavior.failure == Some(MockFailurePoint::Remove))
        {
            return Err(Self::failure(MockFailurePoint::Remove));
        }
        if state.containers.remove(container_id).is_some() {
            state.removed_ids.push(container_id.to_string());
        }
        Ok(())
    }

    fn list_tool_owned(&self) -> Result<Vec<RuntimeContainer>, ContainerRuntimeError> {
        let state = self.state.lock().unwrap();
        if state
            .containers
            .values()
            .any(|container| container.behavior.failure == Some(MockFailurePoint::List))
        {
            return Err(Self::failure(MockFailurePoint::List));
        }
        Ok(state
            .containers
            .iter()
            .filter(|(_, container)| {
                container.plan.labels.get("ai-ui.tool").map(String::as_str) == Some("true")
            })
            .map(|(id, container)| RuntimeContainer {
                id: id.clone(),
                name: container.plan.name.clone(),
            })
            .collect())
    }

    fn list_owned(
        &self,
        owner_scope: &str,
    ) -> Result<Vec<RuntimeContainer>, ContainerRuntimeError> {
        let state = self.state.lock().unwrap();
        if state.containers.values().any(|container| {
            container.behavior.failure == Some(MockFailurePoint::List)
                && container
                    .plan
                    .labels
                    .get("ai-ui.owner_scope")
                    .map(String::as_str)
                    == Some(owner_scope)
        }) {
            return Err(Self::failure(MockFailurePoint::List));
        }
        Ok(state
            .containers
            .iter()
            .filter(|(_, container)| {
                container.plan.labels.get("ai-ui.tool").map(String::as_str) == Some("true")
                    && container
                        .plan
                        .labels
                        .get("ai-ui.owner_scope")
                        .map(String::as_str)
                        == Some(owner_scope)
            })
            .map(|(id, container)| RuntimeContainer {
                id: id.clone(),
                name: container.plan.name.clone(),
            })
            .collect())
    }
}

fn safe_mock_artifact_path(
    root: &std::path::Path,
    relative: &str,
) -> Result<PathBuf, ContainerRuntimeError> {
    let path = std::path::Path::new(relative);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(ContainerRuntimeError::new(
            ContainerFailureCode::ContainerArtifactCollectionFailed,
            "mock artifact path escapes output root",
        ));
    }
    Ok(root.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn plan(root: PathBuf) -> ContainerCreatePlan {
        ContainerCreatePlan {
            version: CONTAINER_RUNTIME_CONTRACT_VERSION,
            name: "ai-ui-mock-1".into(),
            image: "ai-ui/runner:v1".into(),
            image_digest: Some("sha256:mock".into()),
            labels: BTreeMap::from([
                ("ai-ui.tool".into(), "true".into()),
                ("ai-ui.owner_scope".into(), "scope-1".into()),
            ]),
            mounts: vec![RuntimeMount {
                source: root,
                target: "/output".into(),
                access: MountAccess::ArtifactOutput,
            }],
            environment: BTreeMap::from([("TMP".into(), "/tmp".into())]),
            network: EnvironmentNetworkPolicy::None,
            limits: EnvironmentResourceLimits {
                memory_mib: Some(64),
                cpu_millis: Some(500),
                pids: Some(32),
            },
            working_dir: "/work".into(),
            runner_executable: "/opt/ai-ui/ai-ui-test".into(),
            runner_arguments: vec!["run".into(), "/work/test.json".into()],
            read_only_root: true,
            tmpfs: vec!["/tmp:rw,noexec,nosuid,size=16m".into()],
        }
    }

    #[test]
    fn create_plan_is_declarative_validated_and_has_no_shell_field() {
        let root = env::temp_dir().join("ai-ui-runtime-plan");
        let plan = plan(root);
        plan.validate().unwrap();
        let value = serde_json::to_value(plan).unwrap();
        assert!(value.get("shell").is_none());
        assert!(value.get("eval").is_none());
        assert!(value.get("command").is_none());
        assert_eq!(value["runner_arguments"][0], "run");
    }

    #[test]
    fn mock_runtime_tracks_only_owned_scope_and_removes_exact_container() {
        let runtime = MockContainerRuntime::default();
        let root = env::temp_dir().join("ai-ui-runtime-owned");
        let container = runtime.create(&plan(root)).unwrap();
        assert_eq!(runtime.list_tool_owned().unwrap().len(), 1);
        assert_eq!(runtime.list_owned("scope-1").unwrap().len(), 1);
        assert!(runtime.list_owned("another-scope").unwrap().is_empty());
        runtime.remove(&container.id, true).unwrap();
        assert_eq!(runtime.remaining(), 0);
        assert_eq!(runtime.removed_ids(), vec![container.id]);
    }
}
