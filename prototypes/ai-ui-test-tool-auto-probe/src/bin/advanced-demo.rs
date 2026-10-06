//! Makepad surface used by the advanced semantic interaction E2E.

use makepad_widgets::*;

use ai_ui_test_tool_auto_probe::observation::{
    AsyncOperation, AsyncOperationKind, AsyncOperationState, ExternalRequestPhase,
    ExternalRequestState, OBSERVATION_VERSION, ObservationFailure, RuntimeMilestone,
    TelemetrySnapshot, TraceEvent,
};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use ai_ui_test_tool_auto_probe::probe::{MakepadUiProbe, ProbeConfig, ProbeGeometry};
#[cfg(windows)]
use std::{cell::RefCell, sync::Arc};
#[cfg(windows)]
use toolkit_windows_automation::{
    AccessibilityAction, AccessibilityActionHandler, AccessibilityActionRequest, AccessibilityNode,
    AccessibilityRect, AccessibilityRole, AccessibilityTree, WindowsAccessibilityBridge,
};
#[cfg(windows)]
use windows_uia::Win32::{
    Foundation::{HWND, LPARAM, TRUE},
    UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, SW_HIDE,
        ShowWindow,
    },
};
#[cfg(windows)]
use windows_uia::core::BOOL;

app_main!(App);

const WINDOW_TITLE: &str = "AI UI Advanced Makepad Probe";
const CRASH_BEFORE_ACTION_INJECTION_ENV: &str = "AI_UI_TEST_INJECT_CRASH_BEFORE_ACTION";
const HIDE_WINDOW_INJECTION_ENV: &str = "AI_UI_TEST_INJECT_HIDE_WINDOW";
const ASYNC_FAILURE_MODE_ENV: &str = "AI_UI_TEST_ASYNC_FAILURE_MODE";
// Keep every intermediate state observable after the runner's post-action
// settle interval and two sequential telemetry-tree reads. The second read
// is needed for the public `wait_for_api_state(WaitingResponse)` proof.
const ASYNC_PHASE_DELAY_SECONDS: f64 = 1.0;
// Four writes leave at least three observable changes after the artifact
// appears, while allowing the public bounded task wait to reach terminal state.
const ASYNC_UNSTABLE_FILE_WRITES: u8 = 4;
const ASYNC_ARTIFACT_ROOT: &str = "fixture";
const ASYNC_ARTIFACT_PATH: &str = "async-output.json";
const ASYNC_ARTIFACT_RELATIVE_PATH: &str = "fixture/async-output.json";

#[cfg(windows)]
thread_local! {
    static ACCESSIBILITY_BRIDGE: RefCell<Option<WindowsAccessibilityBridge>> = const { RefCell::new(None) };
    static MAKEPAD_PROBE: RefCell<MakepadUiProbe> = RefCell::new(MakepadUiProbe::new(ProbeConfig::default()));
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct AccessibilityUiAction(AccessibilityActionRequest);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum AsyncFailureMode {
    #[default]
    Success,
    TaskFails,
    ApiTimeout,
    ApiHttpFailure,
    ApiInvalidResponse,
    ApiNotAccepted,
    FileNeverAppears,
    FileNeverStable,
    MissingMilestone,
}

impl AsyncFailureMode {
    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "" | "success" => Some(Self::Success),
            "task_fails" => Some(Self::TaskFails),
            "api_timeout" => Some(Self::ApiTimeout),
            "api_http_failure" => Some(Self::ApiHttpFailure),
            "api_invalid_response" => Some(Self::ApiInvalidResponse),
            "api_not_accepted" => Some(Self::ApiNotAccepted),
            "file_never_appears" => Some(Self::FileNeverAppears),
            "file_never_stable" => Some(Self::FileNeverStable),
            "missing_milestone" => Some(Self::MissingMilestone),
            _ => None,
        }
    }

    fn from_environment() -> Self {
        let configured = std::env::var(ASYNC_FAILURE_MODE_ENV).unwrap_or_default();
        if let Some(mode) = Self::parse(&configured) {
            return mode;
        }
        eprintln!("AI_UI_ASYNC_FAILURE_MODE_INVALID");
        Self::Success
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::TaskFails => "task_fails",
            Self::ApiTimeout => "api_timeout",
            Self::ApiHttpFailure => "api_http_failure",
            Self::ApiInvalidResponse => "api_invalid_response",
            Self::ApiNotAccepted => "api_not_accepted",
            Self::FileNeverAppears => "file_never_appears",
            Self::FileNeverStable => "file_never_stable",
            Self::MissingMilestone => "missing_milestone",
        }
    }
}

#[derive(Clone, Debug, Default)]
struct AsyncFixtureState {
    stage: u8,
    failure_mode: AsyncFailureMode,
    unstable_writes: u8,
    task: Option<AsyncOperation>,
    request: Option<ExternalRequestState>,
    milestones: Vec<RuntimeMilestone>,
    logs: Vec<TraceEvent>,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    input_value: String,
    #[rust]
    result_value: String,
    #[rust]
    status_value: String,
    #[rust]
    tree_expanded: bool,
    #[rust]
    modal_open: bool,
    #[rust]
    context_open: bool,
    #[rust]
    async_pending: bool,
    #[rust]
    async_timer: Timer,
    #[rust]
    async_fixture: AsyncFixtureState,
    #[rust]
    crash_before_action_pending: bool,
    #[rust]
    crash_before_action_timer: Timer,
    #[rust]
    hide_window_pending: bool,
    #[rust]
    hide_window_timer: Timer,
    #[rust]
    drop_count: usize,
    #[rust]
    drag_armed: bool,
}

