//! Declarative .aiui.json model and validation. This module is transport-agnostic.
use crate::{catalog, script_controls};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, fs, path::Path};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Script {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub application: Application,
    #[serde(default)]
    pub options: ScriptOptions,
    #[serde(default)]
    pub vars: Map<String, Value>,
    #[serde(default)]
    pub setup: Vec<Step>,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub teardown: Vec<Step>,
    #[serde(default)]
    pub cases: Vec<ScriptCase>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub generated_by: Option<String>,
    #[serde(default)]
    pub generated: bool,
    #[serde(default)]
    pub generator_version: Option<String>,
    #[serde(default)]
    pub inventory_version: Option<u32>,
    #[serde(default)]
    pub generated_from: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub regression: Option<RegressionMetadata>,
    #[serde(default)]
    pub secret_vars: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegressionMetadata {
    pub bug_id: String,
    pub description: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Application {
    #[serde(default)]
    pub command: Option<String>,
    /// Literal process arguments passed directly to the application (without a shell).
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub window_title: Option<String>,
    /// Test-only deterministic fixture mode. This is deliberately not a
    /// generic command argument or arbitrary child-process environment.
    #[serde(default)]
    pub fixture_async_failure_mode: Option<String>,
}

pub const FIXTURE_ASYNC_FAILURE_MODES: &[&str] = &[
    "success",
    "task_fails",
    "api_timeout",
    "api_http_failure",
    "api_invalid_response",
    "api_not_accepted",
    "file_never_appears",
    "file_never_stable",
    "missing_milestone",
];

pub fn is_fixture_async_failure_mode(value: &str) -> bool {
    FIXTURE_ASYNC_FAILURE_MODES.contains(&value)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScriptOptions {
    #[serde(default = "default_true")]
    pub stop_on_error: bool,
    #[serde(default)]
    pub collect_failures: bool,
    #[serde(default = "default_true")]
    pub evidence_on_failure: bool,
    #[serde(default)]
    pub semantic_only: bool,
    #[serde(default)]
    pub timeouts: Timeouts,
}
impl Default for ScriptOptions {
    fn default() -> Self {
        Self {
            stop_on_error: true,
            collect_failures: false,
            evidence_on_failure: true,
            semantic_only: false,
            timeouts: Timeouts::default(),
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Timeouts {
    #[serde(default)]
    pub action_ms: Option<u64>,
    #[serde(default)]
    pub wait_ms: Option<u64>,
    #[serde(default)]
    pub application_start_ms: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub op: String,
    #[serde(flatten)]
    pub arguments: Map<String, Value>,
    #[serde(default)]
    pub save_as: Option<String>,
    #[serde(default)]
    pub save_session_as: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub expect_error: Option<String>,
    #[serde(default)]
    #[serde(rename = "step_id")]
    pub step_id: Option<String>,
    #[serde(default)]
    pub redact: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScriptCase {
    pub name: String,
    #[serde(flatten)]
    pub values: Map<String, Value>,
}
fn default_true() -> bool {
    true
}

pub const SCRIPT_SCHEMA: &str = include_str!("../ai-ui-test-script.schema.json");

pub fn from_value(value: Value) -> Result<Script, Vec<String>> {
    let value = script_controls::normalize_script_controls(value)?;
    validate_schema_value(&value)?;
    let script: Script =
        serde_json::from_value(value).map_err(|error| vec![format!("invalid script: {error}")])?;
    validate(&script)?;
    Ok(script)
}

pub fn validate_schema_value(value: &Value) -> Result<(), Vec<String>> {
    let value = script_controls::normalize_script_controls(value.clone())?;
    let Some(object) = value.as_object() else {
        return Err(vec!["schema: script must be an object".into()]);
    };
    let allowed = [
        "version",
        "name",
        "application",
        "options",
        "vars",
        "setup",
        "steps",
        "teardown",
        "cases",
        "tags",
        "generated_by",
        "generated",
        "generator_version",
        "inventory_version",
        "generated_from",
        "intent",
        "regression",
        "secret_vars",
    ];
    let mut errors = object
        .keys()
        .filter(|key| !allowed.contains(&key.as_str()))
        .map(|key| format!("schema: unknown top-level property {key}"))
        .collect::<Vec<_>>();
    for required in ["version", "name", "application", "options", "steps"] {
        if !object.contains_key(required) {
            errors.push(format!("schema: missing required property {required}"));
        }
    }
    if object.get("version").and_then(Value::as_u64) != Some(1) {
        errors.push("schema: version must be integer 1".into());
    }
    if !object
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| !name.is_empty())
    {
        errors.push("schema: name must be a non-empty string".into());
    }
    for field in ["application", "options", "vars"] {
        if object.get(field).is_some_and(|value| !value.is_object()) {
            errors.push(format!("schema: {field} must be an object"));
        }
    }
    if let Some(application) = object.get("application").and_then(Value::as_object) {
        let allowed = [
            "command",
            "arguments",
            "window_title",
            "fixture_async_failure_mode",
        ];
        for key in application
            .keys()
            .filter(|key| !allowed.contains(&key.as_str()))
        {
            errors.push(format!("schema: unknown application property {key}"));
        }
        for field in ["command", "window_title"] {
            if application
                .get(field)
                .is_some_and(|value| !value.is_string() && !value.is_null())
            {
                errors.push(format!(
                    "schema: application.{field} must be a string or null"
                ));
            }
        }
        if let Some(arguments) = application.get("arguments") {
            match arguments.as_array() {
                Some(arguments) => {
                    for (index, argument) in arguments.iter().enumerate() {
                        match argument.as_str() {
                            Some(argument) if !argument.contains('\0') => {}
                            Some(_) => errors.push(format!(
                                "schema: application.arguments[{index}] must not contain a NUL character"
                            )),
                            None => errors.push(format!(
                                "schema: application.arguments[{index}] must be a string"
                            )),
                        }
                    }
                }
                None => {
                    errors.push("schema: application.arguments must be an array of strings".into())
                }
            }
        }
        if let Some(mode) = application
            .get("fixture_async_failure_mode")
            .filter(|value| !value.is_null())
        {
            match mode.as_str() {
                Some(mode) if is_fixture_async_failure_mode(mode) => {}
                Some(mode) => errors.push(format!(
                    "schema: unsupported application.fixture_async_failure_mode {mode}"
                )),
                None => errors.push(
                    "schema: application.fixture_async_failure_mode must be a string or null"
                        .into(),
                ),
            }
        }
    }
    for field in ["setup", "steps", "teardown", "cases", "tags", "secret_vars"] {
        if object.get(field).is_some_and(|value| !value.is_array()) {
            errors.push(format!("schema: {field} must be an array"));
        }
    }
    if object
        .get("steps")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        errors.push("schema: steps must not be empty".into());
    }
    for field in ["setup", "steps", "teardown"] {
        if let Some(steps) = object.get(field).and_then(Value::as_array) {
            for (index, step) in steps.iter().enumerate() {
                let Some(step) = step.as_object() else {
                    errors.push(format!("schema: {field}[{index}] must be an object"));
                    continue;
                };
                if !step
                    .get("op")
                    .and_then(Value::as_str)
                    .is_some_and(|operation| !operation.is_empty())
                {
                    errors.push(format!("schema: {field}[{index}].op must be a string"));
                }
                if step.get("redact").is_some_and(|value| !value.is_boolean()) {
                    errors.push(format!("schema: {field}[{index}].redact must be boolean"));
                }
            }
        }
    }
    for field in [
        "generated_by",
        "generator_version",
        "generated_from",
        "intent",
    ] {
        if object
            .get(field)
            .is_some_and(|value| !value.is_string() && !value.is_null())
        {
            errors.push(format!("schema: {field} must be a string or null"));
        }
    }
    if object
        .get("generated")
        .is_some_and(|value| !value.is_boolean())
    {
        errors.push("schema: generated must be boolean".into());
    }
    if object
        .get("inventory_version")
        .is_some_and(|value| !value.is_u64() && !value.is_null())
    {
        errors.push("schema: inventory_version must be an integer or null".into());
    }
    if let Some(regression) = object.get("regression").filter(|value| !value.is_null()) {
        let Some(regression) = regression.as_object() else {
            errors.push("schema: regression must be an object or null".into());
            return if errors.is_empty() {
                Ok(())
            } else {
                Err(errors)
            };
        };
        for required in ["bug_id", "description"] {
            if !regression
                .get(required)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
            {
                errors.push(format!(
                    "schema: regression.{required} must be a non-empty string"
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
pub fn load(path: impl AsRef<Path>) -> Result<Script, Vec<String>> {
    let text = fs::read_to_string(path.as_ref()).map_err(|e| vec![e.to_string()])?;
    let value: Value =
        serde_json::from_str(&text).map_err(|e| vec![format!("invalid JSON: {e}")])?;
    from_value(value)
}
pub fn validate(script: &Script) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if script.version != 1 {
        errors.push("version must be 1".into());
    }
    if script.name.trim().is_empty() {
        errors.push("name must not be empty".into());
    }
    if let Some(mode) = script.application.fixture_async_failure_mode.as_deref() {
        if !is_fixture_async_failure_mode(mode) {
            errors.push(format!("unsupported fixture_async_failure_mode: {mode}"));
        }
    }
    if script.generated {
        if script.generated_by.as_deref().is_none_or(str::is_empty) {
            errors.push("generated script requires generated_by".into());
        }
        if script
            .generator_version
            .as_deref()
            .is_none_or(str::is_empty)
        {
            errors.push("generated script requires generator_version".into());
        }
        if script.inventory_version.is_none() {
            errors.push("generated script requires inventory_version".into());
        }
        if script.generated_from.as_deref().is_none_or(str::is_empty) {
            errors.push("generated script requires generated_from".into());
        }
        if script.intent.as_deref().is_none_or(str::is_empty) {
            errors.push("generated script requires intent".into());
        }
    }
    if let Some(regression) = &script.regression {
        if regression.bug_id.trim().is_empty() || regression.description.trim().is_empty() {
            errors.push("regression bug_id and description must not be empty".into());
        }
    }
    let unique_secrets = script.secret_vars.iter().collect::<BTreeSet<_>>();
    if unique_secrets.len() != script.secret_vars.len() {
        errors.push("secret_vars must be unique".into());
    }
    if script.steps.is_empty() {
        errors.push("steps must not be empty".into());
    }
    let mut ids = BTreeSet::new();
    for (phase, steps) in [
        ("setup", &script.setup),
        ("steps", &script.steps),
        ("teardown", &script.teardown),
    ] {
        for (index, step) in steps.iter().enumerate() {
            if step.op.trim().is_empty() {
                errors.push(format!("{phase}[{index}].op is required"));
                continue;
            }
            if !known_operation(&step.op) {
                errors.push(format!("{phase}[{index}] unknown operation: {}", step.op));
            }
            if let Some(operation) = catalog::operation(&step.op) {
                for required in &operation.required {
                    if matches!(required.as_str(), "session_id" | "input_lock_token") {
                        continue;
                    }
                    if !step.arguments.contains_key(required) {
                        errors.push(format!("{phase}[{index}] {} requires {required}", step.op));
                    }
                }
            }
            for error in script_controls::validate_step_controls(&step.op, &step.arguments) {
                errors.push(format!("{phase}[{index}] {error}"));
            }
            if let Some(id) = &step.step_id {
                if !ids.insert(id) {
                    errors.push(format!("duplicate step id: {id}"));
                }
            }
            if step.op.ends_with("_relative")
                && !step.arguments.contains_key("position")
                && !step.arguments.contains_key("from")
            {
                errors.push(format!(
                    "{phase}[{index}] {} requires position or from",
                    step.op
                ));
            }
            if matches!(
                step.op.as_str(),
                "click"
                    | "click_once_and_wait_for_outcome"
                    | "set_value"
                    | "assert_visible"
                    | "assert_interactable"
                    | "wait_for_automation_id"
            ) && step.step_id.is_none()
                && !step.arguments.contains_key("id")
            {
                errors.push(format!("{phase}[{index}] {} requires id", step.op));
            }
            if step.op == "session.open" && step.save_session_as.is_none() && step.save_as.is_none()
            {
                errors.push(format!(
                    "{phase}[{index}] session.open requires save_session_as or save_as"
                ));
            }
            if step.op == "session.close"
                && step.session.is_none()
                && !step.arguments.contains_key("session_id")
            {
                errors.push(format!(
                    "{phase}[{index}] session.close requires session or session_id"
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
pub fn known_operation(op: &str) -> bool {
    catalog::operation(op).is_some()
}
pub fn operation_catalog() -> &'static catalog::OperationCatalog {
    catalog::operation_catalog()
}
pub fn substitute(value: &Value, vars: &Map<String, Value>) -> Value {
    match value {
        Value::String(text) if text.starts_with("${") && text.ends_with('}') => {
            resolve_reference(&text[2..text.len() - 1], vars).unwrap_or_else(|| value.clone())
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| substitute(v, vars)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), substitute(v, vars)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn resolve_reference(reference: &str, vars: &Map<String, Value>) -> Option<Value> {
    if let Some(value) = vars.get(reference) {
        return Some(value.clone());
    }

    let (root, path) = reference.split_once('.')?;
    let mut current = vars.get(root)?;
    for segment in path.split('.') {
        current = current.as_object()?.get(segment)?;
    }
    Some(current.clone())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn invalid_shape_is_rejected_before_execution() {
        let script = Script {
            version: 1,
            name: "bad".into(),
            application: Application::default(),
            options: ScriptOptions::default(),
            vars: Map::new(),
            setup: vec![],
            steps: vec![Step {
                op: "click_relative".into(),
                arguments: Map::from_iter([(String::from("x"), json!(45))]),
                save_as: None,
                save_session_as: None,
                session: None,
                depends_on: vec![],
                expect_error: None,
                step_id: None,
                redact: false,
            }],
            teardown: vec![],
            cases: vec![],
            tags: vec![],
            generated_by: None,
            generated: false,
            generator_version: None,
            inventory_version: None,
            generated_from: None,
            intent: None,
            regression: None,
            secret_vars: vec![],
        };
        assert!(validate(&script).is_err());
    }
    #[test]
    fn variables_substitute_recursively() {
        let vars = Map::from_iter([
            (String::from("user"), json!("alice")),
            (String::from("lock"), json!({"token":"lock-token"})),
        ]);
        assert_eq!(
            substitute(&json!({"value":"${user}"}), &vars)["value"],
            "alice"
        );
        assert_eq!(
            substitute(&json!({"token":"${lock.token}"}), &vars)["token"],
            "lock-token"
        );
    }
    #[test]
    fn advanced_spatial_and_keyboard_operations_are_declarative() {
        for operation in [
            "pointer_down",
            "pointer_up",
            "double_click_relative",
            "hover_relative",
            "wheel_relative",
            "hotkey",
            "wait_visible",
            "wait_idle",
        ] {
            assert!(known_operation(operation), "missing {operation}");
        }
    }

    #[test]
    fn fixture_async_mode_is_strict_and_schema_visible() {
        let base = |mode: Value| {
            json!({
                "version": 1,
                "name": "fixture-mode",
                "application": {"fixture_async_failure_mode": mode},
                "options": {},
                "steps": [{"op": "session.open", "save_session_as": "app"}]
            })
        };
        let valid = from_value(base(json!("file_never_appears"))).unwrap();
        assert_eq!(
            valid.application.fixture_async_failure_mode.as_deref(),
            Some("file_never_appears")
        );
        assert!(from_value(base(json!("arbitrary_shell_payload"))).is_err());
    }
}
