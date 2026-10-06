use makepad_widgets::widget::WidgetActionData;
use makepad_widgets::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use makepad_toolkit_core::content::{PageImage, PageParagraph, PageTable, ParagraphFlow};
use makepad_toolkit_core::geometry::{
    LogicalPoint, LogicalRect, ScreenPoint, ScreenRect, ViewportPoint,
};
use makepad_toolkit_core::model::{
    DocumentFrame, OverlayRect, PageContentItem, PageRectPrimitive, PageText, UiColor,
};
use makepad_toolkit_core::pages::{PagePlacement, PageStackLayout};
use makepad_toolkit_core::rulers::{
    HorizontalRuler, RulerAxis, RulerMarker, RulerMarkerVisual, VerticalRuler,
};
use makepad_toolkit_core::text_layout::{
    DocumentFontFamily, DocumentFontRegistry, ParagraphLayout, ParagraphLayoutInput,
    PositionedLine, TextMeasurer, TextMetrics, TextRun, TextRunStyle, layout_paragraph,
};
use makepad_toolkit_core::viewport::{CanvasRuntime, RuntimeFrameState};

const OVERSCAN_PAGES: usize = 1;
const MAX_IMAGE_TEXTURES: usize = 12;
const TABLE_CELL_PADDING: f64 = 5.0;
const RULER_MARKER_HIT_WIDTH: f64 = 16.0;