impl App {
    fn set_label(&self, cx: &mut Cx, id: &[LiveId], text: &str) {
        self.ui.label(cx, id).set_text(cx, text);
    }

    fn update_labels(&self, cx: &mut Cx) {
        self.set_label(cx, ids!(result_message), &self.result_value);
        self.set_label(cx, ids!(status), &self.status_value);
    }

    fn status(&mut self, cx: &mut Cx, value: impl Into<String>) {
        self.status_value = value.into();
        self.update_labels(cx);
    }

    fn submit(&mut self, cx: &mut Cx) {
        self.result_value = format!("Result: {}", self.input_value);
        self.update_labels(cx);
        self.status(cx, "Status: greeting created");
    }

    fn open_dialog(&mut self, cx: &mut Cx) {
        self.modal_open = true;
        self.ui.modal(cx, ids!(dialog_modal)).open(cx);
        self.ui
            .label(cx, ids!(dialog_value))
            .set_text(cx, &self.result_value);
        self.status(cx, "Status: dialog opened");
    }

    fn close_dialog(&mut self, cx: &mut Cx) {
        self.modal_open = false;
        self.ui.modal(cx, ids!(dialog_modal)).close(cx);
        self.status(cx, "Status: dialog closed");
    }

    fn set_context(&mut self, cx: &mut Cx, visible: bool) {
        self.context_open = visible;
        self.ui
            .widget(cx, ids!(context_menu))
            .set_visible(cx, visible);
        self.status(
            cx,
            if visible {
                "Status: context menu visible"
            } else {
                "Status: context menu closed"
            },
        );
    }

    fn trigger_async(&mut self, cx: &mut Cx) {
        self.async_pending = true;
        self.ui
            .widget(cx, ids!(async_result))
            .set_visible(cx, false);
        self.status(cx, "Status: async operation started");
        self.begin_async_fixture(cx);
        self.async_timer = cx.start_timeout(ASYNC_PHASE_DELAY_SECONDS);
    }

    fn complete_async(&mut self, cx: &mut Cx) {
        self.async_pending = false;
        self.ui.widget(cx, ids!(async_result)).set_visible(cx, true);
        self.status(cx, "Status: async result visible");
    }

