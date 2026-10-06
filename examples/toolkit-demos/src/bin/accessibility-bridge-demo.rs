use makepad_widgets::*;

#[cfg(windows)]
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};
#[cfg(windows)]
use toolkit_windows_automation::{
    AccessibilityAction, AccessibilityActionHandler, AccessibilityActionRequest,
    AccessibilityActions, AccessibilityNode, AccessibilityRect, AccessibilityRole,
    AccessibilitySelection, AccessibilityTree, WindowsAccessibilityBridge,
};
#[cfg(windows)]
use windows_uia::{
    Win32::{
        Foundation::{HWND, POINT, RECT},
        Graphics::Gdi::ClientToScreen,
        UI::{
            HiDpi::GetDpiForWindow,
            WindowsAndMessaging::{FindWindowW, GetWindowRect, GetWindowThreadProcessId},
        },
    },
    core::PCWSTR,
};

app_main!(App);

const WINDOW_TITLE: &str = "Rust Makepad Toolkit - Accessibility Bridge";

#[cfg(windows)]
thread_local! {
    static ACCESSIBILITY_BRIDGE: RefCell<Option<WindowsAccessibilityBridge>> = const { RefCell::new(None) };
    static ACCESSIBILITY_GENERATION: Cell<u64> = const { Cell::new(0) };
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct AccessibilityUiAction(AccessibilityActionRequest);

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    selected_zoom: usize,
    #[rust]
    reset_invocations: usize,
}

impl App {
    fn set_zoom(&mut self, cx: &mut Cx, selected_zoom: usize) {
        self.selected_zoom = selected_zoom;
        self.update_status(cx);
    }

    fn reset_scroll(&mut self, cx: &mut Cx) {
        self.reset_invocations += 1;
        self.update_status(cx);
    }

    fn update_status(&self, cx: &mut Cx) {
        self.ui.label(cx, ids!(status)).set_text(
            cx,
            &format!(
                "Zoom: {}% | Reset invocations: {}",
                zoom_percent(self.selected_zoom),
                self.reset_invocations
            ),
        );
    }

    #[cfg(windows)]
    fn handle_accessibility_action(&mut self, cx: &mut Cx, request: AccessibilityActionRequest) {
        match (request.node_id.as_str(), request.action) {
            ("zoom-50", AccessibilityAction::Select) => self.set_zoom(cx, 0),
            ("zoom-100", AccessibilityAction::Select) => self.set_zoom(cx, 1),
            ("zoom-200", AccessibilityAction::Select) => self.set_zoom(cx, 2),
            ("reset-scroll", AccessibilityAction::Invoke) => self.reset_scroll(cx),
            _ => return,
        }
        self.refresh_accessibility(cx);
    }

