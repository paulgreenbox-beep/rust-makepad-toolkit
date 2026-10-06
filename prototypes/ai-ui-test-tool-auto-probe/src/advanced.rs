//! Capability-driven semantic interaction host for the automatic Makepad probe.
//!
//! Runtime/UIA operations stay behind the existing TestHost contract. Pointer
//! and keyboard injection are deliberately isolated here as fallbacks for
//! gestures that UI Automation cannot express.

use crate::host::{HostError, HostResult, TestHost};
use crate::spatial::{
    CoordinateSpace, DpiMetadata, Modifier, RelativePoint, RelativeRect, ResolvedPoint,
    parse_modifiers,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use toolkit_windows_automation::{
    ControlInfo, ControlTreeResult, ElementState, Rect, WindowFrameTarget, WindowFrameTargetKind,
    WindowInfo, WindowInteractionInfo, WindowState,
};

#[cfg(windows)]
use windows_uia::Win32::UI::{
    HiDpi::GetDpiForWindow,
    Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
        KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
        MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_CONTROL,
    },
    WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    },
};

const WAIT_MS: u64 = 5_000;
const WHEEL: i32 = 120;
const POINTER_TARGET_SETTLE_MS: u64 = 250;
const WHEEL_DISPATCH_SETTLE_MS: u64 = 100;
const ENSURE_VISIBLE_TIMEOUT_SECS: u64 = 12;
const ENSURE_VISIBLE_MAX_WHEEL_STEPS: i32 = 4;
const RECENT_ACTIONS: usize = 20;

#[derive(Clone, Debug, Serialize)]
pub struct SemanticControl {
    pub id: String,
    pub role: String,
    pub name: String,
    pub bounds: Rect,
    pub visible: bool,
    pub enabled: bool,
    pub focused: bool,
    pub selected: Option<bool>,
    pub checked: Option<bool>,
    pub expanded: Option<bool>,
    pub editable: Option<bool>,
    pub readonly: Option<bool>,
    pub modal: Option<bool>,
    pub active: Option<bool>,
    pub scrollable: Option<bool>,
    pub scroll_position: Option<f64>,
    pub value: Option<String>,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct RecentAction {
    sequence: usize,
    operation: String,
    arguments: Value,
    success: bool,
    response: Value,
}

pub struct AdvancedHost {
    base: TestHost,
    window: Option<WindowInfo>,
    output_dir: PathBuf,
    last_tree: Option<ControlTreeResult>,
    recent: Vec<RecentAction>,
    sequence: usize,
    pointer_button_state: Option<String>,
    pressed_modifiers: Vec<Modifier>,
    input_lock: bool,
}

impl AdvancedHost {
    pub fn new(app_path: impl Into<PathBuf>, output_dir: impl Into<PathBuf>) -> HostResult<Self> {
        Self::with_title(app_path, crate::host::DEFAULT_WINDOW_TITLE, output_dir)
    }

    pub fn with_title(
        app_path: impl Into<PathBuf>,
        window_title: impl Into<String>,
        output_dir: impl Into<PathBuf>,
    ) -> HostResult<Self> {
        Self::with_title_and_environment(app_path, window_title, output_dir, BTreeMap::new())
    }

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

    pub(crate) fn with_title_and_environment_and_arguments(
        app_path: impl Into<PathBuf>,
        window_title: impl Into<String>,
        output_dir: impl Into<PathBuf>,
        launch_environment: BTreeMap<String, String>,
        launch_arguments: Vec<String>,
    ) -> HostResult<Self> {
        let output_dir = output_dir.into();
        Ok(Self {
            base: TestHost::with_title_and_environment_and_arguments(
                app_path,
                window_title,
                &output_dir,
                launch_environment,
                launch_arguments,
            )?,
            window: None,
            output_dir,
            last_tree: None,
            recent: Vec::new(),
            sequence: 0,
            pointer_button_state: None,
            pressed_modifiers: Vec::new(),
            input_lock: false,
        })
    }

    pub fn call(&mut self, operation: &str, arguments: Value) -> Value {
        let result = self.call_result(operation, arguments.clone());
        if result.is_err() {
            self.cleanup_pointer();
        }
        let response = crate::contract::canonicalize_response(
            operation,
            match result {
                Ok(value) => json!({"success": true, "result": value}),
                Err(error) => json!({"success": false, "error": error}),
            },
            None,
        );
        self.record(operation, arguments, response.clone());
        response
    }

    pub fn recent_actions(&self) -> Vec<Value> {
        self.recent
            .iter()
            .map(|entry| serde_json::to_value(entry).unwrap_or(Value::Null))
            .collect()
    }

    pub fn test_host(&self) -> &TestHost {
        &self.base
    }

    pub fn test_host_mut(&mut self) -> &mut TestHost {
        &mut self.base
    }

    pub fn current_window(&self) -> Option<&WindowInfo> {
        self.window.as_ref()
    }

    /// Bind subsequent operations to a different top-level HWND owned by this
    /// session's launched process. The opaque ID is checked by TestHost.
    pub fn select_window(&mut self, window_id: &str) -> HostResult<WindowInfo> {
        let selected = self.base.select_window(window_id)?;
        self.window = Some(selected.clone());
        self.last_tree = None;
        Ok(selected)
    }

    pub fn last_tree(&self) -> Option<&ControlTreeResult> {
        self.last_tree.as_ref().or_else(|| self.base.last_tree())
    }

    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    pub fn write_failure_evidence(&mut self) -> HostResult<()> {
        fs::create_dir_all(&self.output_dir)
            .map_err(|error| io_error("create failure evidence directory", error))?;
        if let Ok(tree) = self.inspect_ui() {
            write_json(&self.output_dir.join("last-ui-tree.json"), &tree)?;
        }
        write_json(
            &self.output_dir.join("recent-ui-changes.json"),
            &self.base.get_ui_changes(),
        )?;
        write_json(
            &self.output_dir.join("recent-actions.json"),
            &self.recent_actions(),
        )?;
        if let Ok(value) = self.screenshot_value() {
            if let Some(source) = value.get("path").and_then(Value::as_str) {
                let _ = fs::copy(source, self.output_dir.join("failure-screenshot.png"));
            }
        }
        Ok(())
    }

    fn call_result(&mut self, operation: &str, arguments: Value) -> HostResult<Value> {
        match operation {
            "launch" => self.launch().and_then(to_value),
            "restart_app" => self.restart().and_then(to_value),
            "shutdown" => self.shutdown().map(|_| Value::Null),
            "inspect_ui" => self.inspect_ui().and_then(to_value),
            "inspect_semantic" => self.inspect_semantic(),
            "click" | "input" | "read" | "wait" | "assert" | "screenshot" => self.legacy(operation, arguments),
            "get_logs" => Ok(json!(self.base.get_logs())),
            "get_ui_changes" => Ok(json!(self.base.get_ui_changes())),
            "inspect_spatial_target" => self.inspect_spatial_target(&arguments),
            "move_pointer" | "windows_move_pointer" => self.move_pointer(&arguments),
            "pointer_down" | "windows_pointer_down" => self.pointer_button(&arguments, true),
            "pointer_up" | "windows_pointer_up" => self.pointer_button(&arguments, false),
            "click_relative" | "windows_click_relative" => self.click_relative(&arguments, Pointer::Click),
            "double_click_relative" | "windows_double_click_relative" => self.click_relative(&arguments, Pointer::DoubleClick),
            "right_click_relative" | "windows_right_click_relative" => self.click_relative(&arguments, Pointer::RightClick),
            "drag_relative" | "windows_drag_relative" => self.drag_relative(&arguments),
            "select_region" | "windows_select_region" => self.select_region(&arguments),
            "hover_relative" => self.hover_relative(&arguments),
            "wheel_relative" => self.wheel_relative(&arguments),
            "capture_region" | "windows_capture_region" => self.capture_region(&arguments),
            "inspect_window_interaction" => self.inspect_window_interaction(),
            "windows_drag_title" => self.drag_title(&arguments),
            "windows_resize_frame" => self.resize_frame(&arguments),
            "windows_double_click_title" => self.double_click_title(&arguments),
            "windows_set_window_state" => self.set_window_state(&arguments),
            "windows_set_window_bounds" => self.set_window_bounds(&arguments),
            "windows_run_sequence" => self.run_spatial_sequence(&arguments),
            "double_click" => self.pointer_for(&arguments, Pointer::DoubleClick),
            "right_click" => self.pointer_for(&arguments, Pointer::RightClick),
            "hover" => self.pointer_for(&arguments, Pointer::Hover),
            "drag" => self.drag(&arguments),
            "drag_to" => self.drag_to(&arguments),
            "scroll" => self.scroll(&arguments),
            "scroll_to" | "ensure_visible" => {
                let id = self.required(&arguments, "id")?;
                self.ensure_visible(&id)
            }
            "scroll_to_start" => self.scroll_edge(&arguments, false),
            "scroll_to_end" => self.scroll_edge(&arguments, true),
            "scroll_page" => self.scroll_page(&arguments),
            "select" => self.select(&self.required(&arguments, "id")?),
            "add_to_selection" => self.add_selection(&self.required(&arguments, "id")?),
            "remove_from_selection" => self.remove_selection(&self.required(&arguments, "id")?),
            "clear_selection" => self.clear_selection(arguments.get("container_id").and_then(Value::as_str)),
            "select_many" => self.select_many(&arguments),
            "select_range" => self.select_range(&arguments),
            "select_row" | "select_cell" => self.select(&table_target(&arguments)?),
            "select_rows" | "select_cells" => self.select_many(&json!({"ids": arguments.get("ids").cloned().ok_or_else(|| invalid("ids is required"))?})),
            "read_cell" => self.legacy("read", json!({"id": table_target(&arguments)?})),
            "activate_row" | "activate_cell" => self.legacy("click", json!({"id": table_target(&arguments)?})),
            "toggle" => self.toggle(&self.required(&arguments, "id")?),
            "choose" => self.choose(&arguments),
            "expand" | "open" => self.expand(&self.required(&arguments, "id")?),
            "collapse" | "close" => self.expand(&self.required(&arguments, "id")?),
            "focus" => self.focus(&self.required(&arguments, "id")?),
            "activate_window" => self.activate_window(),
            "close_window" => self.send_keys("ALT+F4"),
            "press" => self.send_keys(&normalize_key(&self.required(&arguments, "key")?)),
            "press_repeat" => {
                let count = arguments
                    .get("count")
                    .and_then(Value::as_u64)
                    .filter(|count| *count > 0)
                    .ok_or_else(|| invalid("count must be a positive integer"))?;
                self.send_key_repeat(
                    &normalize_key(&self.required(&arguments, "key")?),
                    count as usize,
                )
            },
            "hotkey" => self.hotkey(&arguments),
            "type_text" => self.send_text(&self.required(&arguments, "text")?),
            "wait_visible" => self.wait_state(&self.required(&arguments, "id")?, |state| state.visible, duration(&arguments)),
            "wait_hidden" => self.wait_hidden(&self.required(&arguments, "id")?, duration(&arguments)),
            "wait_enabled" => self.wait_state(&self.required(&arguments, "id")?, |state| state.enabled, duration(&arguments)),
            "wait_disabled" => self.wait_state(&self.required(&arguments, "id")?, |state| !state.enabled, duration(&arguments)),
            "wait_value" => self.wait_value(&arguments),
            "wait_selected" => self.wait_state(&self.required(&arguments, "id")?, |state| state.selected == Some(true), duration(&arguments)),
            "wait_count" => self.wait_count(&arguments),
            "wait_idle" => self.wait_idle(duration(&arguments)),
            _ => { let response = self.base.call(operation, arguments); self.unwrap_response(response) },
        }
    }

