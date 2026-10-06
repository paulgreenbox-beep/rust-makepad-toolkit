//! Reliability orchestration for the Makepad UI automation prototype.
//!
//! This layer owns test-session identity and proves action outcomes. The
//! generic Makepad probe remains the semantic source, while WindowsAutomation
//! remains the native UIA/capture backend.

use crate::{
    advanced::AdvancedHost,
    contract::{
        CONTRACT_VERSION, SCHEMA_VERSION, TOOL_VERSION, canonicalize_response,
        validate_evidence_bundle,
    },
    fixtures,
    host::{HostError, HostResult, RuntimeHealth},
    spatial::RelativeRect,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufReader, BufWriter},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};
use toolkit_windows_automation::{
    ControlInfo, ControlTreeOptions, ControlTreeResult, ElementState, Rect, TreeView, WindowInfo,
    WindowQuery, now_unix_ms,
};

#[cfg(windows)]
use windows_uia::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible},
};

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_POLL_MS: u64 = 75;
const RUNTIME_SETTLE_MS: u64 = 180;
const EVIDENCE_EXPORT_FAILURE_INJECTION_ENV: &str = "AI_UI_TEST_INJECT_EVIDENCE_EXPORT_FAILURE";

#[path = "reliability_observation.rs"]
mod reliability_observation;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReliabilityConfig {
    pub minimum_interactable_width: i32,
    pub minimum_interactable_height: i32,
    pub default_timeout_ms: u64,
    pub default_poll_ms: u64,
    pub runtime_settle_ms: u64,
}