    fn begin_async_fixture(&mut self, cx: &mut Cx) {
        let now = now_unix_ms();
        let operation_id = "fixture.async.workflow".to_owned();
        let request_id = "fixture.request.workflow".to_owned();
        let failure_mode = AsyncFailureMode::from_environment();
        self.async_fixture = AsyncFixtureState {
            stage: 0,
            failure_mode,
            task: Some(AsyncOperation {
                operation_id: operation_id.clone(),
                kind: AsyncOperationKind::Cpu,
                name: "Deterministic Makepad async workflow".into(),
                state: AsyncOperationState::Running,
                stage: Some("cpu".into()),
                progress: Some(0.1),
                created_at_unix_ms: Some(now),
                started_at_unix_ms: Some(now),
                updated_at_unix_ms: Some(now),
                elapsed_ms: Some(0),
                attempt: 1,
                metadata: serde_json::json!({
                    "artifact":{"root":ASYNC_ARTIFACT_ROOT,"relative_path":ASYNC_ARTIFACT_PATH},
                    "failure_mode":failure_mode.as_str()
                }),
                ..AsyncOperation::default()
            }),
            request: Some(ExternalRequestState {
                request_id,
                observer_alias: "fixture".into(),
                service: "fixture".into(),
                method: "SIMULATED".into(),
                endpoint_alias: "fixture.local".into(),
                target_alias: "fixture.local".into(),
                state: AsyncOperationState::Queued,
                phase: ExternalRequestPhase::Sending,
                attempt: 1,
                started_at_unix_ms: Some(now),
                elapsed_ms: Some(0),
                updated_at_unix_ms: now,
                ..ExternalRequestState::default()
            }),
            ..AsyncFixtureState::default()
        };
        if let Err(error) = clear_async_fixture_artifact() {
            self.fixture_trace(
                "warn",
                "file.cleanup_failed",
                "fixture artifact cleanup before a new run failed",
                serde_json::json!({"error":error}),
            );
        }
        self.fixture_milestone("task.started", AsyncOperationState::Running);
        self.fixture_trace(
            "info",
            "task.started",
            "deterministic async task started",
            serde_json::json!({"operation_id":operation_id,"failure_mode":failure_mode.as_str()}),
        );
        self.publish_fixture_telemetry(cx);
    }
    fn advance_async_fixture(&mut self, cx: &mut Cx) {
        let now = now_unix_ms();
        let failure_mode = self.async_fixture.failure_mode;
        match self.async_fixture.stage {
            0 => {
                if failure_mode == AsyncFailureMode::TaskFails {
                    self.fail_async_fixture(
                        cx,
                        AsyncOperationState::Failed,
                        AsyncOperationState::Cancelled,
                        ExternalRequestPhase::Cancelled,
                        "TASK_FAILED_BEFORE_API",
                        "task.failed_before_api",
                        "fixture task failed before it dispatched an external request",
                    );
                    return;
                }
                self.async_fixture.stage = if failure_mode == AsyncFailureMode::ApiTimeout {
                    90
                } else {
                    1
                };
                if let Some(task) = &mut self.async_fixture.task {
                    task.state = AsyncOperationState::WaitingExternal;
                    task.stage = Some("external.wait_response".into());
                    task.progress = Some(0.4);
                    task.updated_at_unix_ms = Some(now);
                    task.elapsed_ms = task.started_at_unix_ms.map(|started| now.saturating_sub(started));
                }
                if let Some(request) = &mut self.async_fixture.request {
                    request.state = AsyncOperationState::WaitingExternal;
                    request.phase = ExternalRequestPhase::WaitingResponse;
                    request.elapsed_ms = request
                        .started_at_unix_ms
                        .map(|started| now.saturating_sub(started));
                    request.updated_at_unix_ms = now;
                }
                self.fixture_milestone("external.wait_response", AsyncOperationState::WaitingExternal);
                self.fixture_trace(
                    "info",
                    "external.wait_response",
                    "fixture request is waiting for its deterministic response",
                    serde_json::json!({}),
                );
                self.status(cx, "Status: async request waiting for response");
                self.publish_fixture_telemetry(cx);
                self.async_timer = cx.start_timeout(ASYNC_PHASE_DELAY_SECONDS);
            }
            1 => {
                if failure_mode == AsyncFailureMode::ApiHttpFailure {
                    if let Some(request) = &mut self.async_fixture.request {
                        request.transport_completed = true;
                        request.response_received = true;
                        request.status_code = Some(503);
                        request.bytes_received = Some(0);
                        request.response = Some(serde_json::json!({"error":"fixture HTTP failure"}));
                    }
                    self.fail_async_fixture(
                        cx,
                        AsyncOperationState::Failed,
                        AsyncOperationState::Failed,
                        ExternalRequestPhase::Failed,
                        "EXTERNAL_REQUEST_FAILED",
                        "external.http_failed",
                        "fixture returned a deterministic HTTP failure response",
                    );
                    return;
                }
                if failure_mode == AsyncFailureMode::ApiInvalidResponse {
                    if let Some(request) = &mut self.async_fixture.request {
                        request.transport_completed = true;
                        request.response_received = true;
                        request.status_code = Some(200);
                        request.bytes_received = Some(64);
                        request.response = Some(serde_json::json!({"accepted_candidate":"invalid"}));
                    }
                    self.fail_async_fixture(
                        cx,
                        AsyncOperationState::Failed,
                        AsyncOperationState::Failed,
                        ExternalRequestPhase::Failed,
                        "EXTERNAL_RESPONSE_INVALID",
                        "external.response_invalid",
                        "fixture returned a transport-successful but invalid application response",
                    );
                    return;
                }
                if failure_mode == AsyncFailureMode::ApiNotAccepted {
                    if let Some(request) = &mut self.async_fixture.request {
                        request.transport_completed = true;
                        request.response_received = true;
                        request.status_code = Some(200);
                        request.bytes_received = Some(64);
                        request.response = Some(serde_json::json!({"accepted_candidate":false}));
                    }
                    self.fail_async_fixture(
                        cx, AsyncOperationState::Failed, AsyncOperationState::Failed,
                        ExternalRequestPhase::Failed, "EXTERNAL_RESPONSE_NOT_ACCEPTED",
                        "external.response_not_accepted",
                        "fixture transport succeeded but the application intentionally rejected its response",
                    );
                    return;
                }
                self.async_fixture.stage = 2;
                if let Some(task) = &mut self.async_fixture.task {
                    task.state = AsyncOperationState::ProcessingResponse;
                    task.stage = Some("external.response.received".into());
                    task.progress = Some(0.7);
                    task.updated_at_unix_ms = Some(now);
                    task.elapsed_ms = task.started_at_unix_ms.map(|started| now.saturating_sub(started));
                }
                if let Some(request) = &mut self.async_fixture.request {
                    request.state = AsyncOperationState::ProcessingResponse;
                    request.phase = ExternalRequestPhase::ProcessingResponse;
                    request.transport_completed = true;
                    request.response_received = true;
                    request.status_code = Some(200);
                    request.bytes_received = Some(64);
                    request.elapsed_ms = request
                        .started_at_unix_ms
                        .map(|started| now.saturating_sub(started));
                    request.response = Some(serde_json::json!({"accepted_candidate":true,"source":"fixture"}));
                    request.updated_at_unix_ms = now;
                }
                self.fixture_milestone("external.response.received", AsyncOperationState::ProcessingResponse);
                self.fixture_trace(
                    "info",
                    "external.response.received",
                    "fixture response received; application acceptance remains pending",
                    serde_json::json!({"status_code":200}),
                );
                self.status(cx, "Status: async response received");
                self.publish_fixture_telemetry(cx);
                self.async_timer = cx.start_timeout(ASYNC_PHASE_DELAY_SECONDS);
            }
            90 => self.fail_async_fixture(
                cx,
                AsyncOperationState::TimedOut,
                AsyncOperationState::TimedOut,
                ExternalRequestPhase::TimedOut,
                "EXTERNAL_REQUEST_TIMEOUT",
                "external.timed_out",
                "fixture external request reached its deterministic timeout",
            ),
            _ => match failure_mode {
                AsyncFailureMode::FileNeverAppears => self.fail_after_accepted_response(
                    cx,
                    "FILE_NOT_FOUND",
                    "file.missing",
                    "fixture accepted the response but intentionally did not create its output file",
                ),
                AsyncFailureMode::FileNeverStable => self.advance_unstable_file(cx),
                _ => self.finish_async_success(cx),
            },
        }
    }