    fn launch(&mut self) -> HostResult<WindowInfo> {
        let window = self.base.launch()?;
        self.window = Some(window.clone());
        Ok(window)
    }
    fn restart(&mut self) -> HostResult<WindowInfo> {
        let window = self.base.restart_app()?;
        self.window = Some(window.clone());
        self.last_tree = None;
        Ok(window)
    }
    fn shutdown(&mut self) -> HostResult<()> {
        self.cleanup_pointer();
        self.base.shutdown()?;
        self.window = None;
        self.last_tree = None;
        Ok(())
    }

    fn inspect_ui(&mut self) -> HostResult<ControlTreeResult> {
        let tree = self.base.inspect_ui()?;
        self.last_tree = Some(tree.clone());
        Ok(tree)
    }

    fn inspect_semantic(&mut self) -> HostResult<Value> {
        let tree = self.inspect_ui()?;
        Ok(json!({"tree":tree,"elements":tree.elements.iter().map(semantic).collect::<Vec<_>>() }))
    }

    fn legacy(&mut self, operation: &str, arguments: Value) -> HostResult<Value> {
        let response = self.base.call(operation, arguments);
        if response["success"] == true {
            if operation == "inspect_ui" {
                self.last_tree = serde_json::from_value(response["result"].clone()).ok();
            }
            Ok(response["result"].clone())
        } else {
            self.unwrap_response(response)
        }
    }

    fn unwrap_response(&self, response: Value) -> HostResult<Value> {
        if response["success"] == true {
            Ok(response["result"].clone())
        } else {
            let error = serde_json::from_value(response["error"].clone())
                .map_err(|error| invalid(error.to_string()))?;
            Err(error)
        }
    }

    fn resolve(&mut self, id: &str) -> HostResult<ControlInfo> {
        let tree = self.inspect_ui()?;
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
            .collect::<Vec<_>>();
        if suffix.len() == 1 {
            return Ok(suffix[0].clone());
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
            .take(8)
            .map(|control| control.automation_id.clone())
            .collect::<Vec<_>>();
        Err(HostError {
            code: "ELEMENT_NOT_FOUND".into(),
            message: format!("no semantic element matched {id}"),
            details: json!({"error":"element_not_found","requested":id,"current_window":self.window.as_ref().map(|window| window.title.clone()),"similar":similar}),
        })
    }

    fn actionable(&self, control: &ControlInfo, action: &str) -> HostResult<()> {
        if !control.visible {
            return Err(HostError {
                code: "ELEMENT_NOT_VISIBLE".into(),
                message: format!("{action} target is not visible"),
                details: json!({"error":"element_not_visible","id":control.automation_id,"suggestion":"call ensure_visible first"}),
            });
        }
        if !control.enabled {
            return Err(HostError {
                code: "UNSUPPORTED_ACTION".into(),
                message: format!("{action} target is disabled"),
                details: json!({"error":"unsupported_action","action":action,"role":control.control_type,"supported_actions":supported(control)}),
            });
        }
        Ok(())
    }

    fn pointer_for(&mut self, arguments: &Value, pointer: Pointer) -> HostResult<Value> {
        let id = self.required(arguments, "id")?;
        let control = self.resolve(&id)?;
        self.actionable(&control, pointer.name())?;
        let result = self.pointer_at(control.bounds, pointer)?;
        let _ = self.inspect_ui();
        Ok(
            json!({"id":control.automation_id,"action":pointer.name(),"fallback":true,"pointer":result}),
        )
    }

    fn drag(&mut self, arguments: &Value) -> HostResult<Value> {
        let source = self.resolve(&required_alias(arguments, &["id", "source_id"])?)?;
        let target = self.resolve(&required_alias(arguments, &["to", "target_id"])?)?;
        self.actionable(&source, "drag")?;
        self.actionable(&target, "drag")?;
        let result = self.drag_at(
            source.bounds,
            target
                .bounds
                .center()
                .ok_or_else(|| invalid("drag target has no actionable bounds"))?,
        )?;
        let _ = self.inspect_ui();
        Ok(
            json!({"source":source.automation_id,"target":target.automation_id,"fallback":true,"pointer":result}),
        )
    }

    fn drag_to(&mut self, arguments: &Value) -> HostResult<Value> {
        let source = self.resolve(&required_alias(arguments, &["id", "source_id"])?)?;
        let target = self.resolve(&required_alias(arguments, &["target_id"])?)?;
        self.actionable(&source, "drag_to")?;
        self.actionable(&target, "drag_to")?;
        let result = self.drag_at(
            source.bounds,
            target
                .bounds
                .center()
                .ok_or_else(|| invalid("drag target has no actionable bounds"))?,
        )?;
        let _ = self.inspect_ui();
        Ok(
            json!({"source":source.automation_id,"target":target.automation_id,"fallback":true,"pointer":result}),
        )
    }

    fn scroll(&mut self, arguments: &Value) -> HostResult<Value> {
        let control = self.resolve(&self.required(arguments, "id")?)?;
        let delta = scroll_delta(arguments);
        let result = self.scroll_at(control.bounds, delta)?;
        let _ = self.inspect_ui();
        Ok(json!({"id":control.automation_id,"delta_y":delta,"fallback":true,"pointer":result}))
    }

    fn ensure_visible(&mut self, id: &str) -> HostResult<Value> {
        let deadline = Instant::now() + Duration::from_secs(ENSURE_VISIBLE_TIMEOUT_SECS);
        let mut attempts = 0;
        loop {
            let control = self.resolve(id)?;
            if control.visible && control.bounds.is_actionable() {
                return Ok(json!({"id":control.automation_id,"visible":true,"attempts":attempts}));
            }
            if Instant::now() >= deadline {
                return Err(HostError {
                    code: "ELEMENT_NOT_VISIBLE".into(),
                    message: format!("could not make {id} visible"),
                    details: json!({"error":"element_not_visible","id":id,"attempts":attempts,"suggestion":"inspect the nearest scroll container"}),
                });
            }
            let container = self.scroll_container(&control)?;
            let delta = ensure_visible_scroll_delta(control.bounds, container.bounds);
            self.scroll_at(container.bounds, delta)?;
            self.inspect_ui()?;
            attempts += 1;
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn scroll_container(&self, target: &ControlInfo) -> HostResult<ControlInfo> {
        let tree = self
            .last_tree
            .as_ref()
            .ok_or_else(|| invalid("UI tree is not available"))?;
        tree.elements.iter().filter(|control| control.control_type == "List" && target.automation_id.starts_with(&(control.automation_id.clone()+"/")) && control.bounds.is_actionable()).max_by_key(|control| control.automation_id.len()).cloned().ok_or_else(|| HostError { code:"UNSUPPORTED_ACTION".into(), message:"no scroll container owns the target".into(), details:json!({"error":"unsupported_action","action":"scroll","id":target.automation_id}) })
    }

    fn scroll_edge(&mut self, arguments: &Value, end: bool) -> HostResult<Value> {
        let control = self.resolve(&self.required(arguments, "id")?)?;
        let container = self.scroll_container(&control)?;
        for _ in 0..12 {
            self.scroll_at(container.bounds, if end { -WHEEL } else { WHEEL })?;
        }
        let _ = self.inspect_ui();
        Ok(
            json!({"id":control.automation_id,"edge":if end {"end"} else {"start"},"container":container.automation_id,"fallback":true}),
        )
    }

    fn scroll_page(&mut self, arguments: &Value) -> HostResult<Value> {
        let id = self.required(arguments, "id")?;
        let direction = arguments
            .get("direction")
            .and_then(Value::as_str)
            .unwrap_or("down");
        let control = self.resolve(&id)?;
        let container = self.scroll_container(&control)?;
        let delta = if matches!(direction, "up" | "backward") {
            WHEEL
        } else {
            -WHEEL
        };
        let pointer = self.scroll_at(container.bounds, delta)?;
        Ok(
            json!({"id":id,"direction":direction,"container":container.automation_id,"fallback":true,"pointer":pointer}),
        )
    }

    fn selection_container(&self, id: &str) -> Option<ControlInfo> {
        self.last_tree
            .as_ref()?
            .elements
            .iter()
            .filter(|control| {
                control.control_type == "List"
                    && id.starts_with(&(control.automation_id.clone() + "/"))
            })
            .max_by_key(|control| control.automation_id.len())
            .cloned()
    }

    fn select(&mut self, id: &str) -> HostResult<Value> {
        let mut control = self.resolve(id)?;
        if !control.visible {
            let target_id = control.automation_id.clone();
            self.ensure_visible(&target_id)?;
            control = self.resolve(&target_id)?;
        }
        self.actionable(&control, "select")?;
        if control.selected == Some(true) {
            return Ok(
                json!({"id":control.automation_id,"selected":true,"action_method":"already_selected"}),
            );
        }
        let result = self.legacy("click", json!({"id":control.automation_id}))?;
        let state = self.wait_state(
            &control.automation_id,
            |state| state.selected == Some(true),
            Duration::from_secs(3),
        )?;
        Ok(
            json!({"id":control.automation_id,"selected":state["selected"].clone(),"action_method":"runtime_selection","result":result}),
        )
    }

    fn add_selection(&mut self, id: &str) -> HostResult<Value> {
        self.ctrl_selection(id, true)
    }
    fn remove_selection(&mut self, id: &str) -> HostResult<Value> {
        self.ctrl_selection(id, false)
    }

    fn ctrl_selection(&mut self, id: &str, selected: bool) -> HostResult<Value> {
        let mut control = self.resolve(id)?;
        if !control.visible {
            let target_id = control.automation_id.clone();
            self.ensure_visible(&target_id)?;
            control = self.resolve(&target_id)?;
        }
        self.actionable(
            &control,
            if selected {
                "add_to_selection"
            } else {
                "remove_from_selection"
            },
        )?;
        if control.selected == Some(selected) {
            return Ok(
                json!({"id":control.automation_id,"selected":selected,"action_method":"already_in_requested_state"}),
            );
        }
        let pointer = self.ctrl_click(control.bounds)?;
        let state = self.wait_state(
            &control.automation_id,
            |state| state.selected == Some(selected),
            Duration::from_secs(3),
        )?;
        Ok(
            json!({"id":control.automation_id,"selected":state["selected"].clone(),"action_method":"synthesized_ctrl_pointer","pointer":pointer}),
        )
    }

    fn clear_selection(&mut self, container: Option<&str>) -> HostResult<Value> {
        let tree = self.inspect_ui()?;
        let selected = tree
            .elements
            .iter()
            .filter(|control| {
                control.selected == Some(true)
                    && container
                        .is_none_or(|id| control.automation_id.starts_with(&(id.to_owned() + "/")))
            })
            .cloned()
            .collect::<Vec<_>>();
        for control in &selected {
            let target_id = control.automation_id.clone();
            self.legacy("click", json!({"id": target_id}))?;
            self.wait_state(
                &control.automation_id,
                |state| state.selected == Some(false),
                Duration::from_secs(3),
            )?;
        }
        let _ = self.inspect_ui();
        Ok(
            json!({"container_id":container,"cleared":selected.iter().map(|control|control.automation_id.clone()).collect::<Vec<_>>(),"selection_count":0,"selection_mode":"multiple"}),
        )
    }

    fn select_many(&mut self, arguments: &Value) -> HostResult<Value> {
        let ids = arguments
            .get("ids")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("ids must be an array"))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("ids must contain strings"))
            })
            .collect::<HostResult<Vec<_>>>()?;
        if ids.is_empty() {
            return Err(invalid("ids must not be empty"));
        }
        let container = self
            .selection_container(&ids[0])
            .map(|control| control.automation_id);
        self.clear_selection(container.as_deref())?;
        self.select(&ids[0])?;
        for id in ids.iter().skip(1) {
            self.add_selection(id)?;
        }
        Ok(
            json!({"selected":ids,"selection_count":self.count_selected(container.as_deref())?,"selection_mode":"multiple"}),
        )
    }

