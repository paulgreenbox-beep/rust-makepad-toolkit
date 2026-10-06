use crate::content::{PageImage, PageParagraph, PageTable};
use crate::geometry::{LogicalPoint, LogicalRect};
use crate::pages::PageStack;
use crate::rulers::{HorizontalRuler, RulerMarker, VerticalRuler};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiColor(pub u32);

impl UiColor {
    pub const BLACK: Self = Self(0x111827ff);
    pub const MUTED: Self = Self(0x64748bff);
    pub const BLUE: Self = Self(0x2563ebff);
    pub const RED: Self = Self(0xef4444ff);
    pub const GREEN: Self = Self(0x16a34aff);
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageText {
    pub page_index: usize,
    pub origin: LogicalPoint,
    pub text: String,
    pub font_size: f64,
    pub color: UiColor,
    pub bold: bool,
    pub italic: bool,
}

impl PageText {
    pub fn new(
        page_index: usize,
        origin: LogicalPoint,
        text: impl Into<String>,
        font_size: f64,
    ) -> Self {
        Self {
            page_index,
            origin,
            text: text.into(),
            font_size,
            color: UiColor::BLACK,
            bold: false,
            italic: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageRectPrimitive {
    pub page_index: usize,
    pub rect: LogicalRect,
    pub fill: UiColor,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PageContentItem {
    Text(PageText),
    Paragraph(PageParagraph),
    Table(PageTable),
    Image(PageImage),
    Rect(PageRectPrimitive),
}

#[derive(Clone, Debug, PartialEq)]
pub struct OverlayRect {
    pub page_index: usize,
    pub rect: LogicalRect,
    pub color: UiColor,
    pub line_width: f64,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlayLayer {
    pub rects: Vec<OverlayRect>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DocumentFrame {
    pub pages: PageStack,
    pub content: Vec<PageContentItem>,
    pub overlay: OverlayLayer,
    pub horizontal_ruler: Option<HorizontalRuler>,
    pub vertical_ruler: Option<VerticalRuler>,
    pub ruler_markers: Vec<RulerMarker>,
}

impl DocumentFrame {
    pub fn new(pages: PageStack) -> Self {
        Self {
            pages,
            content: Vec::new(),
            overlay: OverlayLayer::default(),
            horizontal_ruler: None,
            vertical_ruler: None,
            ruler_markers: Vec::new(),
        }
    }

    pub fn with_rulers(mut self) -> Self {
        self.horizontal_ruler = Some(HorizontalRuler::default());
        self.vertical_ruler = Some(VerticalRuler::default());
        self
    }

    pub fn with_ruler_marker(mut self, marker: RulerMarker) -> Self {
        self.ruler_markers.push(marker);
        self
    }
}
