//! Demo application for the automatic Makepad UI Probe.

use makepad_widgets::*;

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
use windows_uia::{
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId},
    },
    core::PCWSTR,
};

app_main!(App);

const WINDOW_TITLE: &str = "AI UI Test Tool - Makepad Form";

#[cfg(windows)]
thread_local! {
    static ACCESSIBILITY_BRIDGE: RefCell<Option<WindowsAccessibilityBridge>> =
        const { RefCell::new(None) };
    static MAKEPAD_PROBE: RefCell<MakepadUiProbe> =
        RefCell::new(MakepadUiProbe::new(ProbeConfig::default()));
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct AccessibilityUiAction(AccessibilityActionRequest);

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
    checked: bool,
    #[rust]
    submit_count: usize,
}

impl App {
    fn submit(&mut self, cx: &mut Cx) {
        self.result_value = format!("Result: {}", self.input_value);
        self.submit_count += 1;
        self.update_labels(cx);
    }

    fn update_labels(&self, cx: &mut Cx) {
        self.ui
            .label(cx, ids!(result_message))
            .set_text(cx, &self.result_value);
        self.ui
            .label(cx, ids!(status))
            .set_text(cx, &self.status_value);
    }

    fn set_status(&mut self, cx: &mut Cx, status: impl Into<String>) {
        self.status_value = status.into();
        self.update_labels(cx);
    }

    #[cfg(windows)]
    fn handle_accessibility_action(&mut self, cx: &mut Cx, request: AccessibilityActionRequest) {
        let node_id = request.node_id.clone();
        let action = request.action.clone();
        let result =
            MAKEPAD_PROBE.with(|probe| probe.borrow_mut().dispatch_action(cx, &node_id, &action));
        if let Err(error) = result {
            eprintln!("Makepad UI Probe action failed for {node_id}: {error}");
            return;
        }
        match action {
            AccessibilityAction::SetValue(value) if node_id.ends_with("/input.name") => {
                self.input_value = value;
            }
            AccessibilityAction::Invoke if node_id.ends_with("/button.open.modal") => {
                self.open_dialog(cx);
            }
            AccessibilityAction::Invoke if node_id.ends_with("/button.close.modal") => {
                self.close_dialog(cx);
            }
            AccessibilityAction::Invoke if node_id.ends_with("/button.dynamic") => {
                self.reveal_dynamic(cx);
            }
            _ => {}
        }
    }

    fn open_dialog(&mut self, cx: &mut Cx) {
        self.ui.modal(cx, ids!(dialog_modal)).open(cx);
        self.ui
            .label(cx, ids!(dialog_value))
            .set_text(cx, &format!("Current result: {}", self.result_value));
        self.set_status(cx, "Status: dialog opened");
    }

    fn close_dialog(&mut self, cx: &mut Cx) {
        self.ui.modal(cx, ids!(dialog_modal)).close(cx);
        self.set_status(cx, "Status: dialog closed");
    }

    fn reveal_dynamic(&mut self, cx: &mut Cx) {
        self.ui
            .widget(cx, ids!(dynamic_banner))
            .set_visible(cx, true);
        self.set_status(cx, "Status: dynamic element revealed");
    }