    fn select_range(&mut self, arguments: &Value) -> HostResult<Value> {
        let start = self.required(arguments, "start_id")?;
        let end = self.required(arguments, "end_id")?;
        let container = self
            .selection_container(&start)
            .ok_or_else(|| invalid("range endpoints have no List selection container"))?;
        let candidates = self
            .last_tree
            .as_ref()
            .ok_or_else(|| invalid("UI tree is not available"))?
            .elements
            .iter()
            .filter(|control| {
                control
                    .automation_id
                    .starts_with(&(container.automation_id.clone() + "/"))
                    && control.selected.is_some()
            })
            .cloned()
            .collect::<Vec<_>>();
        let a = candidates
            .iter()
            .position(|control| {
                control.automation_id == start || control.automation_id.ends_with(&start)
            })
            .ok_or_else(|| invalid("range start was not found"))?;
        let b = candidates
            .iter()
            .position(|control| {
                control.automation_id == end || control.automation_id.ends_with(&end)
            })
            .ok_or_else(|| invalid("range end was not found"))?;
        let (low, high) = if a <= b { (a, b) } else { (b, a) };
        self.clear_selection(Some(&container.automation_id))?;
        for control in &candidates[low..=high] {
            self.select(&control.automation_id)?;
        }
        Ok(
            json!({"start_id":start,"end_id":end,"selection_count":self.count_selected(Some(&container.automation_id))?,"selection_mode":"range"}),
        )
    }

    fn count_selected(&mut self, container: Option<&str>) -> HostResult<usize> {
        Ok(self
            .inspect_ui()?
            .elements
            .iter()
            .filter(|control| {
                control.selected == Some(true)
                    && container
                        .is_none_or(|id| control.automation_id.starts_with(&(id.to_owned() + "/")))
            })
            .count())
    }

    fn toggle(&mut self, id: &str) -> HostResult<Value> {
        let control = self.resolve(id)?;
        self.actionable(&control, "toggle")?;
        let result = self.legacy("click", json!({"id":control.automation_id}))?;
        let state = self.read_state(&control.automation_id)?;
        Ok(
            json!({"id":control.automation_id,"checked":state.selected,"selected":state.selected,"result":result}),
        )
    }

    fn choose(&mut self, arguments: &Value) -> HostResult<Value> {
        let id = self.required(arguments, "id")?;
        let value = self.required(arguments, "value")?;
        let control = self.resolve(&id)?;
        if control.control_type != "ComboBox" {
            return Err(HostError {
                code: "UNSUPPORTED_ACTION".into(),
                message: "choose requires ComboBox".into(),
                details: json!({"error":"unsupported_action","action":"choose","role":control.control_type,"supported_actions":supported(&control)}),
            });
        }
        let result = self.legacy("input", json!({"id":control.automation_id,"text":value}))?;
        let state = self.read_state(&control.automation_id)?;
        Ok(
            json!({"id":control.automation_id,"value":state.value,"action_method":"ValuePattern","result":result}),
        )
    }

    fn expand(&mut self, id: &str) -> HostResult<Value> {
        let control = self.resolve(id)?;
        self.actionable(&control, "open")?;
        let result = self.legacy("click", json!({"id":control.automation_id}))?;
        let state = self.read_state(&control.automation_id)?;
        Ok(
            json!({"id":control.automation_id,"expanded":state.selected,"visible":state.visible,"result":result}),
        )
    }
    fn focus(&mut self, id: &str) -> HostResult<Value> {
        let control = self.resolve(id)?;
        self.actionable(&control, "focus")?;
        let window_id = self.window_id()?;
        let state = self
            .base
            .automation_mut()
            .focus_element(&window_id, &control.element_id)?;
        Ok(json!(state))
    }
    fn set_window_bounds(&mut self, a: &Value) -> HostResult<Value> {
        let window_id = self.window_id()?;
        let width = a
            .get("width")
            .and_then(Value::as_i64)
            .ok_or_else(|| invalid("width must be an integer"))? as i32;
        let height = a
            .get("height")
            .and_then(Value::as_i64)
            .ok_or_else(|| invalid("height must be an integer"))? as i32;
        let x = a.get("x").and_then(Value::as_i64).map(|v| v as i32);
        let y = a.get("y").and_then(Value::as_i64).map(|v| v as i32);
        Ok(json!(
            self.base
                .automation_mut()
                .set_window_bounds(&window_id, x, y, width, height)?
        ))
    }

    fn inspect_window_interaction(&mut self) -> HostResult<Value> {
        Ok(json!(self.fresh_window_interaction()?))
    }

    fn set_window_state(&mut self, arguments: &Value) -> HostResult<Value> {
        let state = parse_window_state(arguments)?;
        let before = self.fresh_window_interaction()?;
        let window_id = self.window_id()?;
        let transition = self
            .base
            .automation_mut()
            .set_window_state(&window_id, state)?;
        if !transition.success {
            return Err(HostError {
                code: "WINDOW_STATE_TRANSITION_FAILED".into(),
                message: "ShowWindow did not reach the requested state before the bounded readback deadline".into(),
                details: json!({"before":before,"transition":transition}),
            });
        }
        let after = self.fresh_window_interaction()?;
        let runtime_health = self.runtime_health_after_window_action("windows_set_window_state")?;
        Ok(json!({
            "programmatic":true,
            "physical_input":false,
            "before":before,
            "transition":transition,
            "after":after,
            "runtime_health":runtime_health
        }))
    }

    fn drag_title(&mut self, arguments: &Value) -> HostResult<Value> {
        let delta_x = required_i32(arguments, "delta_x")?;
        let delta_y = required_i32(arguments, "delta_y")?;
        if delta_x == 0 && delta_y == 0 {
            return Err(invalid(
                "windows_drag_title requires a non-zero delta_x or delta_y",
            ));
        }
        let (before, target, title_region) = self.title_frame_target(arguments)?;
        let steps = gesture_steps(arguments);
        let duration_ms = gesture_duration_ms(arguments);
        let end = (
            target.screen_point.x.saturating_add(delta_x),
            target.screen_point.y.saturating_add(delta_y),
        );
        self.focus_and_move(target.screen_point.x, target.screen_point.y)?;
        let injected = self.drag_screen(
            (target.screen_point.x, target.screen_point.y),
            end,
            steps,
            duration_ms,
            &[],
            "left",
        )?;
        let (_, converged) = self.converged_window_after_gesture()?;
        let after = self.fresh_window_interaction()?;
        if !converged {
            return Err(HostError {
                code: "WINDOW_MOVE_VERIFICATION_FAILED".into(),
                message: "window bounds did not converge after the physical title drag".into(),
                details: json!({"before":before,"after":after,"target":target}),
            });
        }
        let moved = before.window.bounds.x != after.window.bounds.x
            || before.window.bounds.y != after.window.bounds.y;
        if !moved {
            return Err(HostError {
                code: "WINDOW_BOUNDS_NOT_CHANGED".into(),
                message: "physical title drag was dispatched but the top-level HWND position did not change".into(),
                details: json!({"before":before,"after":after,"target":target,"requested_delta":{"x":delta_x,"y":delta_y}}),
            });
        }
        let layout = self.layout_after_window_action()?;
        let runtime_health = self.runtime_health_after_window_action("windows_drag_title")?;
        Ok(json!({
            "physical_input":true,
            "pointer_sequence":"move → left_down → drag → left_up",
            "pre_bounds":before.window.bounds,
            "post_bounds":after.window.bounds,
            "pre":before,
            "post":after,
            "frame_target":target,
            "semantic_title_region":title_region,
            "requested_delta":{"x":delta_x,"y":delta_y},
            "actual_delta":{"x":after.window.bounds.x.saturating_sub(before.window.bounds.x),"y":after.window.bounds.y.saturating_sub(before.window.bounds.y)},
            "steps":steps,
            "duration_ms":duration_ms,
            "injected":injected,
            "readback_converged":converged,
            "pointer_cleanup":{"button_released":true,"modifiers_released":true,"host_input_armed":self.input_lock},
            "layout":layout,
            "runtime_health":runtime_health
        }))
    }

