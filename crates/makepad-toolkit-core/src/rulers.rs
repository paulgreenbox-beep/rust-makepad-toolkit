use crate::pages::{LOGICAL_PIXELS_PER_INCH, PageMargins};

pub const CENTIMETERS_PER_INCH: f64 = 2.54;
pub const LOGICAL_PIXELS_PER_CENTIMETER: f64 = LOGICAL_PIXELS_PER_INCH / CENTIMETERS_PER_INCH;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulerAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RulerMarkerVisual {
    #[default]
    Bar,
    TriangleDown,
    TriangleUp,
    TriangleLeft,
    TriangleRight,
}

/// A format-neutral editable marker positioned in page-local logical units.
#[derive(Clone, Debug, PartialEq)]
pub struct RulerMarker {
    pub id: String,
    pub label: String,
    pub page_index: usize,
    pub axis: RulerAxis,
    pub position: f64,
    pub visual: RulerMarkerVisual,
    pub editable: bool,
}

impl RulerMarker {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        page_index: usize,
        axis: RulerAxis,
        position: f64,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            page_index,
            axis,
            position,
            visual: RulerMarkerVisual::Bar,
            editable: true,
        }
    }

    pub fn with_visual(mut self, visual: RulerMarkerVisual) -> Self {
        self.visual = visual;
        self
    }

    pub fn read_only(mut self) -> Self {
        self.editable = false;
        self
    }

    pub fn clamped_position(&self, logical_length: f64) -> f64 {
        if !logical_length.is_finite() || logical_length <= 0.0 || !self.position.is_finite() {
            return 0.0;
        }
        self.position.clamp(0.0, logical_length)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RulerTick {
    pub position: f64,
    pub length_fraction: f64,
    pub major: bool,
    pub label: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RulerScale {
    pub subdivisions_per_centimeter: u8,
}

impl Default for RulerScale {
    fn default() -> Self {
        Self {
            subdivisions_per_centimeter: 10,
        }
    }
}

impl RulerScale {
    pub fn ticks(&self, logical_length: f64) -> Vec<RulerTick> {
        if logical_length <= 0.0 || !logical_length.is_finite() {
            return Vec::new();
        }
        let subdivisions = self.subdivisions_per_centimeter.max(1) as usize;
        let step = LOGICAL_PIXELS_PER_CENTIMETER / subdivisions as f64;
        let count = (logical_length / step).floor() as usize;
        (0..=count)
            .map(|index| {
                let subdivision = index % subdivisions;
                let major = subdivision == 0;
                let half = subdivisions.is_multiple_of(2) && subdivision == subdivisions / 2;
                RulerTick {
                    position: index as f64 * step,
                    length_fraction: if major {
                        1.0
                    } else if half {
                        0.65
                    } else {
                        0.38
                    },
                    major,
                    label: major.then(|| (index / subdivisions).to_string()),
                }
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RulerMarginBands {
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HorizontalRuler {
    pub thickness: f64,
    pub scale: RulerScale,
}

impl Default for HorizontalRuler {
    fn default() -> Self {
        Self {
            thickness: 22.0,
            scale: RulerScale::default(),
        }
    }
}

impl HorizontalRuler {
    pub fn margin_bands(&self, page_width: f64, margins: PageMargins) -> RulerMarginBands {
        RulerMarginBands {
            start: margins.left.clamp(0.0, page_width.max(0.0)),
            end: (page_width - margins.right).clamp(0.0, page_width.max(0.0)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VerticalRuler {
    pub thickness: f64,
    pub scale: RulerScale,
}

impl Default for VerticalRuler {
    fn default() -> Self {
        Self {
            thickness: 22.0,
            scale: RulerScale::default(),
        }
    }
}

impl VerticalRuler {
    pub fn margin_bands(&self, page_height: f64, margins: PageMargins) -> RulerMarginBands {
        RulerMarginBands {
            start: margins.top.clamp(0.0, page_height.max(0.0)),
            end: (page_height - margins.bottom).clamp(0.0, page_height.max(0.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_scale_has_centimeter_labels_and_millimeter_ticks() {
        let ticks = RulerScale::default().ticks(LOGICAL_PIXELS_PER_CENTIMETER * 3.0);
        assert!(ticks.len() >= 30);
        assert_eq!(ticks[0].label.as_deref(), Some("0"));
        assert_eq!(ticks[10].label.as_deref(), Some("1"));
        assert!(ticks[5].length_fraction > ticks[1].length_fraction);
    }

    #[test]
    fn horizontal_and_vertical_margins_use_page_geometry() {
        let margins = PageMargins::new(72.0, 80.0, 90.0, 100.0);
        assert_eq!(
            HorizontalRuler::default().margin_bands(800.0, margins),
            RulerMarginBands {
                start: 72.0,
                end: 710.0
            }
        );
        assert_eq!(
            VerticalRuler::default().margin_bands(1100.0, margins),
            RulerMarginBands {
                start: 80.0,
                end: 1000.0
            }
        );
    }

    #[test]
    fn ruler_marker_position_is_page_local_and_bounded() {
        let marker = RulerMarker::new(
            "paragraph.first-line",
            "First line indent",
            2,
            RulerAxis::Horizontal,
            940.0,
        )
        .with_visual(RulerMarkerVisual::TriangleDown);
        assert_eq!(marker.page_index, 2);
        assert_eq!(marker.clamped_position(800.0), 800.0);
        assert_eq!(marker.clamped_position(1_000.0), 940.0);
    }
}
