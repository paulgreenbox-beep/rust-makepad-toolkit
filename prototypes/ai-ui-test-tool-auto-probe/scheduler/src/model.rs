use crate::environment::{
    ExecutionEnvironmentMetadata, ExecutionEnvironmentRequirement, WorkerEnvironmentCapabilities,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

pub const TEST_JOB_VERSION: u32 = 1;
pub const SCHEDULER_SNAPSHOT_VERSION: u32 = 1;
pub const WINDOWS_INTERACTIVE_DESKTOP: &str = "windows.interactive_desktop";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationSpec {
    /// Stable policy key. Different paths may intentionally share one key.
    pub key: String,
    /// Trusted scheduler configuration, never interpreted as a shell command.
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_application_parallelism")]
    pub max_parallel: usize,
}

fn default_application_parallelism() -> usize {
    1
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobSource {
    Script { path: PathBuf },
}

impl JobSource {
    pub fn script_path(&self) -> &PathBuf {
        match self {
            Self::Script { path } => path,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactSpec {
    pub output_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionRequirements {
    #[serde(default = "default_platform")]
    pub platform: String,
    #[serde(default)]
    pub interactive_desktop: bool,
    #[serde(default = "default_true")]
    pub gpu: bool,
    #[serde(default = "default_true")]
    pub semantic_ui: bool,
    #[serde(default)]
    pub physical_input: bool,
    #[serde(default = "default_true")]
    pub makepad: bool,
    #[serde(default)]
    pub semantic_only: bool,
    #[serde(default)]
    pub execution_environment: ExecutionEnvironmentRequirement,
}

impl Default for ExecutionRequirements {
    fn default() -> Self {
        Self {
            platform: default_platform(),
            interactive_desktop: false,
            gpu: true,
            semantic_ui: true,
            physical_input: false,
            makepad: true,
            semantic_only: false,
            execution_environment: ExecutionEnvironmentRequirement::default(),
        }
    }
}

fn default_platform() -> String {
    "windows".into()
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default)]
    pub retry_test_failures: bool,
    #[serde(default = "default_true")]
    pub retry_infrastructure_failures: bool,
    #[serde(default)]
    pub retry_timeouts: bool,
    #[serde(default = "default_true")]
    pub retry_worker_lost: bool,
    /// Must be explicitly enabled for destructive or unknown-idempotency scripts.
    #[serde(default)]
    pub retry_non_idempotent: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            retry_test_failures: false,
            retry_infrastructure_failures: true,
            retry_timeouts: false,
            retry_worker_lost: true,
            retry_non_idempotent: false,
        }
    }
}

fn default_max_attempts() -> u32 {
    2
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestJob {
    pub version: u32,
    pub job_id: String,
    pub run_id: String,
    pub application: ApplicationSpec,
    pub source: JobSource,
    #[serde(default)]
    pub fixtures: Option<PathBuf>,
    #[serde(default)]
    pub priority: i32,
    pub timeout_ms: u64,
    #[serde(default)]
    pub retry: RetryPolicy,
    #[serde(default)]
    pub requirements: ExecutionRequirements,
    pub artifacts: ArtifactSpec,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    pub created_at_ms: u64,
    /// Derived from the versioned operation catalog at submission time.
    #[serde(default)]
    pub retry_safe: bool,
}

impl TestJob {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.version != TEST_JOB_VERSION {
            errors.push(format!("job version must be {TEST_JOB_VERSION}"));
        }
        if self.job_id.trim().is_empty() {
            errors.push("job_id must not be empty".into());
        }
        if self.run_id.trim().is_empty() {
            errors.push("run_id must not be empty".into());
        }
        if self.application.key.trim().is_empty() {
            errors.push("application.key must not be empty".into());
        }
        if self.application.executable.as_os_str().is_empty() {
            errors.push("application.executable must not be empty".into());
        }
        if self.application.max_parallel == 0 {
            errors.push("application.max_parallel must be greater than zero".into());
        }
        if self.source.script_path().as_os_str().is_empty() {
            errors.push("source script path must not be empty".into());
        }
        if self.timeout_ms == 0 {
            errors.push("timeout_ms must be greater than zero".into());
        }
        if self.retry.max_attempts == 0 {
            errors.push("retry.max_attempts must be greater than zero".into());
        }
        if self.artifacts.output_dir.as_os_str().is_empty() {
            errors.push("artifacts.output_dir must not be empty".into());
        }
        if self.requirements.semantic_only
            && (self.requirements.physical_input || self.requirements.interactive_desktop)
        {
            errors
                .push("semantic_only cannot require physical_input or interactive_desktop".into());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobState {
    Queued,
    Ready,
    Leased,
    Starting,
    Running,
    Passed,
    Failed,
    TimedOut,
    Cancelled,
    WorkerLost,
    InfrastructureFailed,
    RetryPending,
    Blocked,
    Skipped,
}

impl JobState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Passed
                | Self::Failed
                | Self::TimedOut
                | Self::Cancelled
                | Self::WorkerLost
                | Self::InfrastructureFailed
                | Self::Skipped
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailureClassification {
    TestFailure,
    InfrastructureFailure,
    Timeout,
    Cancelled,
    WorkerLost,
}

impl FailureClassification {
    pub fn state(self) -> JobState {
        match self {
            Self::TestFailure => JobState::Failed,
            Self::InfrastructureFailure => JobState::InfrastructureFailed,
            Self::Timeout => JobState::TimedOut,
            Self::Cancelled => JobState::Cancelled,
            Self::WorkerLost => JobState::WorkerLost,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDiagnostics {
    #[serde(default)]
    pub runner_pid: Option<u32>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub stdout_path: Option<PathBuf>,
    #[serde(default)]
    pub stderr_path: Option<PathBuf>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub attempt_id: String,
    pub job_id: String,
    pub number: u32,
    pub worker_id: String,
    pub worker_instance_id: String,
    pub lease_id: String,
    pub status: JobState,
    pub leased_at_ms: u64,
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    #[serde(default)]
    pub finished_at_ms: Option<u64>,
    #[serde(default)]
    pub classification: Option<FailureClassification>,
    #[serde(default)]
    pub script_report: Option<PathBuf>,
    #[serde(default)]
    pub evidence_refs: Vec<PathBuf>,
    #[serde(default)]
    pub runtime: Option<RuntimeDiagnostics>,
    #[serde(default)]
    pub execution_environment: Option<ExecutionEnvironmentMetadata>,
    #[serde(default)]
    pub environment_report: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobRecord {
    pub job: TestJob,
    pub state: JobState,
    pub attempt_ids: Vec<String>,
    #[serde(default)]
    pub active_lease_id: Option<String>,
    #[serde(default)]
    pub final_classification: Option<FailureClassification>,
    pub updated_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunStatus {
    Queued,
    Running,
    Passed,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Passed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub status: RunStatus,
    pub job_ids: Vec<String>,
    pub created_at_ms: u64,
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    #[serde(default)]
    pub finished_at_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkerState {
    Idle,
    Busy,
    Draining,
    Offline,
    Lost,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerCapabilities {
    pub platform: String,
    pub interactive_desktop: bool,
    pub gpu: bool,
    pub max_parallel: usize,
    pub semantic_ui: bool,
    pub physical_input: bool,
    pub makepad: bool,
    #[serde(default)]
    pub execution_environment: WorkerEnvironmentCapabilities,
}

impl Default for WorkerCapabilities {
    fn default() -> Self {
        Self {
            platform: default_platform(),
            interactive_desktop: true,
            gpu: true,
            max_parallel: 1,
            semantic_ui: true,
            physical_input: true,
            makepad: true,
            execution_environment: WorkerEnvironmentCapabilities::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRecord {
    pub worker_id: String,
    pub worker_instance_id: String,
    pub registered_at_ms: u64,
    pub last_heartbeat_ms: u64,
    pub capabilities: WorkerCapabilities,
    pub capacity: usize,
    pub current_jobs: Vec<String>,
    pub state: WorkerState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub lease_id: String,
    pub job_id: String,
    pub worker_id: String,
    pub worker_instance_id: String,
    pub attempt_id: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub renewed_at_ms: u64,
    #[serde(default)]
    pub resources: Vec<String>,
    pub application_key: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SchedulerEvent {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub run_id: String,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub attempt_id: Option<String>,
    #[serde(default)]
    pub worker_id: Option<String>,
    pub event: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> TestJob {
        TestJob {
            version: TEST_JOB_VERSION,
            job_id: "job:1".into(),
            run_id: "run:1".into(),
            application: ApplicationSpec {
                key: "advanced-demo".into(),
                executable: "advanced-demo.exe".into(),
                args: Vec::new(),
                max_parallel: 1,
            },
            source: JobSource::Script {
                path: "smoke.aiui.json".into(),
            },
            fixtures: None,
            priority: 50,
            timeout_ms: 120_000,
            retry: RetryPolicy::default(),
            requirements: ExecutionRequirements::default(),
            artifacts: ArtifactSpec {
                output_dir: "reports/run-1/job-1".into(),
            },
            metadata: BTreeMap::new(),
            created_at_ms: 1,
            retry_safe: true,
        }
    }

    #[test]
    fn test_job_contract_is_versioned_declarative_and_round_trips() {
        let job = job();
        job.validate().unwrap();
        let value = serde_json::to_value(&job).unwrap();
        assert_eq!(value["version"], TEST_JOB_VERSION);
        assert!(value.get("shell").is_none());
        assert!(value.get("eval").is_none());
        assert!(value.get("command").is_none());
        assert_eq!(serde_json::from_value::<TestJob>(value).unwrap(), job);
    }

    #[test]
    fn semantic_only_rejects_desktop_global_input() {
        let mut job = job();
        job.requirements.semantic_only = true;
        job.requirements.physical_input = true;
        assert!(job.validate().is_err());
    }
}