    fn resize_frame(&mut self, arguments: &Value) -> HostResult<Value> {
        let target = parse_frame_target(arguments)?;
        if !target.is_resize_target() {
            return Err(invalid(
                "windows_resize_frame target must be an edge or corner, not title",
            ));
        }
        let delta_x = optional_i32(arguments, "delta_x")?.unwrap_or(0);
        let delta_y = optional_i32(arguments, "delta_y")?.unwrap_or(0);
        let (horizontal, vertical) = resize_axes(target);
        if (horizontal && delta_x == 0) || (vertical && delta_y == 0) {
            return Err(invalid(
                "windows_resize_frame requires a non-zero delta for every affected axis",
            ));
        }
        let before = self.fresh_window_interaction()?;
        let window_id = self.window_id()?;
        let frame_target = self
            .base
            .automation_mut()
            .resolve_window_frame_target(&window_id, target)?;
        let steps = gesture_steps(arguments);
        let duration_ms = gesture_duration_ms(arguments);
        let end = (
            frame_target.screen_point.x.saturating_add(delta_x),
            frame_target.screen_point.y.saturating_add(delta_y),
        );
        self.focus_and_move(frame_target.screen_point.x, frame_target.screen_point.y)?;
        let injected = self.drag_screen(
            (frame_target.screen_point.x, frame_target.screen_point.y),
            end,
            steps,
            duration_ms,
            &[],
            "left",
        )?;
        let (_, converged) = self.converged_window_after_gesture()?;
        let after = self.fresh_window_interaction()?;
        if !converged {
            return Err(HostError {
                code: "WINDOW_RESIZE_VERIFICATION_FAILED".into(),
                message: "window bounds did not converge after the physical frame resize".into(),
                details: json!({"before":before,"after":after,"target":frame_target}),
            });
        }
        let horizontal_changed = !horizontal
            || resize_boundary_changed(before.window.bounds, after.window.bounds, target, true);
        let vertical_changed = !vertical
            || resize_boundary_changed(before.window.bounds, after.window.bounds, target, false);
        let changed = before.window.bounds != after.window.bounds;
        let constraint_observed = resize_shrink_requested(target, delta_x, delta_y)
            && (before.window.bounds.width != after.window.bounds.width
                || before.window.bounds.height != after.window.bounds.height)
            && !resize_matches_requested_delta(
                before.window.bounds,
                after.window.bounds,
                target,
                delta_x,
                delta_y,
            );
        if !changed {
            return Err(HostError {
                code: if resize_shrink_requested(target, delta_x, delta_y) {
                    "WINDOW_RESIZE_CONSTRAINT".into()
                } else {
                    "WINDOW_BOUNDS_NOT_CHANGED".into()
                },
                message: "physical frame drag completed without a verified top-level bounds change"
                    .into(),
                details: json!({"before":before,"after":after,"target":frame_target,"requested_delta":{"x":delta_x,"y":delta_y}}),
            });
        }
        if !horizontal_changed || !vertical_changed {
            return Err(HostError {
                code: "WINDOW_RESIZE_VERIFICATION_FAILED".into(),
                message:
                    "the requested edge or corner did not change every required window boundary"
                        .into(),
                details: json!({"before":before,"after":after,"target":frame_target,"horizontal_changed":horizontal_changed,"vertical_changed":vertical_changed}),
            });
        }
        let layout = self.layout_after_window_action()?;
        let runtime_health = self.runtime_health_after_window_action("windows_resize_frame")?;
        Ok(json!({
            "physical_input":true,
            "pointer_sequence":"move → left_down → drag → left_up",
            "pre_bounds":before.window.bounds,
            "post_bounds":after.window.bounds,
            "pre":before,
            "post":after,
            "frame_target":frame_target,
            "requested_delta":{"x":delta_x,"y":delta_y},
            "steps":steps,
            "duration_ms":duration_ms,
            "injected":injected,
            "readback_converged":converged,
            "constraint_observed":constraint_observed,
            "pointer_cleanup":{"button_released":true,"modifiers_released":true,"host_input_armed":self.input_lock},
            "layout":layout,
            "runtime_health":runtime_health
        }))
    }

    fn double_click_title(&mut self, arguments: &Value) -> HostResult<Value> {
        let (before, target, title_region) = self.title_frame_target(arguments)?;
        let expected_state = match before.state {
            WindowState::Normal => WindowState::Maximized,
            WindowState::Maximized => WindowState::Normal,
            WindowState::Minimized => {
                return Err(HostError {
                    code: "WINDOW_NOT_MOVABLE".into(),
                    message: "a minimized window cannot receive a physical title double-click"
                        .into(),
                    details: json!({"before":before}),
                });
            }
        };
        let injected = self.click_frame_point(
            target.screen_point.x,
            target.screen_point.y,
            Pointer::DoubleClick,
        )?;
        let (_, converged) = self.converged_window_after_gesture()?;
        let after = self.fresh_window_interaction()?;
        if !converged || after.state != expected_state {
            return Err(HostError {
                code: "WINDOW_STATE_TRANSITION_FAILED".into(),
                message:
                    "physical title double-click did not reach the expected maximize/restore state"
                        .into(),
                details: json!({"before":before,"after":after,"target":target,"expected_state":expected_state,"converged":converged}),
            });
        }
        let layout = self.layout_after_window_action()?;
        let runtime_health =
            self.runtime_health_after_window_action("windows_double_click_title")?;
        Ok(json!({
            "physical_input":true,
            "pointer_sequence":"move → left_down/up → left_down/up",
            "pre":before,
            "post":after,
            "frame_target":target,
            "semantic_title_region":title_region,
            "expected_state":expected_state,
            "injected":injected,
            "readback_converged":converged,
            "pointer_cleanup":{"button_released":true,"modifiers_released":true,"host_input_armed":self.input_lock},
            "layout":layout,
            "runtime_health":runtime_health
        }))
    }

    fn fresh_window_interaction(&mut self) -> HostResult<WindowInteractionInfo> {
        let refreshed = self.base.refresh_window()?;
        self.window = Some(refreshed);
        let window_id = self.window_id()?;
        Ok(self
            .base
            .automation_mut()
            .inspect_window_interaction(&window_id)?)
    }

    fn title_frame_target(
        &mut self,
        arguments: &Value,
    ) -> HostResult<(WindowInteractionInfo, WindowFrameTarget, Option<Value>)> {
        let before = self.fresh_window_interaction()?;
        let window_id = self.window_id()?;
        if let Some(region_id) = arguments
            .get("title_region_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            let control = self.resolve(region_id)?;
            if !control.visible || control.offscreen || !control.bounds.is_actionable() {
                return Err(HostError {
                    code: "WINDOW_TITLE_REGION_UNAVAILABLE".into(),
                    message: "the semantic title region is not visible and actionable".into(),
                    details: json!({"id":region_id,"bounds":control.bounds,"visible":control.visible,"offscreen":control.offscreen}),
                });
            }
            let target = self
                .base
                .automation_mut()
                .resolve_window_title_target_in_region(&window_id, control.bounds)?;
            return Ok((
                before,
                target,
                Some(
                    json!({"id":control.automation_id,"element_id":control.element_id,"bounds":control.bounds}),
                ),
            ));
        }
        let target = self
            .base
            .automation_mut()
            .resolve_window_frame_target(&window_id, WindowFrameTargetKind::Title)?;
        Ok((before, target, None))
    }

    fn converged_window_after_gesture(&mut self) -> HostResult<(WindowInfo, bool)> {
        let window_id = self.window_id()?;
        let mut previous = self.base.automation_mut().get_window_info(&window_id)?;
        for _ in 0..3 {
            thread::sleep(Duration::from_millis(55));
            let current = self.base.automation_mut().get_window_info(&window_id)?;
            if current.bounds == previous.bounds {
                self.window = Some(current.clone());
                return Ok((current, true));
            }
            previous = current;
        }
        self.window = Some(previous.clone());
        Ok((previous, false))
    }

    fn layout_after_window_action(&mut self) -> HostResult<Value> {
        let tree = self.inspect_ui()?;
        let visible = tree
            .elements
            .iter()
            .filter(|control| control.visible)
            .count();
        let actionable = tree
            .elements
            .iter()
            .filter(|control| {
                control.visible
                    && control.enabled
                    && !control.offscreen
                    && control.bounds.is_actionable()
            })
            .count();
        let invalid_visible_bounds = tree
            .elements
            .iter()
            .filter(|control| {
                control.visible && !control.offscreen && !control.bounds.is_actionable()
            })
            .map(|control| control.automation_id.clone())
            .collect::<Vec<_>>();
        Ok(json!({
            "tree_elements":tree.total_enumerated,
            "visible_elements":visible,
            "actionable_elements":actionable,
            "invalid_visible_bounds":invalid_visible_bounds,
            "bounds_source":"fresh_uia_tree"
        }))
    }

    fn runtime_health_after_window_action(&mut self, operation: &str) -> HostResult<Value> {
        let health = self.base.get_runtime_health();
        if !health.healthy {
            return Err(HostError {
                code: "RUNTIME_UNHEALTHY".into(),
                message: format!("runtime became unhealthy after {operation}"),
                details: json!({"operation":operation,"runtime_health":health}),
            });
        }
        Ok(json!(health))
    }

    #[cfg(windows)]
    fn click_frame_point(&mut self, x: i32, y: i32, pointer: Pointer) -> HostResult<usize> {
        self.focus_and_move(x, y)?;
        let pair = match pointer {
            Pointer::Click | Pointer::DoubleClick => {
                vec![mouse(MOUSEEVENTF_LEFTDOWN), mouse(MOUSEEVENTF_LEFTUP)]
            }
            Pointer::RightClick => vec![mouse(MOUSEEVENTF_RIGHTDOWN), mouse(MOUSEEVENTF_RIGHTUP)],
            Pointer::Hover => Vec::new(),
        };
        let mut inputs = pair.clone();
        if matches!(pointer, Pointer::DoubleClick) {
            inputs.extend(pair);
        }
        self.arm_input("left", &[]);
        match send_inputs(&inputs) {
            Ok(injected) => {
                self.disarm_input();
                Ok(injected)
            }
            Err(error) => {
                self.cleanup_pointer();
                Err(error)
            }
        }
    }
    fn activate_window(&mut self) -> HostResult<Value> {
        let window_id = self.window_id()?;
        Ok(json!(self.base.automation_mut().focus_window(&window_id)?))
    }
    fn send_keys(&mut self, keys: &str) -> HostResult<Value> {
        let window_id = self.window_id()?;
        Ok(json!(
            self.base.automation_mut().send_keys(&window_id, keys)?
        ))
    }

    fn send_text(&mut self, text: &str) -> HostResult<Value> {
        let window_id = self.window_id()?;
        Ok(json!(
            self.base.automation_mut().send_text(&window_id, text)?
        ))
    }