    #[cfg(windows)]
    fn refresh_accessibility(&self, cx: &Cx) {
        let Ok(hwnd) = makepad_hwnd() else {
            return;
        };
        let generation = ACCESSIBILITY_GENERATION.with(|generation| {
            let next = generation.get().wrapping_add(1);
            generation.set(next);
            next
        });
        let Ok(tree) = self.accessibility_tree(cx, hwnd, generation) else {
            return;
        };

        ACCESSIBILITY_BRIDGE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if let Some(bridge) = slot.as_ref() {
                if let Err(error) = bridge.update(tree) {
                    eprintln!("accessibility bridge update failed: {error}");
                }
                return;
            }

            let handler: AccessibilityActionHandler = Arc::new(|request| {
                Cx::post_action(AccessibilityUiAction(request));
                Ok(())
            });
            match WindowsAccessibilityBridge::attach(hwnd, tree, handler) {
                Ok(bridge) => {
                    eprintln!("accessibility bridge attached to Makepad HWND");
                    *slot = Some(bridge);
                }
                Err(error) => eprintln!("accessibility bridge attach failed: {error}"),
            }
        });
    }

    #[cfg(windows)]
    fn accessibility_tree(
        &self,
        cx: &Cx,
        hwnd: HWND,
        generation: u64,
    ) -> Result<AccessibilityTree, String> {
        let screen = ScreenTransform::for_window(hwnd)?;
        let mut window_rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut window_rect) }.map_err(|error| error.to_string())?;

        let mut root = AccessibilityNode::new(
            "window",
            AccessibilityRole::Window,
            WINDOW_TITLE,
            AccessibilityRect {
                x: window_rect.left as f64,
                y: window_rect.top as f64,
                width: (window_rect.right - window_rect.left).max(0) as f64,
                height: (window_rect.bottom - window_rect.top).max(0) as f64,
            },
        );

        let mut toolbar = AccessibilityNode::new(
            "toolbar",
            AccessibilityRole::ToolBar,
            "Controls",
            screen.rect(self.ui.view(cx, ids!(toolbar)).area().rect(cx)),
        );
        let mut zoom_group = AccessibilityNode::new(
            "zoom",
            AccessibilityRole::Group,
            "Zoom",
            screen.rect(self.ui.view(cx, ids!(zoom_group)).area().rect(cx)),
        );
        zoom_group.selection = Some(AccessibilitySelection {
            can_select_multiple: false,
            is_selection_required: true,
        });

        for (node_id, name, selected_zoom, widget_id) in [
            ("zoom-50", "50 percent", 0, ids!(zoom_50)),
            ("zoom-100", "100 percent", 1, ids!(zoom_100)),
            ("zoom-200", "200 percent", 2, ids!(zoom_200)),
        ] {
            let mut node = AccessibilityNode::new(
                node_id,
                AccessibilityRole::RadioButton,
                name,
                screen.rect(self.ui.button(cx, widget_id).area().rect(cx)),
            );
            node.selected = Some(self.selected_zoom == selected_zoom);
            node.actions = AccessibilityActions {
                select: true,
                ..Default::default()
            };
            zoom_group.children.push(node);
        }

        let mut reset = AccessibilityNode::new(
            "reset-scroll",
            AccessibilityRole::Button,
            "Reset scroll",
            screen.rect(self.ui.button(cx, ids!(reset_scroll)).area().rect(cx)),
        );
        reset.actions = AccessibilityActions {
            invoke: true,
            ..Default::default()
        };
        let status = AccessibilityNode::new(
            "status",
            AccessibilityRole::Text,
            format!(
                "Zoom {} percent; Reset invocations: {}",
                zoom_percent(self.selected_zoom),
                self.reset_invocations
            ),
            screen.rect(self.ui.label(cx, ids!(status)).area().rect(cx)),
        );
        let preview = AccessibilityNode::new(
            "preview",
            AccessibilityRole::Pane,
            "Virtual canvas preview",
            screen.rect(self.ui.view(cx, ids!(preview)).area().rect(cx)),
        );

        toolbar.children.push(zoom_group);
        toolbar.children.push(reset);
        toolbar.children.push(status);
        root.children.push(toolbar);
        root.children.push(preview);
        AccessibilityTree::new(root)
            .map(|tree| tree.with_generation(generation))
            .map_err(|error| error.to_string())
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.selected_zoom = 1;
        self.update_status(cx);
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

        for (widget_id, selected_zoom) in
            [(ids!(zoom_50), 0), (ids!(zoom_100), 1), (ids!(zoom_200), 2)]
        {
            if self.ui.button(cx, widget_id).clicked(actions) {
                self.set_zoom(cx, selected_zoom);
            }
        }
        if self.ui.button(cx, ids!(reset_scroll)).clicked(actions) {
            self.reset_scroll(cx);
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

fn zoom_percent(selected_zoom: usize) -> usize {
    [50, 100, 200].get(selected_zoom).copied().unwrap_or(100)
}

#[cfg(windows)]
struct ScreenTransform {
    origin: POINT,
    dpi_factor: f64,
}

#[cfg(windows)]
impl ScreenTransform {
    fn for_window(hwnd: HWND) -> Result<Self, String> {
        let mut origin = POINT::default();
        unsafe { ClientToScreen(hwnd, &mut origin) }
            .ok()
            .map_err(|error| error.to_string())?;
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        Ok(Self {
            origin,
            dpi_factor: dpi as f64 / 96.0,
        })
    }

    fn rect(&self, rect: Rect) -> AccessibilityRect {
        AccessibilityRect {
            x: self.origin.x as f64 + rect.pos.x * self.dpi_factor,
            y: self.origin.y as f64 + rect.pos.y * self.dpi_factor,
            width: rect.size.x * self.dpi_factor,
            height: rect.size.y * self.dpi_factor,
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
                window.inner_size: vec2(900, 520)
                window.title: "Rust Makepad Toolkit - Accessibility Bridge"
                body +: {
                    width: Fill height: Fill flow: Down spacing: 12
                    padding: 18 show_bg: true draw_bg.color: #x1f2937

                    toolbar := View {
                        width: Fill height: Fit flow: Right spacing: 12
                        align: Align { y: 0.5 }
                        zoom_group := View {
                            width: Fit height: Fit flow: Right spacing: 8
                            Label { text: "Zoom" }
                            zoom_50 := Button { text: "50%" }
                            zoom_100 := Button { text: "100%" }
                            zoom_200 := Button { text: "200%" }
                        }
                        reset_scroll := Button { text: "Reset scroll" }
                        status := Label { text: "Zoom: 100% | Reset invocations: 0" }
                    }

                    preview := View {
                        width: Fill height: Fill flow: Down spacing: 10
                        padding: 28 show_bg: true draw_bg.color: #xdbeafe
                        Label { text: "Custom-drawn Makepad content" }
                        Label { text: "UI Automation sees this window through the generic bridge." }
                    }
                }
            }
        }
    }
}