impl Default for ReliabilityConfig {
    fn default() -> Self {
        Self {
            minimum_interactable_width: 1,
            minimum_interactable_height: 1,
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
            default_poll_ms: DEFAULT_POLL_MS,
            runtime_settle_ms: RUNTIME_SETTLE_MS,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub process_id: u32,
    pub executable: String,
    pub alive: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionWindow {
    pub session_id: String,
    pub process_id: u32,
    pub window_handle: String,
    pub window_id: String,
    pub name: String,
    pub control_type: String,
    pub automation_id: String,
    pub bounds: Rect,
    pub visible: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceReference {
    pub path: String,
    pub reason: String,
    pub timestamp_unix_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionRecord {
    pub action_id: String,
    pub timestamp_unix_ms: u128,
    pub session_id: String,
    pub process_id: Option<u32>,
    pub window_id: Option<String>,
    pub action: String,
    pub target: Option<String>,
    pub result: Value,
    pub postcondition: Option<Value>,
    pub runtime_health: Option<RuntimeHealth>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutomationSession {
    pub session_id: String,
    pub processes: Vec<ProcessIdentity>,
    pub windows: Vec<SessionWindow>,
    pub active_window: Option<String>,
    pub action_log: Vec<ActionRecord>,
    pub evidence: Vec<EvidenceReference>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WindowFindQuery {
    pub session_id: Option<String>,
    pub process_id: Option<u32>,
    pub automation_id: Option<String>,
    pub title: Option<String>,
    pub name: Option<String>,
    pub control_type: Option<String>,
    pub root_type: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Interactability {
    pub interactable: bool,
    pub visible: bool,
    pub enabled: bool,
    pub offscreen: bool,
    pub valid_bounds: bool,
    pub window_visible: bool,
    pub clipping_known: bool,
    pub minimum_width: i32,
    pub minimum_height: i32,
    pub reasons: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum CanvasHealth {
    Healthy,
    ZeroViewport,
    EmptyClip,
    Hidden,
    DrawNotCalled,
    ContentOutsideViewport,
    StaleLayout,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanvasHealthReport {
    pub id: String,
    pub health: CanvasHealth,
    pub bounds: Rect,
    pub visible: bool,
    pub offscreen: bool,
    pub draw_called: Option<bool>,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceBundle {
    pub path: String,
    pub reason: String,
    pub files: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputLockLease {
    pub session_id: String,
    pub token: String,
    pub acquired_at_unix_ms: u128,
}

#[derive(Clone, Debug)]
struct InputLockState {
    lease: Option<InputLockLease>,
    sequence: u64,
}

impl Default for InputLockState {
    fn default() -> Self {
        Self {
            lease: None,
            sequence: 1,
        }
    }
}

static INPUT_LOCK: OnceLock<Mutex<InputLockState>> = OnceLock::new();

fn input_lock() -> &'static Mutex<InputLockState> {
    INPUT_LOCK.get_or_init(|| Mutex::new(InputLockState::default()))
}

struct ManagedSession {
    state: AutomationSession,
    host: AdvancedHost,
}

pub struct ReliabilityManager {
    app_path: PathBuf,
    window_title: String,
    output_root: PathBuf,
    launch_environment: BTreeMap<String, String>,
    launch_arguments: Vec<String>,
    config: ReliabilityConfig,
    next_session: u64,
    sessions: BTreeMap<String, ManagedSession>,
}

impl ReliabilityManager {
    pub fn new(
        app_path: impl Into<PathBuf>,
        window_title: impl Into<String>,
        output_root: impl Into<PathBuf>,
    ) -> Self {
        Self::with_config(
            app_path,
            window_title,
            output_root,
            ReliabilityConfig::default(),
        )
    }

    pub(crate) fn with_launch_environment(
        app_path: impl Into<PathBuf>,
        window_title: impl Into<String>,
        output_root: impl Into<PathBuf>,
        launch_environment: BTreeMap<String, String>,
    ) -> Self {
        Self::with_launch_environment_and_arguments(
            app_path,
            window_title,
            output_root,
            launch_environment,
            Vec::new(),
        )
    }

    pub(crate) fn with_launch_environment_and_arguments(
        app_path: impl Into<PathBuf>,
        window_title: impl Into<String>,
        output_root: impl Into<PathBuf>,
        launch_environment: BTreeMap<String, String>,
        launch_arguments: Vec<String>,
    ) -> Self {
        let mut manager = Self::new(app_path, window_title, output_root);
        manager.launch_environment = launch_environment;
        manager.launch_arguments = launch_arguments;
        manager
    }

    pub fn with_config(
        app_path: impl Into<PathBuf>,
        window_title: impl Into<String>,
        output_root: impl Into<PathBuf>,
        config: ReliabilityConfig,
    ) -> Self {
        Self {
            app_path: app_path.into(),
            window_title: window_title.into(),
            output_root: output_root.into(),
            launch_environment: BTreeMap::new(),
            launch_arguments: Vec::new(),
            config,
            next_session: 1,
            sessions: BTreeMap::new(),
        }
    }

    /// JSON operation surface used by AI/MCP adapters and declarative sequences.
    pub fn call(&mut self, operation: &str, arguments: Value) -> Value {
        let mut arguments = arguments;
        let (redaction_literals, redact_sensitive_response) = take_call_redaction(&mut arguments);
        let session_id = arguments
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let target = target_from_arguments(&arguments);
        let action_id = session_id.as_deref().and_then(|id| {
            self.sessions
                .get(id)
                .map(|session| format!("{}:action:{}", id, session.state.action_log.len() + 1))
        });
        let postcondition = arguments.get("postcondition").map(|value| {
            fixtures::redact_value(value, &redaction_literals, redact_sensitive_response)
        });
        let mut result = self.call_result(operation, arguments.clone());
        let preliminary_response = canonicalize_response(
            operation,
            match &result {
                Ok(value) => json!({"success":true,"result":value}),
                Err(error) => json!({"success":false,"error":error}),
            },
            action_id.as_deref(),
        );

        let preliminary_response = fixtures::redact_value(
            &preliminary_response,
            &redaction_literals,
            redact_sensitive_response,
        );

        if let Some(session_id) = session_id.as_deref() {
            if self.sessions.contains_key(session_id) && operation != "session.close" {
                self.record_action(
                    session_id,
                    operation,
                    action_id
                        .clone()
                        .unwrap_or_else(|| format!("action:{}", operation)),
                    target,
                    preliminary_response,
                    postcondition,
                );
            }
        }

        if let (Some(session_id), Err(error)) = (session_id.as_deref(), &mut result) {
            if operation != "windows_export_evidence"
                && error.details.get("evidence_path").is_none()
                && self.sessions.contains_key(session_id)
            {
                match self.export_evidence(session_id, operation) {
                    Ok(bundle) => {
                        let mut details = error.details.as_object().cloned().unwrap_or_default();
                        details.insert("evidence_path".into(), Value::String(bundle.path));
                        error.details = Value::Object(details);
                    }
                    Err(evidence_error) => attach_evidence_capture_failure(error, &evidence_error),
                }
            }
        }

        let response = canonicalize_response(
            operation,
            match &result {
                Ok(value) => json!({"success":true,"result":value}),
                Err(error) => json!({"success":false,"error":error}),
            },
            action_id.as_deref(),
        );
        let response =
            fixtures::redact_value(&response, &redaction_literals, redact_sensitive_response);
        if let Some(session_id) = session_id.as_deref() {
            if let Some(last) = self
                .sessions
                .get_mut(session_id)
                .and_then(|session| session.state.action_log.last_mut())
                .filter(|last| last.action == operation)
            {
                last.result = response.clone();
            }
        }
        response
    }

    pub fn open_session(&mut self) -> HostResult<AutomationSession> {
        fs::create_dir_all(&self.output_root)
            .map_err(|error| io_error("create reliability output root", error))?;
        let sequence = self.next_session;
        self.next_session += 1;
        let provisional_id = format!("session:makepad:{sequence}");
        let session_dir = self.output_root.join(safe_path_segment(&provisional_id));
        let mut host = AdvancedHost::with_title_and_environment_and_arguments(
            &self.app_path,
            self.window_title.clone(),
            &session_dir,
            self.launch_environment.clone(),
            self.launch_arguments.clone(),
        )?;
        let window: WindowInfo = serde_json::from_value(call_host(&mut host, "launch", json!({}))?)
            .map_err(|error| invalid(error.to_string()))?;
        let process_id = window.process_id;
        let session_id = format!("session:makepad:{process_id}:{sequence}");
        let tree = wait_for_initial_tree(&mut host, Duration::from_secs(8))?;
        let root = tree.elements.first();
        let session_window = SessionWindow {
            session_id: session_id.clone(),
            process_id,
            window_handle: window.hwnd.clone(),
            window_id: window.window_id.clone(),
            name: window.title.clone(),
            control_type: root
                .map(|control| control.control_type.clone())
                .unwrap_or_else(|| "Window".into()),
            automation_id: root
                .map(|control| control.automation_id.clone())
                .unwrap_or_default(),
            bounds: window.bounds,
            visible: root
                .map(|control| control.visible)
                .unwrap_or(window.visible),
        };
        let state = AutomationSession {
            session_id: session_id.clone(),
            processes: vec![ProcessIdentity {
                process_id,
                executable: window.process_name.clone(),
                alive: true,
            }],
            windows: vec![session_window],
            active_window: Some(window.window_id),
            action_log: Vec::new(),
            evidence: Vec::new(),
        };
        self.sessions.insert(
            session_id,
            ManagedSession {
                state: state.clone(),
                host,
            },
        );
        Ok(state)
    }

    pub fn list_sessions(&mut self) -> Vec<AutomationSession> {
        for session in self.sessions.values_mut() {
            let health = session.host.test_host_mut().get_runtime_health();
            if let Some(process) = session.state.processes.first_mut() {
                process.alive = health.process_alive;
            }
        }
        self.sessions
            .values()
            .map(|session| session.state.clone())
            .collect()
    }

    pub fn close_session(&mut self, session_id: &str) -> HostResult<AutomationSession> {
        let mut session = self.sessions.remove(session_id).ok_or_else(|| {
            host_error(
                "SESSION_NOT_FOUND",
                format!("automation session {session_id} was not found"),
                json!({"session_id":session_id}),
            )
        })?;
        let _ = session.host.call("shutdown", json!({}));
        for process in &mut session.state.processes {
            process.alive = false;
        }
        release_lock_for_closed_session(session_id);
        Ok(session.state)
    }

    fn refresh_session_windows(&mut self, session_id: &str) -> HostResult<()> {
        let session = self.session_mut(session_id)?;
        let process_id = session
            .state
            .processes
            .first()
            .map(|process| process.process_id)
            .ok_or_else(|| {
                host_error(
                    "PROCESS_NOT_FOUND",
                    "automation session has no registered process",
                    json!({"session_id":session_id}),
                )
            })?;
        let known_handles = session
            .state
            .windows
            .iter()
            .map(|window| window.window_handle.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let mut native_windows =
            session
                .host
                .test_host_mut()
                .automation_mut()
                .list_windows(&WindowQuery {
                    process_id: Some(process_id),
                    ..WindowQuery::default()
                })?;
        native_windows.retain(|window| {
            window.visible
                || !window.title.is_empty()
                || known_handles.contains(&window.hwnd.to_ascii_lowercase())
        });

        let previous_active = session.state.active_window.clone();
        let foreground = native_windows
            .iter()
            .find(|window| window.foreground)
            .map(|window| window.window_id.clone());
        let mut windows = Vec::with_capacity(native_windows.len());
        for native in native_windows {
            let root = session
                .host
                .test_host_mut()
                .automation_mut()
                .get_control_tree(
                    &native.window_id,
                    &ControlTreeOptions {
                        max_depth: Some(0),
                        max_elements: Some(1),
                        view: TreeView::Control,
                        ..ControlTreeOptions::default()
                    },
                )
                .ok()
                .and_then(|tree| tree.elements.into_iter().next());
            windows.push(session_window_from_native(
                session_id,
                &native,
                root.as_ref(),
            ));
        }
        let active_window = previous_active
            .filter(|active| windows.iter().any(|window| &window.window_id == active))
            .or(foreground)
            .or_else(|| windows.first().map(|window| window.window_id.clone()));
        session.state.windows = windows;
        session.state.active_window = active_window;
        Ok(())
    }

    pub fn find_window(&mut self, query: &WindowFindQuery) -> HostResult<SessionWindow> {
        let session_ids = self
            .sessions
            .keys()
            .filter(|session_id| {
                query
                    .session_id
                    .as_ref()
                    .is_none_or(|expected| expected == *session_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        for session_id in &session_ids {
            self.refresh_session_windows(session_id)?;
        }

        let mut candidates = self
            .sessions
            .values()
            .filter(|session| {
                query
                    .session_id
                    .as_ref()
                    .is_none_or(|expected| expected == &session.state.session_id)
            })
            .flat_map(|session| session.state.windows.iter())
            .filter(|window| window_matches(window, query))
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            (left.process_id, &left.window_id).cmp(&(right.process_id, &right.window_id))
        });
        match candidates.len() {
            0 => Err(host_error(
                "WINDOW_NOT_FOUND",
                "no live test window matched the requested session/window filters",
                json!({"query":query}),
            )),
            1 => Ok(candidates.remove(0)),
            count => Err(host_error(
                "AMBIGUOUS_WINDOW",
                format!("{count} windows matched; refine session/process/root filters"),
                json!({"query":query,"candidates":candidates}),
            )),
        }
    }

    pub fn resolve_window(
        &mut self,
        session_id: &str,
        window_id: Option<&str>,
        query: Option<&WindowFindQuery>,
    ) -> HostResult<SessionWindow> {
        let session_ids = self.sessions.keys().cloned().collect::<Vec<_>>();
        for candidate_session_id in session_ids {
            self.refresh_session_windows(&candidate_session_id)?;
        }
        let session = self.session(session_id)?;
        if let Some(window_id) = window_id {
            if let Some(window) = session
                .state
                .windows
                .iter()
                .find(|window| window.window_id == window_id)
            {
                return Ok(window.clone());
            }
            let actual_session = self.sessions.values().find_map(|candidate| {
                candidate
                    .state
                    .windows
                    .iter()
                    .any(|window| window.window_id == window_id)
                    .then_some(candidate.state.session_id.clone())
            });
            if let Some(actual_session) = actual_session {
                return Err(host_error(
                    "WINDOW_SESSION_MISMATCH",
                    "window_id belongs to another automation session",
                    json!({
                        "window_id":window_id,
                        "expected_session":session_id,
                        "actual_session":actual_session
                    }),
                ));
            }
            return Err(host_error(
                "WINDOW_NOT_FOUND",
                "window_id is not registered in this automation manager",
                json!({"session_id":session_id,"window_id":window_id}),
            ));
        }
        if let Some(query) = query {
            let mut query = query.clone();
            query.session_id = Some(session_id.to_owned());
            return self.find_window(&query);
        }
        let active = session.state.active_window.as_deref().ok_or_else(|| {
            host_error(
                "WINDOW_NOT_FOUND",
                "session has no active window",
                json!({"session_id":session_id}),
            )
        })?;
        session
            .state
            .windows
            .iter()
            .find(|window| window.window_id == active)
            .cloned()
            .ok_or_else(|| {
                host_error(
                    "WINDOW_NOT_FOUND",
                    "active window identity is stale",
                    json!({"session_id":session_id,"window_id":active}),
                )
            })
    }

    /// Bind the live AdvancedHost and the public session record to one
    /// session-owned opaque HWND before a target-scoped operation.
    fn select_session_window(
        &mut self,
        session_id: &str,
        window_id: &str,
    ) -> HostResult<SessionWindow> {
        let selected = self.resolve_window(session_id, Some(window_id), None)?;
        let session = self.session_mut(session_id)?;
        session.host.select_window(&selected.window_id)?;
        session.state.active_window = Some(selected.window_id.clone());
        Ok(selected)
    }

    pub fn list_test_windows(
        &mut self,
        session_id: Option<&str>,
    ) -> HostResult<Vec<SessionWindow>> {
        let session_ids = match session_id {
            Some(session_id) => vec![session_id.to_owned()],
            None => self.sessions.keys().cloned().collect(),
        };
        for session_id in &session_ids {
            self.refresh_session_windows(session_id)?;
        }
        if let Some(session_id) = session_id {
            return Ok(self.session(session_id)?.state.windows.clone());
        }
        Ok(self
            .sessions
            .values()
            .flat_map(|session| session.state.windows.clone())
            .collect())
    }

    pub fn acquire_input_lock(&self, session_id: &str) -> HostResult<InputLockLease> {
        self.session(session_id)?;
        let mut state = input_lock().lock().map_err(|_| {
            host_error(
                "INPUT_LOCK_FAILURE",
                "physical input lock is poisoned",
                Value::Null,
            )
        })?;
        if let Some(lease) = state.lease.as_ref() {
            if lease.session_id == session_id {
                return Ok(lease.clone());
            }
            return Err(host_error(
                "WINDOW_FOCUS_CONFLICT",
                "physical input is already owned by another automation session",
                json!({"requested_session":session_id,"lock_owner":lease.session_id}),
            ));
        }
        let lease = InputLockLease {
            session_id: session_id.to_owned(),
            token: format!("input-lock:{}:{}", now_unix_ms(), state.sequence),
            acquired_at_unix_ms: now_unix_ms(),
        };
        state.sequence += 1;
        state.lease = Some(lease.clone());
        Ok(lease)
    }

    pub fn release_input_lock(&self, session_id: &str, token: &str) -> HostResult<bool> {
        let mut state = input_lock().lock().map_err(|_| {
            host_error(
                "INPUT_LOCK_FAILURE",
                "physical input lock is poisoned",
                Value::Null,
            )
        })?;
        let Some(lease) = state.lease.as_ref() else {
            return Ok(false);
        };
        if lease.session_id != session_id || lease.token != token {
            return Err(host_error(
                "WINDOW_FOCUS_CONFLICT",
                "physical input lock token/session does not match its owner",
                json!({
                    "requested_session":session_id,
                    "lock_owner":lease.session_id,
                    "token_matches":lease.token == token
                }),
            ));
        }
        state.lease = None;
        Ok(true)
    }

    pub fn export_evidence(
        &mut self,
        session_id: &str,
        reason: &str,
    ) -> HostResult<EvidenceBundle> {
        if evidence_export_failure_injected() {
            return Err(host_error(
                "EVIDENCE_CAPTURE_INJECTED_FAILURE",
                "controlled evidence capture failure injected by remediation suite",
                json!({
                    "injected": true,
                    "environment": EVIDENCE_EXPORT_FAILURE_INJECTION_ENV,
                }),
            ));
        }

        let timestamp = now_unix_ms();
        let bundle_dir = self
            .output_root
            .join(safe_path_segment(session_id))
            .join(format!(
                "evidence-{timestamp}-{}",
                safe_path_segment(reason)
            ));
        let screenshots_dir = bundle_dir.join("screenshots");
        fs::create_dir_all(&screenshots_dir)
            .map_err(|error| io_error("create evidence bundle", error))?;

        let session = self.session_mut(session_id)?;
        let runtime = session.host.test_host_mut().get_runtime_health();
        let last_tree = if runtime.process_alive {
            call_host(&mut session.host, "inspect_ui", json!({}))
                .ok()
                .or_else(|| {
                    session
                        .host
                        .last_tree()
                        .and_then(|tree| serde_json::to_value(tree).ok())
                })
        } else {
            session
                .host
                .last_tree()
                .and_then(|tree| serde_json::to_value(tree).ok())
        }
        .unwrap_or(Value::Null);
        let ui_changes = json!(session.host.test_host().get_ui_changes());
        let actions = session.state.action_log.clone();
        let results = actions
            .iter()
            .filter(|action| action.result["success"] == true)
            .cloned()
            .collect::<Vec<_>>();
        let errors = actions
            .iter()
            .filter(|action| action.result["success"] == false)
            .cloned()
            .collect::<Vec<_>>();

        let mut screenshot_sources = BTreeSet::new();
        for action in &actions {
            collect_png_paths(&action.result, &mut screenshot_sources);
        }
        if runtime.healthy {
            if let Ok(capture) = call_host(&mut session.host, "screenshot", json!({})) {
                collect_png_paths(&capture, &mut screenshot_sources);
            }
        }
        let mut copied_screenshots = Vec::new();
        for (index, source) in screenshot_sources.iter().enumerate() {
            let source_path = Path::new(source);
            if !source_path.is_file() {
                continue;
            }
            let file_name = source_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("capture-{index}.png"));
            let target = screenshots_dir.join(format!("{index:03}-{file_name}"));
            if fs::copy(source_path, &target).is_ok() {
                copied_screenshots.push(target.to_string_lossy().into_owned());
            }
        }

        let evidence_manifest = json!({
            "schema_version": SCHEMA_VERSION,
            "tool_version": TOOL_VERSION,
            "contract_version": CONTRACT_VERSION,
            "actions": actions,
        });
        validate_evidence_bundle(&evidence_manifest).map_err(|errors| {
            host_error(
                "EVIDENCE_CONSISTENCY_FAILED",
                "evidence bundle violates canonical contract",
                json!({"errors":errors}),
            )
        })?;
        let summary = json!({
            "schema_version": SCHEMA_VERSION,
            "tool_version": TOOL_VERSION,
            "contract_version": CONTRACT_VERSION,
            "reason":reason,
            "timestamp_unix_ms":timestamp,
            "session_id":session_id,
            "processes":session.state.processes,
            "windows":session.state.windows,
            "active_window":session.state.active_window,
            "healthy":runtime.healthy,
            "action_count":actions.len(),
            "result_count":results.len(),
            "error_count":errors.len(),
            "screenshots":copied_screenshots,
            "evidence_consistency": "AI_UI_EVIDENCE_CONSISTENCY_PASSED",
        });
        let files = vec![
            ("summary.json", summary),
            ("actions.json", json!(actions)),
            ("results.json", json!(results)),
            ("errors.json", json!(errors)),
            ("ui-changes.json", ui_changes),
            ("last-ui-tree.json", last_tree),
            ("runtime.json", json!(runtime)),
        ];
        let mut written = Vec::new();
        for (name, value) in files {
            let path = bundle_dir.join(name);
            write_json(&path, &value)?;
            written.push(path.to_string_lossy().into_owned());
        }
        written.extend(copied_screenshots);
        let reference = EvidenceReference {
            path: bundle_dir.to_string_lossy().into_owned(),
            reason: reason.to_owned(),
            timestamp_unix_ms: timestamp,
        };
        session.state.evidence.push(reference.clone());
        Ok(EvidenceBundle {
            path: reference.path,
            reason: reason.to_owned(),
            files: written,
        })
    }

    fn call_result(&mut self, operation: &str, arguments: Value) -> HostResult<Value> {
        match operation {
            "session.open" => self.open_session().and_then(to_value),
            "session.list" => Ok(json!(self.list_sessions())),
            "session.resolve_window" => {
                let session_id = required(&arguments, "session_id")?;
                let window_id = arguments.get("window_id").and_then(Value::as_str);
                let query = parse_window_query(&arguments)?;
                self.resolve_window(&session_id, window_id, Some(&query))
                    .and_then(to_value)
            }
            "session.close" => {
                let session_id = required(&arguments, "session_id")?;
                self.close_session(&session_id)
                    .map(|session| closed_session_result(&session.session_id))
            }
            "windows_find_window" | "wait_for_window" => {
                if operation == "wait_for_window" {
                    self.wait_for_window(&arguments)
                } else {
                    self.find_window(&parse_window_query(&arguments)?)
                        .and_then(to_value)
                }
            }
            "list_test_windows" => self
                .list_test_windows(arguments.get("session_id").and_then(Value::as_str))
                .and_then(to_value),
            "acquire_input_lock" => self
                .acquire_input_lock(&required(&arguments, "session_id")?)
                .and_then(to_value),
            "release_input_lock" => self
                .release_input_lock(
                    &required(&arguments, "session_id")?,
                    &required(&arguments, "token")?,
                )
                .map(Value::Bool),
            "windows_run_sequence" => self.run_sequence(&arguments),
            _ => {
                let session_id = required(&arguments, "session_id")?;
                if let Some(window_id) = arguments.get("window_id").and_then(Value::as_str) {
                    self.select_session_window(&session_id, window_id)?;
                }
                self.session_operation(&session_id, operation, &arguments)
            }
        }
    }

    fn session_operation(
        &mut self,
        session_id: &str,
        operation: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        if let Some(result) = self.observation_operation(session_id, operation, arguments)? {
            return Ok(result);
        }

        match operation {
            "get_active_window" => self.get_active_window(session_id).and_then(to_value),
            "activate_window" | "ensure_active_window" => {
                let token = arguments
                    .get("input_lock_token")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.activate_window(session_id, token)
            }
            "inspect_ui" | "get_control_tree" | "inspect_semantic" => {
                self.inspect_compact(session_id, arguments)
            }
            "input" | "set_value" => self.verified_input(session_id, arguments),
            "click" => self.verified_click(session_id, arguments),
            "wait_for_element" | "wait_for_automation_id" => {
                self.wait_for_element(session_id, arguments)
            }
            "wait_for_value" => self.wait_for_value(session_id, arguments),
            "wait_for_state" => self.wait_for_state(session_id, arguments),
            "wait_until" => self.wait_until(session_id, arguments),
            "wait_idle" => self.wait_idle(session_id, arguments),
            "assert_visible" => self.assert_control(session_id, arguments, "visible"),
            "assert_interactable" => self.assert_control(session_id, arguments, "interactable"),
            "assert_valid_bounds" => self.assert_control(session_id, arguments, "valid_bounds"),
            "screenshot" | "screenshot_window" => self.screenshot_window(session_id),
            "screenshot_element" => self.screenshot_element(session_id, arguments),
            "windows_capture_region" => self.capture_region(session_id, arguments),
            "get_runtime_health" => {
                let health = self
                    .session_mut(session_id)?
                    .host
                    .test_host_mut()
                    .get_runtime_health();
                Ok(json!(health))
            }
            "get_runtime_stderr" => {
                let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(80) as usize;
                Ok(json!(
                    self.session(session_id)?
                        .host
                        .test_host()
                        .get_runtime_stderr(limit)
                ))
            }
            "get_canvas_health" => self.canvas_health(session_id, arguments).and_then(to_value),
            "assert_canvas_healthy" => {
                let report = self.canvas_health(session_id, arguments)?;
                if report.health == CanvasHealth::Healthy {
                    to_value(report)
                } else {
                    Err(host_error(
                        "CANVAS_UNHEALTHY",
                        format!("canvas {} is {:?}", report.id, report.health),
                        json!(report),
                    ))
                }
            }
            "windows_list_automation_ids" => self.list_automation_ids(session_id, arguments),
            "windows_assert_automation_ids" => self.assert_automation_ids(session_id, arguments),
            "windows_validate_evidence" => {
                let bundle = arguments
                    .get("bundle")
                    .cloned()
                    .unwrap_or_else(|| arguments.clone());
                let (manifest, evidence_path) =
                    materialize_evidence_manifest(&bundle).map_err(|errors| {
                        host_error(
                            "EVIDENCE_CONSISTENCY_FAILED",
                            "evidence bundle is invalid",
                            json!({"errors":errors}),
                        )
                    })?;
                validate_evidence_bundle(&manifest).map_err(|errors| {
                    host_error(
                        "EVIDENCE_CONSISTENCY_FAILED",
                        "evidence bundle is invalid",
                        json!({"errors":errors}),
                    )
                })?;
                let validated_from_export_receipt = evidence_path.is_some();
                let validated_action_count = manifest
                    .get("actions")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                Ok(json!({
                    "valid":true,
                    "marker":"AI_UI_EVIDENCE_CONSISTENCY_PASSED",
                    "schema_version":SCHEMA_VERSION,
                    "tool_version":TOOL_VERSION,
                    "contract_version":CONTRACT_VERSION,
                    "validated_from_export_receipt":validated_from_export_receipt,
                    "evidence_path":evidence_path,
                    "validated_action_count":validated_action_count,
                }))
            }
            "windows_export_evidence" => self
                .export_evidence(
                    session_id,
                    arguments
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("manual"),
                )
                .and_then(to_value),
            advanced_operation => self.delegate_advanced(session_id, advanced_operation, arguments),
        }
    }

    fn get_active_window(&mut self, session_id: &str) -> HostResult<Value> {
        let window = self.resolve_window(session_id, None, None)?;
        let foreground = foreground_window();
        Ok(json!({
            "window":window,
            "foreground":foreground,
            "verified":foreground.as_ref().is_some_and(|actual|
                actual.window_handle.eq_ignore_ascii_case(&window.window_handle)
                    && actual.process_id == window.process_id),
        }))
    }

    fn activate_window(&mut self, session_id: &str, token: &str) -> HostResult<Value> {
        self.require_input_lock(session_id, token)?;
        let expected = self.resolve_window(session_id, None, None)?;
        let result = {
            let session = self.session_mut(session_id)?;
            call_host(&mut session.host, "activate_window", json!({}))?
        };
        let foreground = foreground_window();
        let verified = foreground.as_ref().is_some_and(|actual| {
            actual
                .window_handle
                .eq_ignore_ascii_case(&expected.window_handle)
                && actual.process_id == expected.process_id
        });
        if !verified {
            return Err(host_error(
                "WINDOW_FOCUS_CONFLICT",
                "target did not own foreground after activation",
                json!({"expected":expected,"actual":foreground,"activation":result}),
            ));
        }
        if let Ok(session) = self.session_mut(session_id) {
            if let Some(window) = session
                .state
                .windows
                .iter_mut()
                .find(|window| window.window_id == expected.window_id)
            {
                window.visible = true;
            }
            session.state.active_window = Some(expected.window_id.clone());
        }
        Ok(
            json!({"activated":true,"verified":true,"window":expected,"foreground":foreground,"result":result}),
        )
    }

    fn inspect_compact(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let tree = self.inspect_tree(session_id)?;
        let window = self.resolve_window(session_id, None, None)?;
        let compact = arguments
            .get("compact")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let ids_only = arguments
            .get("automation_ids_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let visible_only = arguments
            .get("visible_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let interactable_only = arguments
            .get("interactable_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let structured_only = arguments
            .get("structured_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let exact_id = arguments.get("automation_id").and_then(Value::as_str);
        let prefix = arguments
            .get("automation_id_prefix")
            .and_then(Value::as_str);
        let control_type = arguments.get("control_type").and_then(Value::as_str);
        let max_depth = arguments
            .get("max_depth")
            .and_then(Value::as_u64)
            .map(|value| value as usize);
        let depths = element_depths(&tree);

        let mut elements = Vec::new();
        for control in &tree.elements {
            let interactability = interactability(control, &window, &self.config);
            let depth = depths.get(&control.element_id).copied().unwrap_or(0);
            if visible_only && !control.visible
                || interactable_only && !interactability.interactable
                || exact_id.is_some_and(|id| control.automation_id != id)
                || prefix.is_some_and(|value| !control.automation_id.starts_with(value))
                || control_type
                    .is_some_and(|value| !control.control_type.eq_ignore_ascii_case(value))
                || max_depth.is_some_and(|value| depth > value)
                || structured_only && !is_structured(control)
            {
                continue;
            }
            if ids_only {
                elements.push(json!({
                    "automation_id":control.automation_id,
                    "role":control.control_type,
                    "visible":control.visible,
                    "enabled":control.enabled,
                    "selected":control.selected,
                    "interactable":interactability.interactable,
                    "depth":depth,
                }));
            } else if compact {
                elements.push(json!({
                    "automation_id":control.automation_id,
                    "role":control.control_type,
                    "name":control.name,
                    "value":control.value,
                    "bounds":control.bounds,
                    "visible":control.visible,
                    "enabled":control.enabled,
                    "focused":control.focused,
                    "selected":control.selected,
                    "interactable":interactability.interactable,
                    "depth":depth,
                }));
            } else {
                let mut value =
                    serde_json::to_value(control).map_err(|error| invalid(error.to_string()))?;
                if let Some(object) = value.as_object_mut() {
                    object.insert("interactable".into(), json!(interactability.interactable));
                    object.insert("interactability".into(), json!(interactability));
                    object.insert("depth".into(), json!(depth));
                }
                elements.push(value);
            }
        }
        Ok(json!({
            "session_id":session_id,
            "window_id":tree.window_id,
            "backend":tree.backend,
            "compact":compact,
            "automation_ids_only":ids_only,
            "elements":elements,
            "matched":elements.len(),
            "total_enumerated":tree.total_enumerated,
            "truncated":tree.truncated,
        }))
    }

    fn verified_input(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let requested_id = required(arguments, "id")?;
        let requested = arguments
            .get("value")
            .or_else(|| arguments.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("value/text must be a string"))?
            .to_owned();
        let id = self
            .resolve_control(session_id, &requested_id)?
            .automation_id;
        let dispatch = {
            let session = self.session_mut(session_id)?;
            call_host(
                &mut session.host,
                "input",
                json!({"id":id,"text":requested}),
            )
        };
        if let Err(dispatch_error) = dispatch {
            return self.prefer_runtime_failure(session_id, Some(dispatch_error));
        }
        let timeout = duration(arguments, self.config.default_timeout_ms);
        let poll = poll_duration(arguments, self.config.default_poll_ms);
        let deadline = Instant::now() + timeout;
        let mut actual = None;
        let mut state_value = Value::Null;
        loop {
            if let Ok(state) = self.read_state(session_id, &id) {
                actual = state.value.clone().or(state.text.clone());
                state_value = json!(state);
                if actual.as_deref() == Some(requested.as_str()) {
                    break;
                }
            }
            if Instant::now() >= deadline {
                return Err(host_error(
                    "VALUE_VERIFICATION_FAILED",
                    "SetValue was dispatched but read-back did not match",
                    json!({
                        "dispatched":true,
                        "id":id,
                        "requested":requested,
                        "actual":actual,
                        "verified":false,
                        "state":state_value,
                        "timeout_ms":timeout.as_millis()
                    }),
                ));
            }
            thread::sleep(poll);
        }
        let health = self.ensure_runtime_healthy(session_id, true)?;
        let final_state = self.read_state(session_id, &id)?;
        actual = final_state.value.clone().or(final_state.text.clone());
        state_value = json!(final_state);
        if actual.as_deref() != Some(requested.as_str()) {
            return Err(host_error(
                "VALUE_VERIFICATION_FAILED",
                "SetValue read-back changed during the post-action settle window",
                json!({
                    "dispatched":true,
                    "id":id,
                    "requested":requested,
                    "actual":actual,
                    "verified":false,
                    "state":state_value,
                    "runtime_health":health
                }),
            ));
        }
        Ok(json!({
            "dispatched":true,
            "requested":requested,
            "actual":actual,
            "verified":true,
            "target_state_verified":true,
            "application_healthy":health.healthy,
            "state":state_value,
            "runtime_health":health,
        }))
    }

    fn verified_click(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let requested_id = required(arguments, "id")?;
        self.ensure_runtime_healthy(session_id, false)?;
        let control = self.resolve_control(session_id, &requested_id)?;
        let id = control.automation_id.clone();
        let window = self.resolve_window(session_id, None, None)?;
        let target_check = interactability(&control, &window, &self.config);
        if !target_check.interactable {
            return Err(host_error(
                "ELEMENT_NOT_INTERACTABLE",
                "click target failed visibility/enabled/bounds sanity",
                json!({"id":id,"control":control,"interactability":target_check}),
            ));
        }
        let semantic = control.capabilities.invoke
            || control.capabilities.select
            || control.capabilities.native_action;
        if !semantic {
            let token = arguments
                .get("input_lock_token")
                .and_then(Value::as_str)
                .unwrap_or_default();
            self.activate_window(session_id, token)?;
        }
        let pre_action_screenshot = self.screenshot_window(session_id).ok();
        let baseline = self.read_state(session_id, &id).ok();
        let postcondition = arguments.get("postcondition").cloned();
        let dispatch = {
            let session = self.session_mut(session_id)?;
            call_host(&mut session.host, "click", json!({"id":id}))
        };
        let dispatch = match dispatch {
            Ok(value) => value,
            Err(error) => {
                return self
                    .prefer_runtime_failure(session_id, Some(error))
                    .map_err(|error| {
                        attach_diagnostic(
                            error,
                            "pre_action_screenshot",
                            json!(pre_action_screenshot),
                        )
                    });
            }
        };

        let observed = if let Some(condition) = postcondition.as_ref() {
            let timeout = duration(arguments, self.config.default_timeout_ms);
            let poll = poll_duration(arguments, self.config.default_poll_ms);
            let deadline = Instant::now() + timeout;
            loop {
                let observed =
                    match self.evaluate_postcondition(session_id, condition, baseline.as_ref()) {
                        Ok((true, value)) => break value,
                        Ok((false, value)) => value,
                        Err(error) => json!({"evaluation_error":error}),
                    };
                if Instant::now() >= deadline {
                    let health = self
                        .session_mut(session_id)?
                        .host
                        .test_host_mut()
                        .get_runtime_health();
                    if !health.healthy {
                        return Err(attach_diagnostic(
                            runtime_failure_error(&health, Some(json!(dispatch))),
                            "pre_action_screenshot",
                            json!(pre_action_screenshot),
                        ));
                    }
                    return Err(host_error(
                        "ACTION_POSTCONDITION_FAILED",
                        "click was dispatched but its postcondition was not observed",
                        json!({
                            "dispatched":true,
                            "id":id,
                            "postcondition":condition,
                            "observed":observed,
                            "timeout_ms":timeout.as_millis(),
                            "runtime_health":health,
                            "pre_action_screenshot":pre_action_screenshot
                        }),
                    ));
                }
                thread::sleep(poll);
            }
        } else {
            self.read_state(session_id, &id)
                .map(|state| json!(state))
                .unwrap_or_else(|error| json!({"target_after_error":error}))
        };
        let health = match self.ensure_runtime_healthy(session_id, true) {
            Ok(health) => health,
            Err(error) => {
                return Err(attach_diagnostic(
                    error,
                    "pre_action_screenshot",
                    json!(pre_action_screenshot),
                ));
            }
        };
        Ok(json!({
            "action":"click",
            "id":id,
            "dispatched":true,
            "target_state_verified":true,
            "postcondition":postcondition,
            "postcondition_verified":true,
            "application_healthy":health.healthy,
            "semantic_action":semantic,
            "dispatch_result":dispatch,
            "observed":observed,
            "pre_action_screenshot":pre_action_screenshot,
            "runtime_health":health,
        }))
    }

    fn wait_for_window(&mut self, arguments: &Value) -> HostResult<Value> {
        let query = parse_window_query(arguments)?;
        let timeout = duration(arguments, self.config.default_timeout_ms);
        let poll = poll_duration(arguments, self.config.default_poll_ms);
        let deadline = Instant::now() + timeout;
        loop {
            let last = match self.find_window(&query) {
                Ok(window) => return Ok(json!(window)),
                Err(error) if error.code == "AMBIGUOUS_WINDOW" => return Err(error),
                Err(error) => json!(error),
            };
            if Instant::now() >= deadline {
                return Err(wait_timeout(
                    "wait_for_window",
                    timeout,
                    poll,
                    json!({"query":query,"last":last}),
                ));
            }
            thread::sleep(poll);
        }
    }

    fn wait_for_element(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let id = required_any(arguments, &["id", "automation_id"])?;
        let timeout = duration(arguments, self.config.default_timeout_ms);
        let poll = poll_duration(arguments, self.config.default_poll_ms);
        let deadline = Instant::now() + timeout;
        loop {
            let last = match self.resolve_control(session_id, &id) {
                Ok(control) => return Ok(json!(control)),
                Err(error) => json!(error),
            };
            if Instant::now() >= deadline {
                self.ensure_runtime_healthy(session_id, false)?;
                return Err(wait_timeout(
                    "wait_for_element",
                    timeout,
                    poll,
                    json!({"id":id,"last":last}),
                ));
            }
            self.ensure_runtime_healthy(session_id, false)?;
            thread::sleep(poll);
        }
    }

    fn wait_for_value(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let id = required(arguments, "id")?;
        let expected = required_text(arguments, "value")?;
        let contains = arguments
            .get("contains")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let timeout = duration(arguments, self.config.default_timeout_ms);
        let poll = poll_duration(arguments, self.config.default_poll_ms);
        let deadline = Instant::now() + timeout;
        let mut last = Value::Null;
        loop {
            if let Ok(state) = self.read_state(session_id, &id) {
                let actual = state.value.as_deref().or(state.text.as_deref());
                last = json!(state);
                if actual.is_some_and(|actual| {
                    if contains {
                        actual.contains(&expected)
                    } else {
                        actual == expected
                    }
                }) {
                    return Ok(last);
                }
            }
            if Instant::now() >= deadline {
                self.ensure_runtime_healthy(session_id, false)?;
                return Err(wait_timeout(
                    "wait_for_value",
                    timeout,
                    poll,
                    json!({"id":id,"expected":expected,"contains":contains,"last":last}),
                ));
            }
            self.ensure_runtime_healthy(session_id, false)?;
            thread::sleep(poll);
        }
    }

    fn wait_for_state(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let id = required(arguments, "id")?;
        let expected = arguments
            .get("state")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("state must be an object"))?
            .clone();
        let timeout = duration(arguments, self.config.default_timeout_ms);
        let poll = poll_duration(arguments, self.config.default_poll_ms);
        let deadline = Instant::now() + timeout;
        let mut last = Value::Null;
        loop {
            if let Ok(state) = self.read_state(session_id, &id) {
                let window = self.resolve_window(session_id, None, None)?;
                let control = self.resolve_control(session_id, &id)?;
                let interaction = interactability(&control, &window, &self.config);
                last = json!({"state":state,"interactability":interaction});
                if state_matches(&state, &interaction, &expected) {
                    return Ok(last);
                }
            }
            if Instant::now() >= deadline {
                self.ensure_runtime_healthy(session_id, false)?;
                return Err(wait_timeout(
                    "wait_for_state",
                    timeout,
                    poll,
                    json!({"id":id,"expected":expected,"last":last}),
                ));
            }
            self.ensure_runtime_healthy(session_id, false)?;
            thread::sleep(poll);
        }
    }

    fn wait_until(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let condition = arguments
            .get("condition")
            .or_else(|| arguments.get("postcondition"))
            .ok_or_else(|| invalid("condition/postcondition is required"))?
            .clone();
        let timeout = duration(arguments, self.config.default_timeout_ms);
        let poll = poll_duration(arguments, self.config.default_poll_ms);
        let deadline = Instant::now() + timeout;
        loop {
            let last = match self.evaluate_postcondition(session_id, &condition, None) {
                Ok((true, observed)) => return Ok(observed),
                Ok((false, observed)) => observed,
                Err(error) => json!({"evaluation_error":error}),
            };
            if Instant::now() >= deadline {
                self.ensure_runtime_healthy(session_id, false)?;
                return Err(wait_timeout(
                    "wait_until",
                    timeout,
                    poll,
                    json!({"condition":condition,"last":last}),
                ));
            }
            self.ensure_runtime_healthy(session_id, false)?;
            thread::sleep(poll);
        }
    }

    fn wait_idle(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let value = {
            let session = self.session_mut(session_id)?;
            call_host(&mut session.host, "wait_idle", arguments.clone())?
        };
        let health = self.ensure_runtime_healthy(session_id, false)?;
        Ok(json!({"wait":value,"runtime_health":health}))
    }

    fn assert_control(
        &mut self,
        session_id: &str,
        arguments: &Value,
        assertion: &str,
    ) -> HostResult<Value> {
        let id = required(arguments, "id")?;
        let control = self.resolve_control(session_id, &id)?;
        let window = self.resolve_window(session_id, None, None)?;
        let interaction = interactability(&control, &window, &self.config);
        let passed = match assertion {
            "visible" => control.visible,
            "interactable" => interaction.interactable,
            "valid_bounds" => control.bounds.width > 0 && control.bounds.height > 0,
            _ => false,
        };
        if !passed {
            return Err(host_error(
                "ASSERTION_FAILED",
                format!("{assertion} assertion failed for {id}"),
                json!({
                    "assertion":assertion,
                    "control":control,
                    "interactability":interaction,
                    "custom_widget_positive_bounds":control.bounds.width > 0 && control.bounds.height > 0
                }),
            ));
        }
        Ok(json!({
            "assertion":assertion,
            "passed":true,
            "control":control,
            "interactability":interaction,
            "custom_widget_positive_bounds":control.bounds.width > 0 && control.bounds.height > 0
        }))
    }

    fn screenshot_window(&mut self, session_id: &str) -> HostResult<Value> {
        let window = self.resolve_window(session_id, None, None)?;
        let expected = Self::capture_target_window(window)?;
        let capture = {
            let session = self.session_mut(session_id)?;
            call_host(&mut session.host, "screenshot", json!({}))?
        };
        let actual_window = capture.get("window_id").and_then(Value::as_str);
        if actual_window != Some(expected.window_id.as_str()) {
            return Err(host_error(
                "WINDOW_SESSION_MISMATCH",
                "capture result did not belong to the session's active window",
                json!({"expected":expected.window_id,"actual":actual_window,"capture":capture}),
            ));
        }
        let path = capture
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("capture result has no path"))?;
        let stats = inspect_png(Path::new(path))?;
        if stats.blank {
            return Err(host_error(
                "SCREENSHOT_EMPTY",
                "window capture is completely black or empty after backend fallback",
                json!({"capture":capture,"stats":stats}),
            ));
        }
        Ok(json!({"capture":capture,"path":path,"verified_window":expected,"image":stats}))
    }

    fn capture_target_window(window: SessionWindow) -> HostResult<SessionWindow> {
        let native_visible = native_window_visible(&window.window_handle);
        if window.visible && native_visible.unwrap_or(true) {
            Ok(window)
        } else {
            Err(host_error(
                "WINDOW_NOT_FOUND",
                "session active window is hidden or no longer a visible native window",
                json!({"window":window,"native_window_visible":native_visible}),
            ))
        }
    }

    fn screenshot_element(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let id = required(arguments, "id")?;
        let control = self.resolve_control(session_id, &id)?;
        if control.bounds.width <= 0 || control.bounds.height <= 0 {
            return Err(host_error(
                "INVALID_BOUNDS",
                "element has no positive bounds to crop",
                json!({"id":id,"bounds":control.bounds}),
            ));
        }
        let window = self.resolve_window(session_id, None, None)?;
        let capture = self.screenshot_window(session_id)?;
        let source = capture
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("window capture path is missing"))?;
        let output = arguments
            .get("output_path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                self.output_root
                    .join(safe_path_segment(session_id))
                    .join(format!(
                        "element-{}-{}.png",
                        safe_path_segment(&id),
                        now_unix_ms()
                    ))
            });
        let relative = Rect {
            x: control.bounds.x - window.bounds.x,
            y: control.bounds.y - window.bounds.y,
            width: control.bounds.width,
            height: control.bounds.height,
        };
        let stats = crop_png(Path::new(source), &output, relative)?;
        Ok(json!({
            "session_id":session_id,
            "window_id":window.window_id,
            "element_id":id,
            "element_bounds":control.bounds,
            "source_window_capture":source,
            "path":output,
            "image":stats,
        }))
    }

    fn capture_region(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let window = self.resolve_window(session_id, None, None)?;
        let (
            region,
            element_id,
            element_bounds,
            relative_region,
            resolved_screen_region,
            coordinate_space,
        ) = if arguments.get("id").is_some() || arguments.get("region").is_some() {
            let id = required(arguments, "id")?;
            let control = self.resolve_control(session_id, &id)?;
            let (relative_region, screen_region, image_region) =
                Self::resolve_element_relative_capture_region(
                    arguments,
                    control.bounds,
                    window.bounds,
                )?;
            (
                image_region,
                Some(id),
                Some(control.bounds),
                Some(relative_region),
                Some(screen_region),
                "element_relative",
            )
        } else {
            // Preserve the pre-public-contract absolute crop shape for internal/legacy callers.
            let coordinate_space = arguments
                .get("coordinate_space")
                .and_then(Value::as_str)
                .unwrap_or("screen");
            let mut image_region = Rect {
                x: integer(arguments, "x")?,
                y: integer(arguments, "y")?,
                width: integer(arguments, "width")?,
                height: integer(arguments, "height")?,
            };
            if coordinate_space == "screen" {
                image_region.x -= window.bounds.x;
                image_region.y -= window.bounds.y;
            }
            (image_region, None, None, None, None, coordinate_space)
        };
        let capture = self.screenshot_window(session_id)?;
        let source = capture
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("window capture path is missing"))?;
        let output = arguments
            .get("output_path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                self.output_root
                    .join(safe_path_segment(session_id))
                    .join(format!("region-{}.png", now_unix_ms()))
            });
        let stats = crop_png(Path::new(source), &output, region)?;
        Ok(json!({
            "session_id":session_id,
            "window_id":window.window_id,
            "source_window_capture":source,
            "region":region,
            "element_id":element_id,
            "element_bounds":element_bounds,
            "relative_region":relative_region,
            "resolved_screen_region":resolved_screen_region,
            "coordinate_space":coordinate_space,
            "path":output,
            "image":stats,
        }))
    }

    fn resolve_element_relative_capture_region(
        arguments: &Value,
        element_bounds: Rect,
        window_bounds: Rect,
    ) -> HostResult<(RelativeRect, Rect, Rect)> {
        let relative_region = serde_json::from_value::<RelativeRect>(
            arguments
                .get("region")
                .cloned()
                .ok_or_else(|| invalid("region is required"))?,
        )
        .map_err(|error| invalid(format!("region must be an element-relative rect: {error}")))?;
        let screen_region = relative_region.resolve(element_bounds).map_err(|error| {
            invalid(format!(
                "invalid element-relative capture region: {error:?}"
            ))
        })?;
        let image_region = Rect {
            x: screen_region.x - window_bounds.x,
            y: screen_region.y - window_bounds.y,
            width: screen_region.width,
            height: screen_region.height,
        };
        Ok((relative_region, screen_region, image_region))
    }

    fn canvas_health(
        &mut self,
        session_id: &str,
        arguments: &Value,
    ) -> HostResult<CanvasHealthReport> {
        let id = required(arguments, "id")?;
        let control = self.resolve_control(session_id, &id)?;
        let (health, reason) = if control.bounds.width <= 0 || control.bounds.height <= 0 {
            (
                CanvasHealth::ZeroViewport,
                "semantic viewport has zero width or height",
            )
        } else if !control.visible && control.offscreen {
            (
                CanvasHealth::EmptyClip,
                "control has positive bounds but UIA reports an empty/offscreen clip",
            )
        } else if !control.visible {
            (CanvasHealth::Hidden, "control is hidden")
        } else if control.offscreen {
            (
                CanvasHealth::ContentOutsideViewport,
                "control bounds are outside the visible viewport",
            )
        } else {
            (
                CanvasHealth::Healthy,
                "viewport, visibility and bounds are valid; draw-call telemetry is not published",
            )
        };
        Ok(CanvasHealthReport {
            id,
            health,
            bounds: control.bounds,
            visible: control.visible,
            offscreen: control.offscreen,
            draw_called: None,
            reason: reason.into(),
        })
    }

    fn list_automation_ids(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let mut args = arguments.clone();
        if let Some(object) = args.as_object_mut() {
            object.insert("automation_ids_only".into(), Value::Bool(true));
            object.insert("compact".into(), Value::Bool(true));
        }
        self.inspect_compact(session_id, &args)
    }

    fn assert_automation_ids(&mut self, session_id: &str, arguments: &Value) -> HostResult<Value> {
        let expected = arguments
            .get("expected")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("expected must be an array of automation IDs"))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("expected must contain strings"))
            })
            .collect::<HostResult<Vec<_>>>()?;
        let tree = self.inspect_tree(session_id)?;
        let actual = tree
            .elements
            .iter()
            .map(|control| control.automation_id.clone())
            .filter(|id| !id.is_empty())
            .collect::<BTreeSet<_>>();
        let missing = expected
            .iter()
            .filter(|expected| {
                !actual.contains(*expected)
                    && !actual.iter().any(|id| id.ends_with(expected.as_str()))
            })
            .cloned()
            .collect::<Vec<_>>();
        let report = json!({"expected":expected,"missing":missing,"actual_count":actual.len()});
        if !missing.is_empty() {
            return Err(host_error(
                "AUTOMATION_ID_COVERAGE_FAILED",
                "one or more expected automation IDs are missing",
                report,
            ));
        }
        Ok(report)
    }

    fn delegate_advanced(
        &mut self,
        session_id: &str,
        operation: &str,
        arguments: &Value,
    ) -> HostResult<Value> {
        let resolved_control = match arguments.get("id").and_then(Value::as_str) {
            Some(id) => match self.resolve_control(session_id, id) {
                Ok(control) => Some(control),
                Err(error)
                    if operation_allows_absent_target(operation)
                        && error.code == "ELEMENT_NOT_FOUND" =>
                {
                    None
                }
                Err(error) => return Err(error),
            },
            None => None,
        };
        let host_arguments = resolved_control
            .as_ref()
            .map(|control| Self::canonical_control_arguments(arguments, control))
            .transpose()?
            .unwrap_or_else(|| arguments.clone());
        if is_physical_operation(operation) {
            let token = arguments
                .get("input_lock_token")
                .and_then(Value::as_str)
                .unwrap_or_default();
            self.activate_window(session_id, token)?;
            if let Some(control) = resolved_control.as_ref() {
                let window = self.resolve_window(session_id, None, None)?;
                let interaction = interactability(control, &window, &self.config);
                if !interaction.interactable
                    && !physical_operation_allows_non_interactable_target(operation)
                {
                    return Err(host_error(
                        "ELEMENT_NOT_INTERACTABLE",
                        "physical target failed pre-action verification",
                        json!({"id":control.automation_id,"interactability":interaction}),
                    ));
                }
            }
        }
        let value = {
            let session = self.session_mut(session_id)?;
            call_host(&mut session.host, operation, host_arguments)
        };
        let value = match value {
            Ok(value) => value,
            Err(error) => return self.prefer_runtime_failure(session_id, Some(error)),
        };
        if is_action_operation(operation) {
            let health = self.ensure_runtime_healthy(session_id, true)?;
            Ok(json!({
                "dispatched":true,
                "target_state_verified":true,
                "application_healthy":health.healthy,
                "result":value,
                "runtime_health":health
            }))
        } else {
            Ok(value)
        }
    }

    fn run_sequence(&mut self, arguments: &Value) -> HostResult<Value> {
        let session_id = required(arguments, "session_id")?;
        self.session(&session_id)?;
        let steps = arguments
            .get("steps")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("steps must be an array"))?
            .clone();
        let stop_on_error = arguments
            .get("stop_on_error")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let retries = arguments.get("retry").and_then(Value::as_u64).unwrap_or(0) as usize;
        let default_timeout = arguments.get("timeout_ms").cloned();
        let wait_after = arguments
            .get("wait_after_action_ms")
            .or_else(|| arguments.get("wait_after_action"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let evidence_on_failure = arguments
            .get("evidence_on_failure")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let mut results = Vec::new();
        let mut failures = 0;

        for (index, step) in steps.into_iter().enumerate() {
            let mut step_object = step
                .as_object()
                .cloned()
                .ok_or_else(|| invalid(format!("sequence step {index} must be an object")))?;
            let operation = step_object
                .remove("op")
                .and_then(|value| value.as_str().map(str::to_owned))
                .ok_or_else(|| invalid(format!("sequence step {index} has no op")))?;
            step_object.insert("session_id".into(), Value::String(session_id.clone()));
            if !step_object.contains_key("timeout_ms") {
                if let Some(timeout) = default_timeout.clone() {
                    step_object.insert("timeout_ms".into(), timeout);
                }
            }
            let mut attempts = Vec::new();
            let mut succeeded = false;
            for attempt in 0..=retries {
                let response = self.call(&operation, Value::Object(step_object.clone()));
                succeeded = response["success"] == true;
                attempts.push(json!({"attempt":attempt + 1,"response":response}));
                if succeeded {
                    break;
                }
            }
            results.push(json!({
                "index":index,
                "op":operation,
                "success":succeeded,
                "attempts":attempts
            }));
            if !succeeded {
                failures += 1;
                if evidence_on_failure {
                    let _ = self.export_evidence(&session_id, &format!("sequence-step-{index}"));
                }
                if stop_on_error {
                    break;
                }
            } else if wait_after > 0 {
                thread::sleep(Duration::from_millis(wait_after));
            }
        }
        if failures > 0 {
            return Err(host_error(
                "SEQUENCE_FAILED",
                format!("{failures} sequence step(s) failed"),
                json!({"session_id":session_id,"results":results}),
            ));
        }
        Ok(json!({"session_id":session_id,"success":true,"steps":results}))
    }

    fn evaluate_postcondition(
        &mut self,
        session_id: &str,
        condition: &Value,
        baseline: Option<&ElementState>,
    ) -> HostResult<(bool, Value)> {
        let object = condition
            .as_object()
            .ok_or_else(|| invalid("postcondition must be an object"))?;
        if let Some(id) = string_value(object, &["automation_id_visible", "element_appears"]) {
            return match self.resolve_control(session_id, id) {
                Ok(control) => Ok((control.visible, json!(control))),
                Err(error) if error.code == "ELEMENT_NOT_FOUND" => {
                    Ok((false, json!({"id":id,"exists":false})))
                }
                Err(error) => Err(error),
            };
        }
        if let Some(id) = string_value(object, &["automation_id_hidden", "element_disappears"]) {
            return match self.resolve_control(session_id, id) {
                Ok(control) => Ok((!control.visible, json!(control))),
                Err(error) if error.code == "ELEMENT_NOT_FOUND" => {
                    Ok((true, json!({"id":id,"exists":false})))
                }
                Err(error) => Err(error),
            };
        }
        if let Some(spec) = object.get("value_changes") {
            let id = required(spec, "id")?;
            let state = self.read_state(session_id, &id)?;
            let actual = state.value.clone().or(state.text.clone());
            let expected = spec
                .get("to")
                .or_else(|| spec.get("value"))
                .and_then(Value::as_str);
            let changed = expected.map_or_else(
                || {
                    baseline.and_then(|state| state.value.as_ref().or(state.text.as_ref()))
                        != actual.as_ref()
                },
                |expected| actual.as_deref() == Some(expected),
            );
            return Ok((changed, json!({"id":id,"actual":actual,"state":state})));
        }
        if let Some(spec) = object.get("enabled_changes") {
            let id = required(spec, "id")?;
            let state = self.read_state(session_id, &id)?;
            let expected = spec.get("to").and_then(Value::as_bool);
            let changed = expected.map_or_else(
                || baseline.is_some_and(|before| before.enabled != state.enabled),
                |expected| state.enabled == expected,
            );
            return Ok((changed, json!(state)));
        }
        if let Some(spec) = object.get("selected_changes") {
            let id = required(spec, "id")?;
            let state = self.read_state(session_id, &id)?;
            let expected = spec.get("to").and_then(Value::as_bool);
            let changed = expected.map_or_else(
                || baseline.is_some_and(|before| before.selected != state.selected),
                |expected| state.selected == Some(expected),
            );
            return Ok((changed, json!(state)));
        }
        if let Some(spec) = object.get("window_appears") {
            let query = parse_window_query(spec)?;
            return match self.find_window(&query) {
                Ok(window) => Ok((true, json!(window))),
                Err(error) if error.code == "WINDOW_NOT_FOUND" => Ok((false, json!(error))),
                Err(error) => Err(error),
            };
        }
        if let Some(spec) = object.get("window_closes") {
            let query = if let Some(window_id) = spec.as_str() {
                let session_ids = self.sessions.keys().cloned().collect::<Vec<_>>();
                for session_id in session_ids {
                    self.refresh_session_windows(&session_id)?;
                }
                let exists = self.sessions.values().any(|session| {
                    session
                        .state
                        .windows
                        .iter()
                        .any(|window| window.window_id == window_id)
                });
                return Ok((!exists, json!({"window_id":window_id,"exists":exists})));
            } else {
                parse_window_query(spec)?
            };
            return match self.find_window(&query) {
                Ok(window) => Ok((false, json!(window))),
                Err(error) if error.code == "WINDOW_NOT_FOUND" => Ok((true, json!(error))),
                Err(error) => Err(error),
            };
        }
        Err(invalid("unsupported postcondition"))
    }

    fn inspect_tree(&mut self, session_id: &str) -> HostResult<ControlTreeResult> {
        let session = self.session_mut(session_id)?;
        let value = call_host(&mut session.host, "inspect_ui", json!({}))?;
        serde_json::from_value::<ControlTreeResult>(value)
            .map_err(|error| invalid(error.to_string()))
    }

    fn resolve_control(&mut self, session_id: &str, id: &str) -> HostResult<ControlInfo> {
        let tree = self.inspect_tree(session_id)?;
        if let Some(control) = tree
            .elements
            .iter()
            .find(|control| control.automation_id == id || control.element_id == id)
        {
            return Ok(control.clone());
        }
        let suffix = tree
            .elements
            .iter()
            .filter(|control| control.automation_id.ends_with(id))
            .cloned()
            .collect::<Vec<_>>();
        if suffix.len() == 1 {
            return Ok(suffix[0].clone());
        }
        if suffix.len() > 1 {
            return Err(host_error(
                "AMBIGUOUS_ELEMENT",
                format!("multiple automation IDs end with {id}"),
                json!({"requested":id,"candidates":suffix}),
            ));
        }
        let similar = tree
            .elements
            .iter()
            .filter(|control| {
                control
                    .automation_id
                    .to_lowercase()
                    .contains(&id.to_lowercase())
                    || control.name.to_lowercase().contains(&id.to_lowercase())
            })
            .take(20)
            .map(|control| control.automation_id.clone())
            .collect::<Vec<_>>();
        Err(host_error(
            "ELEMENT_NOT_FOUND",
            format!("no semantic element matched {id}"),
            json!({"requested":id,"similar":similar,"session_id":session_id}),
        ))
    }

    fn canonical_control_arguments(arguments: &Value, control: &ControlInfo) -> HostResult<Value> {
        let mut canonical = arguments.clone();
        let object = canonical
            .as_object_mut()
            .ok_or_else(|| invalid("operation arguments must be an object"))?;
        object.insert("id".into(), Value::String(control.automation_id.clone()));
        Ok(canonical)
    }

    fn read_state(&mut self, session_id: &str, id: &str) -> HostResult<ElementState> {
        let automation_id = self.resolve_control(session_id, id)?.automation_id;
        let value = {
            let session = self.session_mut(session_id)?;
            call_host(&mut session.host, "read", json!({"id":automation_id}))?
        };
        serde_json::from_value(value).map_err(|error| invalid(error.to_string()))
    }

    fn ensure_runtime_healthy(
        &mut self,
        session_id: &str,
        settle: bool,
    ) -> HostResult<RuntimeHealth> {
        if settle {
            thread::sleep(Duration::from_millis(self.config.runtime_settle_ms));
        }
        let health = self
            .session_mut(session_id)?
            .host
            .test_host_mut()
            .get_runtime_health();
        if health.healthy {
            Ok(health)
        } else {
            Err(runtime_failure_error(&health, None))
        }
    }

    fn prefer_runtime_failure(
        &mut self,
        session_id: &str,
        fallback: Option<HostError>,
    ) -> HostResult<Value> {
        thread::sleep(Duration::from_millis(self.config.runtime_settle_ms));
        let health = self
            .session_mut(session_id)?
            .host
            .test_host_mut()
            .get_runtime_health();
        if !health.healthy {
            Err(runtime_failure_error(
                &health,
                fallback.as_ref().map(|error| json!(error)),
            ))
        } else {
            Err(fallback.unwrap_or_else(|| {
                host_error(
                    "ACTION_FAILED",
                    "action failed while application remained healthy",
                    Value::Null,
                )
            }))
        }
    }

    fn require_input_lock(&self, session_id: &str, token: &str) -> HostResult<()> {
        let state = input_lock().lock().map_err(|_| {
            host_error(
                "INPUT_LOCK_FAILURE",
                "physical input lock is poisoned",
                Value::Null,
            )
        })?;
        let valid = state
            .lease
            .as_ref()
            .is_some_and(|lease| lease.session_id == session_id && lease.token == token);
        if valid {
            Ok(())
        } else {
            Err(host_error(
                "WINDOW_FOCUS_CONFLICT",
                "physical input requires an input lock owned by the target session",
                json!({"requested_session":session_id,"lock_owner":state.lease}),
            ))
        }
    }

    fn record_action(
        &mut self,
        session_id: &str,
        operation: &str,
        action_id: String,
        target: Option<String>,
        result: Value,
        postcondition: Option<Value>,
    ) {
        let Some(session) = self.sessions.get_mut(session_id) else {
            return;
        };
        let process_id = session
            .state
            .processes
            .first()
            .map(|process| process.process_id);
        let window_id = session.state.active_window.clone();
        let runtime_health = if operation == "session.close" {
            None
        } else {
            Some(session.host.test_host_mut().get_runtime_health())
        };
        session.state.action_log.push(ActionRecord {
            action_id,
            timestamp_unix_ms: now_unix_ms(),
            session_id: session_id.to_owned(),
            process_id,
            window_id,
            action: operation.to_owned(),
            target,
            result,
            postcondition,
            runtime_health,
        });
    }

    fn session(&self, session_id: &str) -> HostResult<&ManagedSession> {
        self.sessions.get(session_id).ok_or_else(|| {
            host_error(
                "SESSION_NOT_FOUND",
                format!("automation session {session_id} was not found"),
                json!({"session_id":session_id}),
            )
        })
    }

    fn session_mut(&mut self, session_id: &str) -> HostResult<&mut ManagedSession> {
        self.sessions.get_mut(session_id).ok_or_else(|| {
            host_error(
                "SESSION_NOT_FOUND",
                format!("automation session {session_id} was not found"),
                json!({"session_id":session_id}),
            )
        })
    }
}

impl Drop for ReliabilityManager {
    fn drop(&mut self) {
        let session_ids = self.sessions.keys().cloned().collect::<Vec<_>>();
        for session_id in session_ids {
            let _ = self.close_session(&session_id);
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct ForegroundWindow {
    window_handle: String,
    process_id: u32,
}

#[cfg(windows)]
fn foreground_window() -> Option<ForegroundWindow> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
    Some(ForegroundWindow {
        window_handle: format!("0x{:X}", hwnd.0 as usize),
        process_id,
    })
}

#[cfg(not(windows))]
fn foreground_window() -> Option<ForegroundWindow> {
    None
}

#[cfg(windows)]
fn native_window_visible(window_handle: &str) -> Option<bool> {
    let raw = window_handle
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    let value = usize::from_str_radix(raw, 16).ok()?;
    let hwnd = HWND(value as *mut std::ffi::c_void);
    Some(unsafe { IsWindowVisible(hwnd).as_bool() })
}

#[cfg(not(windows))]
fn native_window_visible(_window_handle: &str) -> Option<bool> {
    None
}

fn wait_for_initial_tree(
    host: &mut AdvancedHost,
    timeout: Duration,
) -> HostResult<ControlTreeResult> {
    let deadline = Instant::now() + timeout;
    let mut last = None;
    loop {
        match call_host(host, "inspect_ui", json!({})).and_then(|value| {
            serde_json::from_value::<ControlTreeResult>(value)
                .map_err(|error| invalid(error.to_string()))
        }) {
            Ok(tree) if !tree.elements.is_empty() => return Ok(tree),
            Ok(_) => {}
            Err(error) => last = Some(error),
        }
        if Instant::now() >= deadline {
            return Err(last.unwrap_or_else(|| {
                host_error(
                    "WAIT_TIMEOUT",
                    "Makepad semantic root was not published before timeout",
                    json!({"timeout_ms":timeout.as_millis()}),
                )
            }));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn call_host(host: &mut AdvancedHost, operation: &str, arguments: Value) -> HostResult<Value> {
    decode_host_response(host.call(operation, arguments))
}

fn decode_host_response(response: Value) -> HostResult<Value> {
    if response["success"] == true {
        Ok(response["result"].clone())
    } else {
        let error =
            serde_json::from_value::<HostError>(response["error"].clone()).map_err(|error| {
                host_error(
                    "HOST_PROTOCOL_ERROR",
                    format!("host error response could not be decoded: {error}"),
                    response,
                )
            })?;
        Err(error)
    }
}

fn interactability(
    control: &ControlInfo,
    window: &SessionWindow,
    config: &ReliabilityConfig,
) -> Interactability {
    let valid_bounds = control.bounds.width >= config.minimum_interactable_width
        && control.bounds.height >= config.minimum_interactable_height;
    let mut reasons = Vec::new();
    if !control.visible {
        reasons.push("semantic visibility is false".into());
    }
    if !control.enabled {
        reasons.push("control is disabled".into());
    }
    if control.offscreen {
        reasons.push("UIA reports offscreen/clipped control".into());
    }
    if !valid_bounds {
        reasons.push(format!(
            "bounds are below configured {}x{} threshold",
            config.minimum_interactable_width, config.minimum_interactable_height
        ));
    }
    if !window.visible || window.bounds.width <= 0 || window.bounds.height <= 0 {
        reasons.push("owning window is hidden or has invalid bounds".into());
    }
    Interactability {
        interactable: reasons.is_empty(),
        visible: control.visible,
        enabled: control.enabled,
        offscreen: control.offscreen,
        valid_bounds,
        window_visible: window.visible,
        clipping_known: control.offscreen,
        minimum_width: config.minimum_interactable_width,
        minimum_height: config.minimum_interactable_height,
        reasons,
    }
}

fn is_structured(control: &ControlInfo) -> bool {
    !control.automation_id.is_empty()
        && (!control.name.is_empty()
            || control.capabilities.invoke
            || control.capabilities.select
            || control.capabilities.value
            || matches!(
                control.control_type.as_str(),
                "Window" | "Edit" | "Button" | "CheckBox" | "ComboBox" | "List" | "Text"
            ))
}

fn element_depths(tree: &ControlTreeResult) -> BTreeMap<String, usize> {
    let parents = tree
        .elements
        .iter()
        .map(|control| {
            (
                control.element_id.clone(),
                control.parent_element_id.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    tree.elements
        .iter()
        .map(|control| {
            let mut depth = 0;
            let mut current = control.parent_element_id.as_ref();
            while let Some(parent) = current {
                depth += 1;
                if depth > 256 {
                    break;
                }
                current = parents.get(parent).and_then(Option::as_ref);
            }
            (control.element_id.clone(), depth)
        })
        .collect()
}

fn state_matches(
    state: &ElementState,
    interaction: &Interactability,
    expected: &Map<String, Value>,
) -> bool {
    expected.iter().all(|(key, expected)| match key.as_str() {
        "visible" => expected.as_bool() == Some(state.visible),
        "enabled" => expected.as_bool() == Some(state.enabled),
        "focused" => expected.as_bool() == Some(state.focused),
        "selected" => expected.as_bool() == state.selected,
        "interactable" => expected.as_bool() == Some(interaction.interactable),
        "value" => expected.as_str() == state.value.as_deref(),
        "text" => expected.as_str() == state.text.as_deref(),
        "exists" => expected.as_bool() == Some(state.exists),
        _ => false,
    })
}

fn session_window_from_native(
    session_id: &str,
    native: &WindowInfo,
    root: Option<&ControlInfo>,
) -> SessionWindow {
    SessionWindow {
        session_id: session_id.to_owned(),
        process_id: native.process_id,
        window_handle: native.hwnd.clone(),
        window_id: native.window_id.clone(),
        name: native.title.clone(),
        control_type: root
            .map(|control| control.control_type.clone())
            .unwrap_or_else(|| "Window".into()),
        automation_id: root
            .map(|control| control.automation_id.clone())
            .unwrap_or_default(),
        bounds: native.bounds,
        visible: root
            .map(|control| control.visible)
            .unwrap_or(native.visible),
    }
}

fn window_matches(window: &SessionWindow, query: &WindowFindQuery) -> bool {
    query
        .process_id
        .is_none_or(|process_id| process_id == window.process_id)
        && query
            .automation_id
            .as_ref()
            .is_none_or(|value| eq_fold(value, &window.automation_id))
        && query
            .title
            .as_ref()
            .is_none_or(|value| eq_fold(value, &window.name))
        && query
            .name
            .as_ref()
            .is_none_or(|value| eq_fold(value, &window.name))
        && query
            .control_type
            .as_ref()
            .or(query.root_type.as_ref())
            .is_none_or(|value| eq_fold(value, &window.control_type))
}

fn eq_fold(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn parse_window_query(value: &Value) -> HostResult<WindowFindQuery> {
    serde_json::from_value(value.clone()).map_err(|error| invalid(error.to_string()))
}

fn attach_diagnostic(mut error: HostError, key: &str, value: Value) -> HostError {
    let mut details = error.details.as_object().cloned().unwrap_or_default();
    details.insert(key.to_owned(), value);
    error.details = Value::Object(details);
    error
}

fn runtime_failure_error(health: &RuntimeHealth, dispatch_error: Option<Value>) -> HostError {
    host_error(
        "APPLICATION_RUNTIME_FAILURE",
        "Makepad application became unhealthy during or immediately after the action",
        json!({
            "runtime_health":health,
            "stderr_tail":health.stderr_tail,
            "dispatch_error":dispatch_error,
        }),
    )
}

fn wait_timeout(
    operation: &str,
    timeout: Duration,
    poll: Duration,
    diagnostic: Value,
) -> HostError {
    host_error(
        "WAIT_TIMEOUT",
        format!("{operation} timed out"),
        json!({
            "operation":operation,
            "timeout_ms":timeout.as_millis(),
            "poll_strategy":{"kind":"fixed","interval_ms":poll.as_millis()},
            "diagnostic":diagnostic,
        }),
    )
}

fn target_from_arguments(arguments: &Value) -> Option<String> {
    arguments
        .get("id")
        .or_else(|| arguments.get("automation_id"))
        .or_else(|| arguments.get("window_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn string_value<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
}

fn duration(arguments: &Value, default_ms: u64) -> Duration {
    Duration::from_millis(
        arguments
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(default_ms),
    )
}

fn poll_duration(arguments: &Value, default_ms: u64) -> Duration {
    Duration::from_millis(
        arguments
            .get("poll_ms")
            .and_then(Value::as_u64)
            .unwrap_or(default_ms)
            .max(10),
    )
}

fn is_physical_operation(operation: &str) -> bool {
    matches!(
        operation,
        "double_click"
            | "move_pointer"
            | "pointer_down"
            | "pointer_up"
            | "click_relative"
            | "double_click_relative"
            | "right_click_relative"
            | "drag_relative"
            | "select_region"
            | "hover_relative"
            | "wheel_relative"
            | "windows_drag_title"
            | "windows_resize_frame"
            | "windows_double_click_title"
            | "right_click"
            | "hover"
            | "drag"
            | "drag_to"
            | "scroll"
            | "scroll_to"
            | "ensure_visible"
            | "scroll_to_start"
            | "scroll_to_end"
            | "scroll_page"
            | "add_to_selection"
            | "remove_from_selection"
            | "select_many"
            | "select_range"
            | "press"
            | "hotkey"
            | "type_text"
            | "close_window"
    )
}

fn physical_operation_allows_non_interactable_target(operation: &str) -> bool {
    matches!(
        operation,
        "ensure_visible" | "scroll_to_start" | "scroll_to_end"
    )
}

fn operation_allows_absent_target(operation: &str) -> bool {
    matches!(operation, "wait_hidden")
}
fn take_call_redaction(arguments: &mut Value) -> (Vec<String>, bool) {
    let Some(arguments) = arguments.as_object_mut() else {
        return (Vec::new(), false);
    };
    let fields = arguments
        .remove("_aiui_redact_fields")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let redact_sensitive = arguments
        .remove("_aiui_redact_response")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let mut literals = BTreeSet::new();
    for field in fields.iter().filter_map(Value::as_str) {
        if let Some(value) = arguments.get(field) {
            collect_redaction_literals(value, &mut literals);
        }
    }
    (literals.into_iter().collect(), redact_sensitive)
}

fn collect_redaction_literals(value: &Value, literals: &mut BTreeSet<String>) {
    match value {
        Value::String(value) if !value.is_empty() => {
            literals.insert(value.clone());
        }
        Value::Array(values) => {
            for value in values {
                collect_redaction_literals(value, literals);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_redaction_literals(value, literals);
            }
        }
        _ => {}
    }
}

fn is_action_operation(operation: &str) -> bool {
    !matches!(
        operation,
        "read"
            | "get_logs"
            | "get_ui_changes"
            | "inspect_ui"
            | "inspect_semantic"
            | "inspect_window_interaction"
            | "wait_visible"
            | "wait_hidden"
            | "wait_enabled"
            | "wait_disabled"
            | "wait_value"
            | "wait_selected"
            | "wait_count"
            | "wait_idle"
    )
}

fn closed_session_result(session_id: &str) -> Value {
    json!({
        "session_id": session_id,
        "closed": true
    })
}

fn release_lock_for_closed_session(session_id: &str) {
    if let Ok(mut state) = input_lock().lock() {
        if state
            .lease
            .as_ref()
            .is_some_and(|lease| lease.session_id == session_id)
        {
            state.lease = None;
        }
    }
}

fn collect_png_paths(value: &Value, output: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key == "path" {
                    if let Some(path) = value.as_str().filter(|path| path.ends_with(".png")) {
                        output.insert(path.to_owned());
                    }
                }
                collect_png_paths(value, output);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_png_paths(value, output);
            }
        }
        _ => {}
    }
}

#[derive(Clone, Debug, Serialize)]
struct PngStats {
    width: u32,
    height: u32,
    blank: bool,
    distinct_sampled_colors: usize,
}

struct RgbaImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

fn inspect_png(path: &Path) -> HostResult<PngStats> {
    let image = read_png(path)?;
    Ok(png_stats(&image))
}

fn png_stats(image: &RgbaImage) -> PngStats {
    let mut colors = BTreeSet::new();
    let step = (image.pixels.len() / 4 / 4096).max(1);
    let mut all_black_or_transparent = true;
    for pixel in image.pixels.chunks_exact(4).step_by(step) {
        if pixel[3] > 0 && (pixel[0] > 2 || pixel[1] > 2 || pixel[2] > 2) {
            all_black_or_transparent = false;
        }
        colors.insert((pixel[0], pixel[1], pixel[2], pixel[3]));
        if colors.len() > 64 {
            break;
        }
    }
    PngStats {
        width: image.width,
        height: image.height,
        blank: image.width == 0 || image.height == 0 || all_black_or_transparent,
        distinct_sampled_colors: colors.len(),
    }
}

fn read_png(path: &Path) -> HostResult<RgbaImage> {
    let file = File::open(path).map_err(|error| io_error("open PNG", error))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|error| io_error("read PNG header", error))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| invalid("PNG output buffer size overflow"))?;
    let mut buffer = vec![0; size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| io_error("decode PNG", error))?;
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
            return Err(invalid("expanded PNG unexpectedly remained indexed"));
        }
    }
    Ok(RgbaImage {
        width: info.width,
        height: info.height,
        pixels,
    })
}

fn crop_png(source: &Path, output: &Path, region: Rect) -> HostResult<PngStats> {
    let image = read_png(source)?;
    let left = region.x.max(0) as u32;
    let top = region.y.max(0) as u32;
    let right = (region.x.saturating_add(region.width)).max(0) as u32;
    let bottom = (region.y.saturating_add(region.height)).max(0) as u32;
    let right = right.min(image.width);
    let bottom = bottom.min(image.height);
    if left >= right || top >= bottom {
        return Err(host_error(
            "INVALID_BOUNDS",
            "capture region does not intersect the window image",
            json!({"region":region,"image_width":image.width,"image_height":image.height}),
        ));
    }
    let width = right - left;
    let height = bottom - top;
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in top..bottom {
        let start = ((y * image.width + left) * 4) as usize;
        let end = start + width as usize * 4;
        pixels.extend_from_slice(&image.pixels[start..end]);
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error("create crop directory", error))?;
    }
    let file = File::create(output).map_err(|error| io_error("create crop PNG", error))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| io_error("write crop PNG header", error))?;
    writer
        .write_image_data(&pixels)
        .map_err(|error| io_error("write crop PNG", error))?;
    writer
        .finish()
        .map_err(|error| io_error("finish crop PNG", error))?;
    Ok(png_stats(&RgbaImage {
        width,
        height,
        pixels,
    }))
}

fn materialize_evidence_manifest(bundle: &Value) -> Result<(Value, Option<String>), Vec<String>> {
    let Some(path) = bundle.get("path").and_then(Value::as_str) else {
        return Ok((bundle.clone(), None));
    };
    let root = Path::new(path);
    let mut errors = Vec::new();
    if !root.is_dir() {
        errors.push(format!("evidence receipt path is not a directory: {path}"));
    }

    let summary = read_evidence_json(&root.join("summary.json"), "summary", &mut errors);
    let actions = read_evidence_json(&root.join("actions.json"), "actions", &mut errors);
    let results = read_evidence_json(&root.join("results.json"), "results", &mut errors);
    let error_records = read_evidence_json(&root.join("errors.json"), "errors", &mut errors);
    for name in ["ui-changes.json", "last-ui-tree.json", "runtime.json"] {
        let _ = read_evidence_json(&root.join(name), name, &mut errors);
    }

    match bundle.get("files").and_then(Value::as_array) {
        Some(files) => {
            for (index, file) in files.iter().enumerate() {
                match file.as_str() {
                    Some(file) if Path::new(file).is_file() => {}
                    Some(file) => errors.push(format!(
                        "export receipt files[{index}] is missing on disk: {file}"
                    )),
                    None => errors.push(format!("export receipt files[{index}] must be a string")),
                }
            }
        }
        None => errors.push("export receipt is missing files array".into()),
    }

    let mut summary = match summary.and_then(|value| value.as_object().cloned()) {
        Some(summary) => summary,
        None => {
            errors.push("summary must be a JSON object".into());
            Map::new()
        }
    };
    let actions = match actions.and_then(|value| value.as_array().cloned()) {
        Some(actions) => actions,
        None => {
            errors.push("actions must be a JSON array".into());
            Vec::new()
        }
    };
    let result_count = results
        .as_ref()
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let error_count = error_records
        .as_ref()
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if summary.get("action_count").and_then(Value::as_u64) != Some(actions.len() as u64) {
        errors.push("summary action_count does not match actions.json".into());
    }
    if summary.get("result_count").and_then(Value::as_u64) != Some(result_count as u64) {
        errors.push("summary result_count does not match results.json".into());
    }
    if summary.get("error_count").and_then(Value::as_u64) != Some(error_count as u64) {
        errors.push("summary error_count does not match errors.json".into());
    }
    if summary.get("evidence_consistency").and_then(Value::as_str)
        != Some("AI_UI_EVIDENCE_CONSISTENCY_PASSED")
    {
        errors.push("summary does not declare AI_UI_EVIDENCE_CONSISTENCY_PASSED".into());
    }
    summary.insert("actions".into(), Value::Array(actions));

    if errors.is_empty() {
        Ok((Value::Object(summary), Some(path.into())))
    } else {
        Err(errors)
    }
}

fn read_evidence_json(path: &Path, label: &str, errors: &mut Vec<String>) -> Option<Value> {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => Some(value),
            Err(error) => {
                errors.push(format!("{label} is not valid JSON: {error}"));
                None
            }
        },
        Err(error) => {
            errors.push(format!("{label} could not be read: {error}"));
            None
        }
    }
}

fn required(value: &Value, key: &str) -> HostResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| invalid(format!("{key} must be a non-empty string")))
}

fn required_text(value: &Value, key: &str) -> HostResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid(format!("{key} must be a string")))
}

fn required_any(value: &Value, keys: &[&str]) -> HostResult<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| invalid(format!("one of {} is required", keys.join(", "))))
}

fn integer(value: &Value, key: &str) -> HostResult<i32> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|number| i32::try_from(number).ok())
        .ok_or_else(|| invalid(format!("{key} must be a 32-bit integer")))
}