    fn finish_async_success(&mut self, cx: &mut Cx) {
        if let Err(error) = write_async_fixture_artifact("completed", 0) {
            self.fail_after_accepted_response(cx, "FILE_NOT_FOUND", "file.write_failed", &error);
            return;
        }
        let now = now_unix_ms();
        if let Some(task) = &mut self.async_fixture.task {
            task.state = AsyncOperationState::Completed;
            task.stage = Some("completed".into());
            task.progress = Some(1.0);
            task.completed_at_unix_ms = Some(now);
            task.updated_at_unix_ms = Some(now);
            task.elapsed_ms = task
                .started_at_unix_ms
                .map(|started| now.saturating_sub(started));
        }
        if let Some(request) = &mut self.async_fixture.request {
            request.state = AsyncOperationState::Completed;
            request.phase = ExternalRequestPhase::Completed;
            request.application_accepted_response = true;
            request.elapsed_ms = request
                .started_at_unix_ms
                .map(|started| now.saturating_sub(started));
            request.updated_at_unix_ms = now;
        }
        if self.async_fixture.failure_mode == AsyncFailureMode::MissingMilestone {
            self.fixture_trace(
                "warn",
                "milestone.suppressed",
                "fixture intentionally omitted the completion milestone",
                serde_json::json!({"expected":"task.completed"}),
            );
        } else {
            self.fixture_milestone("task.completed", AsyncOperationState::Completed);
        }
        self.fixture_trace(
            "info",
            "task.completed",
            "deterministic async task completed",
            serde_json::json!({"artifact":ASYNC_ARTIFACT_RELATIVE_PATH}),
        );
        self.complete_async(cx);
        self.publish_fixture_telemetry(cx);
    }

    fn accept_external_response_for_file_work(&mut self) {
        if self.async_fixture.unstable_writes != 0 {
            return;
        }
        let now = now_unix_ms();
        let accepted = if let Some(request) = &mut self.async_fixture.request {
            request.state = AsyncOperationState::Completed;
            request.phase = ExternalRequestPhase::Completed;
            request.transport_completed = true;
            request.response_received = true;
            request.status_code = Some(200);
            request.application_accepted_response = true;
            request.elapsed_ms = request
                .started_at_unix_ms
                .map(|started| now.saturating_sub(started));
            request.updated_at_unix_ms = now;
            request.error = None;
            true
        } else {
            false
        };
        if accepted {
            self.fixture_milestone("external.response.accepted", AsyncOperationState::Running);
            self.fixture_trace(
                "info",
                "external.response.accepted",
                "fixture accepted the response before starting deterministic file work",
                serde_json::json!({"artifact":ASYNC_ARTIFACT_RELATIVE_PATH}),
            );
        }
    }

    fn advance_unstable_file(&mut self, cx: &mut Cx) {
        self.accept_external_response_for_file_work();
        let next_write = self.async_fixture.unstable_writes.saturating_add(1);
        if let Err(error) = write_async_fixture_artifact("changing", next_write) {
            self.fail_after_accepted_response(cx, "FILE_NOT_FOUND", "file.write_failed", &error);
            return;
        }
        self.async_fixture.unstable_writes = next_write;
        if next_write >= ASYNC_UNSTABLE_FILE_WRITES {
            self.fail_after_accepted_response(
                cx,
                "FILE_NOT_STABLE",
                "file.not_stable",
                "fixture repeatedly changed its output file until the bounded failure mode completed",
            );
            return;
        }
        let now = now_unix_ms();
        if let Some(task) = &mut self.async_fixture.task {
            task.state = AsyncOperationState::Running;
            task.stage = Some("file.writing".into());
            task.progress = Some(0.8);
            task.updated_at_unix_ms = Some(now);
            task.elapsed_ms = task
                .started_at_unix_ms
                .map(|started| now.saturating_sub(started));
        }
        self.fixture_trace(
            "debug",
            "file.write_progress",
            "fixture rewrote its artifact for the deterministic file-stability failure",
            serde_json::json!({"write":next_write}),
        );
        self.status(cx, "Status: async output is intentionally changing");
        self.publish_fixture_telemetry(cx);
        self.async_timer = cx.start_timeout(ASYNC_PHASE_DELAY_SECONDS);
    }

    fn fail_async_fixture(
        &mut self,
        cx: &mut Cx,
        task_state: AsyncOperationState,
        request_state: AsyncOperationState,
        request_phase: ExternalRequestPhase,
        code: &str,
        event: &str,
        message: &str,
    ) {
        let now = now_unix_ms();
        let failure = ObservationFailure {
            code: code.into(),
            message: message.into(),
            details: serde_json::json!({"failure_mode":self.async_fixture.failure_mode.as_str()}),
        };
        if let Some(task) = &mut self.async_fixture.task {
            task.state = task_state;
            task.stage = Some(event.into());
            task.completed_at_unix_ms = Some(now);
            task.updated_at_unix_ms = Some(now);
            task.elapsed_ms = task
                .started_at_unix_ms
                .map(|started| now.saturating_sub(started));
            task.error = Some(failure.clone());
        }
        if let Some(request) = &mut self.async_fixture.request {
            request.state = request_state;
            request.phase = request_phase;
            request.application_accepted_response = false;
            request.elapsed_ms = request
                .started_at_unix_ms
                .map(|started| now.saturating_sub(started));
            request.updated_at_unix_ms = now;
            request.error = Some(failure);
        }
        self.fixture_milestone(event, task_state);
        self.fixture_trace("error", event, message, serde_json::json!({"code":code}));
        self.status(cx, format!("Status: async fixture failed ({code})"));
        self.async_pending = false;
        self.publish_fixture_telemetry(cx);
    }

