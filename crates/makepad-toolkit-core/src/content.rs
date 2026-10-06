use crate::geometry::{LogicalPoint, LogicalRect};
use crate::model::UiColor;
use crate::text_layout::{TextAlignment, TextRun};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParagraphFlow {
    #[default]
    Body,
    Fixed,
}

/// A stable domain-owned identity for a paragraph that can receive a generic
/// canvas interaction. The toolkit carries the identity but never interprets
/// the domain command behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageParagraphInteraction {
    pub id: String,
}

impl PageParagraphInteraction {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageParagraph {
    pub page_index: usize,
    pub origin: LogicalPoint,
    pub width: f64,
    pub alignment: TextAlignment,
    pub indent_left: f64,
    pub indent_right: f64,
    pub first_line_indent: f64,
    pub space_before: f64,
    pub space_after: f64,
    pub line_spacing: f64,
    pub flow: ParagraphFlow,
    pub marker: Option<TextRun>,
    pub runs: Vec<TextRun>,
    /// Optional domain-owned target for pointer interaction with this paragraph.
    pub interaction: Option<PageParagraphInteraction>,
}

impl PageParagraph {
    pub fn new(page_index: usize, origin: LogicalPoint, width: f64, runs: Vec<TextRun>) -> Self {
        Self {
            page_index,
            origin,
            width: width.max(1.0),
            alignment: TextAlignment::Left,
            indent_left: 0.0,
            indent_right: 0.0,
            first_line_indent: 0.0,
            space_before: 0.0,
            space_after: 0.0,
            line_spacing: 1.0,
            flow: ParagraphFlow::Body,
            marker: None,
            runs,
            interaction: None,
        }
    }

    /// Marks this paragraph as a target for the generic page-canvas pointer
    /// activation lifecycle. The owning application resolves the identifier.
    pub fn with_interaction(mut self, id: impl Into<String>) -> Self {
        self.interaction = Some(PageParagraphInteraction::new(id));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_layout::{TextRun, TextRunStyle};

    #[test]
    fn paragraph_interaction_is_opt_in_and_preserves_domain_identity() {
        let paragraph = PageParagraph::new(
            0,
            LogicalPoint::new(10.0, 20.0),
            120.0,
            vec![TextRun::new("Editable", TextRunStyle::default())],
        );
        assert!(paragraph.interaction.is_none());

        let paragraph = paragraph.with_interaction("consumer:paragraph:42");
        assert_eq!(
            paragraph.interaction,
            Some(PageParagraphInteraction::new("consumer:paragraph:42"))
        );
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageTableCell {
    pub rect: LogicalRect,
    pub text: String,
    pub runs: Vec<TextRun>,
    pub alignment: TextAlignment,
    pub background: Option<UiColor>,
    pub border: Option<UiColor>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageTableRow {
    pub rect: LogicalRect,
    pub cells: Vec<PageTableCell>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageTable {
    pub page_index: usize,
    pub rect: LogicalRect,
    pub columns: Vec<f64>,
    pub rows: Vec<PageTableRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageImage {
    pub page_index: usize,
    pub rect: LogicalRect,
    pub pixel_width: usize,
    pub pixel_height: usize,
    pub bytes: Vec<u8>,
}
