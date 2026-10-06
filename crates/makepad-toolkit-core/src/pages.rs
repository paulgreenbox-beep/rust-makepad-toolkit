use std::ops::Range;

use crate::geometry::{LogicalRect, LogicalSize};

pub const LOGICAL_PIXELS_PER_INCH: f64 = 96.0;
pub const MILLIMETERS_PER_INCH: f64 = 25.4;

pub fn millimeters_to_logical(mm: f64) -> f64 {
    mm * LOGICAL_PIXELS_PER_INCH / MILLIMETERS_PER_INCH
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageMargins {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl PageMargins {
    pub const fn new(left: f64, top: f64, right: f64, bottom: f64) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
}

impl Default for PageMargins {
    fn default() -> Self {
        Self::new(72.0, 72.0, 72.0, 72.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageSurface {
    pub id: String,
    pub label: String,
    pub size: LogicalSize,
    pub margins: PageMargins,
}

impl PageSurface {
    pub fn new(id: impl Into<String>, label: impl Into<String>, size: LogicalSize) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            size: size.non_negative(),
            margins: PageMargins::default(),
        }
    }

    pub fn with_margins(mut self, margins: PageMargins) -> Self {
        self.margins = margins;
        self
    }

    pub fn a4(id: impl Into<String>) -> Self {
        Self::new(
            id,
            "A4 portrait",
            LogicalSize::new(millimeters_to_logical(210.0), millimeters_to_logical(297.0)),
        )
    }

    pub fn a3_landscape(id: impl Into<String>) -> Self {
        Self::new(
            id,
            "A3 landscape",
            LogicalSize::new(millimeters_to_logical(420.0), millimeters_to_logical(297.0)),
        )
    }

    pub fn letter(id: impl Into<String>) -> Self {
        Self::new(
            id,
            "Letter portrait",
            LogicalSize::new(
                8.5 * LOGICAL_PIXELS_PER_INCH,
                11.0 * LOGICAL_PIXELS_PER_INCH,
            ),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PagePlacement {
    pub index: usize,
    pub page_id: String,
    pub rect: LogicalRect,
    pub content_rect: LogicalRect,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageStackLayout {
    pub content_size: LogicalSize,
    pub placements: Vec<PagePlacement>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageStack {
    pub pages: Vec<PageSurface>,
    pub gap: f64,
    pub padding: f64,
}

impl Default for PageStack {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl PageStack {
    pub fn new(pages: Vec<PageSurface>) -> Self {
        Self {
            pages,
            gap: 32.0,
            padding: 40.0,
        }
    }

    pub fn with_spacing(mut self, gap: f64, padding: f64) -> Self {
        self.gap = gap.max(0.0);
        self.padding = padding.max(0.0);
        self
    }

    pub fn layout(&self, viewport: LogicalSize) -> PageStackLayout {
        let widest = self
            .pages
            .iter()
            .map(|page| page.size.width)
            .fold(0.0_f64, f64::max);
        let content_width = viewport.width.max(widest + self.padding * 2.0).max(0.0);
        let mut y = self.padding;
        let mut placements = Vec::with_capacity(self.pages.len());
        for (index, page) in self.pages.iter().enumerate() {
            let x = (content_width - page.size.width) * 0.5;
            let rect = LogicalRect::new(x, y, page.size.width, page.size.height);
            let margins = page.margins;
            let content_rect = rect.inset(margins.left, margins.top, margins.right, margins.bottom);
            placements.push(PagePlacement {
                index,
                page_id: page.id.clone(),
                rect,
                content_rect,
            });
            y += page.size.height;
            if index + 1 < self.pages.len() {
                y += self.gap;
            }
        }
        let natural_height = if self.pages.is_empty() {
            0.0
        } else {
            y + self.padding
        };
        PageStackLayout {
            content_size: LogicalSize::new(content_width, viewport.height.max(natural_height)),
            placements,
        }
    }

    pub fn visible_range(
        &self,
        layout: &PageStackLayout,
        viewport: LogicalRect,
    ) -> Option<Range<usize>> {
        let first = layout
            .placements
            .iter()
            .position(|placement| placement.rect.intersects(viewport));
        let last = layout
            .placements
            .iter()
            .rposition(|placement| placement.rect.intersects(viewport));
        match (first, last) {
            (Some(first), Some(last)) => Some(first..last + 1),
            _ if layout.placements.is_empty() => None,
            _ => {
                let center = viewport.origin.y + viewport.size.height * 0.5;
                let nearest = layout
                    .placements
                    .iter()
                    .enumerate()
                    .min_by(|(_, left), (_, right)| {
                        let left_distance =
                            (left.rect.origin.y + left.rect.size.height * 0.5 - center).abs();
                        let right_distance =
                            (right.rect.origin.y + right.rect.size.height * 0.5 - center).abs();
                        left_distance.total_cmp(&right_distance)
                    })
                    .map(|(index, _)| index)?;
                Some(nearest..nearest + 1)
            }
        }
    }

    pub fn overscan_range(
        &self,
        layout: &PageStackLayout,
        viewport: LogicalRect,
        overscan_pages: usize,
    ) -> Option<Range<usize>> {
        let visible = self.visible_range(layout, viewport)?;
        Some(
            visible.start.saturating_sub(overscan_pages)
                ..(visible.end + overscan_pages).min(layout.placements.len()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }

    #[test]
    fn mixed_pages_are_centered_independently() {
        let stack = PageStack::new(vec![
            PageSurface::a4("a4"),
            PageSurface::a3_landscape("a3"),
            PageSurface::letter("letter"),
        ]);
        let layout = stack.layout(LogicalSize::new(1900.0, 700.0));
        for placement in &layout.placements {
            assert_close(
                placement.rect.origin.x * 2.0 + placement.rect.size.width,
                layout.content_size.width,
            );
        }
        assert!(layout.placements[1].rect.size.width > layout.placements[0].rect.size.width);
    }

    #[test]
    fn content_height_includes_padding_pages_and_gaps() {
        let stack = PageStack::new(vec![PageSurface::a4("1"), PageSurface::letter("2")])
            .with_spacing(24.0, 30.0);
        let layout = stack.layout(LogicalSize::new(100.0, 100.0));
        let expected = 30.0 + stack.pages[0].size.height + 24.0 + stack.pages[1].size.height + 30.0;
        assert_close(layout.content_size.height, expected);
    }

    #[test]
    fn gap_is_exact_between_mixed_pages() {
        let stack = PageStack::new(vec![PageSurface::a4("1"), PageSurface::letter("2")])
            .with_spacing(37.0, 20.0);
        let layout = stack.layout(LogicalSize::new(1200.0, 500.0));
        assert_close(
            layout.placements[1].rect.origin.y - layout.placements[0].rect.bottom(),
            37.0,
        );
    }

    #[test]
    fn visible_and_overscan_ranges_are_bounded() {
        let stack = PageStack::new(
            (0..8)
                .map(|index| PageSurface::a4(index.to_string()))
                .collect(),
        );
        let layout = stack.layout(LogicalSize::new(1000.0, 600.0));
        let third = layout.placements[3].rect;
        let viewport = LogicalRect::new(0.0, third.origin.y + 100.0, 1000.0, 400.0);
        assert_eq!(stack.visible_range(&layout, viewport), Some(3..4));
        assert_eq!(stack.overscan_range(&layout, viewport, 2), Some(1..6));
    }

    #[test]
    fn page_content_rect_uses_its_own_margins() {
        let page =
            PageSurface::a4("margins").with_margins(PageMargins::new(10.0, 20.0, 30.0, 40.0));
        let stack = PageStack::new(vec![page]);
        let layout = stack.layout(LogicalSize::new(1000.0, 800.0));
        let placement = &layout.placements[0];
        assert_close(
            placement.content_rect.origin.x,
            placement.rect.origin.x + 10.0,
        );
        assert_close(
            placement.content_rect.origin.y,
            placement.rect.origin.y + 20.0,
        );
        assert_close(
            placement.content_rect.size.width,
            placement.rect.size.width - 40.0,
        );
    }
}
