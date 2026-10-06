//! Makepad surface used by the advanced semantic interaction E2E.

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
use windows_uia::Win32::{
    Foundation::{HWND, LPARAM, TRUE},
    UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    },
};
#[cfg(windows)]
use windows_uia::core::BOOL;

app_main!(App);

const WINDOW_TITLE: &str = "AI UI Spatial Demo";

#[cfg(windows)]
thread_local! {
    static ACCESSIBILITY_BRIDGE: RefCell<Option<WindowsAccessibilityBridge>> = const { RefCell::new(None) };
    static MAKEPAD_PROBE: RefCell<MakepadUiProbe> = RefCell::new(MakepadUiProbe::new(ProbeConfig {
        target_window_id: Some("window.main".to_owned()),
        ..ProbeConfig::default()
    }));
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
    drag_armed: bool,
    #[rust]
    canvas_selection_count: usize,
    #[rust]
    canvas_zoom: f64,
    #[rust]
    canvas_pan_x: i32,
    #[rust]
    canvas_pan_y: i32,
    #[rust]
    canvas_last_action: String,
    #[rust]
    canvas_wheel_count: usize,
    #[rust]
    crash_during_drag: bool,
}

impl App {
    fn set_label(&self, cx: &mut Cx, id: &[LiveId], text: &str) {
        self.ui.label(cx, id).set_text(cx, text);
    }

    fn update_labels(&self, cx: &mut Cx) {
        self.set_label(cx, ids!(result_message), &self.result_value);
        self.set_label(cx, ids!(status), &self.status_value);
        let canvas_status = format!(
            "selection.count={} selected.ids=[] hover.id=null zoom.value={:.2} pan.x={} pan.y={} wheel.count={} last.action={}",
            self.canvas_selection_count,
            self.canvas_zoom,
            self.canvas_pan_x,
            self.canvas_pan_y,
            self.canvas_wheel_count,
            self.canvas_last_action,
        );
        self.set_label(cx, ids!(canvas_status), &canvas_status);
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
        self.async_timer = cx.start_timeout(0.10);
    }

    fn complete_async(&mut self, cx: &mut Cx) {
        self.async_pending = false;
        self.ui.widget(cx, ids!(async_result)).set_visible(cx, true);
        self.status(cx, "Status: async result visible");
    }

    #[cfg(windows)]
    fn handle_accessibility_action(&mut self, cx: &mut Cx, request: AccessibilityActionRequest) {
        let node_id = request.node_id.clone();
        let action = request.action.clone();
        if matches!(action, AccessibilityAction::Invoke) {
            if node_id.ends_with("/canvas.pan.right") {
                self.canvas_pan_x += 10;
                self.canvas_last_action = "pan:right".into();
                self.status(
                    cx,
                    format!(
                        "Canvas pan.x={} zoom.value={}",
                        self.canvas_pan_x, self.canvas_zoom
                    ),
                );
                return;
            }
            if node_id.ends_with("/canvas.zoom.in") {
                self.canvas_zoom = (self.canvas_zoom + 0.25).min(3.0);
                self.canvas_last_action = "zoom:in".into();
                self.status(
                    cx,
                    format!(
                        "Canvas pan.x={} zoom.value={}",
                        self.canvas_pan_x, self.canvas_zoom
                    ),
                );
                return;
            }
        }
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
        self.drag_armed = false;
        self.canvas_selection_count = 0;
        self.canvas_zoom = 1.0;
        self.canvas_pan_x = 0;
        self.canvas_pan_y = 0;
        self.canvas_last_action = "ready".into();
        self.canvas_wheel_count = 0;
        self.crash_during_drag = false;
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
        #[cfg(windows)]
        self.refresh_accessibility(cx);
    }

