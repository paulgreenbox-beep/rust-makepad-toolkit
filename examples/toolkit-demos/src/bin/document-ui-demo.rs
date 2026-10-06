use makepad_toolkit_widgets::{
    DocumentFrame, LogicalPoint, LogicalRect, OverlayRect, PageContentItem, PageImage, PageMargins,
    PageParagraph, PageStack, PageSurface, PageTable, PageTableCell, PageTableRow, PageText,
    TextAlignment, TextRun, TextRunStyle, UiColor, VirtualCanvas,
};
use makepad_widgets::*;

app_main!(App);

const DEMO_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn text_style(size: f64, color: UiColor, bold: bool, italic: bool) -> TextRunStyle {
    TextRunStyle {
        font_family: None,
        font_size: size,
        color,
        background: None,
        bold,
        italic,
        underline: false,
        strikethrough: false,
    }
}

fn table() -> PageTable {
    let columns = vec![190.0, 420.0];
    let rows = [
        (260.0, "Component", "Toolkit responsibility"),
        (
            308.0,
            "PageStack",
            "Pure placement, visible range and overscan",
        ),
        (
            356.0,
            "VirtualCanvas",
            "Makepad draw, events, clip and GPU resources",
        ),
    ]
    .into_iter()
    .map(|(y, left, right)| PageTableRow {
        rect: LogicalRect::new(92.0, y, 610.0, 48.0),
        cells: vec![
            PageTableCell {
                rect: LogicalRect::new(92.0, y, columns[0], 48.0),
                text: left.to_owned(),
                runs: vec![TextRun::new(
                    left,
                    text_style(12.0, UiColor::BLACK, true, false),
                )],
                alignment: TextAlignment::Left,
                background: Some(UiColor(0xeef2ffff)),
                border: Some(UiColor(0x64748bff)),
            },
            PageTableCell {
                rect: LogicalRect::new(282.0, y, columns[1], 48.0),
                text: right.to_owned(),
                runs: vec![TextRun::new(
                    right,
                    text_style(11.0, UiColor::BLACK, false, false),
                )],
                alignment: TextAlignment::Left,
                background: None,
                border: Some(UiColor(0x64748bff)),
            },
        ],
    })
    .collect();
    PageTable {
        page_index: 0,
        rect: LogicalRect::new(92.0, 260.0, 610.0, 144.0),
        columns,
        rows,
    }
}

fn document_frame() -> DocumentFrame {
    let pages = PageStack::new(vec![
        PageSurface::a4("a4").with_margins(PageMargins::new(76.0, 86.0, 76.0, 86.0)),
        PageSurface::a3_landscape("a3-landscape")
            .with_margins(PageMargins::new(90.0, 72.0, 90.0, 72.0)),
        PageSurface::letter("letter").with_margins(PageMargins::new(72.0, 72.0, 72.0, 72.0)),
    ])
    .with_spacing(36.0, 48.0);
    let mut frame = DocumentFrame::new(pages).with_rulers();

    let mut paragraph = PageParagraph::new(
        0,
        LogicalPoint::new(92.0, 132.0),
        610.0,
        vec![
            TextRun::new(
                "Reusable document UI: ",
                text_style(22.0, UiColor::BLACK, true, false),
            ),
            TextRun::new(
                "Unicode / Кириллица, mixed runs and measured wrapping.",
                text_style(16.0, UiColor::BLUE, false, true),
            ),
        ],
    );
    paragraph.space_after = 14.0;
    frame.content.push(PageContentItem::Paragraph(paragraph));
    frame.content.push(PageContentItem::Table(table()));

    let mut landscape = PageText::new(
        1,
        LogicalPoint::new(110.0, 145.0),
        "Mixed page sizes stay independently centered",
        26.0,
    );
    landscape.bold = true;
    frame.content.push(PageContentItem::Text(landscape));

    frame.content.push(PageContentItem::Text(PageText::new(
        2,
        LogicalPoint::new(96.0, 140.0),
        "ImageFrame primitive (embedded 1×1 PNG scaled into a frame)",
        18.0,
    )));
    frame.content.push(PageContentItem::Image(PageImage {
        page_index: 2,
        rect: LogicalRect::new(96.0, 190.0, 320.0, 220.0),
        pixel_width: 1,
        pixel_height: 1,
        bytes: DEMO_PNG.to_vec(),
    }));

    for (page_index, page) in frame.pages.pages.iter().enumerate() {
        frame.overlay.rects.push(OverlayRect {
            page_index,
            rect: LogicalRect::new(
                page.margins.left,
                page.margins.top,
                page.size.width - page.margins.left - page.margins.right,
                page.size.height - page.margins.top - page.margins.bottom,
            ),
            color: UiColor::BLUE,
            line_width: 1.0,
            label: Some("content rect".to_owned()),
        });
    }
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
            .widget(cx, ids!(document_canvas))
            .borrow_mut::<VirtualCanvas>()
        {
            f(&mut canvas, cx);
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.with_canvas(cx, |canvas, cx| {
            canvas.replace_frame_and_reset_view(cx, Some(document_frame()));
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
                window.inner_size: vec2(1280, 820)
                window.title: "Rust Makepad Toolkit — Document UI"
                body +: {
                    width: Fill height: Fill flow: Down spacing: 8
                    padding: 12 show_bg: true draw_bg.color: #x1f2937
                    View {
                        width: Fill height: Fit flow: Right spacing: 8
                        align: Align { y: 0.5 }
                        Label { text: "Synthetic Document UI" }
                        zoom_50 := Button { text: "50%" }
                        zoom_100 := Button { text: "100%" }
                        zoom_200 := Button { text: "200%" }
                        Label { text: "A4 + A3 landscape + Letter; rulers; text; table; image" }
                    }
                    View {
                        width: Fill height: Fill flow: Overlay new_batch: true
                        clip_x: true clip_y: true show_bg: true draw_bg.color: #xdfe4ea
                        document_canvas := VirtualCanvas { width: Fill height: Fill }
                    }
                }
            }
        }
    }
}