#[derive(Clone, Debug, Default)]
struct MeasuredParagraphPlan {
    content_index: usize,
    lines: Vec<(usize, f64)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct MeasuredTableRowLayout {
    y: f64,
    height: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct MeasuredTableLayout {
    rows: Vec<MeasuredTableRowLayout>,
    height: f64,
}

#[derive(Clone, Debug, Default)]
struct MeasuredBlockPlan {
    content_index: usize,
    page_index: usize,
    y_offset: f64,
    table_layout: Option<MeasuredTableLayout>,
}

#[derive(Clone, Debug, Default)]
struct MeasuredDocumentFlow {
    paragraphs: Vec<MeasuredParagraphPlan>,
    blocks: Vec<MeasuredBlockPlan>,
}

fn measure_table_layout(
    table: &PageTable,
    padding: f64,
    measurer: &mut impl TextMeasurer,
) -> MeasuredTableLayout {
    let padding = padding.max(0.0);
    let table_top = table.rect.origin.y;
    let mut target_y = table_top;
    let mut source_bottom = table_top;
    let mut rows = Vec::with_capacity(table.rows.len());

    for row in &table.rows {
        target_y += (row.rect.origin.y - source_bottom).max(0.0);
        let mut height = row.rect.size.height.max(1.0);
        for cell in &row.cells {
            let runs = if cell.runs.is_empty() && !cell.text.is_empty() {
                vec![TextRun::new(cell.text.clone(), TextRunStyle::default())]
            } else {
                cell.runs.clone()
            };
            let mut input = ParagraphLayoutInput::new(
                LogicalPoint::new(0.0, 0.0),
                (cell.rect.size.width - padding * 2.0).max(1.0),
                runs,
            );
            input.alignment = cell.alignment;
            let layout = layout_paragraph(&input, measurer);
            height = height.max(layout.height + padding * 2.0);
        }
        rows.push(MeasuredTableRowLayout {
            y: target_y,
            height,
        });
        target_y += height;
        source_bottom = row.rect.bottom();
    }

    target_y += (table.rect.bottom() - source_bottom).max(0.0);
    MeasuredTableLayout {
        rows,
        height: (target_y - table_top).max(table.rect.size.height).max(1.0),
    }
}

/// Tables already carry a source page and page-local geometry. Their rows may
/// be remeasured, but the whole table segment must not be moved to the next
/// page merely because glyph-accurate measurement is taller than the source
/// estimate. A later source segment owns the rows on the following page.
fn source_anchored_table_position(
    frame: &DocumentFrame,
    table: &PageTable,
    cursor_page: usize,
    cursor_y: f64,
) -> (usize, f64) {
    let last_page = frame.pages.pages.len().saturating_sub(1);
    let source_page = table.page_index.min(last_page);
    let target_page = source_page.max(cursor_page).min(last_page);
    let page_top = frame
        .pages
        .pages
        .get(target_page)
        .map(|page| page.margins.top)
        .unwrap_or(0.0);
    let target_y = if source_page > cursor_page {
        table.rect.origin.y
    } else {
        cursor_y.max(table.rect.origin.y)
    }
    .max(page_top);
    (target_page, target_y)
}

fn makepad_text_origin_y(baseline_y: f64, ascender: f64) -> f64 {
    baseline_y - ascender.max(0.0)
}

#[derive(Clone)]
struct CachedImageTexture {
    key: u64,
    texture: Texture,
}

#[derive(Clone, Debug)]
struct RulerHitRegion {
    marker_id: String,
    label: String,
    page_index: usize,
    axis: RulerAxis,
    rect: ScreenRect,
    page_origin: ScreenPoint,
    logical_length: f64,
    zoom: f64,
    logical_position: f64,
    editable: bool,
}

#[derive(Clone, Debug)]
struct RulerDragState {
    marker_id: String,
    page_index: usize,
    axis: RulerAxis,
    page_origin: ScreenPoint,
    logical_length: f64,
    zoom: f64,
    logical_position: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ParagraphCursorStop {
    x: f64,
    byte_offset: usize,
}

#[derive(Clone, Debug)]
struct ParagraphHitRegion {
    interaction_id: String,
    page_index: usize,
    line_bounds: ScreenRect,
    editor_bounds: ScreenRect,
    cursor_stops: Vec<ParagraphCursorStop>,
}

#[derive(Clone, Debug)]
struct ParagraphLineHit {
    page_index: usize,
    bounds: ScreenRect,
    cursor_stops: Vec<ParagraphCursorStop>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RulerMarkerSnapshot {
    pub marker_id: String,
    pub label: String,
    pub page_index: usize,
    pub axis: RulerAxis,
    pub logical_position: f64,
    pub bounds: ScreenRect,
    pub editable: bool,
}

/// A visible generic paragraph target. The identifier remains owned by the
/// consumer; the canvas only reports screen geometry and activation.
#[derive(Clone, Debug, PartialEq)]
pub struct ParagraphInteractionSnapshot {
    pub interaction_id: String,
    pub page_index: usize,
    pub bounds: ScreenRect,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VirtualCanvasSnapshot {
    pub visible: bool,
    pub zoom_percent: u16,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub max_scroll_x: f64,
    pub max_scroll_y: f64,
    pub frame: RuntimeFrameState,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum VirtualCanvasAction {
    #[default]
    None,
    ViewportChanged(VirtualCanvasSnapshot),
    RulerMarkerChanged {
        marker_id: String,
        page_index: usize,
        logical_position: f64,
    },
    ParagraphActivated {
        interaction_id: String,
        page_index: usize,
        bounds: ScreenRect,
        cursor_byte_offset: usize,
    },
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct VirtualCanvas {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_shadow: DrawColor,
    #[live]
    draw_page: DrawColor,
    #[live]
    draw_shape: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_text_bold: DrawText,
    #[live]
    draw_text_italic: DrawText,
    #[live]
    draw_text_bold_italic: DrawText,
    #[live]
    draw_text_mono: DrawText,
    #[live]
    draw_text_mono_bold: DrawText,
    #[live]
    draw_text_mono_italic: DrawText,
    #[live]
    draw_text_mono_bold_italic: DrawText,
    #[live]
    draw_image: DrawImage,
    #[rust]
    image_textures: Vec<CachedImageTexture>,
    #[rust]
    action_data: WidgetActionData,
    #[live(true)]
    visible: bool,
    #[rust]
    debug_overlay: bool,
    #[rust]
    frame: Option<DocumentFrame>,
    #[rust]
    runtime: CanvasRuntime,
    #[rust]
    font_registry: DocumentFontRegistry,
    #[rust]
    last_walk: Option<Walk>,
    #[rust]
    ruler_hit_regions: Vec<RulerHitRegion>,
    #[rust]
    ruler_drag: Option<RulerDragState>,
    #[rust]
    paragraph_hit_regions: Vec<ParagraphHitRegion>,
}

impl VirtualCanvas {
    pub fn set_frame(&mut self, cx: &mut Cx, frame: Option<DocumentFrame>) {
        self.frame = frame;
        self.request_redraw(cx);
    }

    pub fn replace_frame_and_reset_view(&mut self, cx: &mut Cx, frame: Option<DocumentFrame>) {
        self.frame = frame;
        let center = self.runtime.viewport_center();
        self.runtime.reset_zoom(center);
        self.runtime.scroll.scroll_to(0.0, 0.0);
        self.request_redraw(cx);
    }

    pub fn set_debug_overlay(&mut self, cx: &mut Cx, enabled: bool) {
        if self.debug_overlay != enabled {
            self.debug_overlay = enabled;
            self.request_redraw(cx);
        }
    }

    pub fn zoom(&self) -> f64 {
        self.runtime.zoom()
    }

    pub fn zoom_percent(&self) -> u16 {
        self.runtime.zoom_percent()
    }

    pub fn set_zoom(&mut self, cx: &mut Cx, zoom: f64) {
        let center = self.runtime.viewport_center();
        if self.runtime.set_zoom(zoom, center) {
            self.viewport_changed(cx);
        }
    }

    pub fn zoom_in(&mut self, cx: &mut Cx) {
        let center = self.runtime.viewport_center();
        if self.runtime.zoom_in(center) {
            self.viewport_changed(cx);
        }
    }

    pub fn zoom_out(&mut self, cx: &mut Cx) {
        let center = self.runtime.viewport_center();
        if self.runtime.zoom_out(center) {
            self.viewport_changed(cx);
        }
    }

    pub fn reset_zoom(&mut self, cx: &mut Cx) {
        let center = self.runtime.viewport_center();
        if self.runtime.reset_zoom(center) {
            self.viewport_changed(cx);
        }
    }

    pub fn scroll_to(&mut self, cx: &mut Cx, x: f64, y: f64) {
        let before = (
            self.runtime.scroll.scroll_x(),
            self.runtime.scroll.scroll_y(),
        );
        self.runtime.scroll.scroll_to(x, y);
        if before
            != (
                self.runtime.scroll.scroll_x(),
                self.runtime.scroll.scroll_y(),
            )
        {
            self.viewport_changed(cx);
        }
    }

    pub fn scroll_by(&mut self, cx: &mut Cx, delta_x: f64, delta_y: f64) {
        let before = (
            self.runtime.scroll.scroll_x(),
            self.runtime.scroll.scroll_y(),
        );
        self.runtime.scroll.scroll_by(delta_x, delta_y);
        if before
            != (
                self.runtime.scroll.scroll_x(),
                self.runtime.scroll.scroll_y(),
            )
        {
            self.viewport_changed(cx);
        }
    }

    pub fn snapshot(&self) -> VirtualCanvasSnapshot {
        VirtualCanvasSnapshot {
            visible: self.visible,
            zoom_percent: self.runtime.zoom_percent(),
            scroll_x: self.runtime.scroll.scroll_x(),
            scroll_y: self.runtime.scroll.scroll_y(),
            max_scroll_x: self.runtime.scroll.max_scroll_x(),
            max_scroll_y: self.runtime.scroll.max_scroll_y(),
            frame: self.runtime.frame_state.clone(),
        }
    }

    pub fn last_walk(&self) -> Option<Walk> {
        self.last_walk
    }

    pub fn ruler_marker_snapshots(&self) -> Vec<RulerMarkerSnapshot> {
        self.ruler_hit_regions
            .iter()
            .map(|region| RulerMarkerSnapshot {
                marker_id: region.marker_id.clone(),
                label: region.label.clone(),
                page_index: region.page_index,
                axis: region.axis,
                logical_position: region.logical_position,
                bounds: region.rect,
                editable: region.editable,
            })
            .collect()
    }

    pub fn paragraph_interaction_snapshots(&self) -> Vec<ParagraphInteractionSnapshot> {
        let mut snapshots = Vec::new();
        for region in &self.paragraph_hit_regions {
            if snapshots
                .iter()
                .any(|snapshot: &ParagraphInteractionSnapshot| {
                    snapshot.interaction_id == region.interaction_id
                        && snapshot.page_index == region.page_index
                })
            {
                continue;
            }
            snapshots.push(ParagraphInteractionSnapshot {
                interaction_id: region.interaction_id.clone(),
                page_index: region.page_index,
                bounds: region.editor_bounds,
            });
        }
        snapshots
    }

    fn request_redraw(&mut self, cx: &mut Cx) {
        if self.visible {
            if matches!(self.draw_bg.area(), Area::Empty) {
                cx.redraw_all();
            } else {
                self.draw_bg.redraw(cx);
            }
        }
    }

    fn viewport_changed(&mut self, cx: &mut Cx) {
        self.request_redraw(cx);
        cx.widget_action_with_data(
            &self.action_data,
            self.uid,
            VirtualCanvasAction::ViewportChanged(self.snapshot()),
        );
    }

    fn handle_key(&mut self, cx: &mut Cx, key: KeyCode) {
        let viewport_height = self.runtime.scroll.viewport_size().height.max(1.0);
        match key {
            KeyCode::ArrowLeft => self.scroll_by(cx, -48.0, 0.0),
            KeyCode::ArrowRight => self.scroll_by(cx, 48.0, 0.0),
            KeyCode::ArrowUp => self.scroll_by(cx, 0.0, -48.0),
            KeyCode::ArrowDown => self.scroll_by(cx, 0.0, 48.0),
            KeyCode::PageUp => self.scroll_by(cx, 0.0, -viewport_height * 0.9),
            KeyCode::PageDown => self.scroll_by(cx, 0.0, viewport_height * 0.9),
            KeyCode::Home => self.scroll_to(cx, 0.0, 0.0),
            KeyCode::End => self.scroll_to(
                cx,
                self.runtime.scroll.max_scroll_x(),
                self.runtime.scroll.max_scroll_y(),
            ),
            _ => {}
        }
    }

    fn handle_wheel(&mut self, cx: &mut Cx, event: &FingerScrollEvent) {
        if event.modifiers.control {
            let viewport = self.runtime.frame_state.viewport_rect;
            let anchor = ViewportPoint::new(
                (event.abs.x - viewport.origin.x).clamp(0.0, viewport.size.width),
                (event.abs.y - viewport.origin.y).clamp(0.0, viewport.size.height),
            );
            let delta = if event.scroll.y.abs() > f64::EPSILON {
                event.scroll.y
            } else {
                event.scroll.x
            };
            let factor = if delta > 0.0 { 0.9 } else { 1.0 / 0.9 };
            if self.runtime.set_zoom(self.runtime.zoom() * factor, anchor) {
                self.viewport_changed(cx);
            }
            return;
        }
        let delta = viewport_scroll_delta(event.modifiers.shift, event.scroll);
        self.scroll_by(cx, delta.x, delta.y);
    }

    fn begin_ruler_drag(&mut self, cx: &mut Cx, point: ScreenPoint) -> bool {
        let Some(region) = self
            .ruler_hit_regions
            .iter()
            .rev()
            .find(|region| region.editable && screen_rect_contains(region.rect, point))
            .cloned()
        else {
            return false;
        };
        self.ruler_drag = Some(RulerDragState {
            marker_id: region.marker_id,
            page_index: region.page_index,
            axis: region.axis,
            page_origin: region.page_origin,
            logical_length: region.logical_length,
            zoom: region.zoom,
            logical_position: region.logical_position,
        });
        self.request_redraw(cx);
        true
    }

    fn update_ruler_drag(&mut self, cx: &mut Cx, point: ScreenPoint) -> bool {
        let Some(drag) = self.ruler_drag.as_ref() else {
            return false;
        };
        let position = ruler_position_from_screen(
            drag.axis,
            drag.page_origin,
            drag.logical_length,
            drag.zoom,
            point,
        );
        if (position - drag.logical_position).abs() > f64::EPSILON {
            if let Some(drag) = self.ruler_drag.as_mut() {
                drag.logical_position = position;
            }
            self.request_redraw(cx);
        }
        true
    }

    fn finish_ruler_drag(&mut self, cx: &mut Cx, point: ScreenPoint) -> bool {
        let Some(mut drag) = self.ruler_drag.take() else {
            return false;
        };
        drag.logical_position = ruler_position_from_screen(
            drag.axis,
            drag.page_origin,
            drag.logical_length,
            drag.zoom,
            point,
        );
        self.request_redraw(cx);
        cx.widget_action_with_data(
            &self.action_data,
            self.uid,
            VirtualCanvasAction::RulerMarkerChanged {
                marker_id: drag.marker_id,
                page_index: drag.page_index,
                logical_position: drag.logical_position,
            },
        );
        true
    }

    fn activate_paragraph(&self, cx: &mut Cx, point: ScreenPoint) -> bool {
        let Some(region) = paragraph_hit_region_at(&self.paragraph_hit_regions, point) else {
            return false;
        };
        cx.widget_action_with_data(
            &self.action_data,
            self.uid,
            VirtualCanvasAction::ParagraphActivated {
                interaction_id: region.interaction_id.clone(),
                page_index: region.page_index,
                bounds: region.editor_bounds,
                cursor_byte_offset: paragraph_cursor_at_x(&region.cursor_stops, point.x),
            },
        );
        true
    }

    fn paragraph_cursor_stops(
        &mut self,
        cx: &mut Cx2d,
        line: &PositionedLine,
        placement: &PagePlacement,
        y_shift: f64,
        start_byte_offset: usize,
    ) -> Vec<ParagraphCursorStop> {
        let mut stops = Vec::new();
        let mut byte_offset = start_byte_offset;
        for run in &line.runs {
            let baseline = line.baseline - y_shift;
            let start = self
                .page_local_point(placement, LogicalPoint::new(run.x, baseline))
                .x;
            stops.push(ParagraphCursorStop {
                x: start,
                byte_offset,
            });
            for (index, character) in run.text.char_indices() {
                let end = index + character.len_utf8();
                let width = self
                    .measure_text_run(cx, &run.style, &run.text[..end])
                    .width;
                let x = self
                    .page_local_point(placement, LogicalPoint::new(run.x + width, baseline))
                    .x;
                stops.push(ParagraphCursorStop {
                    x,
                    byte_offset: byte_offset.saturating_add(end),
                });
            }
            byte_offset = byte_offset.saturating_add(run.text.len());
        }
        stops
    }

    fn rect(&mut self, cx: &mut Cx2d, rect: ScreenRect, color: UiColor) {
        if !rect.is_valid() {
            return;
        }
        self.draw_shape.color = Vec4f::from_u32(color.0);
        self.draw_shape.draw_abs(cx, makepad_rect(rect));
    }

    fn outline(&mut self, cx: &mut Cx2d, rect: ScreenRect, color: UiColor, width: f64) {
        if !rect.is_valid() {
            return;
        }
        let width = width.max(0.5).min(rect.size.width.min(rect.size.height));
        self.rect(
            cx,
            ScreenRect::new(rect.origin.x, rect.origin.y, rect.size.width, width),
            color,
        );
        self.rect(
            cx,
            ScreenRect::new(rect.origin.x, rect.bottom() - width, rect.size.width, width),
            color,
        );
        self.rect(
            cx,
            ScreenRect::new(rect.origin.x, rect.origin.y, width, rect.size.height),
            color,
        );
        self.rect(
            cx,
            ScreenRect::new(rect.right() - width, rect.origin.y, width, rect.size.height),
            color,
        );
    }

    // Low-level immediate-mode helper keeps draw style explicit at each call site.
    #[allow(clippy::too_many_arguments)]
    fn text_at(
        &mut self,
        cx: &mut Cx2d,
        text: &str,
        point: ScreenPoint,
        font_size: f32,
        color: UiColor,
        bold: bool,
        italic: bool,
    ) {
        let draw = if bold {
            &mut self.draw_text_bold
        } else if italic {
            &mut self.draw_text_italic
        } else {
            &mut self.draw_text
        };
        draw.text_style.font_size = font_size.max(1.0);
        draw.color = Vec4f::from_u32(color.0);
        draw.draw_abs(cx, dvec2(point.x, point.y), text);
    }

    fn draw_background_pass(&mut self, cx: &mut Cx2d, viewport: ScreenRect) {
        self.draw_bg.color = Vec4f::from_u32(0xdfe4eaff);
        self.draw_bg.draw_abs(cx, makepad_rect(viewport));
    }

    fn draw_page_surface(&mut self, cx: &mut Cx2d, placement: &PagePlacement, page_label: &str) {
        let page = self.runtime.logical_rect_to_screen(placement.rect);
        let shadow = ScreenRect::new(
            page.origin.x + 5.0,
            page.origin.y + 6.0,
            page.size.width,
            page.size.height,
        );
        self.draw_shadow.color = Vec4f::from_u32(0x00000026);
        self.draw_shadow.draw_abs(cx, makepad_rect(shadow));
        self.draw_page.color = Vec4f::from_u32(0xffffffff);
        self.draw_page.draw_abs(cx, makepad_rect(page));
        self.outline(cx, page, UiColor(0xb8c2ceff), 1.0);
        let content = self.runtime.logical_rect_to_screen(placement.content_rect);
        self.outline(cx, content, UiColor(0xe5e7ebff), 0.75);
        self.text_at(
            cx,
            page_label,
            ScreenPoint::new(page.origin.x + 12.0, page.origin.y + 18.0),
            9.0,
            UiColor::MUTED,
            false,
            false,
        );
    }

    fn page_local_rect(&self, placement: &PagePlacement, local: LogicalRect) -> ScreenRect {
        self.runtime.logical_rect_to_screen(LogicalRect::new(
            placement.rect.origin.x + local.origin.x,
            placement.rect.origin.y + local.origin.y,
            local.size.width,
            local.size.height,
        ))
    }

    fn page_local_point(&self, placement: &PagePlacement, local: LogicalPoint) -> ScreenPoint {
        self.runtime.logical_to_screen(LogicalPoint::new(
            placement.rect.origin.x + local.x,
            placement.rect.origin.y + local.y,
        ))
    }

    fn draw_page_rect_primitive(
        &mut self,
        cx: &mut Cx2d,
        item: &PageRectPrimitive,
        placement: &PagePlacement,
    ) {
        self.rect(cx, self.page_local_rect(placement, item.rect), item.fill);
    }

    fn text_draw_variant(&mut self, style: &TextRunStyle) -> &mut DrawText {
        let monospace = self.font_registry.resolve(style.font_family.as_deref())
            == DocumentFontFamily::Monospace;
        match (monospace, style.bold, style.italic) {
            (true, true, true) => &mut self.draw_text_mono_bold_italic,
            (true, true, false) => &mut self.draw_text_mono_bold,
            (true, false, true) => &mut self.draw_text_mono_italic,
            (true, false, false) => &mut self.draw_text_mono,
            (false, true, true) => &mut self.draw_text_bold_italic,
            (false, true, false) => &mut self.draw_text_bold,
            (false, false, true) => &mut self.draw_text_italic,
            (false, false, false) => &mut self.draw_text,
        }
    }

    fn measure_text_run(&mut self, cx: &mut Cx2d, style: &TextRunStyle, text: &str) -> TextMetrics {
        let draw = self.text_draw_variant(style);

        draw.text_style.font_size = style.font_size.max(1.0) as f32;
        let Some(prepared) = draw.prepare_single_line_run(cx, text) else {
            let count = text.chars().count() as f64;
            return TextMetrics {
                width: count * style.font_size * 0.5,
                ascender: style.font_size * 0.8,
                descender: style.font_size * 0.2,
            };
        };
        TextMetrics {
            width: f64::from(prepared.width_in_lpxs),
            ascender: f64::from(prepared.ascender_in_lpxs.abs()),
            descender: f64::from(prepared.descender_in_lpxs.abs()),
        }
    }

    fn draw_text_run(
        &mut self,
        cx: &mut Cx2d,
        text: &str,
        point: ScreenPoint,
        style: &TextRunStyle,
    ) {
        let zoom = self.runtime.zoom();
        let draw = self.text_draw_variant(style);
        draw.text_style.font_size = (style.font_size * zoom).max(1.0) as f32;

        draw.color = Vec4f::from_u32(style.color.0);
        draw.draw_abs(cx, dvec2(point.x, point.y), text);
    }

    fn draw_text_run_on_baseline(
        &mut self,
        cx: &mut Cx2d,
        text: &str,
        baseline: ScreenPoint,
        style: &TextRunStyle,
    ) {
        let metrics = self.measure_text_run(cx, style, text);
        self.draw_text_run(
            cx,
            text,
            ScreenPoint::new(
                baseline.x,
                makepad_text_origin_y(baseline.y, metrics.ascender),
            ),
            style,
        );
    }

    fn layout_page_paragraph(&mut self, cx: &mut Cx2d, item: &PageParagraph) -> ParagraphLayout {
        let input = ParagraphLayoutInput {
            origin: item.origin,
            width: item.width,
            alignment: item.alignment,
            indent_left: item.indent_left,
            indent_right: item.indent_right,
            first_line_indent: item.first_line_indent,
            space_before: item.space_before,
            space_after: item.space_after,
            line_spacing: item.line_spacing,
            marker: item.marker.clone(),
            runs: item.runs.clone(),
        };
        layout_paragraph(&input, &mut |style, text: String| {
            self.measure_text_run(cx, &style, &text)
        })
    }

    fn draw_positioned_line(
        &mut self,
        cx: &mut Cx2d,
        line: &PositionedLine,
        placement: &PagePlacement,
        y_shift: f64,
    ) {
        for run in &line.runs {
            if let Some(color) = run.style.background {
                self.rect(
                    cx,
                    self.page_local_rect(
                        placement,
                        LogicalRect::new(run.x, line.top - y_shift, run.width, line.height),
                    ),
                    color,
                );
            }
        }
        for run in &line.runs {
            self.draw_text_run_on_baseline(
                cx,
                &run.text,
                self.page_local_point(placement, LogicalPoint::new(run.x, line.baseline - y_shift)),
                &run.style,
            );
            let decoration_color = run.style.color;
            if run.style.underline {
                self.rect(
                    cx,
                    self.page_local_rect(
                        placement,
                        LogicalRect::new(
                            run.x,
                            line.baseline - y_shift + 1.0,
                            run.width,
                            (run.style.font_size * 0.06).max(0.7),
                        ),
                    ),
                    decoration_color,
                );
            }
            if run.style.strikethrough {
                self.rect(
                    cx,
                    self.page_local_rect(
                        placement,
                        LogicalRect::new(
                            run.x,
                            line.baseline - y_shift - run.style.font_size * 0.32,
                            run.width,
                            (run.style.font_size * 0.06).max(0.7),
                        ),
                    ),
                    decoration_color,
                );
            }
        }
        if let Some(marker) = &line.marker {
            self.draw_text_run_on_baseline(
                cx,
                &marker.text,
                self.page_local_point(
                    placement,
                    LogicalPoint::new(marker.x, line.baseline - y_shift),
                ),
                &marker.style,
            );
        }
    }

    fn draw_page_paragraph(
        &mut self,
        cx: &mut Cx2d,
        item: &PageParagraph,
        placement: &PagePlacement,
    ) {
        let layout = self.layout_page_paragraph(cx, item);
        for line in &layout.lines {
            self.draw_positioned_line(cx, line, placement, 0.0);
        }
    }

    fn place_measured_paragraph(
        &self,
        frame: &DocumentFrame,
        item: &PageParagraph,
        layout: &ParagraphLayout,
        mut page_index: usize,
        start_y: f64,
    ) -> (Vec<(usize, f64)>, usize, f64) {
        if frame.pages.pages.is_empty() || layout.lines.is_empty() {
            return (Vec::new(), page_index, start_y);
        }
        page_index = page_index.min(frame.pages.pages.len().saturating_sub(1));
        let first_top = layout
            .lines
            .first()
            .map(|line| line.top)
            .unwrap_or(item.origin.y);
        let mut y_shift = first_top - start_y;
        let mut placements = Vec::with_capacity(layout.lines.len());
        let mut last_bottom = start_y;
        for line in &layout.lines {
            let page = frame
                .pages
                .pages
                .get(page_index)
                .or_else(|| frame.pages.pages.last())
                .expect("non-empty page stack");
            let mut local_top = line.top - y_shift;
            if local_top + line.height > page.size.height - page.margins.bottom
                && local_top > page.margins.top
            {
                page_index = page_index.saturating_add(1);
                let next = frame
                    .pages
                    .pages
                    .get(page_index)
                    .or_else(|| frame.pages.pages.last())
                    .expect("non-empty page stack");
                y_shift = line.top - next.margins.top;
                local_top = line.top - y_shift;
            }
            last_bottom = local_top + line.height;
            placements.push((page_index, y_shift));
        }
        (
            placements,
            page_index,
            (last_bottom + item.space_after).max(
                frame
                    .pages
                    .pages
                    .get(page_index)
                    .or_else(|| frame.pages.pages.last())
                    .map(|page| page.margins.top)
                    .unwrap_or(0.0),
            ),
        )
    }

    fn measured_document_flow(
        &mut self,
        cx: &mut Cx2d,
        frame: &DocumentFrame,
    ) -> MeasuredDocumentFlow {
        if frame.pages.pages.is_empty() {
            return MeasuredDocumentFlow::default();
        }
        let top = frame.pages.pages[0].margins.top;
        let mut cursor_page = 0usize;
        let mut cursor_y = top;
        let mut initialized = false;
        let mut flow = MeasuredDocumentFlow::default();
        for (content_index, item) in frame.content.iter().enumerate() {
            match item {
                PageContentItem::Paragraph(paragraph) if paragraph.flow == ParagraphFlow::Body => {
                    let layout = self.layout_page_paragraph(cx, paragraph);
                    let source_page = paragraph
                        .page_index
                        .min(frame.pages.pages.len().saturating_sub(1));
                    if !initialized {
                        cursor_page = source_page;
                        cursor_y = paragraph.origin.y.max(
                            frame
                                .pages
                                .pages
                                .get(cursor_page)
                                .map(|page| page.margins.top)
                                .unwrap_or(top),
                        );
                        initialized = true;
                    } else if source_page > cursor_page {
                        cursor_page = source_page;
                        cursor_y = paragraph.origin.y.max(
                            frame
                                .pages
                                .pages
                                .get(cursor_page)
                                .map(|page| page.margins.top)
                                .unwrap_or(top),
                        );
                    } else if source_page == cursor_page {
                        cursor_y = cursor_y.max(paragraph.origin.y);
                    }
                    let (lines, end_page, end_y) = self.place_measured_paragraph(
                        frame,
                        paragraph,
                        &layout,
                        cursor_page,
                        cursor_y,
                    );
                    flow.paragraphs.push(MeasuredParagraphPlan {
                        content_index,
                        lines,
                    });
                    cursor_page = end_page;
                    cursor_y = end_y;
                }
                PageContentItem::Table(table) => {
                    let (target_page, target_y) =
                        source_anchored_table_position(frame, table, cursor_page, cursor_y);
                    let table_layout = measure_table_layout(
                        table,
                        TABLE_CELL_PADDING,
                        &mut |style: TextRunStyle, text: String| {
                            self.measure_text_run(cx, &style, &text)
                        },
                    );
                    let height = table_layout.height;
                    flow.blocks.push(MeasuredBlockPlan {
                        content_index,
                        page_index: target_page,
                        y_offset: target_y - table.rect.origin.y,
                        table_layout: Some(table_layout),
                    });
                    cursor_page = target_page;
                    cursor_y = target_y + height;
                    initialized = true;
                }
                PageContentItem::Image(image) => {
                    let source_page = image
                        .page_index
                        .min(frame.pages.pages.len().saturating_sub(1));
                    if source_page > cursor_page {
                        cursor_page = source_page;
                        cursor_y = frame
                            .pages
                            .pages
                            .get(cursor_page)
                            .map(|page| page.margins.top)
                            .unwrap_or(top);
                    } else if source_page == cursor_page {
                        cursor_y = cursor_y.max(image.rect.origin.y);
                    }
                    let mut page = frame
                        .pages
                        .pages
                        .get(cursor_page)
                        .or_else(|| frame.pages.pages.last())
                        .expect("non-empty page stack");
                    let height = image.rect.size.height.max(1.0);
                    if cursor_y > page.margins.top
                        && cursor_y + height > page.size.height - page.margins.bottom
                    {
                        cursor_page = cursor_page.saturating_add(1);
                        page = frame
                            .pages
                            .pages
                            .get(cursor_page)
                            .or_else(|| frame.pages.pages.last())
                            .expect("non-empty page stack");
                        cursor_y = page.margins.top;
                    }
                    let target_y = cursor_y.max(page.margins.top);
                    flow.blocks.push(MeasuredBlockPlan {
                        content_index,
                        page_index: cursor_page,
                        y_offset: target_y - image.rect.origin.y,
                        table_layout: None,
                    });
                    cursor_y = target_y + height;
                    initialized = true;
                }
                _ => {}
            }
        }
        flow
    }

    fn ensure_measured_page_capacity(&mut self, cx: &mut Cx2d, frame: &mut DocumentFrame) {
        if frame.pages.pages.is_empty() {
            return;
        }
        let flow = self.measured_document_flow(cx, frame);
        let required = flow
            .paragraphs
            .iter()
            .flat_map(|plan| plan.lines.iter().map(|(page, _)| *page))
            .chain(flow.blocks.iter().map(|plan| plan.page_index))
            .max()
            .unwrap_or_else(|| frame.pages.pages.len().saturating_sub(1));
        let template = frame.pages.pages.last().cloned();
        while frame.pages.pages.len() <= required {
            let Some(mut page) = template.clone() else {
                break;
            };
            let index = frame.pages.pages.len();
            page.id = format!("measured-page-{}", index + 1);
            page.label = format!("Page {}", index + 1);
            frame.pages.pages.push(page);
        }
    }

    fn draw_measured_document_flow(
        &mut self,
        cx: &mut Cx2d,
        frame: &DocumentFrame,
        layout: &PageStackLayout,
        range: &std::ops::Range<usize>,
    ) {
        let flow = self.measured_document_flow(cx, frame);
        for plan in flow.paragraphs {
            let Some(PageContentItem::Paragraph(paragraph)) = frame.content.get(plan.content_index)
            else {
                continue;
            };
            let paragraph_layout = self.layout_page_paragraph(cx, paragraph);
            let interaction_id = paragraph
                .interaction
                .as_ref()
                .map(|interaction| interaction.id.as_str())
                .filter(|id| !id.is_empty());
            let mut interaction_lines = Vec::new();
            let mut byte_offset = 0usize;
            for (line, (page_index, y_shift)) in paragraph_layout.lines.iter().zip(plan.lines) {
                let line_byte_len = line.runs.iter().map(|run| run.text.len()).sum::<usize>();
                if !range.contains(&page_index) {
                    byte_offset = byte_offset.saturating_add(line_byte_len);
                    continue;
                }
                let Some(placement) = layout.placements.get(page_index) else {
                    byte_offset = byte_offset.saturating_add(line_byte_len);
                    continue;
                };
                let page_rect = self.runtime.logical_rect_to_screen(placement.rect);
                cx.push_clip_rect(makepad_rect(page_rect));
                self.draw_positioned_line(cx, line, placement, y_shift);
                cx.pop_clip_rect();
                if interaction_id.is_some() {
                    interaction_lines.push(ParagraphLineHit {
                        page_index,
                        bounds: self.page_local_rect(
                            placement,
                            LogicalRect::new(
                                paragraph.origin.x,
                                line.top - y_shift,
                                paragraph.width,
                                line.height,
                            ),
                        ),
                        cursor_stops: self.paragraph_cursor_stops(
                            cx,
                            line,
                            placement,
                            y_shift,
                            byte_offset,
                        ),
                    });
                }
                byte_offset = byte_offset.saturating_add(line_byte_len);
            }
            if let Some(interaction_id) = interaction_id {
                let mut editor_bounds_by_page = Vec::<(usize, ScreenRect)>::new();
                for line in &interaction_lines {
                    if let Some((_, bounds)) = editor_bounds_by_page
                        .iter_mut()
                        .find(|(page_index, _)| *page_index == line.page_index)
                    {
                        *bounds = screen_rect_union(*bounds, line.bounds);
                    } else {
                        editor_bounds_by_page.push((line.page_index, line.bounds));
                    }
                }
                for line in interaction_lines {
                    let editor_bounds = editor_bounds_by_page
                        .iter()
                        .find(|(page_index, _)| *page_index == line.page_index)
                        .map(|(_, bounds)| *bounds)
                        .unwrap_or(line.bounds);
                    self.paragraph_hit_regions.push(ParagraphHitRegion {
                        interaction_id: interaction_id.to_owned(),
                        page_index: line.page_index,
                        line_bounds: line.bounds,
                        editor_bounds,
                        cursor_stops: line.cursor_stops,
                    });
                }
            }
        }
        for plan in flow.blocks {
            if !range.contains(&plan.page_index) {
                continue;
            }
            let Some(placement) = layout.placements.get(plan.page_index) else {
                continue;
            };
            let page_rect = self.runtime.logical_rect_to_screen(placement.rect);
            cx.push_clip_rect(makepad_rect(page_rect));
            match frame.content.get(plan.content_index) {
                Some(PageContentItem::Table(table)) => self.draw_page_table_with_offset(
                    cx,
                    table,
                    placement,
                    plan.y_offset,
                    plan.table_layout.as_ref(),
                ),
                Some(PageContentItem::Image(image)) => {
                    self.draw_page_image_with_offset(cx, image, placement, plan.y_offset)
                }
                _ => {}
            }
            cx.pop_clip_rect();
        }
    }

    fn draw_page_table(&mut self, cx: &mut Cx2d, item: &PageTable, placement: &PagePlacement) {
        self.draw_page_table_with_offset(cx, item, placement, 0.0, None);
    }

    fn draw_page_table_with_offset(
        &mut self,
        cx: &mut Cx2d,
        item: &PageTable,
        placement: &PagePlacement,
        y_offset: f64,
        measured: Option<&MeasuredTableLayout>,
    ) {
        for (row_index, row) in item.rows.iter().enumerate() {
            let (row_y, row_height) = measured
                .and_then(|layout| layout.rows.get(row_index))
                .map(|row| (row.y, row.height))
                .unwrap_or((row.rect.origin.y, row.rect.size.height));
            for cell in &row.cells {
                let cell_local = LogicalRect::new(
                    cell.rect.origin.x,
                    row_y + y_offset,
                    cell.rect.size.width,
                    row_height,
                );
                let cell_rect = self.page_local_rect(placement, cell_local);
                if let Some(background) = cell.background {
                    self.rect(cx, cell_rect, background);
                }
                self.outline(
                    cx,
                    cell_rect,
                    cell.border.unwrap_or(UiColor(0x64748bff)),
                    0.8,
                );
                let padding = TABLE_CELL_PADDING;
                let mut paragraph = PageParagraph::new(
                    item.page_index,
                    LogicalPoint::new(cell.rect.origin.x + padding, row_y + padding + y_offset),
                    (cell.rect.size.width - padding * 2.0).max(1.0),
                    cell.runs.clone(),
                );
                paragraph.alignment = cell.alignment;
                cx.push_clip_rect(makepad_rect(cell_rect));
                self.draw_page_paragraph(cx, &paragraph, placement);
                cx.pop_clip_rect();
            }
        }
    }

    fn image_texture(&mut self, cx: &mut Cx2d, item: &PageImage) -> Option<Texture> {
        let mut hasher = DefaultHasher::new();
        item.bytes.hash(&mut hasher);
        item.pixel_width.hash(&mut hasher);
        item.pixel_height.hash(&mut hasher);
        let key = hasher.finish();
        if let Some(cached) = self.image_textures.iter().find(|cached| cached.key == key) {
            return Some(cached.texture.clone());
        }
        let decoded = decode_image_from_data(&item.bytes).ok()?;
        let texture = decoded.into_new_texture(cx.cx.cx);
        if self.image_textures.len() >= MAX_IMAGE_TEXTURES {
            self.image_textures.remove(0);
        }
        self.image_textures.push(CachedImageTexture {
            key,
            texture: texture.clone(),
        });
        Some(texture)
    }

    fn draw_page_image(&mut self, cx: &mut Cx2d, item: &PageImage, placement: &PagePlacement) {
        self.draw_page_image_with_offset(cx, item, placement, 0.0);
    }

    fn draw_page_image_with_offset(
        &mut self,
        cx: &mut Cx2d,
        item: &PageImage,
        placement: &PagePlacement,
        y_offset: f64,
    ) {
        let local = LogicalRect::new(
            item.rect.origin.x,
            item.rect.origin.y + y_offset,
            item.rect.size.width,
            item.rect.size.height,
        );
        let rect = self.page_local_rect(placement, local);
        if let Some(texture) = self.image_texture(cx, item) {
            self.draw_image.draw_vars.set_texture(0, &texture);
            self.draw_image.image_dim_w = item.pixel_width as f32;
            self.draw_image.image_dim_h = item.pixel_height as f32;
            self.draw_image.draw_abs(cx, makepad_rect(rect));
        } else {
            self.rect(cx, rect, UiColor(0xe5e7ebff));
            self.outline(cx, rect, UiColor(0xef4444ff), 1.0);
            self.text_at(
                cx,
                "Image unavailable",
                ScreenPoint::new(rect.origin.x + 6.0, rect.origin.y + 16.0),
                9.0,
                UiColor::RED,
                true,
                false,
            );
        }
    }

    fn draw_page_text(&mut self, cx: &mut Cx2d, item: &PageText, placement: &PagePlacement) {
        let point = self.page_local_point(placement, item.origin);
        self.text_at(
            cx,
            &item.text,
            point,
            (item.font_size * self.runtime.zoom()) as f32,
            item.color,
            item.bold,
            item.italic,
        );
    }

    fn draw_content_pass(
        &mut self,
        cx: &mut Cx2d,
        frame: &DocumentFrame,
        layout: &PageStackLayout,
        range: std::ops::Range<usize>,
    ) {
        for index in range.clone() {
            let Some(placement) = layout.placements.get(index) else {
                continue;
            };
            let page_label = frame
                .pages
                .pages
                .get(index)
                .map(|page| page.label.as_str())
                .unwrap_or("Page");
            self.draw_page_surface(cx, placement, page_label);
            let page_rect = self.runtime.logical_rect_to_screen(placement.rect);
            cx.push_clip_rect(makepad_rect(page_rect));
            for item in frame.content.iter().filter(|item| match item {
                PageContentItem::Text(item) => item.page_index == index,
                PageContentItem::Paragraph(item) => {
                    item.flow == ParagraphFlow::Fixed && item.page_index == index
                }
                PageContentItem::Table(_) => false,
                PageContentItem::Image(_) => false,
                PageContentItem::Rect(item) => item.page_index == index,
            }) {
                match item {
                    PageContentItem::Text(item) => self.draw_page_text(cx, item, placement),
                    PageContentItem::Paragraph(item) => {
                        self.draw_page_paragraph(cx, item, placement)
                    }
                    PageContentItem::Table(item) => self.draw_page_table(cx, item, placement),
                    PageContentItem::Image(item) => self.draw_page_image(cx, item, placement),
                    PageContentItem::Rect(item) => {
                        self.draw_page_rect_primitive(cx, item, placement)
                    }
                }
            }
            cx.pop_clip_rect();
        }

        self.draw_measured_document_flow(cx, frame, layout, &range);
    }
    fn draw_overlay_rect(
        &mut self,
        cx: &mut Cx2d,
        overlay: &OverlayRect,
        placement: &PagePlacement,
    ) {
        let rect = self.page_local_rect(placement, overlay.rect);
        self.outline(cx, rect, overlay.color, overlay.line_width);
        if let Some(label) = overlay.label.as_deref() {
            self.text_at(
                cx,
                label,
                ScreenPoint::new(rect.origin.x + 4.0, rect.origin.y + 12.0),
                9.0,
                overlay.color,
                true,
                false,
            );
        }
    }
    fn ruler_marker_position(&self, marker: &RulerMarker, logical_length: f64) -> f64 {
        self.ruler_drag
            .as_ref()
            .filter(|drag| {
                drag.marker_id == marker.id
                    && drag.page_index == marker.page_index
                    && drag.axis == marker.axis
            })
            .map(|drag| drag.logical_position.clamp(0.0, logical_length.max(0.0)))
            .unwrap_or_else(|| marker.clamped_position(logical_length))
    }

    fn register_ruler_hit(
        &mut self,
        marker: &RulerMarker,
        rect: ScreenRect,
        page_rect: ScreenRect,
        logical_length: f64,
        logical_position: f64,
    ) {
        self.ruler_hit_regions.push(RulerHitRegion {
            marker_id: marker.id.clone(),
            label: marker.label.clone(),
            page_index: marker.page_index,
            axis: marker.axis,
            rect,
            page_origin: page_rect.origin,
            logical_length,
            zoom: self.runtime.zoom(),
            logical_position,
            editable: marker.editable,
        });
    }

    fn draw_horizontal_ruler_marker(
        &mut self,
        cx: &mut Cx2d,
        marker: &RulerMarker,
        ruler_rect: ScreenRect,
        page_rect: ScreenRect,
        logical_length: f64,
    ) {
        let logical_position = self.ruler_marker_position(marker, logical_length);
        let x = page_rect.origin.x + logical_position * self.runtime.zoom();
        let active = self.ruler_drag.as_ref().is_some_and(|drag| {
            drag.marker_id == marker.id && drag.page_index == marker.page_index
        });
        let color = if active {
            UiColor(0x1d4ed8ff)
        } else {
            UiColor(0x2563ebff)
        };
        match marker.visual {
            RulerMarkerVisual::TriangleDown => {
                self.rect(
                    cx,
                    ScreenRect::new(x - 5.0, ruler_rect.origin.y + 2.0, 10.0, 3.0),
                    color,
                );
                self.rect(
                    cx,
                    ScreenRect::new(x - 3.0, ruler_rect.origin.y + 5.0, 6.0, 3.0),
                    color,
                );
                self.rect(
                    cx,
                    ScreenRect::new(x - 1.0, ruler_rect.origin.y + 8.0, 2.0, 5.0),
                    color,
                );
            }
            RulerMarkerVisual::TriangleUp => {
                self.rect(
                    cx,
                    ScreenRect::new(x - 1.0, ruler_rect.bottom() - 13.0, 2.0, 5.0),
                    color,
                );
                self.rect(
                    cx,
                    ScreenRect::new(x - 3.0, ruler_rect.bottom() - 8.0, 6.0, 3.0),
                    color,
                );
                self.rect(
                    cx,
                    ScreenRect::new(x - 5.0, ruler_rect.bottom() - 5.0, 10.0, 3.0),
                    color,
                );
            }
            _ => self.rect(
                cx,
                ScreenRect::new(
                    x - 1.5,
                    ruler_rect.origin.y + 2.0,
                    3.0,
                    ruler_rect.size.height - 4.0,
                ),
                color,
            ),
        }
        let hit_rect = match marker.visual {
            RulerMarkerVisual::TriangleDown => ScreenRect::new(
                x - RULER_MARKER_HIT_WIDTH * 0.5,
                ruler_rect.origin.y,
                RULER_MARKER_HIT_WIDTH,
                ruler_rect.size.height * 0.58,
            ),
            RulerMarkerVisual::TriangleUp => ScreenRect::new(
                x - RULER_MARKER_HIT_WIDTH * 0.5,
                ruler_rect.origin.y + ruler_rect.size.height * 0.42,
                RULER_MARKER_HIT_WIDTH,
                ruler_rect.size.height * 0.58,
            ),
            _ => ScreenRect::new(
                x - RULER_MARKER_HIT_WIDTH * 0.5,
                ruler_rect.origin.y,
                RULER_MARKER_HIT_WIDTH,
                ruler_rect.size.height,
            ),
        };
        self.register_ruler_hit(
            marker,
            hit_rect,
            page_rect,
            logical_length,
            logical_position,
        );
    }

    fn draw_vertical_ruler_marker(
        &mut self,
        cx: &mut Cx2d,
        marker: &RulerMarker,
        ruler_rect: ScreenRect,
        page_rect: ScreenRect,
        logical_length: f64,
    ) {
        let logical_position = self.ruler_marker_position(marker, logical_length);
        let y = page_rect.origin.y + logical_position * self.runtime.zoom();
        let active = self.ruler_drag.as_ref().is_some_and(|drag| {
            drag.marker_id == marker.id && drag.page_index == marker.page_index
        });
        let color = if active {
            UiColor(0x7e22ceff)
        } else {
            UiColor(0x9333eaff)
        };
        match marker.visual {
            RulerMarkerVisual::TriangleRight => {
                self.rect(
                    cx,
                    ScreenRect::new(ruler_rect.origin.x + 2.0, y - 5.0, 3.0, 10.0),
                    color,
                );
                self.rect(
                    cx,
                    ScreenRect::new(ruler_rect.origin.x + 5.0, y - 3.0, 3.0, 6.0),
                    color,
                );
                self.rect(
                    cx,
                    ScreenRect::new(ruler_rect.origin.x + 8.0, y - 1.0, 5.0, 2.0),
                    color,
                );
            }
            RulerMarkerVisual::TriangleLeft => {
                self.rect(
                    cx,
                    ScreenRect::new(ruler_rect.right() - 13.0, y - 1.0, 5.0, 2.0),
                    color,
                );
                self.rect(
                    cx,
                    ScreenRect::new(ruler_rect.right() - 8.0, y - 3.0, 3.0, 6.0),
                    color,
                );
                self.rect(
                    cx,
                    ScreenRect::new(ruler_rect.right() - 5.0, y - 5.0, 3.0, 10.0),
                    color,
                );
            }
            _ => self.rect(
                cx,
                ScreenRect::new(
                    ruler_rect.origin.x + 2.0,
                    y - 1.5,
                    ruler_rect.size.width - 4.0,
                    3.0,
                ),
                color,
            ),
        }
        self.register_ruler_hit(
            marker,
            ScreenRect::new(
                ruler_rect.origin.x,
                y - RULER_MARKER_HIT_WIDTH * 0.5,
                ruler_rect.size.width,
                RULER_MARKER_HIT_WIDTH,
            ),
            page_rect,
            logical_length,
            logical_position,
        );
    }

    fn draw_horizontal_ruler(
        &mut self,
        cx: &mut Cx2d,
        ruler: HorizontalRuler,
        placement: &PagePlacement,
        frame: &DocumentFrame,
    ) {
        let page_rect = self.runtime.logical_rect_to_screen(placement.rect);
        let viewport = self.runtime.frame_state.viewport_rect;
        let thickness = ruler.thickness;
        let ruler_rect = ScreenRect::new(
            page_rect.origin.x,
            (page_rect.origin.y - thickness - 4.0).max(viewport.origin.y + 2.0),
            page_rect.size.width,
            thickness,
        );
        self.rect(cx, ruler_rect, UiColor(0xf8fafcff));
        self.outline(cx, ruler_rect, UiColor(0xcbd5e1ff), 1.0);
        if let Some(page) = frame.pages.pages.get(placement.index) {
            let bands = ruler.margin_bands(page.size.width, page.margins);
            self.rect(
                cx,
                ScreenRect::new(
                    ruler_rect.origin.x,
                    ruler_rect.origin.y,
                    bands.start * self.runtime.zoom(),
                    ruler_rect.size.height,
                ),
                UiColor(0xe2e8f0ff),
            );
            self.rect(
                cx,
                ScreenRect::new(
                    ruler_rect.origin.x + bands.end * self.runtime.zoom(),
                    ruler_rect.origin.y,
                    (page.size.width - bands.end) * self.runtime.zoom(),
                    ruler_rect.size.height,
                ),
                UiColor(0xe2e8f0ff),
            );
            for tick in ruler.scale.ticks(page.size.width) {
                let x = page_rect.origin.x + tick.position * self.runtime.zoom();
                let height = (ruler_rect.size.height - 4.0) * tick.length_fraction;
                self.rect(
                    cx,
                    ScreenRect::new(x, ruler_rect.bottom() - height, 0.8, height),
                    UiColor(0x64748bff),
                );
                if let Some(label) = tick.label.as_deref() {
                    self.text_at(
                        cx,
                        label,
                        ScreenPoint::new(x + 2.0, ruler_rect.origin.y + 9.0),
                        7.0,
                        UiColor::MUTED,
                        false,
                        false,
                    );
                }
            }
            for marker in frame.ruler_markers.iter().filter(|marker| {
                marker.page_index == placement.index && marker.axis == RulerAxis::Horizontal
            }) {
                self.draw_horizontal_ruler_marker(
                    cx,
                    marker,
                    ruler_rect,
                    page_rect,
                    page.size.width,
                );
            }
        }
    }

    fn draw_vertical_ruler(
        &mut self,
        cx: &mut Cx2d,
        ruler: VerticalRuler,
        placement: &PagePlacement,
        frame: &DocumentFrame,
    ) {
        let page_rect = self.runtime.logical_rect_to_screen(placement.rect);
        let viewport = self.runtime.frame_state.viewport_rect;
        let thickness = ruler.thickness;
        let ruler_rect = ScreenRect::new(
            (page_rect.origin.x - thickness - 4.0).max(viewport.origin.x + 2.0),
            page_rect.origin.y,
            thickness,
            page_rect.size.height,
        );
        self.rect(cx, ruler_rect, UiColor(0xf8fafcff));
        self.outline(cx, ruler_rect, UiColor(0xcbd5e1ff), 1.0);
        if let Some(page) = frame.pages.pages.get(placement.index) {
            let bands = ruler.margin_bands(page.size.height, page.margins);
            self.rect(
                cx,
                ScreenRect::new(
                    ruler_rect.origin.x,
                    ruler_rect.origin.y,
                    ruler_rect.size.width,
                    bands.start * self.runtime.zoom(),
                ),
                UiColor(0xe2e8f0ff),
            );
            self.rect(
                cx,
                ScreenRect::new(
                    ruler_rect.origin.x,
                    ruler_rect.origin.y + bands.end * self.runtime.zoom(),
                    ruler_rect.size.width,
                    (page.size.height - bands.end) * self.runtime.zoom(),
                ),
                UiColor(0xe2e8f0ff),
            );
            for tick in ruler.scale.ticks(page.size.height) {
                let y = page_rect.origin.y + tick.position * self.runtime.zoom();
                let width = (ruler_rect.size.width - 4.0) * tick.length_fraction;
                self.rect(
                    cx,
                    ScreenRect::new(ruler_rect.right() - width, y, width, 0.8),
                    UiColor(0x64748bff),
                );
            }
            for marker in frame.ruler_markers.iter().filter(|marker| {
                marker.page_index == placement.index && marker.axis == RulerAxis::Vertical
            }) {
                self.draw_vertical_ruler_marker(
                    cx,
                    marker,
                    ruler_rect,
                    page_rect,
                    page.size.height,
                );
            }
        }
    }

    fn draw_scrollbars(&mut self, cx: &mut Cx2d) {
        let viewport = self.runtime.frame_state.viewport_rect;
        let scroll = self.runtime.scroll;
        if scroll.max_scroll_y() > 0.5 {
            let height = (viewport.size.height * viewport.size.height
                / scroll.content_size().height.max(1.0))
            .clamp(32.0, viewport.size.height);
            let y = viewport.origin.y
                + (viewport.size.height - height)
                    * (scroll.scroll_y() / scroll.max_scroll_y()).clamp(0.0, 1.0);
            self.rect(
                cx,
                ScreenRect::new(viewport.right() - 6.0, y, 4.0, height),
                UiColor(0x64748baa),
            );
        }
        if scroll.max_scroll_x() > 0.5 {
            let width = (viewport.size.width * viewport.size.width
                / scroll.content_size().width.max(1.0))
            .clamp(32.0, viewport.size.width);
            let x = viewport.origin.x
                + (viewport.size.width - width)
                    * (scroll.scroll_x() / scroll.max_scroll_x()).clamp(0.0, 1.0);
            self.rect(
                cx,
                ScreenRect::new(x, viewport.bottom() - 6.0, width, 4.0),
                UiColor(0x64748baa),
            );
        }
    }

    fn draw_overlay_pass(
        &mut self,
        cx: &mut Cx2d,
        frame: &DocumentFrame,
        layout: &PageStackLayout,
    ) {
        for overlay in &frame.overlay.rects {
            if let Some(placement) = layout.placements.get(overlay.page_index) {
                self.draw_overlay_rect(cx, overlay, placement);
            }
        }
        let active_index = self
            .runtime
            .frame_state
            .visible_range
            .as_ref()
            .map(|range| range.start);
        if let Some(active) = active_index.and_then(|index| layout.placements.get(index)) {
            if let Some(ruler) = frame.horizontal_ruler {
                self.draw_horizontal_ruler(cx, ruler, active, frame);
            }
            if let Some(ruler) = frame.vertical_ruler {
                self.draw_vertical_ruler(cx, ruler, active, frame);
            }
        }
        self.draw_scrollbars(cx);
    }

    #[cfg(debug_assertions)]
    fn draw_debug_pass(&mut self, cx: &mut Cx2d, page_count: usize) {
        if !self.debug_overlay {
            return;
        }
        let state = self.runtime.frame_state.clone();
        let viewport = state.viewport_rect;
        let clip = state.clip_rect;
        self.outline(cx, viewport, UiColor(0xdc2626ff), 1.5);
        self.outline(cx, clip, UiColor(0x16a34aff), 1.0);
        let lines = [
            format!(
                "viewport x={:.1} y={:.1} w={:.1} h={:.1}",
                viewport.origin.x, viewport.origin.y, viewport.size.width, viewport.size.height
            ),
            format!(
                "clip x={:.1} y={:.1} w={:.1} h={:.1}",
                clip.origin.x, clip.origin.y, clip.size.width, clip.size.height
            ),
            format!(
                "zoom={}%; scroll=({:.1},{:.1}); content={:.1}x{:.1}",
                self.runtime.zoom_percent(),
                self.runtime.scroll.scroll_x(),
                self.runtime.scroll.scroll_y(),
                state.content_size.width,
                state.content_size.height
            ),
            format!(
                "pages={}; visible={:?}; overscan={:?}; draw_called={}; draw_count={}",
                page_count,
                state.visible_range,
                state.overscan_range,
                state.draw_called,
                state.draw_count
            ),
        ];
        self.rect(
            cx,
            ScreenRect::new(
                viewport.origin.x + 8.0,
                viewport.origin.y + 8.0,
                650.0,
                62.0,
            ),
            UiColor(0xffffffe8),
        );
        for (index, line) in lines.iter().enumerate() {
            self.text_at(
                cx,
                line,
                ScreenPoint::new(
                    viewport.origin.x + 14.0,
                    viewport.origin.y + 22.0 + index as f64 * 13.0,
                ),
                9.0,
                UiColor::BLACK,
                index == 0,
                false,
            );
        }
    }

    #[cfg(not(debug_assertions))]
    fn draw_debug_pass(&mut self, _cx: &mut Cx2d, _page_count: usize) {}
}

// Makepad normalizes input so physical downward/rightward scrolling has a
// positive delta. ScrollViewport uses that same direction: its offset grows
// while the document moves upward/leftward on screen.
fn viewport_scroll_delta(shift: bool, scroll: DVec2) -> DVec2 {
    if shift {
        let delta = if scroll.y.abs() > f64::EPSILON {
            scroll.y
        } else {
            scroll.x
        };
        dvec2(delta, 0.0)
    } else {
        scroll
    }
}
impl WidgetNode for VirtualCanvas {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.draw_bg.area()
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }

    fn set_action_data(&mut self, data: std::sync::Arc<dyn ActionTrait>) {
        self.action_data.set_box(data)
    }

    fn action_data(&self) -> Option<std::sync::Arc<dyn ActionTrait>> {
        self.action_data.clone_data()
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible {
            self.ruler_drag = None;
            self.ruler_hit_regions.clear();
        }
        let change = self.runtime.set_visible(visible);
        if change.redraw_required && matches!(self.draw_bg.area(), Area::Empty) {
            cx.redraw_all();
        } else {
            self.draw_bg.redraw(cx);
        }
    }

    fn visible(&self) -> bool {
        self.visible
    }
}

impl Widget for VirtualCanvas {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible {
            return;
        }
        let area = self.draw_bg.area();
        match event.hits_with_capture_overload(cx, area, true) {
            Hit::FingerDown(event) if event.is_primary_hit() => {
                cx.set_key_focus(area);
                let point = ScreenPoint::new(event.abs.x, event.abs.y);
                if !self.begin_ruler_drag(cx, point) {
                    self.activate_paragraph(cx, point);
                }
            }
            Hit::FingerMove(event) => {
                self.update_ruler_drag(cx, ScreenPoint::new(event.abs.x, event.abs.y));
            }
            Hit::FingerUp(event) => {
                self.finish_ruler_drag(cx, ScreenPoint::new(event.abs.x, event.abs.y));
            }
            Hit::FingerScroll(event) if self.ruler_drag.is_none() => self.handle_wheel(cx, &event),
            Hit::KeyDown(event) => self.handle_key(cx, event.key_code),
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            self.runtime.set_visible(false);
            return DrawStep::done();
        }
        self.runtime.set_visible(true);
        self.ruler_hit_regions.clear();
        self.paragraph_hit_regions.clear();

        self.last_walk = Some(walk);
        let viewport_makepad = cx.walk_turtle(walk);
        let viewport = screen_rect(viewport_makepad);

        self.draw_background_pass(cx, viewport);
        // The current draw's Area clip is finalized only after turtle alignment.
        // Use the just-resolved viewport as this widget's explicit scissor now;
        // Makepad intersects it with every already-active parent clip.
        let clip = viewport;
        let Some(mut frame) = self.frame.clone() else {
            self.runtime
                .prepare_layout(&Default::default(), viewport, clip, OVERSCAN_PAGES);
            self.draw_debug_pass(cx, 0);
            return DrawStep::done();
        };
        self.ensure_measured_page_capacity(cx, &mut frame);
        let layout = self
            .runtime
            .prepare_layout(&frame.pages, viewport, clip, OVERSCAN_PAGES);

        if !clip.is_valid() {
            self.draw_debug_pass(cx, frame.pages.pages.len());
            return DrawStep::done();
        }
        cx.push_clip_rect(makepad_rect(clip));
        if let Some(range) = layout.overscan_range.clone() {
            self.draw_content_pass(cx, &frame, &layout.page_stack, range);
        }
        self.draw_overlay_pass(cx, &frame, &layout.page_stack);
        self.draw_debug_pass(cx, frame.pages.pages.len());
        cx.pop_clip_rect();
        DrawStep::done()
    }
}

fn makepad_rect(rect: ScreenRect) -> Rect {
    Rect {
        pos: dvec2(rect.origin.x, rect.origin.y),
        size: dvec2(rect.size.width, rect.size.height),
    }
}

fn screen_rect_contains(rect: ScreenRect, point: ScreenPoint) -> bool {
    rect.is_valid()
        && point.x >= rect.origin.x
        && point.x <= rect.right()
        && point.y >= rect.origin.y
        && point.y <= rect.bottom()
}

fn screen_rect_union(left: ScreenRect, right: ScreenRect) -> ScreenRect {
    if !left.is_valid() {
        return right;
    }
    if !right.is_valid() {
        return left;
    }
    let x = left.origin.x.min(right.origin.x);
    let y = left.origin.y.min(right.origin.y);
    let right_edge = left.right().max(right.right());
    let bottom_edge = left.bottom().max(right.bottom());
    ScreenRect::new(x, y, right_edge - x, bottom_edge - y)
}

fn paragraph_hit_region_at(
    regions: &[ParagraphHitRegion],
    point: ScreenPoint,
) -> Option<&ParagraphHitRegion> {
    regions
        .iter()
        .rev()
        .find(|region| screen_rect_contains(region.line_bounds, point))
}

fn paragraph_cursor_at_x(stops: &[ParagraphCursorStop], x: f64) -> usize {
    stops
        .iter()
        .min_by(|left, right| {
            (left.x - x)
                .abs()
                .partial_cmp(&(right.x - x).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|stop| stop.byte_offset)
        .unwrap_or(0)
}

fn ruler_position_from_screen(
    axis: RulerAxis,
    page_origin: ScreenPoint,
    logical_length: f64,
    zoom: f64,
    point: ScreenPoint,
) -> f64 {
    let screen_delta = match axis {
        RulerAxis::Horizontal => point.x - page_origin.x,
        RulerAxis::Vertical => point.y - page_origin.y,
    };
    let position = screen_delta / zoom.max(0.000_001);
    if !position.is_finite() || !logical_length.is_finite() || logical_length <= 0.0 {
        0.0
    } else {
        position.clamp(0.0, logical_length)
    }
}

fn screen_rect(rect: Rect) -> ScreenRect {
    ScreenRect::new(rect.pos.x, rect.pos.y, rect.size.x, rect.size.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downward_wheel_delta_advances_document_scroll() {
        let delta = viewport_scroll_delta(false, dvec2(0.0, 120.0));
        assert_eq!(delta.x, 0.0);
        assert_eq!(delta.y, 120.0);
    }

    #[test]
    fn shift_wheel_maps_vertical_delta_to_horizontal_scroll() {
        let delta = viewport_scroll_delta(true, dvec2(0.0, 120.0));
        assert_eq!(delta.x, 120.0);
        assert_eq!(delta.y, 0.0);
    }

    #[test]
    fn makepad_text_origin_is_derived_from_the_layout_baseline() {
        assert_eq!(makepad_text_origin_y(42.0, 11.5), 30.5);
        assert_eq!(makepad_text_origin_y(42.0, -3.0), 42.0);
    }

    #[test]
    fn ruler_pointer_is_converted_once_to_page_local_logical_units() {
        let origin = ScreenPoint::new(120.0, 80.0);
        assert_eq!(
            ruler_position_from_screen(
                RulerAxis::Horizontal,
                origin,
                600.0,
                2.0,
                ScreenPoint::new(320.0, 0.0),
            ),
            100.0
        );
        assert_eq!(
            ruler_position_from_screen(
                RulerAxis::Vertical,
                origin,
                800.0,
                0.5,
                ScreenPoint::new(0.0, 1_000.0),
            ),
            800.0
        );
    }

    #[test]
    fn paragraph_hit_prefers_the_topmost_line_and_uses_nearest_cursor_stop() {
        let region = ParagraphHitRegion {
            interaction_id: "consumer:paragraph:7".to_owned(),
            page_index: 0,
            line_bounds: ScreenRect::new(20.0, 30.0, 100.0, 18.0),
            editor_bounds: ScreenRect::new(20.0, 30.0, 300.0, 54.0),
            cursor_stops: vec![
                ParagraphCursorStop {
                    x: 20.0,
                    byte_offset: 0,
                },
                ParagraphCursorStop {
                    x: 42.0,
                    byte_offset: 2,
                },
                ParagraphCursorStop {
                    x: 65.0,
                    byte_offset: 5,
                },
            ],
        };
        let mut later = region.clone();
        later.interaction_id = "consumer:paragraph:8".to_owned();
        let regions = vec![region, later];
        let point = ScreenPoint::new(61.0, 36.0);
        let hit = paragraph_hit_region_at(&regions, point).expect("line must be interactive");
        assert_eq!(hit.interaction_id, "consumer:paragraph:8");
        assert_eq!(paragraph_cursor_at_x(&hit.cursor_stops, point.x), 5);
    }

    #[test]
    fn source_paged_table_keeps_declared_page_and_local_origin() {
        use makepad_toolkit_core::geometry::LogicalSize;
        use makepad_toolkit_core::pages::{PageMargins, PageStack, PageSurface};

        let pages = PageStack::new(vec![
            PageSurface::new("1", "Page 1", LogicalSize::new(600.0, 800.0))
                .with_margins(PageMargins::new(50.0, 50.0, 50.0, 50.0)),
            PageSurface::new("2", "Page 2", LogicalSize::new(600.0, 800.0))
                .with_margins(PageMargins::new(50.0, 50.0, 50.0, 50.0)),
        ]);
        let frame = DocumentFrame::new(pages);
        let first_segment = PageTable {
            page_index: 0,
            rect: LogicalRect::new(50.0, 100.0, 500.0, 690.0),
            columns: vec![500.0],
            rows: Vec::new(),
        };
        let second_segment = PageTable {
            page_index: 1,
            rect: LogicalRect::new(50.0, 60.0, 500.0, 680.0),
            columns: vec![500.0],
            rows: Vec::new(),
        };

        assert_eq!(
            source_anchored_table_position(&frame, &first_segment, 0, 100.0),
            (0, 100.0)
        );
        assert_eq!(
            source_anchored_table_position(&frame, &second_segment, 0, 790.0),
            (1, 60.0)
        );
    }

    #[test]
    fn measured_table_rows_expand_to_fit_wrapped_cell_text() {
        use makepad_toolkit_core::content::{PageTableCell, PageTableRow};
        use makepad_toolkit_core::text_layout::{TextAlignment, TextRun};

        let style = TextRunStyle {
            font_size: 10.0,
            ..TextRunStyle::default()
        };
        let table = PageTable {
            page_index: 0,
            rect: LogicalRect::new(0.0, 0.0, 40.0, 40.0),
            columns: vec![40.0],
            rows: vec![
                PageTableRow {
                    rect: LogicalRect::new(0.0, 0.0, 40.0, 20.0),
                    cells: vec![PageTableCell {
                        rect: LogicalRect::new(0.0, 0.0, 40.0, 20.0),
                        text: "abcdefghijkl".to_owned(),
                        runs: vec![TextRun::new("abcdefghijkl", style.clone())],
                        alignment: TextAlignment::Left,
                        background: None,
                        border: None,
                    }],
                },
                PageTableRow {
                    rect: LogicalRect::new(0.0, 20.0, 40.0, 20.0),
                    cells: vec![PageTableCell {
                        rect: LogicalRect::new(0.0, 20.0, 40.0, 20.0),
                        text: "ok".to_owned(),
                        runs: vec![TextRun::new("ok", style)],
                        alignment: TextAlignment::Left,
                        background: None,
                        border: None,
                    }],
                },
            ],
        };
        let mut measurer = |style: TextRunStyle, text: String| TextMetrics {
            width: text.chars().count() as f64 * style.font_size * 0.5,
            ascender: style.font_size * 0.8,
            descender: style.font_size * 0.2,
        };

        let layout = measure_table_layout(&table, TABLE_CELL_PADDING, &mut measurer);

        assert_eq!(layout.rows[0].height, 30.0);
        assert_eq!(layout.rows[1].y, 30.0);
        assert_eq!(layout.rows[1].height, 20.0);
        assert_eq!(layout.height, 50.0);
    }

    #[test]
    fn measured_table_rows_never_shrink_source_geometry() {
        let table = PageTable {
            page_index: 0,
            rect: LogicalRect::new(0.0, 12.0, 80.0, 48.0),
            columns: vec![80.0],
            rows: Vec::new(),
        };
        let mut measurer = |_style: TextRunStyle, _text: String| TextMetrics::default();

        let layout = measure_table_layout(&table, TABLE_CELL_PADDING, &mut measurer);

        assert!(layout.rows.is_empty());
        assert_eq!(layout.height, 48.0);
    }
}
