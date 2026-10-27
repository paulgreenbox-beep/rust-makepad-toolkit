use serde::{Deserialize, Serialize};

const MAX_RECORDED_LOSSES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LossItem {
    pub code: String,
    pub severity: LossSeverity,
    pub message: String,
    pub entity_handle: Option<String>,
    pub group_code: Option<i32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LossReport {
    /// SHA-256 of the complete input or output byte stream, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sha256: Option<String>,
    /// Number of source entities examined (or snapshot entities considered).
    #[serde(default)]
    pub entity_count: usize,
    /// Number of entities represented in the converted result.
    #[serde(default)]
    pub converted_count: usize,
    /// Number of entities deliberately omitted by the adapter.
    #[serde(default)]
    pub unsupported_count: usize,
    /// Explicit unit decision for this conversion, if one was resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_resolution: Option<drafting_core::UnitResolution>,
    pub items: Vec<LossItem>,
    /// Fields that were understood as data but are not represented by the Lab model.
    #[serde(default)]
    pub discarded_fields: Vec<LossItem>,
    /// Opaque fields retained byte-for-byte by an adapter. This version does not retain any.
    #[serde(default)]
    pub opaque_fields: Vec<LossItem>,
    #[serde(default)]
    pub warnings: Vec<LossItem>,
    #[serde(default)]
    pub critical_losses: Vec<LossItem>,
    pub omitted_items: usize,
    pub omitted_critical_items: usize,
}

impl LossReport {
    pub fn has_critical(&self) -> bool {
        self.omitted_critical_items > 0
            || self
                .items
                .iter()
                .any(|item| item.severity == LossSeverity::Critical)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.omitted_items == 0
    }

    pub fn summary(&self) -> String {
        let critical = self
            .items
            .iter()
            .filter(|item| item.severity == LossSeverity::Critical)
            .count()
            + self.omitted_critical_items;
        let omitted = self.omitted_items;
        let first = self
            .items
            .iter()
            .take(4)
            .map(|item| format!("{}: {}", item.code, item.message))
            .collect::<Vec<_>>()
            .join("; ");
        if first.is_empty() {
            format!(
                "DXF conversion has {critical} critical losses ({omitted} additional report entries omitted)"
            )
        } else {
            format!(
                "DXF conversion has {critical} critical losses: {first} ({omitted} additional report entries omitted)"
            )
        }
    }

    pub(crate) fn push(
        &mut self,
        code: impl Into<String>,
        severity: LossSeverity,
        message: impl Into<String>,
        entity_handle: Option<String>,
        group_code: Option<i32>,
    ) {
        if self.items.len() >= MAX_RECORDED_LOSSES {
            self.omitted_items = self.omitted_items.saturating_add(1);
            if severity == LossSeverity::Critical {
                self.omitted_critical_items = self.omitted_critical_items.saturating_add(1);
            }
            return;
        }
        let message = message.into();
        let message = truncate_utf8(&message, 512);
        let item = LossItem {
            code: code.into(),
            severity,
            message,
            entity_handle,
            group_code,
        };
        if severity == LossSeverity::Warning {
            self.warnings.push(item.clone());
        }
        if severity == LossSeverity::Critical {
            self.critical_losses.push(item.clone());
        }
        if is_discarded_field(&item.code) {
            self.discarded_fields.push(item.clone());
        }
        self.items.push(item);
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let boundary = value
        .char_indices()
        .take_while(|(index, _)| *index < max_bytes.saturating_sub(3))
        .last()
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    format!("{}…", &value[..boundary])
}

fn is_discarded_field(code: &str) -> bool {
    matches!(
        code,
        "unsupported_tag"
            | "unmapped_header_tag"
            | "unmapped_header_variable"
            | "header_metadata_not_preserved"
            | "source_provenance_not_exported"
            | "xdata_not_preserved"
            | "entity_xdata_not_preserved"
            | "layer_xdata_not_preserved"
            | "entity_lineweight_not_preserved"
            | "layer_lineweight_not_preserved"
            | "layer_linetype_not_preserved"
            | "entity_owner_reference_not_preserved"
            | "layer_owner_reference_not_preserved"
            | "entity_visibility_not_preserved"
            | "layer_visibility_not_preserved"
            | "polyline_width_not_preserved"
            | "aci_color_approximated"
            | "unsupported_entity_geometry"
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossPolicy {
    #[default]
    RejectCritical,
    AllowReported,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_utf8_valid_and_reports_structured_categories() {
        let mut report = LossReport::default();
        report.push(
            "unsupported_tag",
            LossSeverity::Critical,
            format!("{}я", "a".repeat(511)),
            Some("A1".into()),
            Some(999),
        );
        assert!(report.has_critical());
        assert_eq!(report.critical_losses.len(), 1);
        assert_eq!(report.discarded_fields.len(), 1);
        assert!(
            report.items[0]
                .message
                .is_char_boundary(report.items[0].message.len())
        );
        assert!(report.items[0].message.len() <= 512);
    }
}