    fn fail_after_accepted_response(
        &mut self,
        cx: &mut Cx,
        code: &str,
        event: &str,
        message: &str,
    ) {
        let now = now_unix_ms();
        if let Some(task) = &mut self.async_fixture.task {
            task.state = AsyncOperationState::Failed;
            task.stage = Some(event.into());
            task.completed_at_unix_ms = Some(now);
            task.updated_at_unix_ms = Some(now);
            task.elapsed_ms = task
                .started_at_unix_ms
                .map(|started| now.saturating_sub(started));
            task.error = Some(ObservationFailure {
                code: code.into(),
                message: message.into(),
                details: serde_json::json!({"failure_mode":self.async_fixture.failure_mode.as_str()}),
            });
        }
        if let Some(request) = &mut self.async_fixture.request {
            request.state = AsyncOperationState::Completed;
            request.phase = ExternalRequestPhase::Completed;
            request.transport_completed = true;
            request.response_received = true;
            request.status_code = Some(200);
            request.application_accepted_response = true;
            request.elapsed_ms = request
                .started_at_unix_ms
                .map(|started| now.saturating_sub(started));
            request.updated_at_unix_ms = now;
            request.error = None;
        }
        self.fixture_milestone(event, AsyncOperationState::Failed);
        self.fixture_trace("error", event, message, serde_json::json!({"code":code}));
        self.status(cx, format!("Status: async fixture failed ({code})"));
        self.async_pending = false;
        self.publish_fixture_telemetry(cx);
    }
    fn fixture_milestone(&mut self, name: &str, state: AsyncOperationState) {
        let operation_id = self
            .async_fixture
            .task
            .as_ref()
            .map(|task| task.operation_id.clone());
        self.async_fixture.milestones.push(RuntimeMilestone {
            name: name.into(),
            operation_id,
            state: Some(state),
            timestamp_unix_ms: now_unix_ms(),
            metadata: serde_json::json!({"source":"makepad_fixture"}),
        });
        if self.async_fixture.milestones.len() > 32 {
            self.async_fixture.milestones.remove(0);
        }
    }

    fn fixture_trace(
        &mut self,
        level: &str,
        event: &str,
        message: &str,
        fields: serde_json::Value,
    ) {
        let trace = TraceEvent {
            timestamp_unix_ms: now_unix_ms(),
            level: level.into(),
            event: event.into(),
            message: message.into(),
            fields,
        };
        eprintln!(
            "AI_UI_TRACE {}",
            serde_json::to_string(&trace)
                .unwrap_or_else(|_| "{\"event\":\"trace_encode_failed\"}".into())
        );
        self.async_fixture.logs.push(trace);
        if self.async_fixture.logs.len() > 64 {
            self.async_fixture.logs.remove(0);
        }
    }

    fn fixture_snapshot(&self) -> TelemetrySnapshot {
        let mut tasks = BTreeMap::new();
        if let Some(task) = &self.async_fixture.task {
            tasks.insert(task.operation_id.clone(), task.clone());
        }
        let mut external_requests = BTreeMap::new();
        if let Some(request) = &self.async_fixture.request {
            external_requests.insert(request.request_id.clone(), request.clone());
        }
        TelemetrySnapshot {
            version: OBSERVATION_VERSION,
            tasks,
            milestones: self.async_fixture.milestones.clone(),
            external_requests,
            logs: self.async_fixture.logs.clone(),
            runtime: serde_json::json!({
                "healthy":true,
                "source":"makepad_accessibility_bridge",
                "artifact":{"root":ASYNC_ARTIFACT_ROOT,"relative_path":ASYNC_ARTIFACT_PATH},
                "failure_mode":self.async_fixture.failure_mode.as_str(),
                "visual":{
                    "source":"makepad",
                    "viewport":"Unknown",
                    "clip":"Unknown",
                    "z_order":"Unknown",
                    "draw_called":"Unknown",
                    "scroll_offset":"Unknown",
                    "zoom":"Unknown",
                    "layout_generation":"Unknown",
                    "canvas_health":"Unknown"
                }
            }),
        }
    }

    fn publish_fixture_telemetry(&self, cx: &mut Cx) {
        let snapshot = self.fixture_snapshot();
        let value = serde_json::to_string(&snapshot).unwrap_or_else(|error| {
            serde_json::json!({
                "version":OBSERVATION_VERSION,
                "runtime":{"healthy":false,"error":error.to_string()}
            })
            .to_string()
        });
        self.ui
            .text_input(cx, ids!(telemetry_snapshot))
            .set_text(cx, &value);
    }

    #[cfg(windows)]
    fn handle_accessibility_action(&mut self, cx: &mut Cx, request: AccessibilityActionRequest) {
        let node_id = request.node_id.clone();
        let action = request.action.clone();
        let result =
            MAKEPAD_PROBE.with(|probe| probe.borrow_mut().dispatch_action(cx, &node_id, &action));
        if let Err(error) = result {
            eprintln!("advanced Makepad Probe action failed for {node_id}: {error}");
        }
        if matches!(action, AccessibilityAction::SetValue(_)) && node_id.ends_with("/input.name") {
            if let AccessibilityAction::SetValue(value) = action {
                self.input_value = value;
            }
        }
    }