fn safe_path_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn to_value<T: Serialize>(value: T) -> HostResult<Value> {
    serde_json::to_value(value).map_err(|error| invalid(error.to_string()))
}

fn write_json(path: &Path, value: &Value) -> HostResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error("create JSON directory", error))?;
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|error| invalid(error.to_string()))?,
    )
    .map_err(|error| io_error("write JSON", error))
}

fn invalid(message: impl Into<String>) -> HostError {
    host_error("INVALID_ARGUMENT", message, Value::Null)
}

fn io_error(context: &str, error: impl std::fmt::Display) -> HostError {
    host_error("HOST_IO_ERROR", format!("{context}: {error}"), Value::Null)
}

fn host_error(code: &str, message: impl Into<String>, details: Value) -> HostError {
    HostError {
        code: code.into(),
        message: message.into(),
        details,
    }
}

fn evidence_export_failure_injected() -> bool {
    matches!(
        std::env::var(EVIDENCE_EXPORT_FAILURE_INJECTION_ENV).as_deref(),
        Ok("1")
    )
}

fn attach_evidence_capture_failure(primary: &mut HostError, capture: &HostError) {
    let mut details = primary.details.as_object().cloned().unwrap_or_default();
    details.insert(
        "evidence_capture_error".into(),
        json!({
            "code": capture.code.clone(),
            "message": capture.message.clone(),
            "details": capture.details.clone(),
        }),
    );
    primary.details = Value::Object(details);
}

