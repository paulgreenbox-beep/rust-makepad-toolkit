//! Makepad adapter for camera-relative screen segments. No document authority here.
use drafting_core::Point2;
use makepad_widgets::*;
use makepad_widgets::makepad_draw::shader::draw_text::TextOverflow;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

script_mod! {
    use mod.prelude.widgets.*
    use mod.draw
    use mod.shader.*

    mod.draw.DrawDraftSegment = mod.std.set_type_default() do #(DrawDraftSegment::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: true
        draw_depth: 1.0
        pixel: fn() {
            let point = self.pos * self.rect_size
            let edge = self.edge_end - self.edge_start
            let fraction = clamp(dot(point - self.edge_start, edge) / max(dot(edge, edge), 0.000001), 0.0, 1.0)
            let distance = length(point - (self.edge_start + edge * fraction))
            let half_width = self.line_width * 0.5
            let coverage = 1.0 - smoothstep(max(0.0, half_width - 0.65), half_width + 0.65, distance)
            return Pal.premul(vec4(self.line_color.xyz, self.line_color.w * coverage))
        }
    }

    mod.draw.DrawAnalysisTriangle = mod.std.set_type_default() do #(DrawAnalysisTriangle::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: true
        draw_depth: 1.0
        pixel: fn() {
            let p = self.pos * self.rect_size
            let d1 = (p.x-self.tri_b.x)*(self.tri_a.y-self.tri_b.y)-(self.tri_a.x-self.tri_b.x)*(p.y-self.tri_b.y)
            let d2 = (p.x-self.tri_c.x)*(self.tri_b.y-self.tri_c.y)-(self.tri_b.x-self.tri_c.x)*(p.y-self.tri_c.y)
            let d3 = (p.x-self.tri_a.x)*(self.tri_c.y-self.tri_a.y)-(self.tri_c.x-self.tri_a.x)*(p.y-self.tri_a.y)
            if ((d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)) {
                return Pal.premul(self.fill_color)
            }
            return vec4(0.0,0.0,0.0,0.0)
        }
    }

    mod.widgets.DraftingCanvasBase = #(DraftingCanvas::register_widget(vm))
    mod.widgets.DraftingCanvas = set_type_default() do mod.widgets.DraftingCanvasBase{
        width: Fill
        height: Fill
        draw_bg: mod.draw.DrawColor{color: #x101820 draw_depth: 0.0}
        draw_model: mod.draw.DrawDraftSegment{}
        draw_overlay: mod.draw.DrawDraftSegment{}
        draw_analysis: mod.draw.DrawDraftSegment{}
        draw_analysis_triangle: mod.draw.DrawAnalysisTriangle{}
        draw_analysis_text: mod.draw.DrawText{color: #xe8eef5 draw_depth: 2.0 text_style: theme.font_regular{font_size: 10.5}}
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawDraftSegment {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    edge_start: Vec2f,
    #[live]
    edge_end: Vec2f,
    #[live]
    line_color: Vec4f,
    #[live(1.6)]
    line_width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenSegment {
    pub start: Point2,
    pub end: Point2,
    pub color: [f32; 4],
    pub width: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenTriangle { pub vertices: [Point2;3], pub color: [f32;4] }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenLabelPlacement {
    #[default]
    Absolute,
    Hud,
    Legend,
    Footer,
    Callout,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenLabel {
    pub position: Point2,
    pub text: String,
    pub color: [f32; 4],
    pub placement: ScreenLabelPlacement,
}
/// Retained analysis decorations; these never become persistent DraftEntity objects.
#[derive(Clone, Debug, Default)]
pub struct AnalysisCanvasLayer {
    pub triangles: Vec<ScreenTriangle>,
    pub segments: Vec<ScreenSegment>,
    pub labels: Vec<ScreenLabel>,
}

fn analysis_layers_match(left:&Arc<AnalysisCanvasLayer>,right:&Arc<AnalysisCanvasLayer>)->bool {
    Arc::ptr_eq(left,right) || (left.triangles.is_empty()&&left.segments.is_empty()&&left.labels.is_empty()&&right.triangles.is_empty()&&right.segments.is_empty()&&right.labels.is_empty())
}

const ANNOTATION_INSET: f64 = 12.0;
const ANNOTATION_GAP: f64 = 4.0;

fn annotation_rects_overlap(a: Rect, b: Rect) -> bool {
    a.pos.x < b.pos.x + b.size.x + ANNOTATION_GAP
        && a.pos.x + a.size.x + ANNOTATION_GAP > b.pos.x
        && a.pos.y < b.pos.y + b.size.y + ANNOTATION_GAP
        && a.pos.y + a.size.y + ANNOTATION_GAP > b.pos.y
}

/// All values are canvas-local logical pixels. Only text rectangles move.
fn place_analysis_annotation(
    safe: Rect,
    desired: Vec2d,
    size: Vec2d,
    occupied: &[Rect],
) -> Option<Rect> {
    if ![safe.pos.x, safe.pos.y, safe.size.x, safe.size.y,
        desired.x, desired.y, size.x, size.y].iter().all(|v| v.is_finite())
        || size.x <= 0.0 || size.y <= 0.0
        || size.x > safe.size.x || size.y > safe.size.y
    {
        return None;
    }
    let max_x = safe.pos.x + safe.size.x - size.x;
    let max_y = safe.pos.y + safe.size.y - size.y;
    let x = desired.x.clamp(safe.pos.x, max_x);
    let preferred_y = desired.y.clamp(safe.pos.y, max_y);
    let candidates = std::iter::once(preferred_y)
        .chain(std::iter::once(safe.pos.y))
        .chain(occupied.iter().flat_map(|r| [
            r.pos.y - size.y - ANNOTATION_GAP,
            r.pos.y + r.size.y + ANNOTATION_GAP,
        ]));
    candidates
        .filter(|y| y.is_finite() && *y >= safe.pos.y && *y <= max_y)
        .map(|y| Rect { pos: dvec2(x, y), size })
        .filter(|r| !occupied.iter().any(|other| annotation_rects_overlap(*r, *other)))
        .min_by(|a, b| (a.pos.y - preferred_y).abs()
            .total_cmp(&(b.pos.y - preferred_y).abs()))
}

fn draw_analysis_labels(
    draw: &mut DrawText,
    cx: &mut Cx2d,
    canvas: Rect,
    labels: &[ScreenLabel],
) {
    // Planar/legacy producers retain their original absolute drawing contract.
    for label in labels.iter().filter(|l| l.placement == ScreenLabelPlacement::Absolute) {
        if !label.position.is_finite() { continue; }
        draw.color = vec4(label.color[0], label.color[1], label.color[2], label.color[3]);
        draw.draw_abs(cx, canvas.pos + dvec2(label.position.x, label.position.y), &label.text);
    }
    if !labels.iter().any(|l| l.placement != ScreenLabelPlacement::Absolute) {
        return;
    }
    let safe = Rect {
        pos: dvec2(ANNOTATION_INSET, ANNOTATION_INSET),
        size: dvec2(canvas.size.x - 2.0 * ANNOTATION_INSET,
                    canvas.size.y - 2.0 * ANNOTATION_INSET),
    };
    if ![safe.size.x, safe.size.y].iter().all(|v| v.is_finite() && *v > 0.0) {
        return; // The parent owns the clip; a zero-sized canvas proves no pixels.
    }
    // Shape complete strings. Never use ellipsis, max_lines or character counts.
    let old_max_lines = std::mem::replace(&mut draw.max_lines, 0);
    let old_overflow = std::mem::replace(&mut draw.text_overflow, TextOverflow::Clip);
    let scale = f64::from(draw.font_scale.max(0.0001));
    let notice_text = "Some annotations do not fit; see Results / slice table / Report.";
    let notice = draw.layout(cx, 0.0, 0.0, Some((safe.size.x / scale) as f32),
                             true, Align::default(), notice_text);
    let notice_size = dvec2(f64::from(notice.size_in_lpxs.width) * scale,
                            f64::from(notice.size_in_lpxs.height) * scale);
    let notice_rect = Rect {
        pos: dvec2(safe.pos.x,
                    (safe.pos.y + safe.size.y - notice_size.y).max(safe.pos.y)),
        size: notice_size,
    };
    // Reserve the explicit overflow notice before laying out any content.
    let mut occupied = vec![notice_rect];
    let mut prepared = Vec::new();
    let mut overflow = false;
    for label in labels.iter().filter(|l| l.placement != ScreenLabelPlacement::Absolute) {
        if !label.position.is_finite() || label.text.is_empty() { continue; }
        let width = if label.placement == ScreenLabelPlacement::Callout {
            safe.size.x.min(300.0)
        } else {
            safe.size.x
        };
        let laidout = draw.layout(cx, 0.0, 0.0, Some((width / scale) as f32),
                                  true, Align::default(), &label.text);
        let size = dvec2(f64::from(laidout.size_in_lpxs.width) * scale,
                         f64::from(laidout.size_in_lpxs.height) * scale);
        if laidout.rows.is_empty() {
            overflow = true;
            continue;
        }
        prepared.push((label, laidout, size));
    }
    let mut footer_y = notice_rect.pos.y - ANNOTATION_GAP;
    let mut flow_y = safe.pos.y;
    // Footer/HUD/legend rectangles are reserved before any geometry callout.
    for placement in [
        ScreenLabelPlacement::Footer,
        ScreenLabelPlacement::Hud,
        ScreenLabelPlacement::Legend,
        ScreenLabelPlacement::Callout,
    ] {
        for (label, laidout, size) in prepared.iter().filter(|(l, _, _)| l.placement == placement) {
            let desired = match placement {
                ScreenLabelPlacement::Footer => dvec2(safe.pos.x, footer_y - size.y),
                ScreenLabelPlacement::Hud | ScreenLabelPlacement::Legend =>
                    dvec2(safe.pos.x, flow_y),
                _ => dvec2(label.position.x, label.position.y),
            };
            let Some(bounds) = place_analysis_annotation(safe, desired, *size, &occupied) else {
                overflow = true;
                continue;
            };
            draw.color = vec4(label.color[0], label.color[1], label.color[2], label.color[3]);
            draw.draw_walk_laidout(cx, Walk {
                abs_pos: Some(canvas.pos + bounds.pos),
                width: Size::Fixed(bounds.size.x),
                height: Size::Fixed(bounds.size.y),
                ..Default::default()
            }, laidout);
            occupied.push(bounds);
            match placement {
                ScreenLabelPlacement::Footer => footer_y = bounds.pos.y - ANNOTATION_GAP,
                ScreenLabelPlacement::Hud | ScreenLabelPlacement::Legend =>
                    flow_y = bounds.pos.y + bounds.size.y + ANNOTATION_GAP,
                _ => {}
            }
        }
    }
    if overflow && !notice.rows.is_empty() {
        draw.color = vec4(1.0, 0.75, 0.35, 1.0);
        draw.draw_walk_laidout(cx, Walk {
            abs_pos: Some(canvas.pos + notice_rect.pos),
            width: Size::Fixed(notice_rect.size.x),
            height: Size::Fixed(notice_rect.size.y),
            ..Default::default()
        }, &notice);
    }
    draw.max_lines = old_max_lines;
    draw.text_overflow = old_overflow;
}

#[cfg(test)]
mod annotation_layout_tests {
    use super::*;

    #[test]
    fn complete_measured_callout_avoids_hud_and_stays_inside_viewport() {
        let safe = Rect { pos: dvec2(12.0, 12.0), size: dvec2(176.0, 136.0) };
        let occupied = [
            Rect { pos: dvec2(12.0, 12.0), size: dvec2(176.0, 40.0) },
            Rect { pos: dvec2(12.0, 56.0), size: dvec2(176.0, 20.0) },
        ];
        let placed = place_analysis_annotation(
            safe, dvec2(185.0, 20.0), dvec2(90.0, 24.0), &occupied,
        ).expect("a complete measured callout has room below the legend");
        assert!(placed.pos.x >= safe.pos.x && placed.pos.y >= safe.pos.y);
        assert!(placed.pos.x + placed.size.x <= safe.pos.x + safe.size.x);
        assert!(placed.pos.y + placed.size.y <= safe.pos.y + safe.size.y);
        assert!(occupied.iter().all(|r| !annotation_rects_overlap(placed, *r)));
    }

    #[test]
    fn oversized_complete_annotation_is_not_silently_truncated_to_fit() {
        let safe = Rect { pos: dvec2(12.0, 12.0), size: dvec2(176.0, 136.0) };
        assert!(place_analysis_annotation(
            safe, safe.pos, dvec2(90.0, 300.0), &[],
        ).is_none()); // The caller shows the explicit Results/Report overflow notice.
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawAnalysisTriangle {
    #[deref] draw_super: DrawQuad,
    #[live] tri_a: Vec2f,
    #[live] tri_b: Vec2f,
    #[live] tri_c: Vec2f,
    #[live] fill_color: Vec4f,
}

#[derive(Clone, Debug, Default)]
pub struct CanvasFrame {
    pub snapshot_token: String,
    pub source_generation: u64,
    pub model_generation: u64,
    pub document_revision: u64,
    pub camera_revision: u64,
    pub camera_change: Option<CanvasCameraChange>,
    /// CPU time spent by the app preparing this immutable frame before `set_frame`.
    pub cpu_frame_prepare_ms: Option<f64>,
    /// Immutable screen-space model geometry. Keep this Arc stable while only overlays change.
    pub segments: Arc<Vec<ScreenSegment>>,
    /// Selected and hovered geometry, kept apart from the retained model draw list.
    pub highlights: Arc<Vec<ScreenSegment>>,
    pub preview: Vec<ScreenSegment>,
    pub crosshair: Option<Point2>,
    pub analysis: Arc<AnalysisCanvasLayer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Middle,
    Right,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    Down,
    Move,
    Up,
    Hover,
    Leave,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasCameraChange {
    Pan,
    Zoom,
    Fit,
    Resize,
    Other,
}

impl CanvasCameraChange {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pan => "pan",
            Self::Zoom => "zoom",
            Self::Fit => "fit",
            Self::Resize => "resize",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Debug)]
pub enum CanvasInput {
    Pointer {
        phase: PointerPhase,
        button: PointerButton,
        screen: Point2,
    },
    Scroll {
        screen: Point2,
        delta_y: f64,
    },
    Cancel,
    Confirm,
    Delete,
}

#[derive(Clone, Debug, Default)]
pub enum CanvasAction {
    #[default]
    None,
    Input(CanvasInput),
    Resize {
        width: f64,
        height: f64,
        dpi_factor: f64,
    },
}

#[derive(Clone, Debug, Default)]
pub struct DrawSample {
    pub frame_id: u64,
    pub snapshot_token: String,
    pub source_generation: u64,
    pub model_generation: u64,
    pub document_revision: u64,
    pub camera_revision: u64,
    pub camera_change: Option<CanvasCameraChange>,
    pub cpu_frame_prepare_ms: Option<f64>,
    /// CPU-side conversion of screen segments into Makepad draw descriptors.
    pub cpu_instance_prepare_ms: f64,
    /// CPU time in the Makepad draw batching calls; excludes GPU execution and completion.
    pub cpu_submit_ms: f64,
    /// Start-to-start wall time between canvas draw submissions. This is not GPU time.
    pub interdraw_interval_ms: Option<f64>,
    pub model_segments: usize,
    pub highlight_segments: usize,
    pub preview_segments: usize,
    pub submitted_model_segments: usize,
    pub submitted_overlay_segments: usize,
    pub retained_model_segments: usize,
    pub retained_overlay_segments: usize,
    pub submitted_segments: usize,
    pub custom_instance_payload_bytes: usize,
}

#[derive(Clone, Debug, Default)]
pub struct CanvasTelemetry {
    pub draw_count: u64,
    pub model_segments: usize,
    pub highlight_segments: usize,
    pub preview_segments: usize,
    pub submitted_model_segments: usize,
    pub submitted_overlay_segments: usize,
    pub retained_model_segments: usize,
    pub retained_overlay_segments: usize,
    pub submitted_segments: usize,
    pub instance_payload_bytes: usize,
    pub cpu_frame_prepare_ms: Option<f64>,
    pub cpu_instance_prepare_ms: f64,
    pub cpu_submit_ms: f64,
    pub interdraw_interval_ms: Option<f64>,
    pub snapshot_token: String,
    pub source_generation: u64,
    pub model_generation: u64,
    pub document_revision: u64,
    pub camera_revision: u64,
    pub camera_change: Option<CanvasCameraChange>,
    pub samples: VecDeque<DrawSample>,
    /// Analysis counts are separate from legacy drafting segment/payload counters.
    pub analysis_triangles: usize,
    pub analysis_segments: usize,
    pub analysis_labels: usize,
    pub analysis_batch_redrawn: bool,
}

#[derive(Clone, Copy, Debug)]
struct PreparedSegment {
    rect: Rect,
    edge_start: Vec2f,
    edge_end: Vec2f,
    line_color: Vec4f,
    line_width: f32,
}

fn pointer_button_from_device(device: &DigitDevice) -> PointerButton {
    device.mouse_button().map_or(PointerButton::Left, |button| {
        if button.is_middle() {
            PointerButton::Middle
        } else if button.is_primary() {
            PointerButton::Left
        } else {
            PointerButton::Right
        }
    })
}

fn local_point(rect_origin: Vec2d, absolute: Vec2d) -> Point2 {
    Point2 {
        x: absolute.x - rect_origin.x,
        y: absolute.y - rect_origin.y,
    }
}

fn rect_contains_absolute(rect: Rect, absolute: Vec2d) -> bool {
    rect.contains(absolute)
}

fn prepare_segment(canvas_origin: Vec2d, segment: &ScreenSegment) -> Option<PreparedSegment> {
    if !segment.start.x.is_finite()
        || !segment.start.y.is_finite()
        || !segment.end.x.is_finite()
        || !segment.end.y.is_finite()
    {
        return None;
    }
    let margin = segment.width.max(0.5) as f64 * 0.5 + 2.0;
    let left = segment.start.x.min(segment.end.x) - margin;
    let top = segment.start.y.min(segment.end.y) - margin;
    Some(PreparedSegment {
        rect: Rect {
            pos: dvec2(canvas_origin.x + left, canvas_origin.y + top),
            size: dvec2(
                (segment.end.x - segment.start.x).abs() + margin * 2.0,
                (segment.end.y - segment.start.y).abs() + margin * 2.0,
            ),
        },
        edge_start: vec2(
            (segment.start.x - left) as f32,
            (segment.start.y - top) as f32,
        ),
        edge_end: vec2((segment.end.x - left) as f32, (segment.end.y - top) as f32),
        line_color: vec4(
            segment.color[0],
            segment.color[1],
            segment.color[2],
            segment.color[3],
        ),
        line_width: segment.width,
    })
}

fn prepare_segments(canvas_origin: Vec2d, segments: &[ScreenSegment]) -> Vec<PreparedSegment> {
    let mut prepared = Vec::with_capacity(segments.len());
    prepared.extend(
        segments
            .iter()
            .filter_map(|segment| prepare_segment(canvas_origin, segment)),
    );
    prepared
}

fn prepare_overlay_segments(frame: &CanvasFrame, canvas_origin: Vec2d) -> Vec<PreparedSegment> {
    let mut prepared = Vec::with_capacity(
        frame.highlights.len() + frame.preview.len() + usize::from(frame.crosshair.is_some()) * 2,
    );
    prepared.extend(
        frame
            .highlights
            .iter()
            .chain(frame.preview.iter())
            .filter_map(|segment| prepare_segment(canvas_origin, segment)),
    );
    if let Some(point) = frame.crosshair {
        let color = [0.48, 0.58, 0.68, 0.8];
        let crosshair = [
            ScreenSegment {
                start: Point2::new(point.x - 7.0, point.y),
                end: Point2::new(point.x + 7.0, point.y),
                color,
                width: 1.0,
            },
            ScreenSegment {
                start: Point2::new(point.x, point.y - 7.0),
                end: Point2::new(point.x, point.y + 7.0),
                color,
                width: 1.0,
            },
        ];
        prepared.extend(
            crosshair
                .iter()
                .filter_map(|segment| prepare_segment(canvas_origin, segment)),
        );
    }
    prepared
}

#[cfg(test)]
fn prepare_frame_segments(frame: &CanvasFrame, canvas_origin: Vec2d) -> Vec<PreparedSegment> {
    let mut prepared = prepare_segments(canvas_origin, frame.segments.as_slice());
    prepared.extend(prepare_overlay_segments(frame, canvas_origin));
    prepared
}

fn frame_visuals_match(left: &CanvasFrame, right: &CanvasFrame) -> bool {
    Arc::ptr_eq(&left.segments, &right.segments)
        && analysis_layers_match(&left.analysis, &right.analysis)
        && Arc::ptr_eq(&left.highlights, &right.highlights)
        && left.preview == right.preview
        && left.crosshair == right.crosshair
}

fn frame_requires_canvas_redraw(left: &CanvasFrame, right: &CanvasFrame) -> bool {
    !frame_visuals_match(left, right)
        || left.snapshot_token != right.snapshot_token
        || left.document_revision != right.document_revision
        || left.camera_revision != right.camera_revision
        || left.source_generation != right.source_generation
        || left.model_generation != right.model_generation
}

#[derive(Clone, Debug)]
struct ModelDrawCache {
    segments: Arc<Vec<ScreenSegment>>,
    rect: Rect,
    dpi_factor: f64,
    prepared: Arc<Vec<PreparedSegment>>,
}

fn model_cache_matches(
    cache: Option<&ModelDrawCache>,
    segments: &Arc<Vec<ScreenSegment>>,
    rect: Rect,
    dpi_factor: f64,
) -> bool {
    cache.map_or(false, |cache| {
        Arc::ptr_eq(&cache.segments, segments)
            && cache.rect.pos == rect.pos
            && cache.rect.size == rect.size
            && (cache.dpi_factor - dpi_factor).abs() <= 1e-8
    })
}

#[derive(Clone, Debug, Default)]
struct OverlayDrawCache {
    highlights: Arc<Vec<ScreenSegment>>,
    preview: Vec<ScreenSegment>,
    crosshair: Option<Point2>,
    prepared: Arc<Vec<PreparedSegment>>,
}

fn overlay_cache_matches(cache: &OverlayDrawCache, frame: &CanvasFrame) -> bool {
    Arc::ptr_eq(&cache.highlights, &frame.highlights)
        && cache.preview == frame.preview
        && cache.crosshair == frame.crosshair
}

fn descriptors_for_redraw<T>(
    newly_prepared: Option<&Arc<Vec<T>>>,
    retained: Option<&Arc<Vec<T>>>,
) -> Option<Arc<Vec<T>>> {
    newly_prepared.or(retained).cloned()
}

fn custom_instance_payload_bytes(submitted_segments: usize) -> usize {
    // edge_start (2), edge_end (2), line_color (4), line_width (1).
    submitted_segments.saturating_mul(9 * std::mem::size_of::<f32>())
}

fn draw_sample_for_frame(
    frame_id: u64,
    frame: &CanvasFrame,
    cpu_instance_prepare_ms: f64,
    cpu_submit_ms: f64,
    interdraw_interval_ms: Option<f64>,
    submitted_model_segments: usize,
    submitted_overlay_segments: usize,
    retained_model_segments: usize,
    retained_overlay_segments: usize,
) -> DrawSample {
    let submitted_segments = submitted_model_segments + submitted_overlay_segments;
    DrawSample {
        frame_id,
        snapshot_token: frame.snapshot_token.clone(),
        source_generation: frame.source_generation,
        model_generation: frame.model_generation,
        document_revision: frame.document_revision,
        camera_revision: frame.camera_revision,
        camera_change: frame.camera_change,
        cpu_frame_prepare_ms: frame.cpu_frame_prepare_ms,
        cpu_instance_prepare_ms,
        cpu_submit_ms,
        interdraw_interval_ms,
        model_segments: frame.segments.len(),
        highlight_segments: frame.highlights.len(),
        preview_segments: frame.preview.len(),
        submitted_model_segments,
        submitted_overlay_segments,
        retained_model_segments,
        retained_overlay_segments,
        submitted_segments,
        custom_instance_payload_bytes: custom_instance_payload_bytes(submitted_segments),
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct DraftingCanvas {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    #[redraw]
    draw_bg: DrawColor,
    #[live]
    draw_model: DrawDraftSegment,
    #[live]
    draw_overlay: DrawDraftSegment,
    #[live]
    draw_analysis: DrawDraftSegment,
    #[live]
    draw_analysis_triangle: DrawAnalysisTriangle,
    #[live]
    draw_analysis_text: DrawText,
    #[rust]
    area: Area,
    #[rust]
    model_draw_list: Option<DrawList2d>,
    #[rust]
    overlay_draw_list: Option<DrawList2d>,
    #[rust]
    analysis_draw_list: Option<DrawList2d>,
    #[rust]
    analysis_draw_cache: Option<(Arc<AnalysisCanvasLayer>, Rect, f64)>,
    #[rust]
    model_draw_cache: Option<ModelDrawCache>,
    #[rust]
    overlay_draw_cache: OverlayDrawCache,
    #[rust]
    pub rect: Rect,
    #[rust]
    pub dpi_factor: f64,
    #[rust]
    pub frame: CanvasFrame,
    #[rust]
    pub telemetry: CanvasTelemetry,
    #[rust]
    last_pointer_abs: Option<Vec2d>,
    #[rust]
    last_draw_started: Option<Instant>,
}

impl DraftingCanvas {
    pub fn set_frame(&mut self, cx: &mut Cx, frame: CanvasFrame) {
        let frame_changed = frame_requires_canvas_redraw(&self.frame, &frame);
        self.frame = frame;
        if frame_changed {
            // Resize actions are drained before Draw ends; ordinary redraw
            // would discard the request for the newly prepared frame.
            cx.redraw_area_in_draw(self.area);
        }
    }

    /// Areas for the retained model and dynamic overlay batches. These are the actual
    /// DrawDraftSegment instance areas used by Makepad, not the Canvas rectangle area.
    pub fn native_draw_areas(&self) -> (Area, Area) {
        (
            self.draw_model.draw_vars.area,
            self.draw_overlay.draw_vars.area,
        )
    }

    /// The Canvas rectangle area, which identifies its enclosing native pass/viewport.
    pub fn native_pass_area(&self) -> Area {
        self.area
    }

    fn local(&self, absolute: Vec2d) -> Point2 {
        local_point(self.rect.pos, absolute)
    }

    fn emit(&self, cx: &mut Cx, input: CanvasInput) {
        cx.widget_action(self.uid, CanvasAction::Input(input));
    }
}

fn draw_prepared_segment(
    draw_segment: &mut DrawDraftSegment,
    cx: &mut Cx2d,
    segment: PreparedSegment,
) {
    draw_segment.edge_start = segment.edge_start;
    draw_segment.edge_end = segment.edge_end;
    draw_segment.line_color = segment.line_color;
    draw_segment.line_width = segment.line_width;
    draw_segment.draw_abs(cx, segment.rect);
}

impl Widget for DraftingCanvas {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(event) => {
                cx.set_key_focus(self.area);
                self.last_pointer_abs = Some(event.abs);
                self.emit(
                    cx,
                    CanvasInput::Pointer {
                        phase: PointerPhase::Down,
                        button: pointer_button_from_device(&event.device),
                        screen: self.local(event.abs),
                    },
                );
            }
            Hit::FingerMove(event) => {
                self.last_pointer_abs = Some(event.abs);
                self.emit(
                    cx,
                    CanvasInput::Pointer {
                        phase: PointerPhase::Move,
                        button: pointer_button_from_device(&event.device),
                        screen: self.local(event.abs),
                    },
                );
            }
            Hit::FingerUp(event) => {
                self.last_pointer_abs = Some(event.abs);
                self.emit(
                    cx,
                    CanvasInput::Pointer {
                        phase: PointerPhase::Up,
                        button: pointer_button_from_device(&event.device),
                        screen: self.local(event.abs),
                    },
                );
            }
            Hit::FingerHoverIn(event) | Hit::FingerHoverOver(event) => {
                cx.set_cursor(MouseCursor::Crosshair);
                self.last_pointer_abs = Some(event.abs);
                self.emit(
                    cx,
                    CanvasInput::Pointer {
                        phase: PointerPhase::Hover,
                        button: PointerButton::Left,
                        screen: self.local(event.abs),
                    },
                );
            }
            Hit::FingerHoverOut(event) => {
                self.last_pointer_abs = None;
                self.emit(
                    cx,
                    CanvasInput::Pointer {
                        phase: PointerPhase::Leave,
                        button: PointerButton::Left,
                        screen: self.local(event.abs),
                    },
                );
            }
            Hit::FingerScroll(event) => {
                self.last_pointer_abs = Some(event.abs);
                self.emit(
                    cx,
                    CanvasInput::Scroll {
                        screen: self.local(event.abs),
                        delta_y: if event.scroll.y.abs() > f64::EPSILON {
                            event.scroll.y
                        } else {
                            event.scroll.x
                        },
                    },
                );
            }
            Hit::KeyDown(event) => match event.key_code {
                KeyCode::Escape => self.emit(cx, CanvasInput::Cancel),
                KeyCode::ReturnKey => self.emit(cx, CanvasInput::Confirm),
                KeyCode::Delete | KeyCode::Backspace => self.emit(cx, CanvasInput::Delete),
                _ => {}
            },
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let draw_started = Instant::now();
        let interdraw_interval_ms = self
            .last_draw_started
            .replace(draw_started)
            .map(|last| draw_started.duration_since(last).as_secs_f64() * 1000.0);
        cx.begin_turtle(walk, self.layout.with_clip(true, true));
        let rect = cx.turtle().rect();
        let dpi_factor = cx.current_dpi_factor();
        let resized = self.rect.pos != rect.pos
            || self.rect.size != rect.size
            || (self.dpi_factor - dpi_factor).abs() > 1e-8;
        self.rect = rect;
        self.dpi_factor = dpi_factor;
        self.draw_bg.draw_abs(cx, rect);

        if self.model_draw_list.is_none() {
            self.model_draw_list = Some(DrawList2d::new(cx));
        }
        if self.overlay_draw_list.is_none() {
            self.overlay_draw_list = Some(DrawList2d::new(cx));
        }
        if self.analysis_draw_list.is_none() {
            self.analysis_draw_list = Some(DrawList2d::new(cx));
        }
        let analysis_needs_redraw = resized || self.analysis_draw_cache.as_ref().is_none_or(|(layer,old_rect,old_dpi)| {
            !analysis_layers_match(layer,&self.frame.analysis) || *old_rect!=rect || (*old_dpi-dpi_factor).abs()>1e-8
        });
        let mut analysis_batch_redrawn = false;
        {
            let draw_list = self.analysis_draw_list.as_mut().unwrap();
            if !draw_list.begin_maybe(cx,analysis_needs_redraw).is_not_redrawing() {
                for triangle in &self.frame.analysis.triangles {
                    let min_x=triangle.vertices.iter().map(|p|p.x).fold(f64::INFINITY,f64::min);
                    let min_y=triangle.vertices.iter().map(|p|p.y).fold(f64::INFINITY,f64::min);
                    let max_x=triangle.vertices.iter().map(|p|p.x).fold(f64::NEG_INFINITY,f64::max);
                    let max_y=triangle.vertices.iter().map(|p|p.y).fold(f64::NEG_INFINITY,f64::max);
                    if ![min_x,min_y,max_x,max_y].iter().all(|v|v.is_finite()) || max_x-min_x<1e-6 || max_y-min_y<1e-6 { continue; }
                    let local = |p:Point2| vec2((p.x-min_x) as f32,(p.y-min_y) as f32);
                    self.draw_analysis_triangle.tri_a=local(triangle.vertices[0]);
                    self.draw_analysis_triangle.tri_b=local(triangle.vertices[1]);
                    self.draw_analysis_triangle.tri_c=local(triangle.vertices[2]);
                    self.draw_analysis_triangle.fill_color=vec4(triangle.color[0],triangle.color[1],triangle.color[2],triangle.color[3]);
                    self.draw_analysis_triangle.draw_abs(cx,Rect{pos:rect.pos+dvec2(min_x,min_y),size:dvec2(max_x-min_x,max_y-min_y)});
                }
                let segments=prepare_segments(rect.pos,&self.frame.analysis.segments);
                if !segments.is_empty() {
                    self.draw_analysis.begin_many_instances(cx);
                    for segment in segments { draw_prepared_segment(&mut self.draw_analysis,cx,segment); }
                    self.draw_analysis.end_many_instances(cx);
                }
                draw_analysis_labels(&mut self.draw_analysis_text, cx, rect, &self.frame.analysis.labels);
                draw_list.end(cx);
                self.analysis_draw_cache=Some((Arc::clone(&self.frame.analysis),rect,dpi_factor));
                analysis_batch_redrawn=true;
            }
        }

        let model_needs_redraw = resized
            || !model_cache_matches(
                self.model_draw_cache.as_ref(),
                &self.frame.segments,
                rect,
                dpi_factor,
            );
        let overlay_needs_redraw =
            resized || !overlay_cache_matches(&self.overlay_draw_cache, &self.frame);

        let instance_prepare_started = Instant::now();
        let prepared_model = model_needs_redraw
            .then(|| Arc::new(prepare_segments(rect.pos, self.frame.segments.as_slice())));
        let prepared_overlay =
            overlay_needs_redraw.then(|| Arc::new(prepare_overlay_segments(&self.frame, rect.pos)));
        let mut cpu_instance_prepare_ms = if model_needs_redraw || overlay_needs_redraw {
            instance_prepare_started.elapsed().as_secs_f64() * 1000.0
        } else {
            0.0
        };
        let mut forced_descriptor_prepare_ms = 0.0;
        let cached_model_prepared = self
            .model_draw_cache
            .as_ref()
            .map(|cache| Arc::clone(&cache.prepared));
        let cached_overlay_prepared = Arc::clone(&self.overlay_draw_cache.prepared);

        let submit_started = Instant::now();
        let mut submitted_model_segments = 0;
        let mut retained_model_segments = self
            .model_draw_cache
            .as_ref()
            .map_or(0, |cache| cache.prepared.len());
        {
            let draw_list = self.model_draw_list.as_mut().unwrap();
            let redrawing = draw_list.begin_maybe(cx, model_needs_redraw);
            if !redrawing.is_not_redrawing() {
                let prepared =
                    descriptors_for_redraw(prepared_model.as_ref(), cached_model_prepared.as_ref())
                        .unwrap_or_else(|| {
                            let prepare_started = Instant::now();
                            let prepared = Arc::new(prepare_segments(
                                rect.pos,
                                self.frame.segments.as_slice(),
                            ));
                            let elapsed_ms = prepare_started.elapsed().as_secs_f64() * 1000.0;
                            cpu_instance_prepare_ms += elapsed_ms;
                            forced_descriptor_prepare_ms += elapsed_ms;
                            prepared
                        });
                if !prepared.is_empty() {
                    self.draw_model.begin_many_instances(cx);
                    for segment in prepared.iter().copied() {
                        draw_prepared_segment(&mut self.draw_model, cx, segment);
                    }
                    self.draw_model.end_many_instances(cx);
                } else {
                    self.draw_model.draw_vars.area = Area::default();
                }
                draw_list.end(cx);
                submitted_model_segments = prepared.len();
                retained_model_segments = 0;
                self.model_draw_cache = Some(ModelDrawCache {
                    segments: Arc::clone(&self.frame.segments),
                    rect,
                    dpi_factor,
                    prepared,
                });
            }
        }

        let mut submitted_overlay_segments = 0;
        let mut retained_overlay_segments = self.overlay_draw_cache.prepared.len();
        {
            let draw_list = self.overlay_draw_list.as_mut().unwrap();
            let redrawing = draw_list.begin_maybe(cx, overlay_needs_redraw);
            if !redrawing.is_not_redrawing() {
                let prepared = descriptors_for_redraw(
                    prepared_overlay.as_ref(),
                    Some(&cached_overlay_prepared),
                )
                .unwrap_or_else(|| {
                    let prepare_started = Instant::now();
                    let prepared = Arc::new(prepare_overlay_segments(&self.frame, rect.pos));
                    let elapsed_ms = prepare_started.elapsed().as_secs_f64() * 1000.0;
                    cpu_instance_prepare_ms += elapsed_ms;
                    forced_descriptor_prepare_ms += elapsed_ms;
                    prepared
                });
                if !prepared.is_empty() {
                    self.draw_overlay.begin_many_instances(cx);
                    for segment in prepared.iter().copied() {
                        draw_prepared_segment(&mut self.draw_overlay, cx, segment);
                    }
                    self.draw_overlay.end_many_instances(cx);
                } else {
                    self.draw_overlay.draw_vars.area = Area::default();
                }
                draw_list.end(cx);
                submitted_overlay_segments = prepared.len();
                retained_overlay_segments = 0;
                self.overlay_draw_cache = OverlayDrawCache {
                    highlights: Arc::clone(&self.frame.highlights),
                    preview: self.frame.preview.clone(),
                    crosshair: self.frame.crosshair,
                    prepared,
                };
            }
        }

        let cpu_submit_ms = (submit_started.elapsed().as_secs_f64() * 1000.0
            - forced_descriptor_prepare_ms)
            .max(0.0);
        cx.end_turtle_with_area(&mut self.area);
        self.telemetry.draw_count = self.telemetry.draw_count.saturating_add(1);
        self.telemetry.analysis_triangles=self.frame.analysis.triangles.len();
        self.telemetry.analysis_segments=self.frame.analysis.segments.len();
        self.telemetry.analysis_labels=self.frame.analysis.labels.len();
        self.telemetry.analysis_batch_redrawn=analysis_batch_redrawn;
        self.telemetry.model_segments = self.frame.segments.len();
        self.telemetry.highlight_segments = self.frame.highlights.len();
        self.telemetry.preview_segments = self.frame.preview.len();
        self.telemetry.submitted_model_segments = submitted_model_segments;
        self.telemetry.submitted_overlay_segments = submitted_overlay_segments;
        self.telemetry.retained_model_segments = retained_model_segments;
        self.telemetry.retained_overlay_segments = retained_overlay_segments;
        self.telemetry.submitted_segments =
            submitted_model_segments.saturating_add(submitted_overlay_segments);
        // Excludes Makepad's base instance fields, staging allocations and driver/GPU time.
        self.telemetry.instance_payload_bytes =
            custom_instance_payload_bytes(self.telemetry.submitted_segments);
        self.telemetry.cpu_frame_prepare_ms = self.frame.cpu_frame_prepare_ms;
        self.telemetry.cpu_instance_prepare_ms = cpu_instance_prepare_ms;
        self.telemetry.cpu_submit_ms = cpu_submit_ms;
        self.telemetry.interdraw_interval_ms = interdraw_interval_ms;
        self.telemetry.snapshot_token = self.frame.snapshot_token.clone();
        self.telemetry.source_generation = self.frame.source_generation;
        self.telemetry.model_generation = self.frame.model_generation;
        self.telemetry.document_revision = self.frame.document_revision;
        self.telemetry.camera_revision = self.frame.camera_revision;
        self.telemetry.camera_change = self.frame.camera_change;
        self.telemetry.samples.push_back(draw_sample_for_frame(
            self.telemetry.draw_count,
            &self.frame,
            cpu_instance_prepare_ms,
            cpu_submit_ms,
            interdraw_interval_ms,
            submitted_model_segments,
            submitted_overlay_segments,
            retained_model_segments,
            retained_overlay_segments,
        ));
        if self.telemetry.samples.len() > 128 {
            self.telemetry.samples.pop_front();
        }
        if resized {
            cx.widget_action(
                self.uid,
                CanvasAction::Resize {
                    width: rect.size.x,
                    height: rect.size.y,
                    dpi_factor,
                },
            );
            if let Some(absolute) = self.last_pointer_abs {
                if rect_contains_absolute(rect, absolute) {
                    self.emit(
                        cx,
                        CanvasInput::Pointer {
                            phase: PointerPhase::Hover,
                            button: PointerButton::Left,
                            screen: self.local(absolute),
                        },
                    );
                } else {
                    self.last_pointer_abs = None;
                    self.emit(
                        cx,
                        CanvasInput::Pointer {
                            phase: PointerPhase::Leave,
                            button: PointerButton::Left,
                            screen: self.local(absolute),
                        },
                    );
                }
            }
        }
        DrawStep::done()
    }
}

impl DraftingCanvasRef {
    pub fn actions(&self, actions: &Actions) -> Vec<CanvasAction> {
        actions
            .filter_widget_actions(self.widget_uid())
            .map(|action| action.cast::<CanvasAction>())
            .filter(|action| !matches!(action, CanvasAction::None))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_button_mapping_keeps_secondary_through_down_and_up_conversion() {
        assert_eq!(
            pointer_button_from_device(&DigitDevice::Mouse {
                button: MouseButton::SECONDARY,
            }),
            PointerButton::Right
        );
        assert_eq!(
            pointer_button_from_device(&DigitDevice::Mouse {
                button: MouseButton::MIDDLE,
            }),
            PointerButton::Middle
        );
        assert_eq!(
            pointer_button_from_device(&DigitDevice::Mouse {
                button: MouseButton::PRIMARY,
            }),
            PointerButton::Left
        );
        assert_eq!(
            pointer_button_from_device(&DigitDevice::Touch { uid: 4 }),
            PointerButton::Left
        );
    }

    #[test]
    fn absolute_pointer_coordinates_translate_when_canvas_origin_moves() {
        let first_origin = dvec2(140.0, 70.0);
        let first_absolute = dvec2(175.0, 111.0);
        let second_origin = dvec2(92.0, 215.0);
        let second_absolute = dvec2(127.0, 256.0);
        let first = local_point(first_origin, first_absolute);
        let second = local_point(second_origin, second_absolute);
        assert_eq!((first.x, first.y), (35.0, 41.0));
        assert_eq!((second.x, second.y), (first.x, first.y));
    }

    #[test]
    fn resize_hit_test_uses_the_new_canvas_rectangle() {
        let shifted = Rect {
            pos: dvec2(120.0, 40.0),
            size: dvec2(300.0, 200.0),
        };
        assert!(rect_contains_absolute(shifted, dvec2(120.0, 40.0)));
        assert!(rect_contains_absolute(shifted, dvec2(420.0, 240.0)));
        assert!(!rect_contains_absolute(shifted, dvec2(421.0, 240.0)));
        assert!(!rect_contains_absolute(shifted, dvec2(119.0, 100.0)));
    }

    #[test]
    fn frame_preparation_omits_non_finite_segments_and_keeps_preview_and_crosshair() {
        let frame = CanvasFrame {
            segments: Arc::new(vec![
                ScreenSegment {
                    start: Point2::new(4.0, 6.0),
                    end: Point2::new(24.0, 6.0),
                    color: [0.2, 0.4, 0.6, 1.0],
                    width: 2.0,
                },
                ScreenSegment {
                    start: Point2::new(f64::NAN, 6.0),
                    end: Point2::new(24.0, 6.0),
                    color: [0.2, 0.4, 0.6, 1.0],
                    width: 2.0,
                },
            ]),
            preview: vec![ScreenSegment {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(0.0, 10.0),
                color: [1.0, 0.6, 0.2, 1.0],
                width: 2.0,
            }],
            crosshair: Some(Point2::new(30.0, 40.0)),
            ..Default::default()
        };
        let prepared = prepare_frame_segments(&frame, dvec2(100.0, 50.0));
        assert_eq!(prepared.len(), 4);
        assert_eq!(prepared[0].rect.pos, dvec2(101.0, 53.0));
        assert_eq!(prepared[1].rect.pos, dvec2(97.0, 47.0));
    }

    #[test]
    fn frame_preparation_orders_highlights_before_preview_and_crosshair() {
        let frame = CanvasFrame {
            segments: Arc::new(vec![ScreenSegment {
                start: Point2::new(4.0, 6.0),
                end: Point2::new(24.0, 6.0),
                color: [0.2, 0.4, 0.6, 1.0],
                width: 2.0,
            }]),
            highlights: Arc::new(vec![ScreenSegment {
                start: Point2::new(12.0, 14.0),
                end: Point2::new(20.0, 14.0),
                color: [0.9, 0.8, 0.2, 1.0],
                width: 2.0,
            }]),
            preview: vec![ScreenSegment {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(0.0, 10.0),
                color: [1.0, 0.6, 0.2, 1.0],
                width: 2.0,
            }],
            crosshair: Some(Point2::new(30.0, 40.0)),
            ..Default::default()
        };

        let prepared = prepare_frame_segments(&frame, dvec2(100.0, 50.0));

        assert_eq!(prepared.len(), 5);
        assert_eq!(prepared[0].rect.pos, dvec2(101.0, 53.0));
        assert_eq!(prepared[1].rect.pos, dvec2(109.0, 61.0));
        assert_eq!(prepared[2].rect.pos, dvec2(97.0, 47.0));
        assert_eq!(prepared[3].rect.pos, dvec2(120.5, 87.5));
        assert_eq!(prepared[4].rect.pos, dvec2(127.5, 80.5));
    }

    #[test]
    fn draw_sample_keeps_immutable_frame_provenance_and_camera_change() {
        let frame = CanvasFrame {
            snapshot_token: "drawing-2:r19:c8:g3:hash".into(),
            source_generation: 4,
            model_generation: 3,
            document_revision: 19,
            camera_revision: 8,
            camera_change: Some(CanvasCameraChange::Pan),
            cpu_frame_prepare_ms: Some(0.75),
            segments: Arc::new(vec![
                ScreenSegment {
                    start: Point2::new(0.0, 0.0),
                    end: Point2::new(10.0, 0.0),
                    color: [0.2, 0.4, 0.6, 1.0],
                    width: 1.0,
                };
                5
            ]),
            highlights: Arc::new(vec![ScreenSegment {
                start: Point2::new(0.0, 1.0),
                end: Point2::new(10.0, 1.0),
                color: [1.0, 0.4, 0.2, 1.0],
                width: 1.0,
            }]),
            ..Default::default()
        };
        let sample = draw_sample_for_frame(27, &frame, 0.2, 0.3, Some(16.7), 1, 1, 4, 0);
        assert_eq!(sample.frame_id, 27);
        assert_eq!(sample.snapshot_token, frame.snapshot_token);
        assert_eq!(sample.source_generation, 4);
        assert_eq!(sample.model_generation, 3);
        assert_eq!(sample.document_revision, 19);
        assert_eq!(sample.camera_revision, 8);
        assert_eq!(sample.camera_change, Some(CanvasCameraChange::Pan));
        assert_eq!(
            sample.camera_change.map(CanvasCameraChange::name),
            Some("pan")
        );
        assert_eq!(sample.cpu_frame_prepare_ms, Some(0.75));
        assert_eq!(sample.interdraw_interval_ms, Some(16.7));
        assert_eq!(sample.model_segments, 5);
        assert_eq!(sample.highlight_segments, 1);
        assert_eq!(sample.preview_segments, 0);
        assert_eq!(sample.submitted_model_segments, 1);
        assert_eq!(sample.submitted_overlay_segments, 1);
        assert_eq!(sample.retained_model_segments, 4);
        assert_eq!(sample.retained_overlay_segments, 0);
        assert_eq!(sample.submitted_segments, 2);
        assert_eq!(sample.custom_instance_payload_bytes, 72);
    }

    #[test]
    fn locked_only_commit_requests_provenance_redraw_with_cached_lists() {
        // A temporary value fixture exercises the accepted Canvas provenance
        // invariant without linking the standalone Lab server/session into AI Revit.
        let mut document = drafting_core::LabDocument::new();
        let drawing_id = *document.drawings().keys().next().unwrap();
        document
            .apply(drafting_core::CommandEnvelope {
                command_id: "canvas-locked-only-create".into(),
                expected_revision: 0,
                operations: vec![drafting_core::DraftOperation::Create {
                    drawing_id,
                    layer_id: drafting_core::LayerId::new(1),
                    geometry: drafting_core::EntityGeometry::Line {
                        start: Point2::new(1.0, 2.0),
                        end: Point2::new(8.0, 2.0),
                    },
                    style: Default::default(),
                }],
            })
            .unwrap();
        let mut cache = drafting_core::DraftingCache::new(Default::default()).unwrap();
        cache.reset(&document,drawing_id,1).unwrap();
        let camera = drafting_core::Camera2D::default();
        let options = drafting_core::DisplayOptions::default();
        let before = cache.prepare(&document,1,&camera,&options).unwrap();
        let lock_revision = document.revision();
        let receipt = document
            .apply(drafting_core::CommandEnvelope {
                command_id: "canvas-locked-only-lock".into(),
                expected_revision: lock_revision,
                operations: vec![drafting_core::DraftOperation::SetLayer {
                    drawing_id,
                    layer_id: drafting_core::LayerId::new(1),
                    visible: None,
                    locked: Some(true),
                    style: None,
                }],
            })
            .unwrap();
        cache.sync_after_commit(&document,&receipt,1).unwrap();
        let after = cache.prepare(&document,1,&camera,&options).unwrap();

        assert_eq!(receipt.revision_after, receipt.revision_before + 1);
        assert!(after.source.document_revision > before.source.document_revision);
        assert_eq!(after.source.document_revision, receipt.revision_after);
        assert_ne!(before.token, after.token);
        assert!(Arc::ptr_eq(&before.base, &after.base));
        assert!(Arc::ptr_eq(&before.overlay, &after.overlay));

        let segments = Arc::new(vec![ScreenSegment {
            start: Point2::new(1.0, 2.0),
            end: Point2::new(8.0, 2.0),
            color: [0.2, 0.4, 0.6, 1.0],
            width: 1.0,
        }]);
        let highlights = Arc::new(Vec::new());
        let current = CanvasFrame {
            snapshot_token: before.token.clone(),
            source_generation: before.generations.geometry,
            model_generation: before.generations.geometry,
            document_revision: before.source.document_revision,
            camera_revision: before.camera.revision,
            segments: Arc::clone(&segments),
            highlights: Arc::clone(&highlights),
            ..Default::default()
        };
        let unchanged = current.clone();
        let updated = CanvasFrame {
            snapshot_token: after.token.clone(),
            source_generation: after.generations.geometry,
            model_generation: after.generations.geometry,
            document_revision: after.source.document_revision,
            camera_revision: after.camera.revision,
            segments: Arc::clone(&segments),
            highlights: Arc::clone(&highlights),
            ..Default::default()
        };
        let rect = Rect {
            pos: dvec2(10.0, 20.0),
            size: dvec2(300.0, 200.0),
        };
        let model_cache = ModelDrawCache {
            segments: Arc::clone(&current.segments),
            rect,
            dpi_factor: 1.0,
            prepared: Arc::new(prepare_segments(rect.pos, current.segments.as_slice())),
        };
        let overlay_cache = OverlayDrawCache {
            highlights: Arc::clone(&current.highlights),
            preview: current.preview.clone(),
            crosshair: current.crosshair,
            prepared: Arc::new(prepare_overlay_segments(&current, rect.pos)),
        };

        assert!(frame_visuals_match(&current, &updated));
        assert!(!frame_requires_canvas_redraw(&current, &unchanged));
        let classification_only = CanvasFrame {
            camera_change: Some(CanvasCameraChange::Pan),
            ..current.clone()
        };
        assert!(!frame_requires_canvas_redraw(&current, &classification_only));
        assert!(frame_requires_canvas_redraw(&current, &updated));
        assert!(model_cache_matches(
            Some(&model_cache),
            &updated.segments,
            rect,
            1.0
        ));
        assert!(overlay_cache_matches(&overlay_cache, &updated));
        assert_eq!(updated.snapshot_token, after.token);
        assert_eq!(updated.document_revision, after.source.document_revision);
        assert_eq!(updated.camera_revision, after.camera.revision);
        assert_eq!(updated.source_generation, after.generations.geometry);
        assert_eq!(updated.model_generation, after.generations.geometry);

        let prepared_model = prepare_segments(rect.pos, updated.segments.as_slice());
        let sample = draw_sample_for_frame(
            2,
            &updated,
            0.0,
            0.0,
            Some(16.7),
            0,
            0,
            prepared_model.len(),
            0,
        );
        assert_eq!(sample.snapshot_token, after.token);
        assert_eq!(sample.document_revision, after.source.document_revision);
        assert_eq!(sample.source_generation, after.generations.geometry);
        assert_eq!(sample.model_generation, after.generations.geometry);
        assert_eq!(sample.submitted_model_segments, 0);
        assert_eq!(sample.retained_model_segments, prepared_model.len());
    }

    #[test]
    fn model_draw_cache_survives_overlay_changes_but_tracks_arc_and_viewport() {
        let segment = ScreenSegment {
            start: Point2::new(2.0, 3.0),
            end: Point2::new(12.0, 3.0),
            color: [0.2, 0.4, 0.6, 1.0],
            width: 1.0,
        };
        let model = Arc::new(vec![segment.clone()]);
        let rect = Rect {
            pos: dvec2(10.0, 20.0),
            size: dvec2(300.0, 200.0),
        };
        let cache = ModelDrawCache {
            segments: Arc::clone(&model),
            rect,
            dpi_factor: 1.0,
            prepared: Arc::new(prepare_segments(dvec2(0.0, 0.0), &[segment])),
        };
        let frame = CanvasFrame {
            segments: Arc::clone(&model),
            highlights: Arc::new(vec![segment.clone()]),
            ..Default::default()
        };
        let overlay_changed = CanvasFrame {
            highlights: Arc::new(Vec::new()),
            preview: vec![segment.clone()],
            ..frame.clone()
        };

        assert!(model_cache_matches(
            Some(&cache),
            &overlay_changed.segments,
            rect,
            1.0
        ));
        assert!(!frame_visuals_match(&frame, &overlay_changed));
        assert!(!model_cache_matches(
            Some(&cache),
            &Arc::new(vec![segment]),
            rect,
            1.0
        ));
        assert!(!model_cache_matches(
            Some(&cache),
            &model,
            Rect {
                pos: dvec2(11.0, 20.0),
                ..rect
            },
            1.0
        ));
        assert!(!model_cache_matches(
            Some(&cache),
            &model,
            Rect {
                size: dvec2(301.0, 200.0),
                ..rect
            },
            1.0
        ));
        assert!(!model_cache_matches(Some(&cache), &model, rect, 1.5));
    }

    #[test]
    fn overlay_cache_change_does_not_replace_model_arc() {
        let model = Arc::new(vec![ScreenSegment {
            start: Point2::new(0.0, 0.0),
            end: Point2::new(5.0, 0.0),
            color: [0.0, 0.0, 0.0, 1.0],
            width: 1.0,
        }]);
        let first = CanvasFrame {
            segments: Arc::clone(&model),
            ..Default::default()
        };
        let cache = OverlayDrawCache {
            highlights: Arc::clone(&first.highlights),
            preview: first.preview.clone(),
            crosshair: first.crosshair,
            prepared: Arc::new(Vec::new()),
        };
        let overlay_update = CanvasFrame {
            highlights: Arc::new(vec![ScreenSegment {
                start: Point2::new(8.0, 9.0),
                end: Point2::new(16.0, 9.0),
                color: [1.0, 0.5, 0.0, 1.0],
                width: 2.0,
            }]),
            ..first.clone()
        };

        assert!(!overlay_cache_matches(&cache, &overlay_update));
        assert!(Arc::ptr_eq(&first.segments, &overlay_update.segments));
    }

    #[test]
    fn forced_drawlist_redraw_reuses_cached_descriptors() {
        let source = ScreenSegment {
            start: Point2::new(1.0, 2.0),
            end: Point2::new(9.0, 2.0),
            color: [0.2, 0.4, 0.6, 1.0],
            width: 1.0,
        };
        let cached = Arc::new(prepare_segments(dvec2(20.0, 30.0), &[source]));
        let reused = descriptors_for_redraw::<PreparedSegment>(None, Some(&cached)).unwrap();

        assert!(Arc::ptr_eq(&cached, &reused));
        assert_eq!(reused.len(), 1);

        let newly_prepared = Arc::new(Vec::new());
        let chosen = descriptors_for_redraw(Some(&newly_prepared), Some(&cached)).unwrap();
        assert!(Arc::ptr_eq(&newly_prepared, &chosen));
        assert!(chosen.is_empty());
    }
}
