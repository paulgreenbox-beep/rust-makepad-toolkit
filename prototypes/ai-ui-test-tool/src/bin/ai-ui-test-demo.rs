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
    AccessibilityTree, WindowsAccessibilityBridge,
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

const WINDOW_TITLE: &str = "AI UI Test Tool - Makepad Form";

#[cfg(windows)]
thread_local! {
    static ACCESSIBILITY_BRIDGE: RefCell<Option<WindowsAccessibilityBridge>> =
        const { RefCell::new(None) };
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
    input_value: String,
    #[rust]
    result_value: String,
    #[rust]
    submit_count: usize,
}

impl App {
    fn submit(&mut self, cx: &mut Cx) {
        self.result_value = format!("Result: {}", self.input_value);
        self.submit_count += 1;
        self.update_result(cx);
    }

    fn update_result(&self, cx: &mut Cx) {
        self.ui
            .label(cx, ids!(result))
            .set_text(cx, &self.result_value);
        self.ui
            .label(cx, ids!(status))
            .set_text(cx, &format!("Submissions: {}", self.submit_count));
    }

    #[cfg(windows)]
    fn handle_accessibility_action(&mut self, cx: &mut Cx, request: AccessibilityActionRequest) {
        match (request.node_id.as_str(), request.action) {
            ("input.name", AccessibilityAction::SetValue(value)) => {
                self.input_value = value.clone();
                self.ui
                    .text_input(cx, ids!(input_name))
                    .set_text(cx, &value);
            }
            ("input.name", AccessibilityAction::Focus) => {
                self.ui.text_input(cx, ids!(input_name)).set_key_focus(cx);
            }
            ("button.create", AccessibilityAction::Invoke) => self.submit(cx),
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
                let _ = bridge.update(tree);
                return;
            }

            let handler: AccessibilityActionHandler = Arc::new(|request| {
                Cx::post_action(AccessibilityUiAction(request));
                Ok(())
            });
            if let Ok(bridge) = WindowsAccessibilityBridge::attach(hwnd, tree, handler) {
                *slot = Some(bridge);
                eprintln!("AI UI Test Tool accessibility bridge attached");
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
            "window.main",
            AccessibilityRole::Window,
            WINDOW_TITLE,
            AccessibilityRect {
                x: window_rect.left as f64,
                y: window_rect.top as f64,
                width: (window_rect.right - window_rect.left).max(0) as f64,
                height: (window_rect.bottom - window_rect.top).max(0) as f64,
            },
        );

        let mut form = AccessibilityNode::new(
            "form.main",
            AccessibilityRole::Group,
            "Greeting form",
            screen.rect(self.ui.widget(cx, ids!(form)).area().rect(cx)),
        );

        let mut input = AccessibilityNode::new(
            "input.name",
            AccessibilityRole::Edit,
            "Name",
            screen.rect(self.ui.widget(cx, ids!(input_name)).area().rect(cx)),
        );
        input.value = Some(self.input_value.clone());
        input.actions = AccessibilityActions {
            focus: true,
            set_value: true,
            ..Default::default()
        };

        let mut button = AccessibilityNode::new(
            "button.create",
            AccessibilityRole::Button,
            "Create greeting",
            screen.rect(self.ui.widget(cx, ids!(create_button)).area().rect(cx)),
        );
        button.actions.invoke = true;

        let mut result = AccessibilityNode::new(
            "result.message",
            AccessibilityRole::Text,
            "Result",
            screen.rect(self.ui.widget(cx, ids!(result)).area().rect(cx)),
        );
        result.value = Some(self.result_value.clone());

        let mut status = AccessibilityNode::new(
            "status",
            AccessibilityRole::Text,
            "Status",
            screen.rect(self.ui.widget(cx, ids!(status)).area().rect(cx)),
        );
        status.value = Some(format!("Submissions: {}", self.submit_count));

        form.children.push(input);
        form.children.push(button);
        root.children.push(form);
        root.children.push(result);
        root.children.push(status);
        AccessibilityTree::new(root)
            .map(|tree| tree.with_generation(generation))
            .map_err(|error| error.to_string())
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.input_value.clear();
        self.result_value = "Result: waiting for input".into();
        self.submit_count = 0;
        self.update_result(cx);
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
        }
        if self.ui.button(cx, ids!(create_button)).clicked(actions) {
            self.submit(cx);
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
                window.inner_size: vec2(720, 420)
                window.title: "AI UI Test Tool - Makepad Form"
                body +: {
                    width: Fill height: Fill flow: Down spacing: 14
                    padding: 24 show_bg: true draw_bg.color: #x172033

                    title := Label {
                        text: "Makepad form under AI test"
                    }
                    subtitle := Label {
                        text: "The Test Host drives this form through semantic UIA."
                    }

                    form := View {
                        width: Fill height: Fit flow: Right spacing: 12
                        input_name := TextInput {
                            width: Fill height: 42
                            empty_text: "Enter a name"
                        }
                        create_button := Button {
                            text: "Create greeting"
                        }
                    }

                    result := Label {
                        width: Fill height: Fit
                        text: "Result: waiting for input"
                    }
                    status := Label {
                        width: Fill height: Fit
                        text: "Submissions: 0"
                    }
                }
            }
        }
    }
}