    fn send_key_repeat(&mut self, key: &str, count: usize) -> HostResult<Value> {
        let window_id = self.window_id()?;
        Ok(json!(
            self.base
                .automation_mut()
                .send_key_repeat(&window_id, key, count)?
        ))
    }
    fn hotkey(&mut self, arguments: &Value) -> HostResult<Value> {
        let keys = arguments
            .get("keys")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("keys must be an array"))?;
        let chord = keys
            .iter()
            .map(|key| {
                key.as_str()
                    .map(normalize_key)
                    .ok_or_else(|| invalid("keys must contain strings"))
            })
            .collect::<HostResult<Vec<_>>>()?
            .join("+");
        Ok(json!({"keys":keys,"normalized":chord,"result":self.send_keys(&chord)?,"fallback":true}))
    }

    fn wait_state<F: Fn(&ElementState) -> bool>(
        &mut self,
        id: &str,
        predicate: F,
        timeout: Duration,
    ) -> HostResult<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.read_state(id) {
                Ok(state) if predicate(&state) => return Ok(json!(state)),
                Ok(state) if Instant::now() >= deadline => {
                    return Err(timeout_error("wait", id, json!({"state":state})));
                }
                Err(error) if Instant::now() >= deadline => return Err(error),
                _ => thread::sleep(Duration::from_millis(75)),
            }
        }
    }

    fn wait_hidden(&mut self, id: &str, timeout: Duration) -> HostResult<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let tree = self.inspect_ui()?;
            let hidden = tree
                .elements
                .iter()
                .find(|control| control.automation_id == id || control.automation_id.ends_with(id))
                .is_none_or(|control| !control.visible);
            if hidden {
                return Ok(json!({"id":id,"visible":false}));
            }
            if Instant::now() >= deadline {
                return Err(timeout_error("wait_hidden", id, json!({"visible":true})));
            }
            thread::sleep(Duration::from_millis(75));
        }
    }

    fn wait_value(&mut self, arguments: &Value) -> HostResult<Value> {
        let id = self.required(arguments, "id")?;
        let expected = self.required_text(arguments, "value")?;
        let contains = arguments
            .get("contains")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.wait_state(
            &id,
            |state| {
                state.value.as_deref().is_some_and(|value| {
                    if contains {
                        value.contains(&expected)
                    } else {
                        value == expected
                    }
                })
            },
            duration(arguments),
        )
    }
    fn wait_count(&mut self, arguments: &Value) -> HostResult<Value> {
        let id = required_alias(arguments, &["id", "container_id"])?;
        let expected = arguments
            .get("count")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("count must be an integer"))? as usize;
        let deadline = Instant::now() + duration(arguments);
        loop {
            let count = self.count_selected(Some(&id))?;
            if count == expected {
                return Ok(json!({"container_id":id,"count":count}));
            }
            if Instant::now() >= deadline {
                return Err(timeout_error(
                    "wait_count",
                    &id,
                    json!({"count":count,"expected":expected}),
                ));
            }
            thread::sleep(Duration::from_millis(75));
        }
    }
    fn wait_idle(&mut self, timeout: Duration) -> HostResult<Value> {
        let started = Instant::now();
        let deadline = Instant::now() + timeout;
        let mut previous = None;
        let mut stable = 0;
        let mut observations = 0usize;
        let mut state_changes_observed = 0usize;
        loop {
            let tree = self.inspect_ui()?;
            let value = serde_json::to_value(&tree).unwrap_or(Value::Null);
            observations += 1;
            if previous.as_ref() == Some(&value) {
                stable += 1
            } else {
                if previous.is_some() {
                    state_changes_observed += 1;
                }
                stable = 0
            }
            previous = Some(value);
            if stable >= 2 {
                return Ok(json!({
                    "idle":true,
                    "heuristic":"three identical UIA snapshots",
                    "observations":observations,
                    "state_changes_observed":state_changes_observed,
                    "settled_after_ms":started.elapsed().as_millis()
                }));
            }
            if Instant::now() >= deadline {
                return Err(timeout_error(
                    "wait_idle",
                    "window",
                    json!({"stable_snapshots":stable+1}),
                ));
            }
            thread::sleep(Duration::from_millis(80));
        }
    }

    fn read_state(&mut self, id: &str) -> HostResult<ElementState> {
        serde_json::from_value(self.legacy("read", json!({"id":id}))?)
            .map_err(|error| invalid(error.to_string()))
    }
    fn screenshot_value(&mut self) -> HostResult<Value> {
        self.legacy("screenshot", json!({}))
    }
    fn window_id(&self) -> HostResult<String> {
        self.window
            .as_ref()
            .map(|window| window.window_id.clone())
            .ok_or_else(|| HostError {
                code: "APP_NOT_RUNNING".into(),
                message: "the test application is not running".into(),
                details: Value::Null,
            })
    }
    fn required(&self, arguments: &Value, key: &str) -> HostResult<String> {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| invalid(format!("{key} must be a non-empty string")))
    }

    fn required_text(&self, arguments: &Value, key: &str) -> HostResult<String> {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| invalid(format!("{key} must be a string")))
    }

    #[cfg(windows)]
    fn pointer_at(&mut self, bounds: Rect, pointer: Pointer) -> HostResult<Value> {
        let (x, y) = bounds
            .center()
            .ok_or_else(|| invalid("target has no actionable bounds"))?;
        self.focus_and_move(x, y)?;
        let inputs = match pointer {
            Pointer::Click => vec![mouse(MOUSEEVENTF_LEFTDOWN), mouse(MOUSEEVENTF_LEFTUP)],
            Pointer::DoubleClick => vec![
                mouse(MOUSEEVENTF_LEFTDOWN),
                mouse(MOUSEEVENTF_LEFTUP),
                mouse(MOUSEEVENTF_LEFTDOWN),
                mouse(MOUSEEVENTF_LEFTUP),
            ],
            Pointer::RightClick => vec![mouse(MOUSEEVENTF_RIGHTDOWN), mouse(MOUSEEVENTF_RIGHTUP)],
            Pointer::Hover => Vec::new(),
        };
        Ok(json!({"x":x,"y":y,"requested":inputs.len(),"injected":send_inputs(&inputs)?}))
    }
    #[cfg(not(windows))]
    fn pointer_at(&mut self, _: Rect, _: Pointer) -> HostResult<Value> {
        Err(invalid(
            "physical pointer fallback is only available on Windows",
        ))
    }
    #[cfg(windows)]
    fn ctrl_click(&mut self, bounds: Rect) -> HostResult<Value> {
        let (x, y) = bounds
            .center()
            .ok_or_else(|| invalid("target has no actionable bounds"))?;
        self.focus_and_move(x, y)?;
        let inputs = vec![
            key(VK_CONTROL, false),
            mouse(MOUSEEVENTF_LEFTDOWN),
            mouse(MOUSEEVENTF_LEFTUP),
            key(VK_CONTROL, true),
        ];
        self.arm_input("left", &[Modifier::CTRL]);
        match send_inputs(&inputs) {
            Ok(injected) => {
                self.disarm_input();
                Ok(json!({"x":x,"y":y,"requested":inputs.len(),"injected":injected}))
            }
            Err(error) => {
                self.cleanup_pointer();
                Err(error)
            }
        }
    }
    #[cfg(not(windows))]
    fn ctrl_click(&mut self, _: Rect) -> HostResult<Value> {
        Err(invalid(
            "physical pointer fallback is only available on Windows",
        ))
    }
    #[cfg(windows)]
    fn drag_at(&mut self, source: Rect, end: (i32, i32)) -> HostResult<Value> {
        let start = source
            .center()
            .ok_or_else(|| invalid("drag source has no actionable bounds"))?;
        self.focus_and_move(start.0, start.1)?;
        let injected = self.drag_screen(start, end, 5, 125, &[], "left")?;
        Ok(json!({"start":start,"end":end,"requested":2,"injected":injected}))
    }
    #[cfg(not(windows))]
    fn drag_at(&mut self, _: Rect, _: (i32, i32)) -> HostResult<Value> {
        Err(invalid(
            "physical drag fallback is only available on Windows",
        ))
    }
    #[cfg(windows)]
    fn scroll_at(&mut self, bounds: Rect, delta: i32) -> HostResult<Value> {
        let (x, y) = bounds
            .center()
            .ok_or_else(|| invalid("scroll target has no actionable bounds"))?;
        self.focus_and_move(x, y)?;
        let inputs = if delta == 0 {
            Vec::new()
        } else {
            vec![mouse_data(MOUSEEVENTF_WHEEL, delta as u32)]
        };
        let injected = if inputs.is_empty() {
            0
        } else {
            // Makepad derives a wheel event's hit point from the last dispatched move.
            thread::sleep(Duration::from_millis(POINTER_TARGET_SETTLE_MS));
            let injected = send_inputs(&inputs)?;
            thread::sleep(Duration::from_millis(WHEEL_DISPATCH_SETTLE_MS));
            injected
        };
        Ok(json!({"x":x,"y":y,"delta_y":delta,"requested":inputs.len(),"injected":injected}))
    }
    #[cfg(not(windows))]
    fn scroll_at(&mut self, _: Rect, _: i32) -> HostResult<Value> {
        Err(invalid(
            "physical scroll fallback is only available on Windows",
        ))
    }

    fn spatial_control(
        &mut self,
        a: &Value,
        action: &str,
    ) -> HostResult<(ControlInfo, RelativePoint, ResolvedPoint)> {
        let id = self.required(a, "id")?;
        let c = self.resolve(&id)?;
        self.actionable(&c, action)?;
        let health = spatial_canvas_health(&c);
        if a.get("require_canvas_healthy")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && health != "Healthy"
        {
            self.window = Some(self.base.refresh_window()?);
            return Err(HostError {
                code: "CANVAS_UNHEALTHY".into(),
                message: format!("spatial target is {health}"),
                details: json!({"id": id, "canvas_health": health, "bounds": c.bounds}),
            });
        }
        if c.offscreen || !c.bounds.is_actionable() {
            return Err(HostError {
                code: "INVALID_SPATIAL_BOUNDS".into(),
                message: format!("{action} target has invalid bounds"),
                details: json!({"id": id, "bounds": c.bounds, "offscreen": c.offscreen, "bounds_timestamp": toolkit_windows_automation::now_unix_ms(), "bounds_source": "fresh_uia_tree"}),
            });
        }
        let p: RelativePoint = serde_json::from_value(
            a.get("position")
                .cloned()
                .ok_or_else(|| invalid("position is required"))?,
        )
        .map_err(|e| invalid(e.to_string()))?;
        let screen = p.resolve(c.bounds).map_err(|e| invalid(format!("{e:?}")))?;
        Ok((
            c.clone(),
            p,
            ResolvedPoint {
                relative: p,
                element_bounds: c.bounds,
                screen,
                dpi: self.dpi_metadata(),
            },
        ))
    }
    fn run_spatial_sequence(&mut self, arguments: &Value) -> HostResult<Value> {
        let actions = arguments
            .get("actions")
            .or_else(|| arguments.get("sequence"))
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("actions must be an array"))?;
        let mut results = Vec::with_capacity(actions.len());
        for action in actions {
            let operation = action
                .get("op")
                .or_else(|| action.get("operation"))
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("each spatial action requires op"))?;
            results.push(
                json!({"op": operation, "result": self.call_result(operation, action.clone())?}),
            );
        }
        Ok(
            json!({"replayed": results.len(), "actions": results, "coordinate_representation": "element_relative"}),
        )
    }

    #[cfg(windows)]
    fn dpi_metadata(&self) -> DpiMetadata {
        let Some(window) = self.window.as_ref() else {
            return DpiMetadata::default();
        };
        let raw = window.hwnd.trim_start_matches("0x");
        let Ok(value) = usize::from_str_radix(raw, 16) else {
            return DpiMetadata::default();
        };
        let dpi = unsafe {
            GetDpiForWindow(windows_uia::Win32::Foundation::HWND(
                value as *mut std::ffi::c_void,
            ))
        }
        .max(96);
        DpiMetadata {
            dpi,
            scale_factor: dpi as f64 / 96.0,
            coordinate_space: CoordinateSpace::ScreenPixels,
        }
    }

    #[cfg(not(windows))]
    fn dpi_metadata(&self) -> DpiMetadata {
        DpiMetadata::default()
    }
    fn inspect_spatial_target(&mut self, a: &Value) -> HostResult<Value> {
        self.window = Some(self.base.refresh_window()?);
        let id = self.required(a, "id")?;
        let c = self.resolve(&id)?;
        Ok(
            json!({"semantic_identity":{"id":c.automation_id,"element_id":c.element_id,"role":c.control_type,"name":c.name},"bounds":c.bounds,"viewport":c.bounds,"window":self.window,"dpi":self.dpi_metadata(),"interactable":c.visible&&c.enabled&&!c.offscreen&&c.bounds.is_actionable(),"canvas_health":spatial_canvas_health(&c),"bounds_timestamp":toolkit_windows_automation::now_unix_ms(),"bounds_source":"fresh_uia_tree","supported_semantic_actions":supported(&c),"supported_spatial_actions":["move_pointer","pointer_down","pointer_up","click_relative","double_click_relative","right_click_relative","drag_relative","select_region","hover_relative","wheel_relative","capture_region"]}),
        )
    }
    fn move_pointer(&mut self, a: &Value) -> HostResult<Value> {
        let (c, p, r) = self.spatial_control(a, "move_pointer")?;
        self.focus_and_move(r.screen.0, r.screen.1)?;
        Ok(
            json!({"resolved_element":c.automation_id,"element_bounds":c.bounds,"relative_position":p,"resolved_screen_point":r.screen,"window":self.window,"dpi":r.dpi,"pointer_moved":true,"application_health":self.base.get_runtime_health()}),
        )
    }
    fn pointer_button(&mut self, a: &Value, down: bool) -> HostResult<Value> {
        let button = a.get("button").and_then(Value::as_str).unwrap_or("left");
        let f = match button_flags(button, down) {
            Ok(flags) => flags,
            Err(error) => {
                self.cleanup_pointer();
                return Err(error);
            }
        };
        if down {
            let (_, _, r) = self.spatial_control(a, "pointer_down")?;
            self.focus_and_move(r.screen.0, r.screen.1)?;
            self.arm_input(button, &[]);
            let n = match send_inputs(&[mouse(f)]) {
                Ok(injected) => injected,
                Err(error) => {
                    self.cleanup_pointer();
                    return Err(error);
                }
            };
            Ok(
                json!({"button":button,"down":true,"resolved_screen_point":r.screen,"injected":n,"pointer_state":{"button":button,"down":true},"input_lock":true}),
            )
        } else {
            let result = send_inputs(&[mouse(f)]);
            self.cleanup_pointer();
            let n = result?;
            Ok(
                json!({"button":button,"down":false,"injected":n,"pointer_state":{"button":button,"down":false},"input_lock":false}),
            )
        }
    }
    fn click_relative(&mut self, a: &Value, p: Pointer) -> HostResult<Value> {
        let (c, rel, r) = self.spatial_control(a, p.name())?;
        self.focus_and_move(r.screen.0, r.screen.1)?;
        let modifiers =
            parse_modifiers(a.get("modifiers")).map_err(|e| invalid(format!("{e:?}")))?;
        let pair = if matches!(p, Pointer::RightClick) {
            vec![mouse(MOUSEEVENTF_RIGHTDOWN), mouse(MOUSEEVENTF_RIGHTUP)]
        } else {
            vec![mouse(MOUSEEVENTF_LEFTDOWN), mouse(MOUSEEVENTF_LEFTUP)]
        };
        let mut inputs = modifier_inputs(&modifiers, false);
        inputs.extend(pair.clone());
        if matches!(p, Pointer::DoubleClick) {
            inputs.extend(pair);
        }
        inputs.extend(modifier_inputs(&modifiers, true));
        let button = if matches!(p, Pointer::RightClick) {
            "right"
        } else {
            "left"
        };
        self.arm_input(button, &modifiers);
        let n = match send_inputs(&inputs) {
            Ok(injected) => {
                self.disarm_input();
                injected
            }
            Err(error) => {
                self.cleanup_pointer();
                return Err(error);
            }
        };
        Ok(
            json!({"resolved_element":c.automation_id,"element_bounds":c.bounds,"relative_position":rel,"resolved_screen_point":r.screen,"window":self.window,"dpi":r.dpi,"injected":n,"postcondition_verified":false,"pointer_cleanup":{"button_released":true,"modifiers_released":true,"host_input_armed":self.input_lock}}),
        )
    }
    fn drag_relative(&mut self, a: &Value) -> HostResult<Value> {
        let id = self.required(a, "id")?;
        let c = self.resolve(&id)?;
        self.actionable(&c, "drag_relative")?;
        let from: RelativePoint = serde_json::from_value(
            a.get("from")
                .cloned()
                .ok_or_else(|| invalid("from is required"))?,
        )
        .map_err(|e| invalid(e.to_string()))?;
        let to: RelativePoint = serde_json::from_value(
            a.get("to")
                .cloned()
                .ok_or_else(|| invalid("to is required"))?,
        )
        .map_err(|e| invalid(e.to_string()))?;
        let start = from
            .resolve(c.bounds)
            .map_err(|e| invalid(format!("{e:?}")))?;
        let end = to
            .resolve(c.bounds)
            .map_err(|e| invalid(format!("{e:?}")))?;
        let modifiers =
            parse_modifiers(a.get("modifiers")).map_err(|e| invalid(format!("{e:?}")))?;
        let steps = a
            .get("steps")
            .and_then(Value::as_u64)
            .unwrap_or(5)
            .clamp(1, 100) as i32;
        let ms = a.get("duration_ms").and_then(Value::as_u64).unwrap_or(250);
        let button = a.get("button").and_then(Value::as_str).unwrap_or("left");
        self.focus_and_move(start.0, start.1)?;
        let n = self.drag_screen(start, end, steps, ms, &modifiers, button)?;
        Ok(
            json!({"resolved_element":c.automation_id,"element_bounds":c.bounds,"from":from,"to":to,"start_screen":start,"end_screen":end,"steps":steps,"duration_ms":ms,"button":button,"injected":n,"postcondition_verified":false,"pointer_cleanup":{"button_released":true,"modifiers_released":true,"host_input_armed":self.input_lock}}),
        )
    }
    fn select_region(&mut self, a: &Value) -> HostResult<Value> {
        let id = self.required(a, "id")?;
        let c = self.resolve(&id)?;
        let from: RelativePoint = serde_json::from_value(
            a.get("from")
                .cloned()
                .ok_or_else(|| invalid("from is required"))?,
        )
        .map_err(|e| invalid(e.to_string()))?;
        let to: RelativePoint = serde_json::from_value(
            a.get("to")
                .cloned()
                .ok_or_else(|| invalid("to is required"))?,
        )
        .map_err(|e| invalid(e.to_string()))?;
        let s = from
            .resolve(c.bounds)
            .map_err(|e| invalid(format!("{e:?}")))?;
        let e = to
            .resolve(c.bounds)
            .map_err(|e| invalid(format!("{e:?}")))?;
        let rect = Rect {
            x: s.0.min(e.0),
            y: s.1.min(e.1),
            width: (s.0 - e.0).unsigned_abs() as i32 + 1,
            height: (s.1 - e.1).unsigned_abs() as i32 + 1,
        };
        let modifiers =
            parse_modifiers(a.get("modifiers")).map_err(|e| invalid(format!("{e:?}")))?;
        let steps = a
            .get("steps")
            .and_then(Value::as_u64)
            .unwrap_or(8)
            .clamp(1, 100) as i32;
        let ms = a.get("duration_ms").and_then(Value::as_u64).unwrap_or(300);
        self.focus_and_move(s.0, s.1)?;
        let n = self.drag_screen(s, e, steps, ms, &modifiers, "left")?;
        Ok(
            json!({"element_id":c.automation_id,"relative_rect":{"from":from,"to":to},"element_pixel_rect":rect,"window_rect":rect,"screen_rect":rect,"gesture_direction":{"from":from,"to":to},"steps":steps,"duration_ms":ms,"button":"left","injected":n,"postcondition_verified":false,"verification_mode":"visual","pointer_cleanup":{"button_released":true,"modifiers_released":true,"host_input_armed":self.input_lock}}),
        )
    }
    fn hover_relative(&mut self, a: &Value) -> HostResult<Value> {
        self.move_pointer(a)
    }
    fn wheel_relative(&mut self, a: &Value) -> HostResult<Value> {
        let (_, _, r) = self.spatial_control(a, "wheel_relative")?;
        self.focus_and_move(r.screen.0, r.screen.1)?;
        let d = a.get("delta_y").and_then(Value::as_i64).unwrap_or(0) as i32;
        let n = if d == 0 {
            0
        } else {
            // Keep the point-resolution contract identical to semantic scrolling.
            thread::sleep(Duration::from_millis(POINTER_TARGET_SETTLE_MS));
            let injected = send_inputs(&[mouse_data(MOUSEEVENTF_WHEEL, d as u32)])?;
            thread::sleep(Duration::from_millis(WHEEL_DISPATCH_SETTLE_MS));
            injected
        };
        Ok(
            json!({"resolved_screen_point":r.screen,"delta_y":d,"injected":n,"coordinate_space":"element_relative"}),
        )
    }
    fn capture_region(&mut self, a: &Value) -> HostResult<Value> {
        let id = self.required(a, "id")?;
        let c = self.resolve(&id)?;
        let region: RelativeRect = serde_json::from_value(
            a.get("region")
                .cloned()
                .ok_or_else(|| invalid("region is required"))?,
        )
        .map_err(|e| invalid(e.to_string()))?;
        let resolved = region
            .resolve(c.bounds)
            .map_err(|e| invalid(format!("{e:?}")))?;
        Ok(
            json!({"element_id":c.automation_id,"relative_region":region,"resolved_region":resolved,"element_bounds":c.bounds,"screenshot":self.screenshot_value()?,"coordinate_space":"element_relative"}),
        )
    }
    #[cfg(windows)]
    fn focus_and_move(&mut self, x: i32, y: i32) -> HostResult<()> {
        let id = self.window_id()?;
        let mut last_focus = None;
        for attempt in 0..3 {
            match self.base.automation_mut().focus_window(&id) {
                Ok(f) if f.success => {
                    last_focus = Some(f);
                    break;
                }
                Ok(f) => {
                    last_focus = Some(f);
                }
                Err(error) => {
                    if attempt == 2 {
                        return Err(error.into());
                    }
                }
            }
            thread::sleep(Duration::from_millis(90));
        }
        if !last_focus.as_ref().is_some_and(|focus| focus.success) {
            return Err(HostError {
                code: "WINDOW_NOT_FOCUSED".into(),
                message: "target window did not become foreground after retries".into(),
                details: json!({"attempts": 3, "last_focus": last_focus}),
            });
        }
        cursor_move_event(x, y)
    }
    #[cfg(not(windows))]
    fn focus_and_move(&mut self, _: i32, _: i32) -> HostResult<()> {
        Err(invalid(
            "physical pointer fallback is only available on Windows",
        ))
    }
    #[cfg(windows)]
    fn drag_screen(
        &mut self,
        s: (i32, i32),
        e: (i32, i32),
        steps: i32,
        ms: u64,
        modifiers: &[Modifier],
        button: &str,
    ) -> HostResult<usize> {
        self.arm_input(button, modifiers);
        let result = (|| {
            let mut n = send_inputs(&modifier_inputs(modifiers, false))?;
            let down = button_flags(button, true)?;
            let up = button_flags(button, false)?;
            n += send_inputs(&[mouse(down)])?;
            let wait = ms / steps as u64;
            for i in 1..=steps {
                let target = (s.0 + (e.0 - s.0) * i / steps, s.1 + (e.1 - s.1) * i / steps);
                cursor_move_event(target.0, target.1)?;
                if wait > 0 {
                    thread::sleep(Duration::from_millis(wait));
                }
            }
            n += send_inputs(&[mouse(up)])?;
            n += send_inputs(&modifier_inputs(modifiers, true))?;
            Ok(n)
        })();
        match result {
            Ok(n) => {
                self.disarm_input();
                Ok(n)
            }
            Err(error) => {
                self.cleanup_pointer();
                Err(error)
            }
        }
    }
    #[cfg(not(windows))]
    fn drag_screen(
        &mut self,
        _: (i32, i32),
        _: (i32, i32),
        _: i32,
        _: u64,
        _: &[Modifier],
        _: &str,
    ) -> HostResult<usize> {
        Err(invalid(
            "physical pointer fallback is only available on Windows",
        ))
    }
    fn cleanup_pointer(&mut self) {
        #[cfg(windows)]
        {
            let mut inputs = Vec::new();
            if self.pointer_button_state.is_some() {
                inputs.extend([
                    mouse(MOUSEEVENTF_LEFTUP),
                    mouse(MOUSEEVENTF_RIGHTUP),
                    mouse(MOUSEEVENTF_MIDDLEUP),
                ]);
            }
            inputs.extend(modifier_inputs(&self.pressed_modifiers, true));
            if !inputs.is_empty() {
                let _ = send_inputs(&inputs);
            }
        }
        self.disarm_input();
    }
    fn arm_input(&mut self, button: &str, modifiers: &[Modifier]) {
        self.pointer_button_state = Some(button.into());
        self.pressed_modifiers = modifiers.to_vec();
        self.input_lock = true;
    }
    fn disarm_input(&mut self) {
        self.pointer_button_state = None;
        self.pressed_modifiers.clear();
        self.input_lock = false;
    }
    fn record(&mut self, operation: &str, arguments: Value, response: Value) {
        self.sequence += 1;
        self.recent.push(RecentAction {
            sequence: self.sequence,
            operation: operation.into(),
            arguments,
            success: response["success"] == true,
            response,
        });
        if self.recent.len() > RECENT_ACTIONS {
            self.recent.remove(0);
        }
    }
}

