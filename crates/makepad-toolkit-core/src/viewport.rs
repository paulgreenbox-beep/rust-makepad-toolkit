use std::ops::Range;

use crate::geometry::{
    LogicalPoint, LogicalRect, LogicalSize, ScreenPoint, ScreenRect, ScreenSize, ViewportPoint,
};
use crate::pages::{PageStack, PageStackLayout};

pub const MIN_ZOOM: f64 = 0.25;
pub const MAX_ZOOM: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomViewport {
    zoom: f64,
}

impl Default for ZoomViewport {
    fn default() -> Self {
        Self { zoom: 1.0 }
    }
}

impl ZoomViewport {
    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    pub fn zoom_percent(&self) -> u16 {
        (self.zoom * 100.0).round() as u16
    }

    pub fn set_zoom(&mut self, requested: f64) -> bool {
        let next = if requested.is_finite() {
            requested.clamp(MIN_ZOOM, MAX_ZOOM)
        } else {
            self.zoom
        };
        let changed = (next - self.zoom).abs() > f64::EPSILON;
        self.zoom = next;
        changed
    }

    pub fn zoom_in(&mut self) -> bool {
        self.set_zoom(self.zoom * 1.25)
    }

    pub fn zoom_out(&mut self) -> bool {
        self.set_zoom(self.zoom / 1.25)
    }

    pub fn reset(&mut self) -> bool {
        self.set_zoom(1.0)
    }

    pub fn logical_to_viewport(
        &self,
        logical: LogicalPoint,
        scroll: &ScrollViewport,
    ) -> ViewportPoint {
        ViewportPoint::new(
            logical.x * self.zoom - scroll.scroll_x(),
            logical.y * self.zoom - scroll.scroll_y(),
        )
    }

    pub fn viewport_to_logical(
        &self,
        viewport: ViewportPoint,
        scroll: &ScrollViewport,
    ) -> LogicalPoint {
        LogicalPoint::new(
            (viewport.x + scroll.scroll_x()) / self.zoom,
            (viewport.y + scroll.scroll_y()) / self.zoom,
        )
    }

    pub fn logical_to_screen(
        &self,
        logical: LogicalPoint,
        viewport_origin: ScreenPoint,
        scroll: &ScrollViewport,
    ) -> ScreenPoint {
        let point = self.logical_to_viewport(logical, scroll);
        ScreenPoint::new(viewport_origin.x + point.x, viewport_origin.y + point.y)
    }

    pub fn screen_to_logical(
        &self,
        screen: ScreenPoint,
        viewport_origin: ScreenPoint,
        scroll: &ScrollViewport,
    ) -> LogicalPoint {
        self.viewport_to_logical(
            ViewportPoint::new(screen.x - viewport_origin.x, screen.y - viewport_origin.y),
            scroll,
        )
    }

    pub fn logical_rect_to_screen(
        &self,
        logical: LogicalRect,
        viewport_origin: ScreenPoint,
        scroll: &ScrollViewport,
    ) -> ScreenRect {
        let origin = self.logical_to_screen(logical.origin, viewport_origin, scroll);
        ScreenRect::new(
            origin.x,
            origin.y,
            logical.size.width * self.zoom,
            logical.size.height * self.zoom,
        )
    }

