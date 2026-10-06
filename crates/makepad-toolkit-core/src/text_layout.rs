use crate::geometry::LogicalPoint;
use crate::model::UiColor;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlignment {
    #[default]
    Left,
    Center,
    Right,
    Justified,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextRunStyle {
    pub font_family: Option<String>,
    /// Document font size in logical pixels at 100% zoom.
    pub font_size: f64,
    pub color: UiColor,
    pub background: Option<UiColor>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DocumentFontFamily {
    #[default]
    Proportional,
    Monospace,
}

/// Resolves document-declared font names to the bundled Makepad families.
/// Unknown and unavailable document fonts intentionally fall back to the
/// proportional document family instead of silently changing layout rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocumentFontRegistry;

impl DocumentFontRegistry {
    pub fn resolve(&self, name: Option<&str>) -> DocumentFontFamily {
        let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
            return DocumentFontFamily::Proportional;
        };
        let normalized = name.to_ascii_lowercase();
        match normalized.as_str() {
            "courier" | "courier new" | "consolas" | "cascadia mono" | "jetbrains mono"
            | "liberation mono" | "monospace" => DocumentFontFamily::Monospace,
            _ => DocumentFontFamily::Proportional,
        }
    }

    pub fn is_supported(&self, name: Option<&str>) -> bool {
        matches!(self.resolve(name), DocumentFontFamily::Monospace)
            || name
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .is_none()
            || name.is_some_and(|name| {
                matches!(
                    name.trim().to_ascii_lowercase().as_str(),
                    "arial" | "calibri" | "cambria" | "times new roman" | "ibm plex sans"
                )
            })
    }
}

impl Default for TextRunStyle {
    fn default() -> Self {
        Self {
            font_family: None,
            font_size: 14.666_666_666_7,
            color: UiColor::BLACK,
            background: None,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub text: String,
    pub style: TextRunStyle,
}

impl TextRun {
    pub fn new(text: impl Into<String>, style: TextRunStyle) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParagraphLayoutInput {
    pub origin: LogicalPoint,
    pub width: f64,
    pub alignment: TextAlignment,
    pub indent_left: f64,
    pub indent_right: f64,
    pub first_line_indent: f64,
    pub space_before: f64,
    pub space_after: f64,
    pub line_spacing: f64,
    pub marker: Option<TextRun>,
    pub runs: Vec<TextRun>,
}

impl ParagraphLayoutInput {
    pub fn new(origin: LogicalPoint, width: f64, runs: Vec<TextRun>) -> Self {
        Self {
            origin,
            width: width.max(1.0),
            alignment: TextAlignment::Left,
            indent_left: 0.0,
            indent_right: 0.0,
            first_line_indent: 0.0,
            space_before: 0.0,
            space_after: 0.0,
            line_spacing: 1.0,
            marker: None,
            runs,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextMetrics {
    pub width: f64,
    pub ascender: f64,
    pub descender: f64,
}

impl TextMetrics {
    pub fn line_height(self) -> f64 {
        (self.ascender + self.descender).max(1.0)
    }
}

pub trait TextMeasurer {
    fn measure(&mut self, style: TextRunStyle, text: String) -> TextMetrics;
}

impl<F> TextMeasurer for F
where
    F: FnMut(TextRunStyle, String) -> TextMetrics,
{
    fn measure(&mut self, style: TextRunStyle, text: String) -> TextMetrics {
        self(style, text)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PositionedRun {
    pub text: String,
    pub style: TextRunStyle,
    pub x: f64,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PositionedLine {
    pub top: f64,
    pub baseline: f64,
    pub width: f64,
    pub height: f64,
    pub runs: Vec<PositionedRun>,
    pub marker: Option<PositionedRun>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParagraphLayout {
    pub lines: Vec<PositionedLine>,
    pub height: f64,
    pub measured_width: f64,
}

#[derive(Clone, Debug)]
struct LineBuffer {
    runs: Vec<TextRun>,
    width: f64,
    height: f64,
    ascender: f64,
    descender: f64,
}

impl LineBuffer {
    fn new() -> Self {
        Self {
            runs: Vec::new(),
            width: 0.0,
            height: 0.0,
            ascender: 0.0,
            descender: 0.0,
        }
    }

    fn is_empty(&self) -> bool {
        self.runs.iter().all(|run| run.text.is_empty())
    }

    fn push_char<M: TextMeasurer>(
        &mut self,
        style: &TextRunStyle,
        character: char,
        measurer: &mut M,
    ) {
        if let Some(last) = self.runs.last_mut()
            && last.style == *style
        {
            last.text.push(character);
        } else {
            self.runs
                .push(TextRun::new(character.to_string(), style.clone()));
        }
        let metrics = measure_runs(&self.runs, measurer);
        self.width = metrics.width;
        self.height = metrics.line_height();
        self.ascender = metrics.ascender;
        self.descender = metrics.descender;
    }

    fn pop_last_char<M: TextMeasurer>(&mut self, measurer: &mut M) {
        if let Some(last) = self.runs.last_mut() {
            last.text.pop();
            if last.text.is_empty() {
                self.runs.pop();
            }
        }
        let metrics = measure_runs(&self.runs, measurer);
        self.width = metrics.width;
        self.height = metrics.line_height();
        self.ascender = metrics.ascender;
        self.descender = metrics.descender;
    }
}

fn measure_runs<M: TextMeasurer>(runs: &[TextRun], measurer: &mut M) -> TextMetrics {
    let mut result = TextMetrics::default();
    for run in runs {
        let metrics = measurer.measure(run.style.clone(), run.text.clone());
        result.width += metrics.width;
        result.ascender = result.ascender.max(metrics.ascender);
        result.descender = result.descender.max(metrics.descender);
    }
    if result.ascender <= 0.0 && result.descender <= 0.0 {
        let style = runs
            .first()
            .map(|run| &run.style)
            .cloned()
            .unwrap_or_default();
        let metrics = measurer.measure(style, "Ag".to_owned());
        result.ascender = metrics.ascender;
        result.descender = metrics.descender;
    }
    result
}

fn append_positioned_runs(
    runs: &[TextRun],
    start_x: f64,
    measurer: &mut impl TextMeasurer,
) -> Vec<PositionedRun> {
    let mut x = start_x;
    runs.iter()
        .filter(|run| !run.text.is_empty())
        .map(|run| {
            let width = measurer.measure(run.style.clone(), run.text.clone()).width;
            let positioned = PositionedRun {
                text: run.text.clone(),
                style: run.style.clone(),
                x,
                width,
            };
            x += width;
            positioned
        })
        .collect()
}

fn finish_line(lines: &mut Vec<LineBuffer>, line: &mut LineBuffer, force_empty: bool) {
    if line.is_empty() && !force_empty && !lines.is_empty() {
        return;
    }
    let replacement = std::mem::replace(line, LineBuffer::new());
    lines.push(replacement);
}

pub fn layout_paragraph(
    input: &ParagraphLayoutInput,
    measurer: &mut impl TextMeasurer,
) -> ParagraphLayout {
    let left = input.indent_left.max(0.0);
    let right = input.indent_right.max(0.0);
    let first_indent = input.first_line_indent;
    let base_width = (input.width - left - right).max(1.0);
    let mut line = LineBuffer::new();
    let mut lines = Vec::new();

    for run in &input.runs {
        for character in run.text.chars() {
            if character == '\n' {
                finish_line(&mut lines, &mut line, true);
                continue;
            }
            let first = lines.is_empty();
            let available = (base_width - if first { first_indent } else { 0.0 }).max(1.0);
            line.push_char(&run.style, character, measurer);
            if line.width > available
                && line.runs.iter().any(|value| value.text.chars().count() > 1)
            {
                let last_style = line.runs.last().map(|value| value.style.clone());
                line.pop_last_char(measurer);
                finish_line(&mut lines, &mut line, true);
                if let Some(style) = last_style {
                    line.push_char(&style, character, measurer);
                }
            }
        }
    }
    finish_line(&mut lines, &mut line, true);

    if lines.is_empty() {
        lines.push(LineBuffer::new());
    }

    let marker_metrics = input
        .marker
        .as_ref()
        .map(|marker| measurer.measure(marker.style.clone(), marker.text.clone()));
    let marker_width = marker_metrics
        .map(|metrics| metrics.width + 6.0)
        .unwrap_or(0.0);
    let line_spacing = input.line_spacing.max(0.1);
    let mut result = ParagraphLayout::default();
    let mut y = input.origin.y + input.space_before.max(0.0);

    for (line_index, line) in lines.iter().enumerate() {
        let first = line_index == 0;
        let indent = if first { first_indent } else { 0.0 };
        let available = (base_width - indent).max(1.0);
        let alignment_offset = match input.alignment {
            TextAlignment::Center => (available - line.width).max(0.0) * 0.5,
            TextAlignment::Right => (available - line.width).max(0.0),
            TextAlignment::Left | TextAlignment::Justified => 0.0,
        };
        let x = input.origin.x + left + indent + alignment_offset;
        let height = (line.height.max(1.0) * line_spacing).max(1.0);
        let marker = if first {
            input.marker.as_ref().map(|marker| PositionedRun {
                text: marker.text.clone(),
                style: marker.style.clone(),
                x: (x - marker_width).max(input.origin.x),
                width: marker_metrics.map(|metrics| metrics.width).unwrap_or(0.0),
            })
        } else {
            None
        };
        result.measured_width = result.measured_width.max(line.width);
        result.lines.push(PositionedLine {
            top: y,
            baseline: y + line.ascender,
            width: line.width,
            height,
            runs: append_positioned_runs(&line.runs, x, measurer),
            marker,
        });
        y += height;
    }
    result.height = (y - input.origin.y + input.space_after.max(0.0)).max(1.0);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(style: TextRunStyle, text: String) -> TextMetrics {
        TextMetrics {
            width: text.chars().count() as f64
                * style.font_size
                * if style.bold { 0.62 } else { 0.5 },
            ascender: style.font_size * 0.8,
            descender: style.font_size * 0.2,
        }
    }

    #[test]
    fn document_font_registry_resolves_bundled_families() {
        let registry = DocumentFontRegistry;
        assert_eq!(
            registry.resolve(Some("Liberation Mono")),
            DocumentFontFamily::Monospace
        );
        assert_eq!(
            registry.resolve(Some("Calibri")),
            DocumentFontFamily::Proportional
        );
        assert!(!registry.is_supported(Some("A font that is not bundled")));
        assert!(registry.is_supported(Some("Courier New")));
    }

    #[test]
    fn measured_width_controls_wrapping() {
        let style = TextRunStyle::default();
        let input = ParagraphLayoutInput::new(
            LogicalPoint::new(0.0, 0.0),
            40.0,
            vec![TextRun::new("abcdefgh", style)],
        );
        let layout = layout_paragraph(&input, &mut measure);
        assert!(layout.lines.len() > 1);
        assert!(layout.lines.iter().all(|line| line.width <= 40.0));
    }

    #[test]
    fn mixed_runs_keep_style_and_unicode() {
        let normal = TextRunStyle::default();
        let mut bold = normal.clone();
        bold.bold = true;
        let input = ParagraphLayoutInput::new(
            LogicalPoint::new(2.0, 3.0),
            500.0,
            vec![
                TextRun::new("Привет ", normal),
                TextRun::new("World 123", bold.clone()),
            ],
        );
        let layout = layout_paragraph(&input, &mut measure);
        assert_eq!(layout.lines.len(), 1);
        assert_eq!(layout.lines[0].runs[0].text, "Привет ");
        assert_eq!(layout.lines[0].runs[1].style, bold);
    }

    #[test]
    fn paragraph_alignment_and_spacing_are_applied() {
        let input = ParagraphLayoutInput {
            origin: LogicalPoint::new(10.0, 20.0),
            width: 200.0,
            alignment: TextAlignment::Right,
            indent_left: 4.0,
            indent_right: 6.0,
            first_line_indent: 8.0,
            space_before: 12.0,
            space_after: 7.0,
            line_spacing: 1.5,
            marker: None,
            runs: vec![TextRun::new("abc", TextRunStyle::default())],
        };
        let layout = layout_paragraph(&input, &mut measure);
        assert!(layout.lines[0].runs[0].x > 10.0);
        assert!(layout.height > 12.0 + 7.0);
    }
}