#[derive(Clone, Copy)]
enum Pointer {
    Click,
    DoubleClick,
    RightClick,
    Hover,
}
impl Pointer {
    fn name(self) -> &'static str {
        match self {
            Self::Click => "click_relative",
            Self::DoubleClick => "double_click_relative",
            Self::RightClick => "right_click",
            Self::Hover => "hover",
        }
    }
}

#[cfg(windows)]
fn button_flags(
    button: &str,
    down: bool,
) -> HostResult<windows_uia::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS> {
    use windows_uia::Win32::UI::Input::KeyboardAndMouse::{
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    };
    Ok(match (button.to_ascii_lowercase().as_str(), down) {
        ("left", true) => MOUSEEVENTF_LEFTDOWN,
        ("left", false) => MOUSEEVENTF_LEFTUP,
        ("right", true) => MOUSEEVENTF_RIGHTDOWN,
        ("right", false) => MOUSEEVENTF_RIGHTUP,
        ("middle", true) => MOUSEEVENTF_MIDDLEDOWN,
        ("middle", false) => MOUSEEVENTF_MIDDLEUP,
        _ => return Err(invalid("button must be left, right, or middle")),
    })
}
#[cfg(not(windows))]
fn button_flags(_button: &str, _down: bool) -> HostResult<()> {
    Err(invalid(
        "physical pointer fallback is only available on Windows",
    ))
}
fn spatial_canvas_health(control: &ControlInfo) -> &'static str {
    if !control.visible {
        "Hidden"
    } else if control.offscreen {
        "Offscreen"
    } else if !control.bounds.is_actionable() {
        "ZeroViewport"
    } else {
        "Healthy"
    }
}
fn semantic(control: &ControlInfo) -> SemanticControl {
    SemanticControl {
        id: control.automation_id.clone(),
        role: control.control_type.to_ascii_lowercase(),
        name: control.name.clone(),
        bounds: control.bounds,
        visible: control.visible,
        enabled: control.enabled,
        focused: control.focused,
        selected: control.selected,
        checked: (control.control_type == "CheckBox")
            .then_some(control.selected)
            .flatten(),
        expanded: control
            .expand_state
            .as_deref()
            .map(|state| state == "expanded"),
        editable: (control.control_type == "Edit").then_some(true),
        readonly: (control.control_type == "Edit").then_some(!control.capabilities.value),
        modal: (control.control_type == "Pane" && control.automation_id.contains("modal"))
            .then_some(true),
        active: control.focused.then_some(true),
        scrollable: (control.control_type == "List").then_some(true),
        scroll_position: None,
        value: control.value.clone(),
        text: control.text.clone(),
    }
}
fn supported(control: &ControlInfo) -> Vec<&'static str> {
    let mut actions = Vec::new();
    if control.capabilities.invoke {
        actions.push("click")
    };
    if control.capabilities.select {
        actions.push("select")
    };
    if control.capabilities.value {
        actions.push("input")
    };
    if control.capabilities.synthesized_pointer {
        actions.push("focus")
    };
    actions
}
fn table_target(arguments: &Value) -> HostResult<String> {
    let table = arguments
        .get("table_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("table_id is required"))?;
    if let Some(id) = arguments.get("id").and_then(Value::as_str) {
        return Ok(id.into());
    }
    let row = arguments
        .get("row")
        .map(token)
        .ok_or_else(|| invalid("row is required"))?;
    Ok(match arguments.get("column").map(token) {
        Some(column) => format!("{table}/row.{row}/cell.{column}"),
        None => format!("{table}/row.{row}"),
    })
}
fn token(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}
fn duration(arguments: &Value) -> Duration {
    Duration::from_millis(
        arguments
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(WAIT_MS),
    )
}
fn required_alias(arguments: &Value, keys: &[&str]) -> HostResult<String> {
    keys.iter()
        .find_map(|key| {
            arguments
                .get(*key)
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        })
        .ok_or_else(|| {
            invalid(format!(
                "{} must be a non-empty string",
                keys.first().copied().unwrap_or("argument")
            ))
        })
}
fn normalize_key(key: &str) -> String {
    match key.to_ascii_uppercase().as_str() {
        "ESCAPE" => "ESC".into(),
        "RETURN" => "ENTER".into(),
        "ARROWUP" => "UP".into(),
        "ARROWDOWN" => "DOWN".into(),
        "ARROWLEFT" => "LEFT".into(),
        "ARROWRIGHT" => "RIGHT".into(),
        value => value.into(),
    }
}
fn parse_frame_target(arguments: &Value) -> HostResult<WindowFrameTargetKind> {
    serde_json::from_value(
        arguments
            .get("target")
            .cloned()
            .ok_or_else(|| invalid("target is required"))?,
    )
    .map_err(|error| invalid(format!("invalid window frame target: {error}")))
}

fn parse_window_state(arguments: &Value) -> HostResult<WindowState> {
    serde_json::from_value(
        arguments
            .get("state")
            .cloned()
            .ok_or_else(|| invalid("state is required"))?,
    )
    .map_err(|error| invalid(format!("invalid window state: {error}")))
}

fn optional_i32(arguments: &Value, key: &str) -> HostResult<Option<i32>> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };
    let value = value
        .as_i64()
        .ok_or_else(|| invalid(format!("{key} must be an integer")))?;
    i32::try_from(value)
        .map(Some)
        .map_err(|_| invalid(format!("{key} is outside the i32 range")))
}