    #[cfg(windows)]
    fn sync_probe_lifecycle(&self) {
        let modal_visible = self.status_value == "Status: dialog opened";
        MAKEPAD_PROBE.with(|probe| {
            let mut probe = probe.borrow_mut();
            probe.set_visibility_override("window.main", true);
            probe.set_visibility_override("window.main/dialog.modal", modal_visible);
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
                    "AI UI automatic Makepad Probe attached; widget types: {}",
                    snapshot.widget_types.join(", ")
                );
            }
        });
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
        self.status_value = "Status: ready; advanced action disabled".into();
        self.checked = false;
        self.submit_count = 0;
        self.ui
            .button(cx, ids!(button_enable))
            .set_disabled(cx, true);
        self.update_labels(cx);
        #[cfg(windows)]
        self.refresh_accessibility(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        #[cfg(windows)]
        {
            let requested = actions
                .iter()
                .filter_map(|action| action.downcast_ref::<AccessibilityUiAction>())
                .map(|action| action.0.clone())
                .collect::<Vec<_>>();
            for request in requested {
                self.handle_accessibility_action(cx, request);
            }
        }
        if let Some(text) = self.ui.text_input(cx, ids!(input_name)).changed(actions) {
            self.input_value = text;
            self.set_status(cx, "Status: input changed");
        }
        if self.ui.button(cx, ids!(button_create)).clicked(actions) {
            self.submit(cx);
            self.set_status(cx, "Status: greeting created");
        }
        if let Some(value) = self.ui.check_box(cx, ids!(checkbox_ready)).changed(actions) {
            self.checked = value;
            self.ui
                .button(cx, ids!(button_enable))
                .set_disabled(cx, !value);
            self.set_status(
                cx,
                if value {
                    "Status: advanced action enabled"
                } else {
                    "Status: advanced action disabled"
                },
            );
        }
        if self.ui.button(cx, ids!(button_enable)).clicked(actions) {
            self.result_value = "Result: advanced action invoked".into();
            self.set_status(cx, "Status: enabled action completed");
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
        if self.ui.button(cx, ids!(button_dynamic)).clicked(actions) {
            self.reveal_dynamic(cx);
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
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        #[cfg(windows)]
        if matches!(event, Event::Draw(_)) {
            self.refresh_accessibility(cx);
        }
    }
}

#[cfg(windows)]
fn makepad_hwnd() -> Result<HWND, String> {
    let wide = WINDOW_TITLE
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let hwnd =
        unsafe { FindWindowW(None, PCWSTR(wide.as_ptr())) }.map_err(|error| error.to_string())?;
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
    if process_id != std::process::id() {
        return Err("matched Makepad window belongs to another process".into());
    }
    Ok(hwnd)
}

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(820, 680)
                window.title: "AI UI Test Tool - Makepad Form"
                body +: {
                    width: Fill height: Fill flow: Down spacing: 10
                    padding: 20 show_bg: true draw_bg.color: #x172033
                    title := Label { text: "Automatic Makepad UI Probe" }
                    subtitle := Label { text: "The semantic tree is discovered from live WidgetRef children." }
                    form_main := View {
                        width: Fill height: Fit flow: Right spacing: 10
                        input_name := TextInput { width: Fill height: 42 empty_text: "Enter a name" }
                        button_create := Button { text: "Create greeting" }
                    }
                    checkbox_ready := CheckBox { text: "Enable advanced action" }
                    button_enable := Button { text: "Enabled action" }
                    project_list := View {
                        width: Fill height: Fit flow: Down spacing: 4
                        list_title := Label { text: "Selectable project items" }
                        item_alpha := Button { text: "Project Alpha" }
                        item_beta := Button { text: "Project Beta" }
                    }
                    button_open_modal := Button { text: "Open dialog" }
                    button_dynamic := Button { text: "Reveal dynamic element" }
                    dynamic_banner := View {
                        visible: false
                        width: Fill height: Fit
                        padding: 10 show_bg: true draw_bg.color: #x29405b
                        dynamic_label := Label { text: "Dynamic element is visible" }
                    }
                    result_message := Label { width: Fill height: Fit text: "Result: waiting for input" }
                    status := Label { width: Fill height: Fit text: "Status: ready; advanced action disabled" }
                }
                dialog_modal := Modal {
                    content +: {
                        width: 440 height: Fit flow: Down spacing: 12 padding: 20
                        show_bg: true draw_bg.color: #x2a3855
                        dialog_title := Label { text: "Probe-discovered dialog" }
                        dialog_value := Label { text: "Current result: waiting" }
                        button_close_modal := Button { text: "Close dialog" }
                    }
                }
            }
        }
    }
}