    #[cfg(windows)]
    fn sync_probe_lifecycle(&self) {
        MAKEPAD_PROBE.with(|probe| {
            let mut probe = probe.borrow_mut();
            probe.set_visibility_override("window.main", true);
            probe.set_visibility_override("window.main/dialog.modal", self.modal_open);
            probe.set_visibility_override("window.main/body/context.menu", self.context_open);
            probe.set_selected_override("window.main/body/tree.section", self.tree_expanded);
        });
    }

    #[cfg(windows)]
    fn refresh_accessibility(&mut self, cx: &mut Cx) {
        let Ok(hwnd) = makepad_hwnd() else { return };
        let Ok(geometry) = ProbeGeometry::for_window(hwnd) else {
            return;
        };
        self.sync_probe_lifecycle();
        let snapshot =
            MAKEPAD_PROBE.with(|probe| probe.borrow_mut().snapshot(cx, &self.ui, geometry));
        let Ok(mut snapshot) = snapshot else { return };
        normalize_overlay_bounds(&mut snapshot.tree);
        ACCESSIBILITY_BRIDGE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if let Some(bridge) = slot.as_ref() {
                let _ = bridge.update(snapshot.tree);
                return;
            }
            let handler: AccessibilityActionHandler = Arc::new(|request| {
                Cx::post_action(AccessibilityUiAction(request));
                Ok(())
            });
            if let Ok(bridge) = WindowsAccessibilityBridge::attach(hwnd, snapshot.tree, handler) {
                *slot = Some(bridge);
                eprintln!(
                    "advanced Makepad Probe attached; widget types: {}",
                    snapshot.widget_types.join(", ")
                );
            }
        });
    }

    #[cfg(windows)]
    fn item_contains(&self, cx: &Cx, id: &[LiveId], point: Vec2d) -> bool {
        self.ui.widget(cx, id).area().rect(cx).contains(point)
    }
}

#[cfg(windows)]
fn normalize_overlay_bounds(tree: &mut AccessibilityTree) {
    let window_bounds = tree.root.bounds;
    normalize_node_bounds(&mut tree.root, window_bounds);
}

