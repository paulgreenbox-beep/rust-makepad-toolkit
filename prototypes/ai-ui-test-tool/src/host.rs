use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use toolkit_windows_automation::{
    AutomationError, ControlInfo, ControlTreeOptions, ControlTreeResult, ElementState, ErrorCode,
    TreeView, WindowInfo, WindowQuery, WindowsAutomation, now_unix_ms,
};

pub const DEFAULT_WINDOW_TITLE: &str = "AI UI Test Tool - Makepad Form";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    Exists,
    ValueContains(String),
    ValueEquals(String),
    TextContains(String),
    NameContains(String),
    Visible,
    Enabled,
    Selected(bool),
}

impl Default for Condition {
    fn default() -> Self {
        Self::Exists
    }
}

impl Condition {
    fn matches(&self, state: &ElementState) -> bool {
        match self {
            Self::Exists => state.exists,
            Self::ValueContains(expected) => state
                .value
                .as_deref()
                .is_some_and(|actual| actual.contains(expected)),
            Self::ValueEquals(expected) => state.value.as_deref() == Some(expected),
            Self::TextContains(expected) => state
                .text
                .as_deref()
                .is_some_and(|actual| actual.contains(expected)),
            Self::NameContains(expected) => state.name.contains(expected),
            Self::Visible => state.visible,
            Self::Enabled => state.enabled,
            Self::Selected(expected) => state.selected == Some(*expected),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UiChanges {
    pub created: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub sequence: u64,
    pub timestamp_unix_ms: u128,
    pub event: String,
    pub node_id: Option<String>,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeHealth {
    pub process_id: Option<u32>,
    pub process_alive: bool,
    pub exit_code: Option<i32>,
    pub exit_success: Option<bool>,
    pub makepad_runtime_error: bool,
    pub script_runtime_error: bool,
    pub healthy: bool,
    pub stderr_tail: Vec<String>,
    pub checked_at_unix_ms: u128,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostError {
    pub code: String,
    pub message: String,
    pub details: Value,
}

impl HostError {
    fn new(code: impl Into<String>, message: impl Into<String>, details: Value) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details,
        }
    }

    fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new("INVALID_ARGUMENT", message, Value::Null)
    }

    fn app_not_running() -> Self {
        Self::new(
            "APP_NOT_RUNNING",
            "the test application is not running",
            Value::Null,
        )
    }

    fn io(context: &str, error: impl std::fmt::Display) -> Self {
        Self::new("HOST_IO_ERROR", format!("{context}: {error}"), Value::Null)
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for HostError {}

impl From<AutomationError> for HostError {
    fn from(error: AutomationError) -> Self {
        let code = serde_json::to_value(error.code)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "BACKEND_FAILURE".to_owned());
        Self::new(code, error.message, error.details)
    }
}

pub type HostResult<T> = Result<T, HostError>;

pub struct TestHost {
    app_path: PathBuf,
    launch_arguments: Vec<String>,
    window_title: String,
    output_dir: PathBuf,
    launch_environment: BTreeMap<String, String>,
    automation: WindowsAutomation,
    child: Option<ChildGuard>,
    window: Option<WindowInfo>,
    last_tree: Option<ControlTreeResult>,
    last_changes: UiChanges,
    logs: Vec<LogEntry>,
    next_sequence: u64,
}

impl TestHost {
    pub fn new(app_path: impl Into<PathBuf>, output_dir: impl Into<PathBuf>) -> HostResult<Self> {
        Self::with_title(app_path, DEFAULT_WINDOW_TITLE, output_dir)
    }

    pub fn with_title(
        app_path: impl Into<PathBuf>,
        window_title: impl Into<String>,
        output_dir: impl Into<PathBuf>,
    ) -> HostResult<Self> {
        Self::with_title_and_environment(app_path, window_title, output_dir, BTreeMap::new())
    }

    /// Launch-only environment for a trusted fixture. The values are never
    /// included in action records or evidence.
    pub(crate) fn with_title_and_environment(
        app_path: impl Into<PathBuf>,
        window_title: impl Into<String>,
        output_dir: impl Into<PathBuf>,
        launch_environment: BTreeMap<String, String>,
    ) -> HostResult<Self> {
        Self::with_title_and_environment_and_arguments(
            app_path,
            window_title,
            output_dir,
            launch_environment,
            Vec::new(),
        )
    }

    /// Launches literal arguments directly with the executable, never through a shell.
    pub(crate) fn with_title_and_environment_and_arguments(
        app_path: impl Into<PathBuf>,
        window_title: impl Into<String>,
        output_dir: impl Into<PathBuf>,
        launch_environment: BTreeMap<String, String>,
        launch_arguments: Vec<String>,
    ) -> HostResult<Self> {
        Ok(Self {
            app_path: app_path.into(),
            launch_arguments,
            window_title: window_title.into(),
            output_dir: output_dir.into(),
            launch_environment,
            automation: WindowsAutomation::new()?,
            child: None,
            window: None,
            last_tree: None,
            last_changes: UiChanges::default(),
            logs: Vec::new(),
            next_sequence: 0,
        })
    }

    pub fn automation(&self) -> &WindowsAutomation {
        &self.automation
    }

    pub fn automation_mut(&mut self) -> &mut WindowsAutomation {
        &mut self.automation
    }

    pub fn current_window(&self) -> Option<&WindowInfo> {
        self.window.as_ref()
    }

    pub fn refresh_window(&mut self) -> HostResult<WindowInfo> {
        let current = self.window.clone().ok_or_else(HostError::app_not_running)?;
        // A process may own multiple same-titled HWNDs. Refresh through the
        // session-owned opaque ID rather than rediscovering by process/title.
        let refreshed = self.automation.get_window_info(&current.window_id)?;
        self.window = Some(refreshed.clone());
        Ok(refreshed)
    }

    /// Switch the session-owned target HWND by its opaque automation ID.
    /// The ID must resolve to a window in the process launched by this host.
    pub fn select_window(&mut self, window_id: &str) -> HostResult<WindowInfo> {
        let process_id = self.process_id().ok_or_else(HostError::app_not_running)?;
        let selected = self.automation.get_window_info(window_id)?;
        if selected.process_id != process_id {
            return Err(HostError::new(
                "WINDOW_SESSION_MISMATCH",
                "window_id does not belong to the process owned by this test host",
                json!({
                    "window_id": window_id,
                    "expected_process_id": process_id,
                    "actual_process_id": selected.process_id
                }),
            ));
        }
        self.window = Some(selected.clone());
        self.last_tree = None;
        self.last_changes = UiChanges::default();
        self.record(
            "WINDOW_SELECTED",
            None,
            serde_json::to_value(&selected).unwrap_or(Value::Null),
        );
        Ok(selected)
    }

    pub fn process_id(&self) -> Option<u32> {
        self.child.as_ref().map(ChildGuard::id)
    }

    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    pub fn last_tree(&self) -> Option<&ControlTreeResult> {
        self.last_tree.as_ref()
    }

    pub fn get_runtime_stderr(&self, limit: usize) -> Vec<String> {
        self.child
            .as_ref()
            .map(|child| child.stderr_tail(limit))
            .unwrap_or_default()
    }

    pub fn get_runtime_health(&mut self) -> RuntimeHealth {
        let checked_at_unix_ms = now_unix_ms();
        let Some(child) = self.child.as_mut() else {
            return RuntimeHealth {
                process_id: None,
                process_alive: false,
                exit_code: None,
                exit_success: None,
                makepad_runtime_error: false,
                script_runtime_error: false,
                healthy: false,
                stderr_tail: Vec::new(),
                checked_at_unix_ms,
            };
        };
        let process_id = Some(child.id());
        let status = child.try_status().ok().flatten();
        let stderr_tail = child.stderr_tail(80);
        let makepad_runtime_error = stderr_tail.iter().any(|line| {
            let line = line.to_lowercase();
            line.contains("panicked at")
                || line.contains("makepad runtime error")
                || line.contains("fatal runtime")
                || line.contains("non-unwinding panic")
                || line.contains("ai_ui_intentional_runtime_failure")
        });
        let script_runtime_error = stderr_tail.iter().any(|line| {
            let line = line.to_lowercase();
            line.contains("script runtime error")
                || line.contains("script panic")
                || line.contains("splash runtime error")
        });
        let process_alive = status.is_none();
        let exit_code = status.as_ref().and_then(ExitStatus::code);
        let exit_success = status.as_ref().map(ExitStatus::success);
        RuntimeHealth {
            process_id,
            process_alive,
            exit_code,
            exit_success,
            makepad_runtime_error,
            script_runtime_error,
            healthy: process_alive && !makepad_runtime_error && !script_runtime_error,
            stderr_tail,
            checked_at_unix_ms,
        }
    }

    pub fn launch(&mut self) -> HostResult<WindowInfo> {
        if !self.app_path.is_file() {
            return Err(HostError::new(
                "APP_NOT_FOUND",
                format!("test application not found at {}", self.app_path.display()),
                json!({"path": self.app_path}),
            ));
        }
        if self.child.is_some() {
            self.shutdown()?;
        }
        fs::create_dir_all(&self.output_dir)
            .map_err(|error| HostError::io("failed to create host output directory", error))?;
        let child = ChildGuard::spawn(
            &self.app_path,
            &self.launch_arguments,
            &self.output_dir,
            &self.launch_environment,
        )
        .map_err(|error| HostError::io("failed to launch test application", error))?;
        let process_id = child.id();
        self.child = Some(child);
        self.record(
            "APP_START",
            None,
            json!({
                "path": self.app_path,
                "process_id": process_id,
            }),
        );

        let window = match self.wait_for_window(process_id, Duration::from_secs(15)) {
            Ok(window) => window,
            Err(error) => {
                self.terminate_child();
                self.record("APP_START_FAILED", None, json!({"error": error}));
                return Err(error);
            }
        };
        self.window = Some(window.clone());
        self.record(
            "UI_READY",
            None,
            serde_json::to_value(&window).unwrap_or(Value::Null),
        );
        Ok(window)
    }

    pub fn restart_app(&mut self) -> HostResult<WindowInfo> {
        self.shutdown()?;
        self.record("APP_RESTART", None, Value::Null);
        self.launch()
    }

    pub fn shutdown(&mut self) -> HostResult<()> {
        let was_running = self.child.is_some();
        self.terminate_child();
        self.window = None;
        self.last_tree = None;
        self.last_changes = UiChanges::default();
        if was_running {
            self.record("APP_STOP", None, Value::Null);
        }
        Ok(())
    }

    pub fn inspect_ui(&mut self) -> HostResult<ControlTreeResult> {
        let tree = self.refresh_tree()?;
        self.record(
            "UI_INSPECT",
            None,
            json!({
                "elements": tree.total_enumerated,
                "changes": self.last_changes,
            }),
        );
        Ok(tree)
    }

    pub fn click(&mut self, id: &str) -> HostResult<Value> {
        let element_id = self.resolve_element_id(id)?;
        let result = self
            .automation
            .click_element(&self.window_id()?, &element_id)?;
        self.refresh_tree()?;
        let changes = self.last_changes.clone();
        let value = serde_json::to_value(&result)
            .map_err(|error| HostError::io("failed to serialize click result", error))?;
        self.record(
            "CLICK",
            Some(id),
            json!({"result": value, "ui_changes": changes}),
        );
        Ok(value)
    }

    pub fn input(&mut self, id: &str, text: &str) -> HostResult<ElementState> {
        let element_id = self.resolve_element_id(id)?;
        let state = self
            .automation
            .set_value(&self.window_id()?, &element_id, text)?;
        self.refresh_tree()?;
        let changes = self.last_changes.clone();
        self.record(
            "INPUT",
            Some(id),
            json!({"text": text, "state": state, "ui_changes": changes}),
        );
        Ok(state)
    }

    pub fn read(&mut self, id: &str) -> HostResult<ElementState> {
        let state = self.read_raw(id)?;
        self.record(
            "READ",
            Some(id),
            serde_json::to_value(&state).unwrap_or(Value::Null),
        );
        Ok(state)
    }

    pub fn wait(
        &mut self,
        id: &str,
        condition: Condition,
        timeout: Duration,
    ) -> HostResult<ElementState> {
        let deadline = Instant::now() + timeout;
        loop {
            let error = match self.read_raw(id) {
                Ok(state) if condition.matches(&state) => {
                    self.record(
                        "WAIT",
                        Some(id),
                        json!({
                            "condition": condition.clone(),
                            "timeout_ms": timeout.as_millis(),
                            "state": state
                        }),
                    );
                    return Ok(state);
                }
                Ok(state) => HostError::new(
                    "WAIT_CONDITION_NOT_MET",
                    format!("condition was not met for {id}"),
                    json!({"condition": condition.clone(), "state": state}),
                ),
                Err(error) => error,
            };
            if Instant::now() >= deadline {
                self.record("WAIT_TIMEOUT", Some(id), json!({"error": &error}));
                return Err(error);
            }
            thread::sleep(Duration::from_millis(75));
        }
    }
    pub fn assert(&mut self, id: &str, condition: Condition) -> HostResult<bool> {
        let state = self.read_raw(id)?;
        if condition.matches(&state) {
            self.record(
                "ASSERT",
                Some(id),
                json!({"condition": condition, "state": state, "success": true}),
            );
            return Ok(true);
        }
        let error = HostError::new(
            "ASSERTION_FAILED",
            format!("assertion failed for {id}"),
            json!({"condition": condition, "state": state}),
        );
        self.record("ASSERT_FAILED", Some(id), json!({"error": error}));
        Err(error)
    }

    pub fn screenshot(&mut self) -> HostResult<Value> {
        let path = self
            .output_dir
            .join(format!("screenshot_{:04}.png", self.next_sequence + 1));
        let path_string = path.to_string_lossy().into_owned();
        let result = self
            .automation
            .capture_window(&self.window_id()?, Some(&path_string))?;
        let value = serde_json::to_value(&result)
            .map_err(|error| HostError::io("failed to serialize screenshot result", error))?;
        self.record("SCREENSHOT", None, value.clone());
        Ok(value)
    }

    pub fn get_logs(&self) -> Vec<LogEntry> {
        self.logs.clone()
    }

    pub fn get_ui_changes(&self) -> UiChanges {
        self.last_changes.clone()
    }

    pub fn call(&mut self, operation: &str, arguments: Value) -> Value {
        let result = match operation {
            "launch" => self.launch().and_then(serialize),
            "restart_app" => self.restart_app().and_then(serialize),
            "inspect_ui" => self.inspect_ui().and_then(serialize),
            "click" => self
                .required_string(&arguments, "id")
                .and_then(|id| self.click(&id)),
            "input" => self
                .required_string(&arguments, "id")
                .and_then(|id| {
                    self.required_text(&arguments, "text")
                        .map(|text| (id, text))
                })
                .and_then(|(id, text)| self.input(&id, &text).and_then(serialize)),
            "read" => self
                .required_string(&arguments, "id")
                .and_then(|id| self.read(&id).and_then(serialize)),
            "wait" => {
                let id = self.required_string(&arguments, "id");
                let condition = serde_json::from_value::<Condition>(
                    arguments
                        .get("condition")
                        .cloned()
                        .unwrap_or_else(|| json!("exists")),
                )
                .map_err(|error| HostError::invalid_argument(error.to_string()));
                let timeout = arguments
                    .get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(5_000);
                id.and_then(|id| condition.map(|condition| (id, condition)))
                    .and_then(|(id, condition)| {
                        self.wait(&id, condition, Duration::from_millis(timeout))
                            .and_then(serialize)
                    })
            }
            "assert" => {
                let id = self.required_string(&arguments, "id");
                let condition = serde_json::from_value::<Condition>(
                    arguments
                        .get("condition")
                        .cloned()
                        .unwrap_or_else(|| json!("exists")),
                )
                .map_err(|error| HostError::invalid_argument(error.to_string()));
                id.and_then(|id| condition.map(|condition| (id, condition)))
                    .and_then(|(id, condition)| self.assert(&id, condition).map(Value::from))
            }
            "screenshot" => self.screenshot(),
            "get_logs" => Ok(json!(self.get_logs())),
            "get_ui_changes" => Ok(json!(self.get_ui_changes())),
            "shutdown" => self.shutdown().map(|_| Value::Null),
            _ => Err(HostError::new(
                "UNKNOWN_OPERATION",
                format!("unknown AI bridge operation: {operation}"),
                json!({"operation": operation}),
            )),
        };
        match result {
            Ok(result) => json!({"success": true, "result": result}),
            Err(error) => json!({"success": false, "error": error}),
        }
    }

    fn required_string(&self, arguments: &Value, key: &str) -> HostResult<String> {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| HostError::invalid_argument(format!("{key} must be a non-empty string")))
    }

    fn required_text(&self, arguments: &Value, key: &str) -> HostResult<String> {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| HostError::invalid_argument(format!("{key} must be a string")))
    }

    fn wait_for_window(&mut self, process_id: u32, timeout: Duration) -> HostResult<WindowInfo> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.automation.find_window(&WindowQuery {
                process_id: Some(process_id),
                title: Some(self.window_title.clone()),
                ..Default::default()
            }) {
                Ok(window) => return Ok(window),
                Err(error)
                    if error.code == ErrorCode::WindowNotFound && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn refresh_tree(&mut self) -> HostResult<ControlTreeResult> {
        let window_id = self.window_id()?;
        let tree = self.automation.get_control_tree(
            &window_id,
            &ControlTreeOptions {
                max_depth: Some(32),
                max_elements: Some(2_000),
                view: TreeView::Control,
                ..Default::default()
            },
        )?;
        self.last_changes = diff_trees(self.last_tree.as_ref(), &tree);
        self.last_tree = Some(tree.clone());
        Ok(tree)
    }

    fn read_raw(&mut self, id: &str) -> HostResult<ElementState> {
        self.refresh_tree()?;
        let element_id = self.resolve_element_id(id)?;
        Ok(self
            .automation
            .get_element_state(&self.window_id()?, &element_id)?)
    }

    fn resolve_element_id(&self, id: &str) -> HostResult<String> {
        let tree = self
            .last_tree
            .as_ref()
            .ok_or_else(HostError::app_not_running)?;
        if let Some(control) = tree
            .elements
            .iter()
            .find(|control| control.automation_id == id || control.element_id == id)
        {
            return Ok(control.element_id.clone());
        }
        let known_similar_elements = tree
            .elements
            .iter()
            .filter(|control| {
                control.automation_id.contains(id)
                    || control.name.to_lowercase().contains(&id.to_lowercase())
            })
            .map(|control| control.automation_id.clone())
            .collect::<Vec<_>>();
        Err(HostError::new(
            "ELEMENT_NOT_FOUND",
            format!("no UI element matched id {id}"),
            json!({
                "requested_id": id,
                "known_similar_elements": known_similar_elements,
            }),
        ))
    }

    fn window_id(&self) -> HostResult<String> {
        self.window
            .as_ref()
            .map(|window| window.window_id.clone())
            .ok_or_else(HostError::app_not_running)
    }

    fn record(&mut self, event: &str, node_id: Option<&str>, payload: Value) {
        self.next_sequence += 1;
        self.logs.push(LogEntry {
            sequence: self.next_sequence,
            timestamp_unix_ms: now_unix_ms(),
            event: event.to_owned(),
            node_id: node_id.map(str::to_owned),
            payload,
        });
    }

    fn terminate_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.terminate();
        }
    }
}

