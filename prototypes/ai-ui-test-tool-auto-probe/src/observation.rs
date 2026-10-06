//! Safe, bounded observation primitives shared by the public Script API and
//! Windows backend adapters.
//!
//! The module intentionally observes only trusted telemetry, trusted file
//! roots, bounded logs, and captured PNGs. It never dispatches business
//! actions, evaluates code, traverses arbitrary paths, or performs arbitrary
//! HTTP requests.

use crate::script_controls::parse_json_path;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::BufReader,
    path::{Component, Path, PathBuf},
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};

pub const OBSERVATION_VERSION: u32 = 1;
pub const MAX_OBSERVATION_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_POLL_MS: u64 = 50;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationType {
    Ui,
    RuntimeTask,
    RuntimeMilestone,
    ExternalRequest,
    File,
    Visual,
    Trace,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AsyncOperationKind {
    Cpu,
    Http,
    Ocr,
    Ai,
    Database,
    Index,
    Export,
    Custom,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum AsyncOperationState {
    NotStarted,
    Queued,
    Running,
    WaitingExternal,
    ProcessingResponse,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    #[default]
    Unknown,
}

impl AsyncOperationState {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ExternalRequestPhase {
    NotStarted,
    Resolving,
    Connecting,
    Sending,
    WaitingResponse,
    Receiving,
    ProcessingResponse,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AsyncOperation {
    pub operation_id: String,
    #[serde(default)]
    pub kind: AsyncOperationKind,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub state: AsyncOperationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_unix_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_unix_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_unix_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u128>,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ObservationFailure>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ObservationFailure {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RuntimeMilestone {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<AsyncOperationState>,
    #[serde(default)]
    pub timestamp_unix_ms: u128,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExternalRequestState {
    pub request_id: String,
    #[serde(default)]
    pub observer_alias: String,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub endpoint_alias: String,
    /// Kept as a compatibility alias for pre-observation telemetry providers.
    #[serde(default)]
    pub target_alias: String,
    /// Normalized lifecycle used by generic task/operation assertions.
    #[serde(default)]
    pub state: AsyncOperationState,
    /// Transport-specific phase. HTTP 200 alone is intentionally not success.
    #[serde(default)]
    pub phase: ExternalRequestPhase,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u128>,
    #[serde(default)]
    pub transport_completed: bool,
    #[serde(default)]
    pub response_received: bool,
    #[serde(default)]
    pub application_accepted_response: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_received: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ObservationFailure>,
    #[serde(default)]
    pub updated_at_unix_ms: u128,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TraceEvent {
    #[serde(default)]
    pub timestamp_unix_ms: u128,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub fields: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    #[serde(default = "default_observation_version")]
    pub version: u32,
    #[serde(default)]
    pub tasks: BTreeMap<String, AsyncOperation>,
    #[serde(default)]
    pub milestones: Vec<RuntimeMilestone>,
    #[serde(default)]
    pub external_requests: BTreeMap<String, ExternalRequestState>,
    #[serde(default)]
    pub logs: Vec<TraceEvent>,
    #[serde(default)]
    pub runtime: Value,
}

fn default_observation_version() -> u32 {
    OBSERVATION_VERSION
}

impl TelemetrySnapshot {
    pub fn parse(raw: &str) -> Result<Self, ObservationError> {
        let snapshot: Self = serde_json::from_str(raw).map_err(|error| {
            ObservationError::new(
                "TELEMETRY_INVALID",
                "trusted telemetry is not valid JSON",
                json!({"error":error.to_string()}),
            )
        })?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<(), ObservationError> {
        if self.version != OBSERVATION_VERSION {
            return Err(ObservationError::new(
                "TELEMETRY_VERSION_UNSUPPORTED",
                "trusted telemetry version is not supported",
                json!({"version":self.version,"supported":OBSERVATION_VERSION}),
            ));
        }
        if let Some((key, task)) = self
            .tasks
            .iter()
            .find(|(key, task)| key.trim().is_empty() || task.operation_id != **key)
        {
            return Err(ObservationError::new(
                "TELEMETRY_INVALID",
                "task keys must match non-empty operation_id values",
                json!({"key":key,"operation_id":task.operation_id}),
            ));
        }
        if let Some((key, request)) = self.external_requests.iter().find(|(key, request)| {
            key.trim().is_empty()
                || request.request_id != **key
                || request.observer_alias.is_empty()
        }) {
            return Err(ObservationError::new(
                "TELEMETRY_INVALID",
                "external request keys must match request_id and use a trusted observer alias",
                json!({"key":key,"request_id":request.request_id,"observer_alias":request.observer_alias}),
            ));
        }
        Ok(())
    }

    pub fn task(&self, operation_id: &str) -> Result<&AsyncOperation, ObservationError> {
        self.tasks.get(operation_id).ok_or_else(|| {
            ObservationError::new(
                "TASK_NOT_FOUND",
                "runtime task was not found in trusted telemetry",
                json!({"operation_id":operation_id}),
            )
        })
    }

    pub fn external_request(
        &self,
        request_id: &str,
    ) -> Result<&ExternalRequestState, ObservationError> {
        self.external_requests.get(request_id).ok_or_else(|| {
            ObservationError::new(
                "EXTERNAL_REQUEST_NOT_FOUND",
                "external request was not found in trusted telemetry",
                json!({"request_id":request_id}),
            )
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortableEvidenceRef {
    pub relative_path: String,
    pub resolved_path: String,
    /// Local-worker absolute path. `resolved_path` remains for compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_artifact_path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservationResult {
    pub success: bool,
    pub observation_type: ObservationType,
    pub state: String,
    pub elapsed_ms: u128,
    pub last_observation: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_ref: Option<PortableEvidenceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_ref: Option<PortableEvidenceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_ref: Option<PortableEvidenceRef>,
}

impl ObservationResult {
    pub fn success(
        observation_type: ObservationType,
        state: impl Into<String>,
        elapsed_ms: u128,
        last_observation: Value,
        evidence_ref: Option<PortableEvidenceRef>,
    ) -> Self {
        Self {
            success: true,
            observation_type,
            state: state.into(),
            elapsed_ms,
            last_observation,
            failed_phase: None,
            evidence_ref,
            screenshot_ref: None,
            report_ref: None,
        }
    }
}

impl ObservationResult {
    pub fn with_screenshot_ref(mut self, screenshot_ref: Option<PortableEvidenceRef>) -> Self {
        self.screenshot_ref = screenshot_ref;
        self
    }

    pub fn with_report_ref(mut self, report_ref: Option<PortableEvidenceRef>) -> Self {
        self.report_ref = report_ref;
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservationError {
    pub code: String,
    pub message: String,
    pub details: Value,
}

impl ObservationError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, details: Value) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details,
        }
    }

    pub fn as_value(&self) -> Value {
        json!({"code":self.code,"message":self.message,"details":self.details})
    }
}

impl std::fmt::Display for ObservationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ObservationError {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExternalObserversConfig {
    #[serde(default = "default_observation_version")]
    pub version: u32,
    #[serde(default)]
    pub observers: BTreeMap<String, ExternalObserverSpec>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExternalObserverSpec {
    /// "telemetry_bridge" or an explicitly configured "http_poll" adapter.
    pub kind: String,
    /// Trusted endpoint identifier, never a scenario-supplied URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// JSONPath subset used only by a configured future polling adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<String>,
    /// Named terminal states accepted by a configured polling adapter.
    #[serde(default)]
    pub terminal: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub redacted_fields: Vec<String>,
}

impl ExternalObserversConfig {
    pub fn built_in() -> Self {
        let config: Self = serde_json::from_str(include_str!("../external-observers.json"))
            .expect("embedded external-observers.json must be valid");
        config
            .validate()
            .expect("embedded external-observers.json must be trusted");
        config
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ObservationError> {
        let text = fs::read_to_string(path.as_ref()).map_err(|error| {
            ObservationError::new(
                "EXTERNAL_OBSERVER_CONFIG_INVALID",
                "could not read external observers configuration",
                json!({"error":error.to_string()}),
            )
        })?;
        let config: Self = serde_json::from_str(&text).map_err(|error| {
            ObservationError::new(
                "EXTERNAL_OBSERVER_CONFIG_INVALID",
                "external observers configuration is not valid JSON",
                json!({"error":error.to_string()}),
            )
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ObservationError> {
        if self.version != OBSERVATION_VERSION {
            return Err(ObservationError::new(
                "EXTERNAL_OBSERVER_CONFIG_INVALID",
                "unsupported external observers configuration version",
                json!({"version":self.version,"supported":OBSERVATION_VERSION}),
            ));
        }
        for (alias, spec) in &self.observers {
            if alias.trim().is_empty() || alias.contains('/') || alias.contains("://") {
                return Err(ObservationError::new(
                    "EXTERNAL_OBSERVER_CONFIG_INVALID",
                    "observer alias must be a simple trusted identifier",
                    json!({"alias":alias}),
                ));
            }
            if !matches!(spec.kind.as_str(), "telemetry_bridge" | "http_poll") {
                return Err(ObservationError::new(
                    "EXTERNAL_OBSERVER_CONFIG_INVALID",
                    "observer kind is not supported",
                    json!({"alias":alias,"kind":spec.kind}),
                ));
            }
            if spec.kind == "http_poll" && spec.endpoint.as_deref().is_none_or(str::is_empty) {
                return Err(ObservationError::new(
                    "EXTERNAL_OBSERVER_CONFIG_INVALID",
                    "trusted http_poll adapter requires an endpoint identifier in configuration",
                    json!({"alias":alias}),
                ));
            }
            if let Some(path) = spec.extract.as_deref() {
                parse_json_path(path).map_err(|error| {
                    ObservationError::new(
                        "EXTERNAL_OBSERVER_CONFIG_INVALID",
                        "trusted observer extract must use the supported JSONPath subset",
                        json!({"alias":alias,"extract":path,"error":error}),
                    )
                })?;
            }
            if spec.kind == "http_poll" && spec.terminal.is_empty() {
                return Err(ObservationError::new(
                    "EXTERNAL_OBSERVER_CONFIG_INVALID",
                    "trusted http_poll adapter requires terminal states in configuration",
                    json!({"alias":alias}),
                ));
            }
            if spec.terminal.iter().any(|state| state.trim().is_empty()) {
                return Err(ObservationError::new(
                    "EXTERNAL_OBSERVER_CONFIG_INVALID",
                    "trusted observer terminal states must be non-empty strings",
                    json!({"alias":alias}),
                ));
            }
        }
        Ok(())
    }

    pub fn observer(&self, alias: &str) -> Result<&ExternalObserverSpec, ObservationError> {
        self.observers.get(alias).ok_or_else(|| {
            ObservationError::new(
                "EXTERNAL_REQUEST_NOT_FOUND",
                "requested observer alias is not trusted or configured",
                json!({"observer_alias":alias}),
            )
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct TrustedRoots {
    roots: BTreeMap<String, PathBuf>,
}

impl TrustedRoots {
    pub fn new(roots: impl IntoIterator<Item = (String, PathBuf)>) -> Self {
        Self {
            roots: roots.into_iter().collect(),
        }
    }

    pub fn insert(&mut self, alias: impl Into<String>, root: impl Into<PathBuf>) {
        self.roots.insert(alias.into(), root.into());
    }

    pub fn aliases(&self) -> Vec<String> {
        self.roots.keys().cloned().collect()
    }

    /// Resolve a relative path under an explicitly configured root. Parent
    /// traversal, rooted paths, and drive prefixes are rejected before any I/O.
    pub fn resolve(
        &self,
        root_alias: &str,
        relative_path: &str,
    ) -> Result<PathBuf, ObservationError> {
        let root = self.roots.get(root_alias).ok_or_else(|| {
            ObservationError::new(
                "FILE_ROOT_NOT_ALLOWED",
                "file root alias is not trusted",
                json!({"root":root_alias,"trusted_roots":self.aliases()}),
            )
        })?;
        let relative = Path::new(relative_path);
        let mut candidate = root.clone();
        for component in relative.components() {
            match component {
                Component::Normal(segment) => candidate.push(segment),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(ObservationError::new(
                        "FILE_ROOT_NOT_ALLOWED",
                        "file path must stay below its trusted root",
                        json!({"root":root_alias,"path":relative_path}),
                    ));
                }
            }
        }
        self.assert_within(root_alias, &candidate)?;
        Ok(candidate)
    }

    /// Accept a path produced by an owned capture only when it still resolves
    /// inside one of the configured roots.
    pub fn portable_evidence_ref(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<PortableEvidenceRef, ObservationError> {
        let candidate = path.as_ref();
        let resolved = fs::canonicalize(candidate).map_err(|error| {
            ObservationError::new(
                "CAPTURE_FAILED",
                "evidence path could not be resolved",
                json!({"path":candidate,"error":error.to_string()}),
            )
        })?;
        for root in self.roots.values() {
            let root = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
            if resolved.starts_with(&root) {
                let relative = resolved
                    .strip_prefix(&root)
                    .unwrap_or(&resolved)
                    .to_string_lossy()
                    .trim_start_matches(['\\', '/'])
                    .replace('\\', "/");
                return Ok(PortableEvidenceRef {
                    relative_path: relative,
                    resolved_path: resolved.to_string_lossy().into_owned(),
                    absolute_path: Some(resolved.to_string_lossy().into_owned()),
                    environment_path: None,
                    host_artifact_path: None,
                });
            }
        }
        Err(ObservationError::new(
            "FILE_ROOT_NOT_ALLOWED",
            "evidence path is outside every trusted root",
            json!({"path":candidate,"trusted_roots":self.aliases()}),
        ))
    }

    fn assert_within(&self, root_alias: &str, candidate: &Path) -> Result<(), ObservationError> {
        let Some(root) = self.roots.get(root_alias) else {
            return Err(ObservationError::new(
                "FILE_ROOT_NOT_ALLOWED",
                "file root alias is not trusted",
                json!({"root":root_alias}),
            ));
        };
        if !candidate.exists() {
            return Ok(());
        }
        let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let canonical_candidate = fs::canonicalize(candidate).map_err(|error| {
            ObservationError::new(
                "FILE_ROOT_NOT_ALLOWED",
                "file path could not be canonicalized",
                json!({"path":candidate,"error":error.to_string()}),
            )
        })?;
        if canonical_candidate.starts_with(&canonical_root) {
            Ok(())
        } else {
            Err(ObservationError::new(
                "FILE_ROOT_NOT_ALLOWED",
                "resolved file path escapes the trusted root",
                json!({"root":root_alias,"path":candidate}),
            ))
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileState {
    pub root: String,
    pub relative_path: String,
    pub resolved_path: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_unix_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

pub fn inspect_file(
    roots: &TrustedRoots,
    root: &str,
    relative_path: &str,
    include_hash: bool,
) -> Result<FileState, ObservationError> {
    let path = roots.resolve(root, relative_path)?;
    match fs::metadata(&path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(ObservationError::new(
                    "FILE_NOT_FOUND",
                    "configured path is not a regular file",
                    json!({"root":root,"path":relative_path}),
                ));
            }
            let hash = include_hash.then(|| sha256_file(&path)).transpose()?;
            Ok(FileState {
                root: root.into(),
                relative_path: relative_path.into(),
                resolved_path: path.to_string_lossy().into_owned(),
                exists: true,
                size_bytes: Some(metadata.len()),
                modified_unix_ms: modified_unix_ms(&metadata),
                sha256: hash,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileState {
            root: root.into(),
            relative_path: relative_path.into(),
            resolved_path: path.to_string_lossy().into_owned(),
            exists: false,
            ..FileState::default()
        }),
        Err(error) => Err(ObservationError::new(
            "FILE_NOT_FOUND",
            "file metadata could not be read",
            json!({"root":root,"path":relative_path,"error":error.to_string()}),
        )),
    }
}

pub fn assert_file_exists(
    roots: &TrustedRoots,
    root: &str,
    relative_path: &str,
) -> Result<FileState, ObservationError> {
    let state = inspect_file(roots, root, relative_path, false)?;
    if state.exists {
        Ok(state)
    } else {
        Err(ObservationError::new(
            "FILE_NOT_FOUND",
            "expected output file does not exist",
            json!(state),
        ))
    }
}

pub fn assert_file_not_exists(
    roots: &TrustedRoots,
    root: &str,
    relative_path: &str,
) -> Result<FileState, ObservationError> {
    let state = inspect_file(roots, root, relative_path, false)?;
    if !state.exists {
        Ok(state)
    } else {
        Err(ObservationError::new(
            "FILE_UNEXPECTEDLY_EXISTS",
            "file unexpectedly exists",
            json!(state),
        ))
    }
}

pub fn assert_file_size(
    roots: &TrustedRoots,
    root: &str,
    relative_path: &str,
    expected_bytes: u64,
) -> Result<FileState, ObservationError> {
    let state = assert_file_exists(roots, root, relative_path)?;
    if state.size_bytes == Some(expected_bytes) {
        Ok(state)
    } else {
        Err(ObservationError::new(
            "FILE_SIZE_MISMATCH",
            "file size differs from the expected value",
            json!({"expected_bytes":expected_bytes,"actual":state}),
        ))
    }
}

pub fn assert_file_hash(
    roots: &TrustedRoots,
    root: &str,
    relative_path: &str,
    expected_sha256: &str,
) -> Result<FileState, ObservationError> {
    let state = inspect_file(roots, root, relative_path, true)?;
    if !state.exists {
        return Err(ObservationError::new(
            "FILE_NOT_FOUND",
            "expected output file does not exist",
            json!(state),
        ));
    }
    if state
        .sha256
        .as_deref()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected_sha256))
    {
        Ok(state)
    } else {
        Err(ObservationError::new(
            "FILE_HASH_MISMATCH",
            "file SHA-256 differs from the expected value",
            json!({"expected_sha256":expected_sha256,"actual":state}),
        ))
    }
}

pub fn assert_file_count(
    roots: &TrustedRoots,
    root: &str,
    relative_dir: &str,
    pattern: Option<&str>,
    expected_count: usize,
) -> Result<Value, ObservationError> {
    let path = roots.resolve(root, relative_dir)?;
    let entries = fs::read_dir(&path).map_err(|error| {
        ObservationError::new(
            "FILE_NOT_FOUND",
            "trusted directory could not be read",
            json!({"root":root,"path":relative_dir,"error":error.to_string()}),
        )
    })?;
    let mut matched = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if pattern.is_none_or(|pattern| wildcard_matches(pattern, &name)) {
            matched.push(name);
        }
    }
    matched.sort();
    let actual_count = matched.len();
    if actual_count == expected_count {
        Ok(json!({
            "root":root,
            "relative_path":relative_dir,
            "pattern":pattern,
            "count":actual_count,
            "matched":matched
        }))
    } else {
        Err(ObservationError::new(
            "FILE_COUNT_MISMATCH",
            "trusted directory file count differs from the expected value",
            json!({
                "root":root,
                "relative_path":relative_dir,
                "pattern":pattern,
                "expected_count":expected_count,
                "actual_count":actual_count,
                "matched":matched
            }),
        ))
    }
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut pattern_index, mut value_index, mut star, mut retry) = (0, 0, None, 0);
    while value_index < value.len() {
        if pattern.get(pattern_index) == Some(&b'?')
            || pattern.get(pattern_index) == value.get(value_index)
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern.get(pattern_index) == Some(&b'*') {
            star = Some(pattern_index);
            pattern_index += 1;
            retry = value_index;
        } else if let Some(star_index) = star {
            pattern_index = star_index + 1;
            retry += 1;
            value_index = retry;
        } else {
            return false;
        }
    }
    while pattern.get(pattern_index) == Some(&b'*') {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

pub fn wait_for_file(
    roots: &TrustedRoots,
    root: &str,
    relative_path: &str,
    timeout_ms: u64,
) -> Result<FileState, ObservationError> {
    let timeout_ms = bounded_timeout(timeout_ms)?;
    let started = Instant::now();
    loop {
        let state = inspect_file(roots, root, relative_path, false)?;
        if state.exists {
            return Ok(state);
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            return Err(ObservationError::new(
                "FILE_NOT_FOUND",
                "file did not appear before the bounded timeout",
                json!({"timeout_ms":timeout_ms,"last_observation":state}),
            ));
        }
        thread::sleep(Duration::from_millis(DEFAULT_POLL_MS));
    }
}

pub fn wait_for_file_stable(
    roots: &TrustedRoots,
    root: &str,
    relative_path: &str,
    timeout_ms: u64,
    stable_for_ms: u64,
) -> Result<FileState, ObservationError> {
    let timeout_ms = bounded_timeout(timeout_ms)?;
    let stable_for_ms = stable_for_ms.clamp(DEFAULT_POLL_MS, MAX_OBSERVATION_TIMEOUT_MS);
    let started = Instant::now();
    let mut previous = None::<(u64, Option<u128>)>;
    let mut stable_since = None::<Instant>;
    let mut observed_file = false;
    loop {
        let state = inspect_file(roots, root, relative_path, false)?;
        if state.exists {
            observed_file = true;
            let fingerprint = (state.size_bytes.unwrap_or_default(), state.modified_unix_ms);
            if previous == Some(fingerprint) {
                stable_since.get_or_insert_with(Instant::now);
                if stable_since
                    .as_ref()
                    .is_some_and(|since| since.elapsed() >= Duration::from_millis(stable_for_ms))
                {
                    return Ok(state);
                }
            } else {
                previous = Some(fingerprint);
                stable_since = Some(Instant::now());
            }
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            let code = if observed_file {
                "FILE_NOT_STABLE"
            } else {
                "FILE_NOT_FOUND"
            };
            return Err(ObservationError::new(
                code,
                "file did not reach a stable observed state before the bounded timeout",
                json!({"timeout_ms":timeout_ms,"stable_for_ms":stable_for_ms,"last_observation":state}),
            ));
        }
        thread::sleep(Duration::from_millis(DEFAULT_POLL_MS));
    }
}

fn bounded_timeout(timeout_ms: u64) -> Result<u64, ObservationError> {
    if timeout_ms == 0 || timeout_ms > MAX_OBSERVATION_TIMEOUT_MS {
        return Err(ObservationError::new(
            "INVALID_ARGUMENT",
            "observation timeout must be within the bounded range",
            json!({"timeout_ms":timeout_ms,"maximum_ms":MAX_OBSERVATION_TIMEOUT_MS}),
        ));
    }
    Ok(timeout_ms)
}

fn modified_unix_ms(metadata: &fs::Metadata) -> Option<u128> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
}

pub fn sha256_file(path: &Path) -> Result<String, ObservationError> {
    let bytes = fs::read(path).map_err(|error| {
        ObservationError::new(
            "FILE_NOT_FOUND",
            "file could not be read for SHA-256 verification",
            json!({"path":path,"error":error.to_string()}),
        )
    })?;
    Ok(sha256_hex(&bytes))
}

/// Minimal SHA-256 implementation keeps file assertions self-contained and
/// avoids promoting a third-party hash crate into the public runtime boundary.
pub fn sha256_hex(input: &[u8]) -> String {
    const INITIAL: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];
    let mut message = input.to_vec();
    let bit_length = (message.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());
    let mut hash = INITIAL;
    for chunk in message.chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = hash;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temporary_1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temporary_2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temporary_1);
            d = c;
            c = b;
            b = a;
            a = temporary_1.wrapping_add(temporary_2);
        }
        hash[0] = hash[0].wrapping_add(a);
        hash[1] = hash[1].wrapping_add(b);
        hash[2] = hash[2].wrapping_add(c);
        hash[3] = hash[3].wrapping_add(d);
        hash[4] = hash[4].wrapping_add(e);
        hash[5] = hash[5].wrapping_add(f);
        hash[6] = hash[6].wrapping_add(g);
        hash[7] = hash[7].wrapping_add(h);
    }
    hash.iter().map(|word| format!("{word:08x}")).collect()
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PixelRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VisualMetrics {
    pub width: u32,
    pub height: u32,
    pub sampled_pixels: u64,
    pub mean_luma: f64,
    pub luma_variance: f64,
    pub changed_pixels: u64,
    pub changed_fraction: f64,
    pub mean_absolute_difference: f64,
    pub similarity: f64,
}

#[derive(Clone, Debug)]
struct RgbaImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

pub fn assert_not_blank(
    path: &Path,
    region: Option<PixelRegion>,
    masks: &[PixelRegion],
    minimum_variance: f64,
) -> Result<VisualMetrics, ObservationError> {
    let image = read_png(path)?;
    let metrics = single_image_metrics(&image, region, masks)?;
    if metrics.luma_variance > minimum_variance && metrics.sampled_pixels > 0 {
        Ok(metrics)
    } else {
        Err(ObservationError::new(
            "VISUAL_BLANK",
            "captured image region is visually blank",
            json!({"minimum_variance":minimum_variance,"metrics":metrics}),
        ))
    }
}

pub fn assert_region_changed(
    baseline: &Path,
    current: &Path,
    region: Option<PixelRegion>,
    masks: &[PixelRegion],
    minimum_changed_fraction: f64,
) -> Result<VisualMetrics, ObservationError> {
    let metrics = compare_pngs(baseline, current, region, masks)?;
    if metrics.changed_fraction >= minimum_changed_fraction {
        Ok(metrics)
    } else {
        Err(ObservationError::new(
            "VISUAL_REGION_UNCHANGED",
            "visual region did not change by the required fraction",
            json!({"minimum_changed_fraction":minimum_changed_fraction,"metrics":metrics}),
        ))
    }
}

pub fn assert_region_unchanged(
    baseline: &Path,
    current: &Path,
    region: Option<PixelRegion>,
    masks: &[PixelRegion],
    maximum_changed_fraction: f64,
) -> Result<VisualMetrics, ObservationError> {
    let metrics = compare_pngs(baseline, current, region, masks)?;
    if metrics.changed_fraction <= maximum_changed_fraction {
        Ok(metrics)
    } else {
        Err(ObservationError::new(
            "VISUAL_REGION_CHANGED",
            "visual region changed more than the allowed fraction",
            json!({"maximum_changed_fraction":maximum_changed_fraction,"metrics":metrics}),
        ))
    }
}

pub fn assert_similarity(
    baseline: &Path,
    current: &Path,
    region: Option<PixelRegion>,
    masks: &[PixelRegion],
    minimum_similarity: f64,
) -> Result<VisualMetrics, ObservationError> {
    let metrics = compare_pngs(baseline, current, region, masks)?;
    if metrics.similarity >= minimum_similarity {
        Ok(metrics)
    } else {
        Err(ObservationError::new(
            "VISUAL_SIMILARITY_FAILED",
            "visual similarity is lower than the configured threshold",
            json!({"minimum_similarity":minimum_similarity,"metrics":metrics}),
        ))
    }
}

pub fn compare_pngs(
    baseline: &Path,
    current: &Path,
    region: Option<PixelRegion>,
    masks: &[PixelRegion],
) -> Result<VisualMetrics, ObservationError> {
    let before = read_png(baseline)?;
    let after = read_png(current)?;
    if before.width != after.width || before.height != after.height {
        return Err(ObservationError::new(
            "CAPTURE_FAILED",
            "visual comparison requires equal image dimensions",
            json!({"baseline":{"width":before.width,"height":before.height},"current":{"width":after.width,"height":after.height}}),
        ));
    }
    let region = effective_region(&before, region)?;
    let mut sampled = 0_u64;
    let mut changed = 0_u64;
    let mut total_difference = 0.0;
    for y in region.y..region.y + region.height {
        for x in region.x..region.x + region.width {
            if masks.iter().any(|mask| contains(*mask, x, y)) {
                continue;
            }
            let index = ((y * before.width + x) * 4) as usize;
            let difference = (0..3)
                .map(|channel| {
                    (before.pixels[index + channel] as f64 - after.pixels[index + channel] as f64)
                        .abs()
                        / 255.0
                })
                .sum::<f64>()
                / 3.0;
            sampled += 1;
            total_difference += difference;
            if difference > 0.02 {
                changed += 1;
            }
        }
    }
    if sampled == 0 {
        return Err(ObservationError::new(
            "CAPTURE_FAILED",
            "visual comparison region has no unmasked pixels",
            json!({"region":region,"masks":masks}),
        ));
    }
    let mean_absolute_difference = total_difference / sampled as f64;
    Ok(VisualMetrics {
        width: region.width,
        height: region.height,
        sampled_pixels: sampled,
        changed_pixels: changed,
        changed_fraction: changed as f64 / sampled as f64,
        mean_absolute_difference,
        similarity: (1.0 - mean_absolute_difference).clamp(0.0, 1.0),
        ..VisualMetrics::default()
    })
}

fn single_image_metrics(
    image: &RgbaImage,
    region: Option<PixelRegion>,
    masks: &[PixelRegion],
) -> Result<VisualMetrics, ObservationError> {
    let region = effective_region(image, region)?;
    let mut sampled = 0_u64;
    let mut sum = 0.0;
    let mut squared_sum = 0.0;
    for y in region.y..region.y + region.height {
        for x in region.x..region.x + region.width {
            if masks.iter().any(|mask| contains(*mask, x, y)) {
                continue;
            }
            let index = ((y * image.width + x) * 4) as usize;
            let luma = 0.2126 * image.pixels[index] as f64
                + 0.7152 * image.pixels[index + 1] as f64
                + 0.0722 * image.pixels[index + 2] as f64;
            sum += luma;
            squared_sum += luma * luma;
            sampled += 1;
        }
    }
    if sampled == 0 {
        return Err(ObservationError::new(
            "CAPTURE_FAILED",
            "visual assertion region has no unmasked pixels",
            json!({"region":region,"masks":masks}),
        ));
    }
    let mean = sum / sampled as f64;
    Ok(VisualMetrics {
        width: region.width,
        height: region.height,
        sampled_pixels: sampled,
        mean_luma: mean,
        luma_variance: (squared_sum / sampled as f64 - mean * mean).max(0.0),
        ..VisualMetrics::default()
    })
}

fn effective_region(
    image: &RgbaImage,
    requested: Option<PixelRegion>,
) -> Result<PixelRegion, ObservationError> {
    let region = requested.unwrap_or(PixelRegion {
        x: 0,
        y: 0,
        width: image.width,
        height: image.height,
    });
    let right = region.x.saturating_add(region.width).min(image.width);
    let bottom = region.y.saturating_add(region.height).min(image.height);
    if region.x >= right || region.y >= bottom {
        return Err(ObservationError::new(
            "CAPTURE_FAILED",
            "requested visual region does not intersect the captured image",
            json!({"region":region,"image":{"width":image.width,"height":image.height}}),
        ));
    }
    Ok(PixelRegion {
        x: region.x,
        y: region.y,
        width: right - region.x,
        height: bottom - region.y,
    })
}

fn contains(region: PixelRegion, x: u32, y: u32) -> bool {
    x >= region.x
        && y >= region.y
        && x < region.x.saturating_add(region.width)
        && y < region.y.saturating_add(region.height)
}

fn read_png(path: &Path) -> Result<RgbaImage, ObservationError> {
    let file = File::open(path).map_err(|error| {
        ObservationError::new(
            "CAPTURE_FAILED",
            "PNG capture could not be opened",
            json!({"path":path,"error":error.to_string()}),
        )
    })?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|error| {
        ObservationError::new(
            "CAPTURE_FAILED",
            "PNG header could not be decoded",
            json!({"path":path,"error":error.to_string()}),
        )
    })?;
    let size = reader.output_buffer_size().ok_or_else(|| {
        ObservationError::new(
            "CAPTURE_FAILED",
            "PNG output buffer size overflowed",
            json!({"path":path}),
        )
    })?;
    let mut buffer = vec![0; size];
    let info = reader.next_frame(&mut buffer).map_err(|error| {
        ObservationError::new(
            "CAPTURE_FAILED",
            "PNG frame could not be decoded",
            json!({"path":path,"error":error.to_string()}),
        )
    })?;
    let bytes = &buffer[..info.buffer_size()];
    let mut pixels = Vec::with_capacity(info.width as usize * info.height as usize * 4);
    match info.color_type {
        png::ColorType::Rgba => pixels.extend_from_slice(bytes),
        png::ColorType::Rgb => {
            for pixel in bytes.chunks_exact(3) {
                pixels.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for value in bytes {
                pixels.extend_from_slice(&[*value, *value, *value, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in bytes.chunks_exact(2) {
                pixels.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        png::ColorType::Indexed => {
            return Err(ObservationError::new(
                "CAPTURE_FAILED",
                "expanded PNG unexpectedly remained indexed",
                json!({"path":path}),
            ));
        }
    }
    Ok(RgbaImage {
        width: info.width,
        height: info.height,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::BufWriter, time::SystemTime};

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ai-ui-observation-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_png(path: &Path, pixels: &[u8]) {
        let file = File::create(path).unwrap();
        let mut encoder = png::Encoder::new(BufWriter::new(file), 2, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(pixels).unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn sha256_matches_standard_test_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn trusted_roots_reject_traversal_and_scope_file_assertions() {
        let root = test_root("roots");
        fs::write(root.join("artifact.txt"), "hello").unwrap();
        let roots = TrustedRoots::new([(String::from("output"), root.clone())]);
        assert!(roots.resolve("output", "../outside.txt").is_err());
        let state = assert_file_size(&roots, "output", "artifact.txt", 5).unwrap();
        assert_eq!(state.size_bytes, Some(5));
        assert_file_hash(
            &roots,
            "output",
            "artifact.txt",
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        )
        .unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn telemetry_requires_named_trusted_external_observer() {
        let valid = r#"{
            "version":1,
            "tasks":{"task.a":{"operation_id":"task.a","kind":"CPU","name":"A","state":"Running"}},
            "external_requests":{"request.a":{"request_id":"request.a","observer_alias":"fixture","state":"WaitingExternal"}}
        }"#;
        let snapshot = TelemetrySnapshot::parse(valid).unwrap();
        assert_eq!(
            snapshot.task("task.a").unwrap().state,
            AsyncOperationState::Running
        );
        assert!(
            !snapshot
                .external_request("request.a")
                .unwrap()
                .application_accepted_response
        );
    }

    #[test]
    fn file_count_uses_a_trusted_root_and_optional_pattern() {
        let root = test_root("count");
        fs::write(root.join("first.csv"), "a").unwrap();
        fs::write(root.join("second.csv"), "b").unwrap();
        fs::write(root.join("ignore.txt"), "c").unwrap();
        let roots = TrustedRoots::new([(String::from("output"), root.clone())]);
        let count = assert_file_count(&roots, "output", ".", Some("*.csv"), 2).unwrap();
        assert_eq!(count["count"], 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn embedded_observer_aliases_are_trusted_and_non_url() {
        let config = ExternalObserversConfig::built_in();
        assert_eq!(config.observer("fixture").unwrap().kind, "telemetry_bridge");
        assert_eq!(
            config.observer("ocr.job").unwrap().endpoint.as_deref(),
            Some("ocr_status")
        );
    }

    #[test]
    fn visual_assertions_detect_a_real_region_change() {
        let root = test_root("visual");
        let before = root.join("before.png");
        let after = root.join("after.png");
        write_png(
            &before,
            &[0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255],
        );
        write_png(
            &after,
            &[255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255],
        );
        let metrics = assert_region_changed(&before, &after, None, &[], 0.2).unwrap();
        assert_eq!(metrics.changed_pixels, 1);
        assert!(assert_region_unchanged(&before, &after, None, &[], 0.1).is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observation_result_keeps_distinct_canonical_artifact_refs() {
        let artifact = PortableEvidenceRef {
            relative_path: "captures/result.png".into(),
            resolved_path: "C:/evidence/captures/result.png".into(),
            absolute_path: Some("C:/evidence/captures/result.png".into()),
            environment_path: None,
            host_artifact_path: None,
        };
        let result = ObservationResult::success(
            ObservationType::Visual,
            "changed",
            12,
            json!({"changed":true}),
            Some(artifact.clone()),
        )
        .with_screenshot_ref(Some(artifact.clone()))
        .with_report_ref(Some(artifact));
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(
            value["evidence_ref"]["relative_path"],
            "captures/result.png"
        );
        assert_eq!(
            value["screenshot_ref"]["relative_path"],
            "captures/result.png"
        );
        assert_eq!(
            value["report_ref"]["absolute_path"],
            "C:/evidence/captures/result.png"
        );
        let legacy: PortableEvidenceRef = serde_json::from_value(json!({
            "relative_path":"legacy.png",
            "resolved_path":"C:/legacy.png"
        }))
        .unwrap();
        assert!(legacy.absolute_path.is_none());
        assert!(legacy.environment_path.is_none());
        assert!(legacy.host_artifact_path.is_none());
    }
}
