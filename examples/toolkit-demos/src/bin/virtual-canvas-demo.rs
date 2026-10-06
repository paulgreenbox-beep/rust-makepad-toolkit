use makepad_toolkit_widgets::{
    DocumentFrame, LogicalPoint, LogicalRect, LogicalSize, OverlayRect, PageContentItem,
    PageRectPrimitive, PageStack, PageSurface, PageText, UiColor, VirtualCanvas,
};
use makepad_widgets::*;

app_main!(App);

fn canvas_frame() -> DocumentFrame {
    let surface = PageSurface::new(
        "synthetic-canvas",
        "Synthetic canvas",
        LogicalSize::new(1800.0, 1200.0),
    );
    let mut frame = DocumentFrame::new(PageStack::new(vec![surface]).with_spacing(0.0, 48.0));
    frame.content.push(PageContentItem::Rect(PageRectPrimitive {
        page_index: 0,
        rect: LogicalRect::new(80.0, 90.0, 540.0, 320.0),
        fill: UiColor(0xdbeafeff),
    }));
    frame.content.push(PageContentItem::Rect(PageRectPrimitive {
        page_index: 0,
        rect: LogicalRect::new(760.0, 520.0, 780.0, 470.0),
        fill: UiColor(0xdcfce7ff),
    }));
    let mut title = PageText::new(
        0,
        LogicalPoint::new(110.0, 145.0),
        "VirtualCanvas: zoom / scroll / clip / overlay",
        28.0,
    );
    title.bold = true;
    frame.content.push(PageContentItem::Text(title));
    frame.overlay.rects.push(OverlayRect {
        page_index: 0,
        rect: LogicalRect::new(65.0, 65.0, 1510.0, 960.0),
        color: UiColor::BLUE,
        line_width: 2.0,
        label: Some("overlay stays in logical coordinates".to_owned()),
    });
    frame
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl App {
    fn with_canvas(&mut self, cx: &mut Cx, f: impl FnOnce(&mut VirtualCanvas, &mut Cx)) {
        if let Some(mut canvas) = self
            .ui
            .widget(cx, ids!(runtime_canvas))
            .borrow_mut::<VirtualCanvas>()
        {
            f(&mut canvas, cx);
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.with_canvas(cx, |canvas, cx| {
            canvas.replace_frame_and_reset_view(cx, Some(canvas_frame()));
            canvas.set_debug_overlay(cx, true);
        });
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for (id, zoom) in [
            (ids!(zoom_50), 0.5),
            (ids!(zoom_100), 1.0),
            (ids!(zoom_200), 2.0),
        ] {
            if self.ui.button(cx, id).clicked(actions) {
                self.with_canvas(cx, |canvas, cx| canvas.set_zoom(cx, zoom));
            }
        }
        if self.ui.button(cx, ids!(reset_scroll)).clicked(actions) {
            self.with_canvas(cx, |canvas, cx| canvas.scroll_to(cx, 0.0, 0.0));
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_toolkit_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.VirtualCanvas

    startup() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(1180, 760)
                window.title: "Rust Makepad Toolkit — Virtual Canvas"
                body +: {
                    width: Fill height: Fill flow: Down spacing: 8
                    padding: 12 show_bg: true draw_bg.color: #x1f2937
                    View {
                        width: Fill height: Fit flow: Right spacing: 8
                        align: Align { y: 0.5 }
                        Label { text: "Generic VirtualCanvas" }
                        zoom_50 := Button { text: "50%" }
                        zoom_100 := Button { text: "100%" }
                        zoom_200 := Button { text: "200%" }
                        reset_scroll := Button { text: "Scroll 0,0" }
                        Label { text: "Wheel scrolls; Shift+wheel scrolls X; Ctrl+wheel zooms" }
                    }
                    View {
                        width: Fill height: Fill flow: Overlay new_batch: true
                        clip_x: true clip_y: true show_bg: true draw_bg.color: #xdfe4ea
                        runtime_canvas := VirtualCanvas { width: Fill height: Fill }
                    }
                }
            }
        }
    }
}