#[cfg(windows)]
fn normalize_node_bounds(node: &mut AccessibilityNode, window_bounds: AccessibilityRect) {
    if node.role == AccessibilityRole::Pane
        && node.visible
        && (node.bounds.width <= 0.0 || node.bounds.height <= 0.0)
    {
        node.bounds = window_bounds;
    }
    for child in &mut node.children {
        normalize_node_bounds(child, window_bounds);
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.input_value.clear();
        self.result_value = "Result: waiting for input".into();
        self.status_value = "Status: ready".into();
        self.tree_expanded = false;
        self.modal_open = false;
        self.context_open = false;
        self.async_pending = false;
        self.async_fixture = AsyncFixtureState::default();
        self.crash_before_action_pending =
            test_injection_enabled(CRASH_BEFORE_ACTION_INJECTION_ENV);
        if self.crash_before_action_pending {
            self.crash_before_action_timer = cx.start_timeout(2.0);
        }
        self.hide_window_pending = test_injection_enabled(HIDE_WINDOW_INJECTION_ENV);
        if self.hide_window_pending {
            self.hide_window_timer = cx.start_timeout(2.0);
        }
        self.drag_armed = false;
        self.drop_count = 0;
        self.ui
            .button(cx, ids!(button_enable))
            .set_disabled(cx, true);
        self.ui
            .widget(cx, ids!(context_menu))
            .set_visible(cx, false);
        self.ui
            .widget(cx, ids!(tree_children))
            .set_visible(cx, false);
        self.ui
            .widget(cx, ids!(async_result))
            .set_visible(cx, false);
        self.ui
            .drop_down(cx, ids!(settings_mode))
            .set_labels(cx, vec!["basic".into(), "advanced".into(), "expert".into()]);
        self.update_labels(cx);
        self.publish_fixture_telemetry(cx);
        #[cfg(windows)]
        self.refresh_accessibility(cx);
    }

    fn handle_timer(&mut self, cx: &mut Cx, event: &TimerEvent) {
        if self.async_pending && self.async_timer.is_timer(event).is_some() {
            self.advance_async_fixture(cx);
        }
        if self.crash_before_action_pending
            && self.crash_before_action_timer.is_timer(event).is_some()
        {
            self.crash_before_action_pending = false;
            eprintln!("AI_UI_INTENTIONAL_RUNTIME_FAILURE_CRASH_BEFORE_ACTION");
            panic!("intentional Makepad crash before requested action");
        }
        if self.hide_window_pending && self.hide_window_timer.is_timer(event).is_some() {
            self.hide_window_pending = false;
            #[cfg(windows)]
            if let Ok(hwnd) = makepad_hwnd() {
                let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
                eprintln!("AI_UI_INTENTIONAL_WINDOW_HIDDEN");
            } else {
                eprintln!("AI_UI_INTENTIONAL_WINDOW_HIDE_FAILED");
            }
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        #[cfg(windows)]
        {
            let requests = actions
                .iter()
                .filter_map(|action| action.downcast_ref::<AccessibilityUiAction>())
                .map(|action| action.0.clone())
                .collect::<Vec<_>>();
            for request in requests {
                self.handle_accessibility_action(cx, request);
            }
        }
        if let Some(text) = self.ui.text_input(cx, ids!(input_name)).changed(actions) {
            self.input_value = text;
            self.status(cx, "Status: input changed");
        }
        if self.ui.button(cx, ids!(button_create)).clicked(actions) {
            self.submit(cx);
        }
        if let Some(value) = self
            .ui
            .check_box(cx, ids!(settings_option))
            .changed(actions)
        {
            self.ui
                .button(cx, ids!(button_enable))
                .set_disabled(cx, !value);
            self.status(
                cx,
                if value {
                    "Status: setting enabled"
                } else {
                    "Status: setting disabled"
                },
            );
        }
        if self.ui.button(cx, ids!(button_enable)).clicked(actions) {
            self.result_value = "Result: advanced action invoked".into();
            self.status(cx, "Status: enabled action completed");
        }
        if let Some(label) = self
            .ui
            .drop_down(cx, ids!(settings_mode))
            .changed_label(actions)
        {
            self.status(cx, format!("Status: mode {label}"));
        }
        if self.ui.button(cx, ids!(button_open_modal)).clicked(actions) {
            self.open_dialog(cx);
        }
        if self
            .ui
            .button(cx, ids!(button_close_modal))
            .clicked(actions)
        {
            self.close_dialog(cx);
        }
        if self.ui.button(cx, ids!(button_async)).clicked(actions) {
            self.trigger_async(cx);
        }
        if self.ui.button(cx, ids!(button_crash)).clicked(actions) {
            eprintln!("AI_UI_INTENTIONAL_RUNTIME_FAILURE");
            panic!("intentional Makepad runtime failure requested by reliability regression");
        }
        if self.ui.button(cx, ids!(context_delete)).clicked(actions) {
            self.set_context(cx, false);
            self.status(cx, "Status: context item deleted");
        }
        if let Some(expanded) = self.ui.check_box(cx, ids!(tree_section)).changed(actions) {
            self.tree_expanded = expanded;
            self.ui
                .widget(cx, ids!(tree_children))
                .set_visible(cx, self.tree_expanded);
            self.status(
                cx,
                if self.tree_expanded {
                    "Status: tree expanded"
                } else {
                    "Status: tree collapsed"
                },
            );
        }
        #[cfg(windows)]
        self.refresh_accessibility(cx);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::KeyDown(key) = event {
            if key.key_code == KeyCode::Escape && self.modal_open {
                self.close_dialog(cx);
            }
            if key.key_code == KeyCode::KeyS && key.modifiers.control {
                self.status(cx, "Status: Ctrl+S handled");
            }
        }
        #[cfg(windows)]
        {
            if let Event::MouseUp(mouse) = event {
                if mouse.button == MouseButton::SECONDARY {
                    self.set_context(cx, true);
                }
                if mouse.button == MouseButton::PRIMARY && self.drag_armed {
                    self.drag_armed = false;
                    if self.item_contains(cx, ids!(drop_target), mouse.abs) {
                        self.drop_count += 1;
                        self.status(
                            cx,
                            format!("Status: drop completed count={}", self.drop_count),
                        );
                    }
                }
            }
            if let Event::MouseDown(mouse) = event {
                if mouse.button == MouseButton::PRIMARY
                    && self.item_contains(cx, ids!(item_50), mouse.abs)
                {
                    self.drag_armed = true;
                }
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        #[cfg(windows)]
        if matches!(event, Event::Draw(_)) {
            self.refresh_accessibility(cx);
        }
    }
}

#[cfg(windows)]
fn test_injection_enabled(name: &str) -> bool {
    matches!(std::env::var(name).as_deref(), Ok("1"))
}

#[cfg(windows)]
fn makepad_hwnd() -> Result<HWND, String> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let handles = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        handles.push(hwnd);
        TRUE
    }

    fn title(hwnd: HWND) -> String {
        unsafe {
            let length = GetWindowTextLengthW(hwnd).max(0) as usize;
            let mut buffer = vec![0u16; length.saturating_add(1).max(1)];
            let read = GetWindowTextW(hwnd, &mut buffer).max(0) as usize;
            String::from_utf16_lossy(&buffer[..read])
        }
    }

    let mut handles = Vec::<HWND>::new();
    unsafe {
        EnumWindows(
            Some(collect),
            LPARAM((&mut handles as *mut Vec<HWND>) as isize),
        )
    }
    .map_err(|error| error.to_string())?;
    handles
        .into_iter()
        .find(|hwnd| {
            let mut process_id = 0;
            unsafe { GetWindowThreadProcessId(*hwnd, Some(&mut process_id)) };
            process_id == std::process::id() && title(*hwnd) == WINDOW_TITLE
        })
        .ok_or_else(|| "current process Makepad window was not found".into())
}

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(1100, 760)
                window.title: "AI UI Advanced Makepad Probe"
                body +: {
                    width: Fill height: Fill flow: Down spacing: 4 padding: 8 show_bg: true draw_bg.color: #x172033
                    title := Label { height: 20 text: "Advanced semantic interaction probe" }
                    subtitle := Label { height: 20 text: "Runtime tree, selection, context, scroll, drag, keyboard and async state" }
                    form_main := View { width: Fill height: 36 flow: Right spacing: 8
                        input_name := TextInput { width: Fill height: 32 empty_text: "Name" }
                        button_create := Button { text: "Create greeting" }
                    }
                    settings_row := View { width: Fill height: 36 flow: Right spacing: 12
                        settings_option := CheckBox { text: "Settings option" }
                        settings_mode := DropDown { width: 180 height: 32 }
                        button_enable := Button { text: "Enabled action" }
                    }
                    button_async := Button { text: "Trigger async result" }
                    button_crash := Button { text: "Intentional runtime failure" }
                    async_result := View { visible: false width: Fill height: Fit padding: 8 show_bg: true draw_bg.color: #x28553e
                        async_label := Label { text: "Async result is ready" }
                    }
                    telemetry_snapshot := TextInput { visible: false is_read_only: true text: "" }
                    project_list := View { width: Fill height: Fit flow: Down spacing: 4
                        list_title := Label { height: 20 text: "Multi-select scroll list" }
                        scroll_list := ScrollYView { width: Fill height: 78 flow: Down spacing: 2
                            item_0 := CheckBox { text: "Item 0" }
                            item_1 := CheckBox { text: "Item 1" }
                            item_2 := CheckBox { text: "Item 2" }
                            item_3 := CheckBox { text: "Item 3" }
                            item_4 := CheckBox { text: "Item 4" }
                            item_5 := CheckBox { text: "Item 5" }
                            item_6 := CheckBox { text: "Item 6" }
                            spacer := View { width: Fill height: 850 }
                            item_50 := CheckBox { text: "Item 50 - drag me" }
                        }
                    }
                    drop_target := Button { text: "Drop target" }
                    tree_section := CheckBox { text: "Expandable tree section" }
                    tree_children := View { visible: false width: Fill height: Fit padding: 8 show_bg: true draw_bg.color: #x29405b
                        tree_child_label := Label { text: "Tree child is visible" }
                    }
                    context_menu := View { visible: false width: Fill height: Fit padding: 8 flow: Right spacing: 8 show_bg: true draw_bg.color: #x3b2f55
                        context_caption := Label { text: "Context menu" }
                        context_delete := Button { text: "Delete" }
                    }
                    button_open_modal := Button { text: "Open modal" }
                    result_message := Label { width: Fill height: Fit text: "Result: waiting for input" }
                    status := Label { width: Fill height: Fit text: "Status: ready" }
                }
                dialog_modal := Modal {
                    content +: { width: 460 height: Fit flow: Down spacing: 10 padding: 20 show_bg: true draw_bg.color: #x2a3855
                        dialog_title := Label { text: "Advanced probe dialog" }
                        dialog_value := Label { text: "Current result" }
                        button_close_modal := Button { text: "Close dialog" }
                    }
                }
            }
        }
    }
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn async_fixture_artifact() -> Result<PathBuf, String> {
    let root = std::env::var_os("AI_UI_TEST_OUTPUT_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| {
            "AI_UI_TEST_OUTPUT_DIR is not configured by the owned test host".to_owned()
        })?;
    Ok(root.join(ASYNC_ARTIFACT_RELATIVE_PATH))
}

