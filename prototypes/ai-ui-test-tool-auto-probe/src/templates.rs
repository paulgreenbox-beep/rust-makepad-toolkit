//! Declarative, versioned generic test-template registry.

use crate::{catalog, inventory::SafetyClass};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, sync::OnceLock};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestTemplateCatalog {
    pub version: u32,
    pub templates: Vec<TestTemplate>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestTemplate {
    pub id: String,
    pub version: u32,
    #[serde(default)]
    pub requirements: Vec<String>,
    pub safety: SafetyClass,
    pub class: String,
    pub operations: Vec<String>,
    pub description: String,
}

pub fn registry() -> &'static TestTemplateCatalog {
    static REGISTRY: OnceLock<TestTemplateCatalog> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        serde_json::from_str(include_str!("../test-templates.json"))
            .expect("embedded test-templates.json must be valid")
    })
}

pub fn template(id: &str) -> Option<&'static TestTemplate> {
    registry()
        .templates
        .iter()
        .find(|template| template.id == id)
}

pub fn validate_registry() -> Result<(), Vec<String>> {
    validate_catalog(registry())
}

pub fn load_custom(path: impl AsRef<Path>) -> Result<TestTemplateCatalog, String> {
    let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let catalog: TestTemplateCatalog = serde_json::from_str(&raw)
        .map_err(|error| format!("invalid custom template catalog: {error}"))?;
    validate_catalog(&catalog).map_err(|errors| errors.join("; "))?;
    Ok(catalog)
}

pub fn merge_custom(custom: &TestTemplateCatalog) -> Result<TestTemplateCatalog, Vec<String>> {
    let mut merged = registry().clone();
    merged.templates.extend(custom.templates.clone());
    validate_catalog(&merged)?;
    Ok(merged)
}

pub fn validate_catalog(catalog: &TestTemplateCatalog) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    if catalog.version == 0 {
        errors.push("template catalog version must be positive".into());
    }
    for template in &catalog.templates {
        if !ids.insert(template.id.as_str()) {
            errors.push(format!("duplicate template id: {}", template.id));
        }
        if template.version == 0 {
            errors.push(format!("{} has invalid version", template.id));
        }
        if template.operations.is_empty() {
            errors.push(format!("{} has no operations", template.id));
        }
        for operation in &template.operations {
            if catalog::operation(operation).is_none() {
                errors.push(format!(
                    "{} references unknown operation: {operation}",
                    template.id
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_declarative_versioned_and_catalog_valid() {
        validate_registry().unwrap();
        for required in [
            "button.invoke",
            "edit.value",
            "checkbox.toggle",
            "selection.single",
            "spatial.health",
            "spatial.drag",
            "window.resize",
            "window.interaction",
            "window.move",
            "window.frame_resize",
            "window.state",
            "runtime.health",
            "runtime.task",
            "external.request",
            "file.output",
            "trace.milestone",
            "visual.region",
        ] {
            assert!(template(required).is_some(), "missing {required}");
        }
    }
}
