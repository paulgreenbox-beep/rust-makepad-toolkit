use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub const EXECUTION_ENVIRONMENT_CONTRACT_VERSION: u32 = 1;
pub const ENVIRONMENT_REPORT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionEnvironmentKind {
    #[default]
    LocalProcess,
    Container,
    Remote,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentIsolation {
    #[default]
    None,
    Process,
    Filesystem,
    HyperV,
    InteractiveDesktop,
}

impl EnvironmentIsolation {
    pub fn satisfies(self, required: Self) -> bool {
        self >= required
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuiCapabilityLevel {
    #[default]
    Level0,
    Level1,
    Level2,
    Level3,
    Level4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EnvironmentState {
    Created,
    Preparing,
    Ready,
    Running,
    Stopping,
    Stopped,
    Failed,
    Destroyed,
}

pub fn environment_transition_allowed(from: EnvironmentState, to: EnvironmentState) -> bool {
    use EnvironmentState::*;
    matches!(
        (from, to),
        (Created, Preparing | Failed | Destroyed)
            | (Preparing, Ready | Failed | Stopping)
            | (Ready, Running | Stopping | Failed)
            | (Running, Stopping | Stopped | Failed)
            | (Stopping, Stopped | Failed)
            | (Stopped, Failed | Destroyed)
            | (Failed, Destroyed)
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionEnvironmentRequirement {
    #[serde(default = "default_local_environment_key")]
    pub environment_key: String,
    #[serde(default)]
    pub kind: ExecutionEnvironmentKind,
    #[serde(default)]
    pub minimum_isolation: EnvironmentIsolation,
}

impl Default for ExecutionEnvironmentRequirement {
    fn default() -> Self {
        Self {
            environment_key: default_local_environment_key(),
            kind: ExecutionEnvironmentKind::LocalProcess,
            minimum_isolation: EnvironmentIsolation::None,
        }
    }
}

fn default_local_environment_key() -> String {
    "local".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerEnvironmentCapabilities {
    #[serde(default = "default_local_environment_key")]
    pub environment_key: String,
    #[serde(default)]
    pub kind: ExecutionEnvironmentKind,
    #[serde(default)]
    pub isolation: EnvironmentIsolation,
    #[serde(default)]
    pub gui_level: GuiCapabilityLevel,
    #[serde(default)]
    pub reusable: bool,
}

impl Default for WorkerEnvironmentCapabilities {
    fn default() -> Self {
        Self {
            environment_key: default_local_environment_key(),
            kind: ExecutionEnvironmentKind::LocalProcess,
            isolation: EnvironmentIsolation::None,
            gui_level: GuiCapabilityLevel::Level0,
            reusable: false,
        }
    }
}

impl WorkerEnvironmentCapabilities {
    pub fn matches(&self, requirement: &ExecutionEnvironmentRequirement) -> bool {
        self.environment_key == requirement.environment_key
            && self.kind == requirement.kind
            && self.isolation.satisfies(requirement.minimum_isolation)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MountAccess {
    ReadOnly,
    ReadWrite,
    ArtifactOutput,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedMount {
    pub source: PathBuf,
    pub target: String,
    pub access: MountAccess,
}

impl TrustedMount {
    pub fn validate(&self) -> Result<(), String> {
        if !self.source.is_absolute() {
            return Err("trusted mount source must be absolute".into());
        }
        if self.source.parent().is_none() {
            return Err("trusted mount source must not be a filesystem root".into());
        }
        if !sandbox_path_is_absolute(&self.target) {
            return Err("trusted mount target must be an absolute sandbox path".into());
        }
        if self
            .target
            .split(['/', '\\'])
            .any(|component| component == "..")
        {
            return Err("trusted mount target must not contain parent traversal".into());
        }
        if self.target == "/" || self.target.ends_with(":\\") || self.target.ends_with(":/") {
            return Err("trusted mount target must not be a filesystem root".into());
        }
        if self.access == MountAccess::ArtifactOutput
            && !normalized_sandbox_path(&self.target).starts_with("/output")
        {
            return Err("artifact output mounts must target /output".into());
        }
        Ok(())
    }
}

fn sandbox_path_is_absolute(path: &str) -> bool {
    path.starts_with('/')
        || (path.len() >= 3
            && path.as_bytes()[1] == b':'
            && matches!(path.as_bytes()[2], b'/' | b'\\'))
}

fn normalized_sandbox_path(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentNetworkPolicy {
    #[default]
    None,
    Default,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentResourceLimits {
    #[serde(default)]
    pub memory_mib: Option<u64>,
    #[serde(default)]
    pub cpu_millis: Option<u32>,
    #[serde(default)]
    pub pids: Option<u32>,
}

impl EnvironmentResourceLimits {
    pub fn validate(&self) -> Result<(), String> {
        if self.memory_mib == Some(0) {
            return Err("environment memory limit must be greater than zero".into());
        }
        if self.cpu_millis == Some(0) {
            return Err("environment CPU limit must be greater than zero".into());
        }
        if self.pids == Some(0) {
            return Err("environment PID limit must be greater than zero".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentCapabilities {
    pub process_isolation: bool,
    pub filesystem_isolation: bool,
    pub environment_isolation: bool,
    pub gpu: bool,
    pub interactive_gui: bool,
    pub semantic_ui: bool,
    pub uia: bool,
    pub physical_input: bool,
    pub makepad: bool,
    pub isolated_desktop: bool,
    #[serde(default)]
    pub gui_level: GuiCapabilityLevel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionEnvironmentSpec {
    pub version: u32,
    pub key: String,
    pub kind: ExecutionEnvironmentKind,
    #[serde(default = "default_environment_platform")]
    pub platform: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub image_digest: Option<String>,
    #[serde(default)]
    pub runner_path: Option<String>,
    #[serde(default)]
    pub isolation: EnvironmentIsolation,
    #[serde(default = "default_true")]
    pub disposable: bool,
    #[serde(default)]
    pub reusable: bool,
    #[serde(default)]
    pub mounts: Vec<TrustedMount>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub network: EnvironmentNetworkPolicy,
    #[serde(default)]
    pub limits: EnvironmentResourceLimits,
    #[serde(default)]
    pub capabilities: EnvironmentCapabilities,
}

fn default_true() -> bool {
    true
}

fn default_environment_platform() -> String {
    "windows".into()
}

impl ExecutionEnvironmentSpec {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.version != EXECUTION_ENVIRONMENT_CONTRACT_VERSION {
            errors.push(format!(
                "environment spec version must be {EXECUTION_ENVIRONMENT_CONTRACT_VERSION}"
            ));
        }
        if !trusted_alias(&self.key) {
            errors
                .push("environment key must contain only letters, digits, '.', '_' or '-'".into());
        }
        if !trusted_alias(&self.platform) {
            errors.push("environment platform must be a trusted non-empty alias".into());
        }
        if self.disposable == self.reusable {
            errors.push("exactly one of disposable or reusable must be enabled".into());
        }
        if self.kind == ExecutionEnvironmentKind::Container {
            if self.image.as_deref().is_none_or(str::is_empty) {
                errors.push("container environment requires a trusted image".into());
            }
            if self.runner_path.as_deref().is_none_or(str::is_empty) {
                errors.push("container environment requires a trusted runner_path".into());
            }
        }
        for value in self.image.iter().chain(self.runner_path.iter()) {
            if value.chars().any(char::is_control) {
                errors
                    .push("trusted image/runner values must not contain control characters".into());
            }
        }
        let mut targets = BTreeSet::new();
        for mount in &self.mounts {
            if let Err(error) = mount.validate() {
                errors.push(error);
            }
            if !targets.insert(normalized_sandbox_path(&mount.target)) {
                errors.push(format!("duplicate trusted mount target: {}", mount.target));
            }
        }
        for (key, value) in &self.environment {
            if !trusted_environment_key(key) {
                errors.push(format!("invalid trusted environment variable name: {key}"));
            }
            if value.contains('\0') || value.contains('\r') || value.contains('\n') {
                errors.push(format!("invalid trusted environment variable value: {key}"));
            }
        }
        if let Err(error) = self.limits.validate() {
            errors.push(error);
        }
        if self.capabilities.isolated_desktop
            && self.capabilities.gui_level < GuiCapabilityLevel::Level4
        {
            errors.push("isolated_desktop requires a proven GUI capability level 4".into());
        }
        if self.capabilities.interactive_gui
            && self.capabilities.gui_level < GuiCapabilityLevel::Level1
        {
            errors.push("interactive_gui requires a proven GUI capability level 1".into());
        }
        if (self.capabilities.semantic_ui || self.capabilities.uia)
            && self.capabilities.gui_level < GuiCapabilityLevel::Level2
        {
            errors.push("semantic UI/UIA requires a proven GUI capability level 2".into());
        }
        if self.capabilities.physical_input
            && self.capabilities.gui_level < GuiCapabilityLevel::Level3
        {
            errors.push("physical_input requires a proven GUI capability level 3".into());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

fn trusted_alias(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn trusted_environment_key(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedExecutionEnvironmentCatalog {
    pub version: u32,
    pub environments: Vec<ExecutionEnvironmentSpec>,
}

impl TrustedExecutionEnvironmentCatalog {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let bytes = fs::read(path)
            .map_err(|error| format!("read environment catalog {}: {error}", path.display()))?;
        let catalog: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse environment catalog {}: {error}", path.display()))?;
        catalog.validate().map_err(|errors| errors.join("; "))?;
        Ok(catalog)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.version != EXECUTION_ENVIRONMENT_CONTRACT_VERSION {
            errors.push(format!(
                "environment catalog version must be {EXECUTION_ENVIRONMENT_CONTRACT_VERSION}"
            ));
        }
        let mut keys = BTreeSet::new();
        for environment in &self.environments {
            if let Err(environment_errors) = environment.validate() {
                errors.extend(environment_errors);
            }
            if !keys.insert(environment.key.clone()) {
                errors.push(format!("duplicate environment key: {}", environment.key));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn resolve(&self, key: &str) -> Result<&ExecutionEnvironmentSpec, String> {
        self.validate().map_err(|errors| errors.join("; "))?;
        self.environments
            .iter()
            .find(|environment| environment.key == key)
            .ok_or_else(|| format!("trusted execution environment not found: {key}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentHealth {
    pub healthy: bool,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

impl EnvironmentHealth {
    pub fn healthy() -> Self {
        Self {
            healthy: true,
            error_code: None,
            message: None,
        }
    }

    pub fn unhealthy(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            healthy: false,
            error_code: Some(code.into()),
            message: Some(message.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipLabels {
    pub owner_scope: String,
    pub run_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub worker_id: String,
}

impl OwnershipLabels {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("owner_scope", &self.owner_scope),
            ("run_id", &self.run_id),
            ("job_id", &self.job_id),
            ("attempt_id", &self.attempt_id),
            ("worker_id", &self.worker_id),
        ] {
            if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
                return Err(format!("invalid ownership label value: {name}"));
            }
        }
        Ok(())
    }

    pub fn as_map(&self) -> Result<BTreeMap<String, String>, String> {
        self.validate()?;
        Ok(BTreeMap::from([
            ("ai-ui.tool".into(), "true".into()),
            ("ai-ui.owner_scope".into(), self.owner_scope.clone()),
            ("ai-ui.run_id".into(), self.run_id.clone()),
            ("ai-ui.job_id".into(), self.job_id.clone()),
            ("ai-ui.attempt_id".into(), self.attempt_id.clone()),
            ("ai-ui.worker_id".into(), self.worker_id.clone()),
        ]))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentIdentity {
    pub environment_id: String,
    #[serde(default)]
    pub container_id: Option<String>,
    pub worker_id: String,
    pub attempt_id: String,
    pub created_at_ms: u64,
    pub state: EnvironmentState,
    pub capabilities: EnvironmentCapabilities,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentTimings {
    pub environment_prepare_ms: u64,
    pub startup_ms: u64,
    pub execution_ms: u64,
    pub artifact_collection_ms: u64,
    pub cleanup_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentFingerprint {
    pub os: String,
    pub environment_type: ExecutionEnvironmentKind,
    #[serde(default)]
    pub image_digest: Option<String>,
    pub tool_version: String,
    pub runner_version: String,
    #[serde(default)]
    pub application_version: Option<String>,
    pub gpu_available: bool,
    #[serde(default)]
    pub gui_level: GuiCapabilityLevel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleEvent {
    pub state: EnvironmentState,
    pub timestamp_ms: u64,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionEnvironmentMetadata {
    pub version: u32,
    pub environment_type: ExecutionEnvironmentKind,
    pub environment_key: String,
    pub environment_id: String,
    #[serde(default)]
    pub container_id: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub image_digest: Option<String>,
    pub isolation_mode: EnvironmentIsolation,
    pub capabilities: EnvironmentCapabilities,
    pub fingerprint: EnvironmentFingerprint,
    pub timings: EnvironmentTimings,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentReport {
    pub version: u32,
    pub identity: EnvironmentIdentity,
    pub metadata: ExecutionEnvironmentMetadata,
    pub lifecycle: Vec<LifecycleEvent>,
    pub cleanup_succeeded: bool,
    pub owned_resources_remaining: usize,
    #[serde(default)]
    pub failure_code: Option<String>,
    #[serde(default)]
    pub artifact_refs: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContainerFailureCode {
    ContainerCreateFailed,
    ContainerStartFailed,
    ContainerRuntimeUnavailable,
    ContainerImageMissing,
    ContainerMountFailed,
    ContainerExitedUnexpectedly,
    ContainerArtifactCollectionFailed,
    ContainerCleanupFailed,
    ContainerHealthFailed,
    ContainerInvalidConfiguration,
}

impl ContainerFailureCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ContainerCreateFailed => "CONTAINER_CREATE_FAILED",
            Self::ContainerStartFailed => "CONTAINER_START_FAILED",
            Self::ContainerRuntimeUnavailable => "CONTAINER_RUNTIME_UNAVAILABLE",
            Self::ContainerImageMissing => "CONTAINER_IMAGE_MISSING",
            Self::ContainerMountFailed => "CONTAINER_MOUNT_FAILED",
            Self::ContainerExitedUnexpectedly => "CONTAINER_EXITED_UNEXPECTEDLY",
            Self::ContainerArtifactCollectionFailed => "CONTAINER_ARTIFACT_COLLECTION_FAILED",
            Self::ContainerCleanupFailed => "CONTAINER_CLEANUP_FAILED",
            Self::ContainerHealthFailed => "CONTAINER_HEALTH_FAILED",
            Self::ContainerInvalidConfiguration => "CONTAINER_INVALID_CONFIGURATION",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container_spec() -> ExecutionEnvironmentSpec {
        ExecutionEnvironmentSpec {
            version: EXECUTION_ENVIRONMENT_CONTRACT_VERSION,
            key: "container-default".into(),
            kind: ExecutionEnvironmentKind::Container,
            platform: "linux".into(),
            image: Some("ai-ui/makepad-runner:v1".into()),
            image_digest: Some("sha256:abc".into()),
            runner_path: Some("/opt/ai-ui/ai-ui-test".into()),
            isolation: EnvironmentIsolation::Filesystem,
            disposable: true,
            reusable: false,
            mounts: vec![TrustedMount {
                source: std::env::temp_dir().join("ai-ui-output"),
                target: "/output".into(),
                access: MountAccess::ArtifactOutput,
            }],
            environment: BTreeMap::from([("AI_UI_MODE".into(), "test".into())]),
            network: EnvironmentNetworkPolicy::None,
            limits: EnvironmentResourceLimits {
                memory_mib: Some(512),
                cpu_millis: Some(1_000),
                pids: Some(64),
            },
            capabilities: EnvironmentCapabilities {
                process_isolation: true,
                filesystem_isolation: true,
                environment_isolation: true,
                ..EnvironmentCapabilities::default()
            },
        }
    }

    #[test]
    fn lifecycle_is_explicit_and_terminal_states_cannot_restart() {
        let path = [
            EnvironmentState::Created,
            EnvironmentState::Preparing,
            EnvironmentState::Ready,
            EnvironmentState::Running,
            EnvironmentState::Stopping,
            EnvironmentState::Stopped,
            EnvironmentState::Destroyed,
        ];
        for states in path.windows(2) {
            assert!(environment_transition_allowed(states[0], states[1]));
        }
        assert!(!environment_transition_allowed(
            EnvironmentState::Destroyed,
            EnvironmentState::Running
        ));
        assert!(!environment_transition_allowed(
            EnvironmentState::Stopped,
            EnvironmentState::Running
        ));
    }

    #[test]
    fn trusted_spec_round_trips_without_arbitrary_command_fields() {
        let spec = container_spec();
        spec.validate().unwrap();
        let value = serde_json::to_value(&spec).unwrap();
        assert!(value.get("command").is_none());
        assert!(value.get("shell").is_none());
        assert!(value.get("eval").is_none());
        let decoded: ExecutionEnvironmentSpec = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, spec);
    }

    #[test]
    fn mount_policy_rejects_roots_traversal_and_duplicate_targets() {
        let mut spec = container_spec();
        spec.mounts[0].target = "/".into();
        assert!(spec.validate().is_err());

        let mut spec = container_spec();
        spec.mounts[0].target = "/output/../host".into();
        assert!(spec.validate().is_err());

        let mut spec = container_spec();
        spec.mounts.push(spec.mounts[0].clone());
        assert!(spec.validate().is_err());
    }

    #[test]
    fn ownership_labels_are_complete_and_control_character_safe() {
        let labels = OwnershipLabels {
            owner_scope: "worker-scope:1".into(),
            run_id: "run:1".into(),
            job_id: "job:1".into(),
            attempt_id: "attempt:1".into(),
            worker_id: "worker:1".into(),
        };
        let map = labels.as_map().unwrap();
        assert_eq!(map["ai-ui.tool"], "true");
        assert_eq!(map["ai-ui.owner_scope"], "worker-scope:1");
        assert_eq!(map["ai-ui.attempt_id"], "attempt:1");

        let mut invalid = labels;
        invalid.job_id = "secret\nleak".into();
        assert!(invalid.as_map().is_err());
    }

    #[test]
    fn catalog_and_capabilities_match_only_the_trusted_environment() {
        let spec = container_spec();
        let catalog = TrustedExecutionEnvironmentCatalog {
            version: EXECUTION_ENVIRONMENT_CONTRACT_VERSION,
            environments: vec![spec],
        };
        assert_eq!(
            catalog.resolve("container-default").unwrap().key,
            "container-default"
        );
        assert!(catalog.resolve("untrusted").is_err());

        let capabilities = WorkerEnvironmentCapabilities {
            environment_key: "container-default".into(),
            kind: ExecutionEnvironmentKind::Container,
            isolation: EnvironmentIsolation::Filesystem,
            gui_level: GuiCapabilityLevel::Level0,
            reusable: false,
        };
        assert!(capabilities.matches(&ExecutionEnvironmentRequirement {
            environment_key: "container-default".into(),
            kind: ExecutionEnvironmentKind::Container,
            minimum_isolation: EnvironmentIsolation::Process,
        }));
        assert!(!capabilities.matches(&ExecutionEnvironmentRequirement {
            environment_key: "container-default".into(),
            kind: ExecutionEnvironmentKind::Container,
            minimum_isolation: EnvironmentIsolation::InteractiveDesktop,
        }));
    }
}