fn clear_async_fixture_artifact() -> Result<(), String> {
    let artifact = async_fixture_artifact()?;
    if artifact.is_file() {
        fs::remove_file(&artifact).map_err(|error| error.to_string())?;
    } else if artifact.exists() {
        return Err("fixture artifact path is not a regular file".into());
    }
    Ok(())
}

fn write_async_fixture_artifact(status: &str, write_sequence: u8) -> Result<(), String> {
    let artifact = async_fixture_artifact()?;
    let parent = artifact
        .parent()
        .ok_or_else(|| "fixture artifact path has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let payload = serde_json::json!({
        "fixture":"makepad-async",
        "status":status,
        "write_sequence":write_sequence,
        "written_at_unix_ms":now_unix_ms()
    });
    fs::write(
        artifact,
        serde_json::to_vec_pretty(&payload).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_failure_mode_is_a_strict_allowlist() {
        for (value, expected) in [
            ("task_fails", AsyncFailureMode::TaskFails),
            ("api_timeout", AsyncFailureMode::ApiTimeout),
            ("api_http_failure", AsyncFailureMode::ApiHttpFailure),
            ("api_invalid_response", AsyncFailureMode::ApiInvalidResponse),
            ("api_not_accepted", AsyncFailureMode::ApiNotAccepted),
            ("file_never_appears", AsyncFailureMode::FileNeverAppears),
            ("file_never_stable", AsyncFailureMode::FileNeverStable),
            ("missing_milestone", AsyncFailureMode::MissingMilestone),
        ] {
            assert_eq!(AsyncFailureMode::parse(value), Some(expected));
        }
        assert_eq!(AsyncFailureMode::parse("arbitrary"), None);
    }

    #[test]
    fn unstable_fixture_has_multiple_changes_and_finishes_before_task_wait() {
        assert!(ASYNC_UNSTABLE_FILE_WRITES >= 3);
        let elapsed_after_first_write_ms = u64::from(ASYNC_UNSTABLE_FILE_WRITES.saturating_sub(1))
            * (ASYNC_PHASE_DELAY_SECONDS * 1_000.0) as u64;
        assert!(elapsed_after_first_write_ms < 5_000);
    }

    #[test]
    fn async_phase_delay_keeps_transient_states_observable_after_action_settle() {
        assert!(ASYNC_PHASE_DELAY_SECONDS >= 1.0);
    }
}