#[cfg(test)]
mod tests {
    use super::*;
    use toolkit_windows_automation::ActionCapabilities;

    fn window() -> SessionWindow {
        SessionWindow {
            session_id: "session:a".into(),
            process_id: 7,
            window_handle: "0x7".into(),
            window_id: "win:a".into(),
            name: "Probe".into(),
            control_type: "Window".into(),
            automation_id: "window.main".into(),
            bounds: Rect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
            visible: true,
        }
    }

    fn control() -> ControlInfo {
        ControlInfo {
            element_id: "el:a".into(),
            parent_element_id: None,
            control_type: "Button".into(),
            control_type_id: 50_000,
            localized_control_type: "button".into(),
            name: "Save".into(),
            value: None,
            text: None,
            automation_id: "file.save".into(),
            class_name: "Makepad".into(),
            framework_id: "Makepad".into(),
            bounds: Rect {
                x: 10,
                y: 10,
                width: 2,
                height: 2,
            },
            visible: true,
            offscreen: false,
            enabled: true,
            focused: false,
            selected: None,
            expand_state: None,
            capabilities: ActionCapabilities::default(),
            hwnd: None,
        }
    }
    #[test]
    fn canonical_control_arguments_replace_only_the_id() {
        let canonical = ReliabilityManager::canonical_control_arguments(
            &json!({"id":"/save","value":"kept"}),
            &control(),
        )
        .unwrap();
        assert_eq!(canonical["id"], "file.save");
        assert_eq!(canonical["value"], "kept");
    }

