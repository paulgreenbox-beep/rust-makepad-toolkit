//! Versioned public operation catalog.
//!
//! `operations.json` is the sole source of truth. Script validation, physical
//! input locking, coverage analysis and generation all consume this model.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationCatalog {
    pub version: u32,
    pub operations: Vec<OperationSpec>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OperationSpec {
    pub name: String,
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub idempotency: Option<String>,
    #[serde(default)]
    pub physical_input: bool,
    #[serde(default)]
    pub local_only: bool,
    #[serde(default)]
    pub scheduler_classification: Option<String>,
    #[serde(default)]
    pub assertion: bool,
    #[serde(default)]
    pub wait: bool,
    #[serde(default)]
    pub evidence: bool,
    #[serde(default)]
    pub validation: bool,
    #[serde(default)]
    pub mutation: bool,
    #[serde(default)]
    pub verification: bool,
    #[serde(default)]
    pub spatial: bool,
    #[serde(default)]
    pub target: bool,
    #[serde(default)]
    pub capability: Option<String>,
    #[serde(default)]
    pub coverage: Option<String>,
}

pub fn operation_catalog() -> &'static OperationCatalog {
    static CATALOG: OnceLock<OperationCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let source = include_str!("../operations.json").trim_start_matches('\u{feff}');
        serde_json::from_str(source).expect("embedded operations.json must be valid")
    })
}

pub fn operation(name: &str) -> Option<&'static OperationSpec> {
    operation_catalog()
        .operations
        .iter()
        .find(|operation| operation.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn embedded_catalog_is_versioned_unique_and_covers_public_baseline() {
        let catalog = operation_catalog();
        assert_eq!(catalog.version, 1);
        let names = catalog
            .operations
            .iter()
            .map(|operation| operation.name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(names.len(), catalog.operations.len());
        for required in [
            "session.open",
            "inspect_ui",
            "inspect_spatial_target",
            "click",
            "set_value",
            "assert_visible",
            "capture_region",
            "windows_export_evidence",
        ] {
            assert!(names.contains(required), "missing {required}");
        }
    }

    #[test]
    fn scroll_edges_are_classified_as_physical_input() {
        for name in ["scroll_to_start", "scroll_to_end"] {
            assert!(super::operation(name).is_some_and(|spec| spec.physical_input));
        }
    }

    #[test]
    fn window_frame_operations_are_classified_for_the_interactive_desktop() {
        for name in [
            "windows_drag_title",
            "windows_resize_frame",
            "windows_double_click_title",
        ] {
            let spec = super::operation(name).expect("window frame operation is public");
            assert!(
                spec.physical_input,
                "{name} must acquire the global input lock"
            );
            assert_eq!(
                spec.scheduler_classification.as_deref(),
                Some("windows.interactive_desktop")
            );
        }
        for name in [
            "inspect_window_interaction",
            "windows_set_window_bounds",
            "windows_set_window_state",
        ] {
            let spec = super::operation(name).expect("non-physical window operation is public");
            assert!(
                !spec.physical_input,
                "{name} must not claim physical UX proof or acquire the input lock"
            );
        }
    }
    #[test]
    fn observation_operations_are_explicitly_semantic_or_local() {
        for name in [
            "get_task_state",
            "wait_for_task_complete",
            "assert_api_success",
            "wait_for_file_stable",
            "assert_region_changed",
            "assert_trace_event",
            "assert_no_pending_task",
        ] {
            let spec = super::operation(name).expect("observation operation is public");
            assert!(
                !spec.physical_input,
                "{name} must not acquire a desktop input lock"
            );
            assert_eq!(spec.scheduler_classification.as_deref(), Some("semantic"));
        }
        for name in ["repeat_until", "wait_for_outcome"] {
            let spec = super::operation(name).expect("local control is public");
            assert!(spec.local_only, "{name} must execute locally");
            assert!(
                !spec.physical_input,
                "{name} must not acquire desktop input"
            );
            assert!(spec.wait, "{name} must be classified as a wait");
            assert!(
                spec.verification,
                "{name} must be classified as verification"
            );
            assert_eq!(spec.scheduler_classification.as_deref(), Some("local"));
        }
        let click_once = super::operation("click_once_and_wait_for_outcome")
            .expect("one-click postcondition control is public");
        assert!(click_once.physical_input);
        assert!(click_once.mutation);
        assert!(click_once.wait);
        assert!(click_once.verification);
        assert_eq!(
            click_once.scheduler_classification.as_deref(),
            Some("semantic")
        );
    }
}