fn required_i32(arguments: &Value, key: &str) -> HostResult<i32> {
    optional_i32(arguments, key)?.ok_or_else(|| invalid(format!("{key} is required")))
}

fn gesture_steps(arguments: &Value) -> i32 {
    arguments
        .get("steps")
        .and_then(Value::as_u64)
        .unwrap_or(8)
        .clamp(1, 100) as i32
}

fn gesture_duration_ms(arguments: &Value) -> u64 {
    arguments
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(320)
        .clamp(1, 5_000)
}

fn resize_axes(target: WindowFrameTargetKind) -> (bool, bool) {
    match target {
        WindowFrameTargetKind::Left | WindowFrameTargetKind::Right => (true, false),
        WindowFrameTargetKind::Top | WindowFrameTargetKind::Bottom => (false, true),
        WindowFrameTargetKind::TopLeft
        | WindowFrameTargetKind::TopRight
        | WindowFrameTargetKind::BottomLeft
        | WindowFrameTargetKind::BottomRight => (true, true),
        WindowFrameTargetKind::Title => (false, false),
    }
}

fn resize_boundary_changed(
    before: Rect,
    after: Rect,
    target: WindowFrameTargetKind,
    horizontal: bool,
) -> bool {
    if horizontal {
        match target {
            WindowFrameTargetKind::Left
            | WindowFrameTargetKind::TopLeft
            | WindowFrameTargetKind::BottomLeft => {
                before.x != after.x || before.width != after.width
            }
            WindowFrameTargetKind::Right
            | WindowFrameTargetKind::TopRight
            | WindowFrameTargetKind::BottomRight => {
                before.x.saturating_add(before.width) != after.x.saturating_add(after.width)
            }
            _ => true,
        }
    } else {
        match target {
            WindowFrameTargetKind::Top
            | WindowFrameTargetKind::TopLeft
            | WindowFrameTargetKind::TopRight => {
                before.y != after.y || before.height != after.height
            }
            WindowFrameTargetKind::Bottom
            | WindowFrameTargetKind::BottomLeft
            | WindowFrameTargetKind::BottomRight => {
                before.y.saturating_add(before.height) != after.y.saturating_add(after.height)
            }
            _ => true,
        }
    }
}