    #[test]
    fn session_close_result_cannot_leak_historical_nested_errors() {
        assert_eq!(
            closed_session_result("session:test:1"),
            json!({"session_id":"session:test:1","closed":true})
        );
    }

    #[test]
    fn hidden_window_is_not_a_capture_target() {
        let mut hidden = window();
        hidden.visible = false;
        let error = ReliabilityManager::capture_target_window(hidden).unwrap_err();
        assert_eq!(error.code, "WINDOW_NOT_FOUND");
    }

    #[test]
    fn host_error_response_is_not_misread_as_success() {
        let error = decode_host_response(json!({
            "success": false,
            "error": {
                "code": "APP_NOT_FOUND",
                "message": "test application not found",
                "details": {"path": "missing.exe"}
            }
        }))
        .unwrap_err();

        assert_eq!(error.code, "APP_NOT_FOUND");
        assert_eq!(error.message, "test application not found");
        assert_eq!(error.details, json!({"path": "missing.exe"}));
    }

    #[test]
    fn evidence_capture_failure_is_attached_without_replacing_primary_error() {
        let mut primary = host_error(
            "WAIT_TIMEOUT",
            "value did not arrive",
            json!({"timeout_ms": 50}),
        );
        let capture = host_error(
            "EVIDENCE_CAPTURE_INJECTED_FAILURE",
            "controlled evidence failure",
            json!({"injected": true}),
        );

        attach_evidence_capture_failure(&mut primary, &capture);

        assert_eq!(primary.code, "WAIT_TIMEOUT");
        assert_eq!(primary.details["timeout_ms"], 50);
        assert_eq!(
            primary.details["evidence_capture_error"]["code"],
            "EVIDENCE_CAPTURE_INJECTED_FAILURE"
        );
        assert_eq!(
            primary.details["evidence_capture_error"]["details"],
            json!({"injected": true})
        );
    }