    fn handle_timer(&mut self, cx: &mut Cx, event: &TimerEvent) {
        if self.async_pending && self.async_timer.is_timer(event).is_some() {
            self.complete_async(cx);
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
        if self.ui.button(cx, ids!(canvas_red)).clicked(actions) {
            self.canvas_selection_count = 1;
            self.canvas_last_action = "select:red".into();
            self.status(
                cx,
                format!(
                    "Canvas selection.count={} last.action={}",
                    self.canvas_selection_count, self.canvas_last_action
                ),
            );
        }
        if self.ui.button(cx, ids!(canvas_blue)).clicked(actions) {
            self.canvas_selection_count = 1;
            self.canvas_last_action = "select:blue".into();
            self.status(
                cx,
                format!(
                    "Canvas selection.count={} last.action={}",
                    self.canvas_selection_count, self.canvas_last_action
                ),
            );
        }
        if self.ui.button(cx, ids!(canvas_pan_left)).clicked(actions) {
            self.canvas_pan_x -= 10;
            self.canvas_last_action = "pan:left".into();
            self.status(
                cx,
                format!(
                    "Canvas pan.x={} zoom.value={}",
                    self.canvas_pan_x, self.canvas_zoom
                ),
            );
        }
        if self.ui.button(cx, ids!(canvas_pan_right)).clicked(actions) {
            self.canvas_pan_x += 10;
            self.canvas_last_action = "pan:right".into();
            self.status(
                cx,
                format!(
                    "Canvas pan.x={} zoom.value={}",
                    self.canvas_pan_x, self.canvas_zoom
                ),
            );
        }
        if self.ui.button(cx, ids!(canvas_zoom_in)).clicked(actions) {
            self.canvas_zoom = (self.canvas_zoom + 0.25).min(3.0);
            self.canvas_last_action = "zoom:in".into();
            self.status(
                cx,
                format!(
                    "Canvas pan.x={} zoom.value={}",
                    self.canvas_pan_x, self.canvas_zoom
                ),
            );
        }
        if self.ui.button(cx, ids!(canvas_zoom_out)).clicked(actions) {
            self.canvas_zoom = (self.canvas_zoom - 0.25).max(0.5);
            self.canvas_last_action = "zoom:out".into();
            self.status(
                cx,
                format!(
                    "Canvas pan.x={} zoom.value={}",
                    self.canvas_pan_x, self.canvas_zoom
                ),
            );
        }
        if self
            .ui
            .button(cx, ids!(canvas_crash_during_drag))
            .clicked(actions)
        {
            self.crash_during_drag = true;
            self.status(cx, "Status: crash during drag armed");
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
            if let Event::Scroll(scroll) = event {
                if self.item_contains(cx, ids!(canvas_main), scroll.abs) {
                    let delta = if scroll.scroll.y.abs() > f64::EPSILON {
                        scroll.scroll.y
                    } else {
                        scroll.scroll.x
                    };
                    if delta.abs() > f64::EPSILON {
                        self.canvas_wheel_count += 1;
                        self.canvas_zoom = if delta > 0.0 {
                            (self.canvas_zoom + 0.25).min(3.0)
                        } else {
                            (self.canvas_zoom - 0.25).max(0.5)
                        };
                        self.canvas_last_action = if delta > 0.0 {
                            "wheel:up"
                        } else {
                            "wheel:down"
                        }
                        .into();
                        self.status(
                            cx,
                            format!(
                                "Canvas wheel.count={} last.action={}",
                                self.canvas_wheel_count, self.canvas_last_action
                            ),
                        );
                    }
                    scroll.handled_y.set(true);
                }
            }
            if let Event::MouseUp(mouse) = event {
                if mouse.button == MouseButton::SECONDARY {
                    self.set_context(cx, true);
                }
                if self.item_contains(cx, ids!(canvas_pan_right), mouse.abs) {
                    self.canvas_pan_x += 10;
                    self.canvas_last_action = "pan:right".into();
                    self.status(
                        cx,
                        format!(
                            "Canvas pan.x={} zoom.value={}",
                            self.canvas_pan_x, self.canvas_zoom
                        ),
                    );
                }
                if self.item_contains(cx, ids!(canvas_zoom_in), mouse.abs) {
                    self.canvas_zoom = (self.canvas_zoom + 0.25).min(3.0);
                    self.canvas_last_action = "zoom:in".into();
                    self.status(
                        cx,
                        format!(
                            "Canvas pan.x={} zoom.value={}",
                            self.canvas_pan_x, self.canvas_zoom
                        ),
                    );
                }
                if mouse.button == MouseButton::PRIMARY && self.drag_armed {
                    self.drag_armed = false;
                    if self.item_contains(cx, ids!(drop_target), mouse.abs) {
                        self.canvas_selection_count = 0;
                        self.canvas_zoom = 1.0;
                        self.canvas_pan_x = 0;
                        self.canvas_pan_y = 0;
                        self.canvas_last_action = "ready".into();
                        self.status(cx, "Status: drop completed");
                    } else if self.item_contains(cx, ids!(canvas_main), mouse.abs) {
                        let action = if self.canvas_selection_count == 0 {
                            "select:marquee"
                        } else {
                            "drag:completed"
                        };
                        self.canvas_selection_count = 1;
                        self.canvas_last_action = action.into();
                        self.status(
                            cx,
                            format!(
                                "Canvas selection.count={} last.action={}",
                                self.canvas_selection_count, self.canvas_last_action
                            ),
                        );
                    } else {
                        self.canvas_selection_count = 0;
                        self.canvas_last_action = "ready".into();
                        self.update_labels(cx);
                    }
                }
            }
            if let Event::MouseDown(mouse) = event {
                let canvas_pressed = self.item_contains(cx, ids!(canvas_main), mouse.abs);
                if mouse.button == MouseButton::PRIMARY && canvas_pressed && self.crash_during_drag
                {
                    eprintln!("AI_UI_INTENTIONAL_CRASH_DURING_DRAG");
                    panic!("intentional Makepad crash during physical drag");
                }
                if mouse.button == MouseButton::PRIMARY
                    && (self.item_contains(cx, ids!(item_50), mouse.abs) || canvas_pressed)
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
                window.title: "AI UI Spatial Demo"
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
                    canvas_main := View { width: Fill height: 260 flow: Down spacing: 8 padding: 10 show_bg: true draw_bg.color: #x202f4d
                        canvas_caption := Label { text: "Spatial canvas viewport — select, drag, pan, zoom, context menu" }
                        canvas_toolbar := View { width: Fill height: 34 flow: Right spacing: 8
                            canvas_red := Button { text: "Red object" }
                            canvas_blue := Button { text: "Blue object" }
                            canvas_pan_left := Button { text: "Pan ←" }
                            canvas_pan_right := Button { text: "Pan →" }
                            canvas_zoom_out := Button { text: "Zoom −" }
                            canvas_zoom_in := Button { text: "Zoom +" }
                            canvas_crash_during_drag := Button { text: "Arm crash during drag" }
                        }
                        canvas_drop_target := View { width: Fill height: 140 show_bg: true draw_bg.color: #x284b63
                            canvas_status := Label { text: "selection.count=0 selected.ids=[] hover.id=null zoom.value=1.0 pan.x=0 pan.y=0 wheel.count=0 last.action=ready" }
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
                }            }
            // A second same-PID top-level Makepad window makes HWND identity
            // isolation observable without involving a production application.
            aux_window := Window {
                window.inner_size: vec2(420, 250)
                window.position: vec2(80, 80)
                window.title: "AI UI Spatial Demo Auxiliary"
                body +: {
                    width: Fill height: Fill flow: Down spacing: 8 padding: 16 show_bg: true draw_bg.color: #x253247
                    Label { text: "Same-process auxiliary window" }
                    Label { text: "Used only by the Toolkit window-frame isolation fixture." }
                }
            }
        }
    }
}