fn resize_shrink_requested(target: WindowFrameTargetKind, delta_x: i32, delta_y: i32) -> bool {
    matches!(
        target,
        WindowFrameTargetKind::Left
            | WindowFrameTargetKind::TopLeft
            | WindowFrameTargetKind::BottomLeft
    ) && delta_x > 0
        || matches!(
            target,
            WindowFrameTargetKind::Right
                | WindowFrameTargetKind::TopRight
                | WindowFrameTargetKind::BottomRight
        ) && delta_x < 0
        || matches!(
            target,
            WindowFrameTargetKind::Top
                | WindowFrameTargetKind::TopLeft
                | WindowFrameTargetKind::TopRight
        ) && delta_y > 0
        || matches!(
            target,
            WindowFrameTargetKind::Bottom
                | WindowFrameTargetKind::BottomLeft
                | WindowFrameTargetKind::BottomRight
        ) && delta_y < 0
}

fn resize_matches_requested_delta(
    before: Rect,
    after: Rect,
    target: WindowFrameTargetKind,
    delta_x: i32,
    delta_y: i32,
) -> bool {
    let expected = match target {
        WindowFrameTargetKind::Left => Rect {
            x: before.x.saturating_add(delta_x),
            y: before.y,
            width: before.width.saturating_sub(delta_x),
            height: before.height,
        },
        WindowFrameTargetKind::Right => Rect {
            x: before.x,
            y: before.y,
            width: before.width.saturating_add(delta_x),
            height: before.height,
        },
        WindowFrameTargetKind::Top => Rect {
            x: before.x,
            y: before.y.saturating_add(delta_y),
            width: before.width,
            height: before.height.saturating_sub(delta_y),
        },
        WindowFrameTargetKind::Bottom => Rect {
            x: before.x,
            y: before.y,
            width: before.width,
            height: before.height.saturating_add(delta_y),
        },
        WindowFrameTargetKind::TopLeft => Rect {
            x: before.x.saturating_add(delta_x),
            y: before.y.saturating_add(delta_y),
            width: before.width.saturating_sub(delta_x),
            height: before.height.saturating_sub(delta_y),
        },
        WindowFrameTargetKind::TopRight => Rect {
            x: before.x,
            y: before.y.saturating_add(delta_y),
            width: before.width.saturating_add(delta_x),
            height: before.height.saturating_sub(delta_y),
        },
        WindowFrameTargetKind::BottomLeft => Rect {
            x: before.x.saturating_add(delta_x),
            y: before.y,
            width: before.width.saturating_sub(delta_x),
            height: before.height.saturating_add(delta_y),
        },
        WindowFrameTargetKind::BottomRight => Rect {
            x: before.x,
            y: before.y,
            width: before.width.saturating_add(delta_x),
            height: before.height.saturating_add(delta_y),
        },
        WindowFrameTargetKind::Title => before,
    };
    expected == after
}
fn invalid(message: impl Into<String>) -> HostError {
    HostError {
        code: "INVALID_ARGUMENT".into(),
        message: message.into(),
        details: Value::Null,
    }
}
fn io_error(context: &str, error: impl std::fmt::Display) -> HostError {
    HostError {
        code: "HOST_IO_ERROR".into(),
        message: format!("{context}: {error}"),
        details: Value::Null,
    }
}

fn timeout_error(operation: &str, id: &str, details: Value) -> HostError {
    HostError {
        code: "WAIT_TIMEOUT".into(),
        message: format!("{operation} timed out for {id}"),
        details: json!({"error":"wait_timeout","operation":operation,"id":id,"details":details}),
    }
}
fn to_value<T: Serialize>(value: T) -> HostResult<Value> {
    serde_json::to_value(value).map_err(|error| invalid(error.to_string()))
}
fn write_json<T: Serialize>(path: &Path, value: &T) -> HostResult<()> {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|error| invalid(error.to_string()))?,
    )
    .map_err(|error| io_error("write failure evidence", error))
}

#[cfg(windows)]
#[cfg(windows)]
fn modifier_inputs(modifiers: &[Modifier], up: bool) -> Vec<INPUT> {
    modifiers
        .iter()
        .map(|modifier| {
            let vk = match modifier {
                Modifier::CTRL => VIRTUAL_KEY(0x11),
                Modifier::SHIFT => VIRTUAL_KEY(0x10),
                Modifier::ALT => VIRTUAL_KEY(0x12),
            };
            key(vk, up)
        })
        .collect()
}
#[cfg(not(windows))]
fn modifier_inputs(_modifiers: &[Modifier], _up: bool) -> Vec<()> {
    Vec::new()
}

fn mouse(flags: windows_uia::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) -> INPUT {
    mouse_data(flags, 0)
}

#[cfg(windows)]
fn cursor_move_event(x: i32, y: i32) -> HostResult<()> {
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(2);
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(2);
    let dx = (((x - left) as f64 * 65535.0) / (width - 1) as f64)
        .round()
        .clamp(0.0, 65535.0) as i32;
    let dy = (((y - top) as f64 * 65535.0) / (height - 1) as f64)
        .round()
        .clamp(0.0, 65535.0) as i32;
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send_inputs(&[input])?;
    Ok(())
}

#[cfg(windows)]
fn mouse_data(
    flags: windows_uia::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
    data: u32,
) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
#[cfg(windows)]
fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
#[cfg(windows)]
fn send_inputs(inputs: &[INPUT]) -> HostResult<usize> {
    if inputs.is_empty() {
        return Ok(0);
    }
    let injected = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) } as usize;
    if injected != inputs.len() {
        return Err(HostError {
            code: "INPUT_INJECTION_FAILED".into(),
            message: "Windows input injection did not submit every event".into(),
            details: json!({"requested":inputs.len(),"injected":injected}),
        });
    }
    Ok(injected)
}

fn scroll_delta(arguments: &Value) -> i32 {
    arguments
        .get("delta")
        .or_else(|| arguments.get("delta_y"))
        .and_then(Value::as_i64)
        .unwrap_or(0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_aliases_are_stable() {
        assert_eq!(normalize_key("Escape"), "ESC");
        assert_eq!(normalize_key("ArrowDown"), "DOWN");
    }

    #[test]
    fn scroll_delta_uses_public_contract_and_legacy_alias() {
        assert_eq!(scroll_delta(&json!({"delta": -120})), -120);
        assert_eq!(scroll_delta(&json!({"delta_y": 120})), 120);
        assert_eq!(scroll_delta(&json!({"delta": -120, "delta_y": 120})), -120);
    }

    #[test]
    fn ensure_visible_scroll_delta_is_directional_and_bounded() {
        let viewport = Rect {
            x: 0,
            y: 100,
            width: 500,
            height: 80,
        };
        let rect = |y| Rect {
            x: 0,
            y,
            width: 1,
            height: 20,
        };
        let max_scroll = -WHEEL * ENSURE_VISIBLE_MAX_WHEEL_STEPS;
        assert_eq!(ensure_visible_scroll_delta(rect(800), viewport), max_scroll);
        assert_eq!(ensure_visible_scroll_delta(rect(0), viewport), 100);
        assert_eq!(ensure_visible_scroll_delta(rect(71), viewport), 29);
        assert_eq!(ensure_visible_scroll_delta(rect(120), viewport), 0);
    }

    #[test]
    fn public_argument_aliases_prefer_catalog_fields_and_keep_legacy_compatibility() {
        let both = json!({"id":"public-source","source_id":"legacy-source","to":"public-target","target_id":"legacy-target"});
        assert_eq!(
            required_alias(&both, &["id", "source_id"]).unwrap(),
            "public-source"
        );
        assert_eq!(
            required_alias(&both, &["to", "target_id"]).unwrap(),
            "public-target"
        );
        assert_eq!(
            required_alias(&json!({"source_id":"legacy-source"}), &["id", "source_id"]).unwrap(),
            "legacy-source"
        );
        assert_eq!(
            required_alias(
                &json!({"container_id":"legacy-list"}),
                &["id", "container_id"]
            )
            .unwrap(),
            "legacy-list"
        );
    }
}

fn ensure_visible_scroll_delta(target: Rect, container: Rect) -> i32 {
    let top = container.y;
    let bottom = container.y.saturating_add(container.height);
    let target_top = target.y;
    let target_bottom = target.y.saturating_add(target.height);
    if target_bottom > bottom {
        return -wheel_delta_to_reveal(target_bottom - bottom);
    }
    if target_top < top {
        return wheel_delta_to_reveal(top - target_top);
    }
    0
}

fn wheel_delta_to_reveal(distance: i32) -> i32 {
    distance
        .max(1)
        .min(WHEEL.saturating_mul(ENSURE_VISIBLE_MAX_WHEEL_STEPS))
}