    #[test]
    fn empty_expected_value_is_valid_for_waits() {
        assert_eq!(required_text(&json!({"value":""}), "value").unwrap(), "");
        assert!(required(&json!({"value":""}), "value").is_err());
    }

    #[test]
    fn interactability_threshold_is_configurable_not_hardcoded() {
        let mut config = ReliabilityConfig::default();
        assert!(interactability(&control(), &window(), &config).interactable);
        config.minimum_interactable_width = 3;
        assert!(!interactability(&control(), &window(), &config).interactable);
    }

    #[test]
    fn window_filters_include_session_process_root_and_name() {
        let query = WindowFindQuery {
            session_id: Some("session:a".into()),
            process_id: Some(7),
            automation_id: Some("window.main".into()),
            name: Some("probe".into()),
            root_type: Some("window".into()),
            ..Default::default()
        };
        assert!(window_matches(&window(), &query));
    }

    #[test]
    fn custom_widget_bounds_are_checked_separately() {
        let mut control = control();
        control.control_type = "Custom".into();
        control.bounds.width = 0;
        let report = interactability(&control, &window(), &ReliabilityConfig::default());
        assert!(!report.valid_bounds);
        assert!(!report.interactable);
    }

    #[test]
    fn element_relative_capture_region_matches_public_contract() {
        let element_bounds = Rect {
            x: 100,
            y: 200,
            width: 400,
            height: 200,
        };
        let window_bounds = Rect {
            x: 50,
            y: 100,
            width: 1000,
            height: 800,
        };
        let (relative, screen, image) =
            ReliabilityManager::resolve_element_relative_capture_region(
                &json!({"region":{"x":0.25,"y":0.25,"width":0.5,"height":0.5}}),
                element_bounds,
                window_bounds,
            )
            .unwrap();

        assert_eq!(
            relative,
            RelativeRect {
                x: 0.25,
                y: 0.25,
                width: 0.5,
                height: 0.5,
            }
        );
        assert_eq!(
            screen,
            Rect {
                x: 200,
                y: 250,
                width: 200,
                height: 100,
            }
        );
        assert_eq!(
            image,
            Rect {
                x: 150,
                y: 150,
                width: 200,
                height: 100,
            }
        );
    }