impl Drop for TestHost {
    fn drop(&mut self) {
        self.terminate_child();
    }
}

fn serialize<T: Serialize>(value: T) -> HostResult<Value> {
    serde_json::to_value(value).map_err(|error| HostError::io("failed to serialize result", error))
}

fn diff_trees(previous: Option<&ControlTreeResult>, current: &ControlTreeResult) -> UiChanges {
    let current_map = current
        .elements
        .iter()
        .map(|control| (control_key(control), control))
        .collect::<BTreeMap<_, _>>();
    let previous_map = previous
        .map(|tree| {
            tree.elements
                .iter()
                .map(|control| (control_key(control), control))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let created = current_map
        .keys()
        .filter(|key| !previous_map.contains_key(*key))
        .cloned()
        .collect();
    let removed = previous_map
        .keys()
        .filter(|key| !current_map.contains_key(*key))
        .cloned()
        .collect();
    let changed = current_map
        .iter()
        .filter_map(|(key, current)| {
            previous_map
                .get(key)
                .filter(|previous| control_changed(previous, current))
                .map(|_| key.clone())
        })
        .collect();
    UiChanges {
        created,
        removed,
        changed,
    }
}

fn control_key(control: &ControlInfo) -> String {
    if control.automation_id.is_empty() {
        control.element_id.clone()
    } else {
        control.automation_id.clone()
    }
}

fn control_changed(previous: &ControlInfo, current: &ControlInfo) -> bool {
    previous.parent_element_id != current.parent_element_id
        || previous.control_type != current.control_type
        || previous.name != current.name
        || previous.value != current.value
        || previous.text != current.text
        || previous.bounds != current.bounds
        || previous.visible != current.visible
        || previous.enabled != current.enabled
        || previous.focused != current.focused
        || previous.selected != current.selected
}

struct ChildGuard {
    child: Child,
    stderr_lines: Arc<Mutex<VecDeque<String>>>,
    exit_status: Option<ExitStatus>,
}

impl ChildGuard {
    fn spawn(
        path: &Path,
        launch_arguments: &[String],
        output_dir: &Path,
        launch_environment: &BTreeMap<String, String>,
    ) -> std::io::Result<Self> {
        let mut command = Command::new(path);
        command.args(launch_arguments);
        // Makepad applications commonly resolve bundled resources and local
        // service configuration relative to their executable's project
        // directory. A UI test must reproduce that normal launch context
        // instead of inheriting the runner's unrelated working directory.
        if let Some(working_directory) = path.parent() {
            command.current_dir(working_directory);
        }
        // A deterministic fixture must never inherit a mode selected for a
        // previous run. Explicit scoped values below are still allowed.
        command
            .env_remove("AI_UI_TEST_ASYNC_FAILURE_MODE")
            .envs(launch_environment)
            .env("AI_UI_TEST_OUTPUT_DIR", output_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stderr_lines = Arc::new(Mutex::new(VecDeque::with_capacity(1024)));
        if let Some(stderr) = child.stderr.take() {
            let sink = stderr_lines.clone();
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if let Ok(mut lines) = sink.lock() {
                        if lines.len() == 1024 {
                            lines.pop_front();
                        }
                        lines.push_back(line);
                    }
                }
            });
        }
        Ok(Self {
            child,
            stderr_lines,
            exit_status: None,
        })
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn try_status(&mut self) -> std::io::Result<Option<ExitStatus>> {
        if self.exit_status.is_none() {
            self.exit_status = self.child.try_wait()?;
        }
        Ok(self.exit_status.clone())
    }

    fn stderr_tail(&self, limit: usize) -> Vec<String> {
        let Ok(lines) = self.stderr_lines.lock() else {
            return vec!["stderr capture lock was poisoned".to_owned()];
        };
        let mut output = lines
            .iter()
            .filter(|line| {
                let line = line.to_lowercase();
                line.contains("panicked at")
                    || line.contains("runtime error")
                    || line.contains("script panic")
                    || line.contains("ai_ui_intentional_runtime_failure")
            })
            .cloned()
            .collect::<Vec<_>>();
        let skip = lines.len().saturating_sub(limit);
        for line in lines.iter().skip(skip) {
            if !output.contains(line) {
                output.push(line.clone());
            }
        }
        output
    }

    fn terminate(&mut self) {
        if self.try_status().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        if self.exit_status.is_none() {
            self.exit_status = self.child.wait().ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(value: Option<&str>) -> ElementState {
        ElementState {
            exists: true,
            element_id: "el:test:1".into(),
            window_id: "win:test:1".into(),
            visible: true,
            enabled: true,
            focused: false,
            selected: None,
            name: "Result".into(),
            control_type: "Text".into(),
            bounds: Default::default(),
            value: value.map(str::to_owned),
            text: value.map(str::to_owned),
        }
    }

    #[test]
    fn conditions_match_structured_state() {
        assert!(
            Condition::ValueContains("Hello".into()).matches(&state(Some("Result: Hello Runa")))
        );
        assert!(Condition::TextContains("Runa".into()).matches(&state(Some("Result: Hello Runa"))));
        assert!(
            !Condition::ValueEquals("other".into()).matches(&state(Some("Result: Hello Runa")))
        );
    }

    #[test]
    fn child_guard_uses_scoped_environment() {
        let root = std::env::temp_dir().join(format!("ai-ui-child-env-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let probe = root.join("environment-probe.cmd");
        fs::write(
            &probe,
            "@echo off\r\n> \"%AI_UI_TEST_OUTPUT_DIR%\\child-mode.txt\" echo %AI_UI_TEST_ASYNC_FAILURE_MODE%\r\n",
        )
        .unwrap();
        let environment = BTreeMap::from([(
            "AI_UI_TEST_ASYNC_FAILURE_MODE".into(),
            "file_never_appears".into(),
        )]);
        let mut child = ChildGuard::spawn(&probe, &[], &root, &environment).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_status().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(child.try_status().unwrap().is_some());
        let observed = fs::read_to_string(root.join("child-mode.txt")).unwrap();
        assert_eq!(observed.trim(), "file_never_appears");
        let _ = fs::remove_dir_all(root);
    }
}