    pub fn set_zoom_anchored(
        &mut self,
        requested: f64,
        anchor: ViewportPoint,
        scroll: &mut ScrollViewport,
    ) -> bool {
        let old_zoom = self.zoom;
        let logical_anchor = LogicalPoint::new(
            (scroll.scroll_x() + anchor.x) / old_zoom,
            (scroll.scroll_y() + anchor.y) / old_zoom,
        );
        if !self.set_zoom(requested) {
            return false;
        }
        scroll.set_unclamped(
            logical_anchor.x * self.zoom - anchor.x,
            logical_anchor.y * self.zoom - anchor.y,
        );
        true
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollViewport {
    scroll_x: f64,
    scroll_y: f64,
    content_size: ScreenSize,
    viewport_size: ScreenSize,
}

impl ScrollViewport {
    pub fn scroll_x(&self) -> f64 {
        self.scroll_x
    }

    pub fn scroll_y(&self) -> f64 {
        self.scroll_y
    }

    pub fn content_size(&self) -> ScreenSize {
        self.content_size
    }

    pub fn viewport_size(&self) -> ScreenSize {
        self.viewport_size
    }

    pub fn set_extents(&mut self, content_size: ScreenSize, viewport_size: ScreenSize) {
        self.content_size = content_size.non_negative();
        self.viewport_size = viewport_size.non_negative();
        self.clamp();
    }

    pub fn max_scroll_x(&self) -> f64 {
        (self.content_size.width - self.viewport_size.width).max(0.0)
    }

    pub fn max_scroll_y(&self) -> f64 {
        (self.content_size.height - self.viewport_size.height).max(0.0)
    }

    pub fn set_scroll(&mut self, x: f64, y: f64) {
        self.set_unclamped(x, y);
        self.clamp();
    }

    pub fn scroll_to(&mut self, x: f64, y: f64) {
        self.set_scroll(x, y);
    }

    pub fn scroll_by(&mut self, delta_x: f64, delta_y: f64) {
        self.set_scroll(self.scroll_x + delta_x, self.scroll_y + delta_y);
    }

    pub fn clamp(&mut self) {
        self.scroll_x = finite_or_zero(self.scroll_x).clamp(0.0, self.max_scroll_x());
        self.scroll_y = finite_or_zero(self.scroll_y).clamp(0.0, self.max_scroll_y());
    }

    pub(crate) fn set_unclamped(&mut self, x: f64, y: f64) {
        self.scroll_x = finite_or_zero(x);
        self.scroll_y = finite_or_zero(y);
    }
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VisibilityChange {
    pub changed: bool,
    pub redraw_required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisibilityState {
    visible: bool,
}

impl Default for VisibilityState {
    fn default() -> Self {
        Self { visible: true }
    }
}

impl VisibilityState {
    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn set_visible(&mut self, visible: bool) -> VisibilityChange {
        let changed = self.visible != visible;
        let redraw_required = changed && visible;
        self.visible = visible;
        VisibilityChange {
            changed,
            redraw_required,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WalkSnapshot {
    pub resolved_width: f64,
    pub resolved_height: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuntimeFrameState {
    pub viewport_rect: ScreenRect,
    pub clip_rect: ScreenRect,
    pub walk: WalkSnapshot,
    pub draw_called: bool,
    pub draw_count: u64,
    pub content_size: ScreenSize,
    pub visible_range: Option<Range<usize>>,
    pub overscan_range: Option<Range<usize>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuntimeLayout {
    pub page_stack: PageStackLayout,
    pub visible_range: Option<Range<usize>>,
    pub overscan_range: Option<Range<usize>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CanvasRuntime {
    pub zoom: ZoomViewport,
    pub scroll: ScrollViewport,
    pub visibility: VisibilityState,
    pub frame_state: RuntimeFrameState,
}

impl CanvasRuntime {
    pub fn zoom(&self) -> f64 {
        self.zoom.zoom()
    }

    pub fn zoom_percent(&self) -> u16 {
        self.zoom.zoom_percent()
    }

    pub fn set_zoom(&mut self, requested: f64, anchor: ViewportPoint) -> bool {
        self.zoom
            .set_zoom_anchored(requested, anchor, &mut self.scroll)
    }

    pub fn zoom_in(&mut self, anchor: ViewportPoint) -> bool {
        self.set_zoom(self.zoom.zoom() * 1.25, anchor)
    }

    pub fn zoom_out(&mut self, anchor: ViewportPoint) -> bool {
        self.set_zoom(self.zoom.zoom() / 1.25, anchor)
    }

    pub fn reset_zoom(&mut self, anchor: ViewportPoint) -> bool {
        self.set_zoom(1.0, anchor)
    }

    pub fn logical_to_screen(&self, logical: LogicalPoint) -> ScreenPoint {
        self.zoom
            .logical_to_screen(logical, self.frame_state.viewport_rect.origin, &self.scroll)
    }

    pub fn screen_to_logical(&self, screen: ScreenPoint) -> LogicalPoint {
        self.zoom
            .screen_to_logical(screen, self.frame_state.viewport_rect.origin, &self.scroll)
    }

    pub fn logical_rect_to_screen(&self, logical: LogicalRect) -> ScreenRect {
        self.zoom.logical_rect_to_screen(
            logical,
            self.frame_state.viewport_rect.origin,
            &self.scroll,
        )
    }

    pub fn viewport_center(&self) -> ViewportPoint {
        ViewportPoint::new(
            self.frame_state.viewport_rect.size.width * 0.5,
            self.frame_state.viewport_rect.size.height * 0.5,
        )
    }

    pub fn set_visible(&mut self, visible: bool) -> VisibilityChange {
        let change = self.visibility.set_visible(visible);
        if !visible {
            self.frame_state.draw_called = false;
        }
        change
    }

    pub fn prepare_layout(
        &mut self,
        pages: &PageStack,
        viewport_rect: ScreenRect,
        clip_rect: ScreenRect,
        overscan_pages: usize,
    ) -> RuntimeLayout {
        self.frame_state.viewport_rect = viewport_rect;
        self.frame_state.clip_rect = viewport_rect.intersection(clip_rect);
        self.frame_state.walk = WalkSnapshot {
            resolved_width: viewport_rect.size.width,
            resolved_height: viewport_rect.size.height,
        };
        self.frame_state.draw_called = true;
        self.frame_state.draw_count = self.frame_state.draw_count.saturating_add(1);

        let zoom = self.zoom.zoom();
        let viewport_logical = LogicalSize::new(
            viewport_rect.size.width / zoom,
            viewport_rect.size.height / zoom,
        );
        let page_stack = pages.layout(viewport_logical);
        let content_size = ScreenSize::new(
            page_stack.content_size.width * zoom,
            page_stack.content_size.height * zoom,
        );
        self.scroll.set_extents(content_size, viewport_rect.size);
        let visible_logical = LogicalRect::new(
            self.scroll.scroll_x() / zoom,
            self.scroll.scroll_y() / zoom,
            viewport_rect.size.width / zoom,
            viewport_rect.size.height / zoom,
        );
        let visible_range = pages.visible_range(&page_stack, visible_logical);
        let overscan_range = pages.overscan_range(&page_stack, visible_logical, overscan_pages);
        self.frame_state.content_size = content_size;
        self.frame_state.visible_range = visible_range.clone();
        self.frame_state.overscan_range = overscan_range.clone();
        RuntimeLayout {
            page_stack,
            visible_range,
            overscan_range,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::{PageStack, PageSurface};

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }

    #[test]
    fn zoom_clamps_to_required_range() {
        let mut zoom = ZoomViewport::default();
        zoom.set_zoom(0.01);
        assert_close(zoom.zoom(), MIN_ZOOM);
        zoom.set_zoom(99.0);
        assert_close(zoom.zoom(), MAX_ZOOM);
    }

    #[test]
    fn logical_screen_transform_round_trips() {
        let mut runtime = CanvasRuntime::default();
        runtime.frame_state.viewport_rect = ScreenRect::new(100.0, 50.0, 800.0, 600.0);
        runtime.scroll.set_extents(
            ScreenSize::new(2000.0, 2000.0),
            ScreenSize::new(800.0, 600.0),
        );
        runtime.scroll.scroll_to(180.0, 220.0);
        runtime.zoom.set_zoom(1.75);
        let logical = LogicalPoint::new(420.0, 310.0);
        let screen = runtime.logical_to_screen(logical);
        let restored = runtime.screen_to_logical(screen);
        assert_close(restored.x, logical.x);
        assert_close(restored.y, logical.y);
    }

    #[test]
    fn anchored_zoom_keeps_point_under_cursor() {
        let mut runtime = CanvasRuntime::default();
        runtime.frame_state.viewport_rect = ScreenRect::new(0.0, 0.0, 900.0, 700.0);
        runtime.scroll.set_extents(
            ScreenSize::new(3000.0, 3000.0),
            ScreenSize::new(900.0, 700.0),
        );
        runtime.scroll.scroll_to(240.0, 360.0);
        let anchor = ViewportPoint::new(315.0, 275.0);
        let before = runtime.zoom.viewport_to_logical(anchor, &runtime.scroll);
        runtime.set_zoom(2.0, anchor);
        let after = runtime.zoom.viewport_to_logical(anchor, &runtime.scroll);
        assert_close(before.x, after.x);
        assert_close(before.y, after.y);
    }

    #[test]
    fn scroll_clamps_on_both_axes() {
        let mut scroll = ScrollViewport::default();
        scroll.set_extents(
            ScreenSize::new(1000.0, 800.0),
            ScreenSize::new(400.0, 300.0),
        );
        scroll.scroll_to(10_000.0, -50.0);
        assert_close(scroll.scroll_x(), 600.0);
        assert_close(scroll.scroll_y(), 0.0);
    }

    #[test]
    fn visibility_false_to_true_requests_redraw() {
        let mut visibility = VisibilityState::default();
        assert!(!visibility.set_visible(false).redraw_required);
        let change = visibility.set_visible(true);
        assert!(change.changed);
        assert!(change.redraw_required);
        assert!(visibility.visible());
    }

    #[test]
    fn page_count_yields_visible_geometry() {
        let pages = PageStack::new(vec![PageSurface::a4("one")]);
        let mut runtime = CanvasRuntime::default();
        let layout = runtime.prepare_layout(
            &pages,
            ScreenRect::new(20.0, 40.0, 900.0, 650.0),
            ScreenRect::new(20.0, 40.0, 900.0, 650.0),
            1,
        );
        assert_eq!(layout.page_stack.placements.len(), 1);
        let screen = runtime.logical_rect_to_screen(layout.page_stack.placements[0].rect);
        assert!(screen.size.width > 0.0);
        assert!(screen.size.height > 0.0);
        assert!(screen.intersects(runtime.frame_state.viewport_rect));
    }
}