    #[test]
    fn export_receipt_materializes_the_persisted_evidence_manifest() {
        let root = std::env::temp_dir().join(format!(
            "ai-ui-evidence-receipt-{}-{}",
            std::process::id(),
            now_unix_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let summary = json!({
            "schema_version": SCHEMA_VERSION,
            "tool_version": TOOL_VERSION,
            "contract_version": CONTRACT_VERSION,
            "action_count": 0,
            "result_count": 0,
            "error_count": 0,
            "evidence_consistency": "AI_UI_EVIDENCE_CONSISTENCY_PASSED",
        });
        let files = [
            ("summary.json", summary),
            ("actions.json", json!([])),
            ("results.json", json!([])),
            ("errors.json", json!([])),
            ("ui-changes.json", json!([])),
            ("last-ui-tree.json", Value::Null),
            ("runtime.json", json!({"healthy":true})),
        ];
        let mut receipt_files = Vec::new();
        for (name, value) in files {
            let path = root.join(name);
            write_json(&path, &value).unwrap();
            receipt_files.push(path.to_string_lossy().into_owned());
        }

        let (manifest, evidence_path) = materialize_evidence_manifest(&json!({
            "path": root,
            "reason": "unit-test",
            "files": receipt_files,
        }))
        .unwrap();
        assert!(evidence_path.is_some());
        assert_eq!(manifest["actions"], json!([]));
        validate_evidence_bundle(&manifest).unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn visibility_recovery_operations_bypass_interactability_preflight() {
        assert!(physical_operation_allows_non_interactable_target(
            "ensure_visible"
        ));
        assert!(physical_operation_allows_non_interactable_target(
            "scroll_to_end"
        ));
        assert!(!physical_operation_allows_non_interactable_target("drag"));
    }
}
