//! External test fixtures and report/evidence redaction.
//!
//! Generated scripts contain variable references, never fixture values. The
//! runner resolves those references immediately before invoking Public Tool API.

use crate::script::Script;
#[cfg(test)]
use crate::script::Step;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

pub const FIXTURES_VERSION: u32 = 1;
pub const REDACTED: &str = "[REDACTED]";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixtureStore {
    pub version: u32,
    #[serde(default)]
    pub fixtures: BTreeMap<String, FixtureValue>,
}

impl Default for FixtureStore {
    fn default() -> Self {
        Self {
            version: FIXTURES_VERSION,
            fixtures: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixtureValue {
    pub value: Value,
    #[serde(default)]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl FixtureStore {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let raw = fs::read_to_string(path.as_ref()).map_err(|error| error.to_string())?;
        let fixtures: Self =
            serde_json::from_str(&raw).map_err(|error| format!("invalid fixture file: {error}"))?;
        fixtures.validate()?;
        Ok(fixtures)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != FIXTURES_VERSION {
            return Err(format!("unsupported fixture version: {}", self.version));
        }
        if let Some(name) = self
            .fixtures
            .keys()
            .find(|name| name.trim().is_empty() || name.contains("${"))
        {
            return Err(format!("invalid fixture name: {name:?}"));
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&FixtureValue> {
        self.fixtures.get(name)
    }

    /// Merge only fixture variables referenced by the script.
    pub fn merge_for_script(
        &self,
        script: &Script,
        vars: &mut Map<String, Value>,
    ) -> Result<(), String> {
        let references = script_variable_references(script);
        let produced = produced_variables(script);
        let mut missing = Vec::new();
        for name in references {
            if vars.contains_key(&name) || produced.contains(&name) {
                continue;
            }
            if let Some(fixture) = self.fixtures.get(&name) {
                vars.insert(name, fixture.value.clone());
            } else if name
                .split_once('.')
                .is_some_and(|(root, _)| vars.contains_key(root) || produced.contains(root))
            {
                // A property of a value saved by an earlier step is resolved at runtime.
                continue;
            } else {
                missing.push(name);
            }
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!("missing fixture variables: {}", missing.join(", ")))
        }
    }

    pub fn secret_variable_names(&self, script: &Script) -> BTreeSet<String> {
        let mut names = script.secret_vars.iter().cloned().collect::<BTreeSet<_>>();
        names.extend(
            self.fixtures
                .iter()
                .filter(|(_, fixture)| fixture.secret)
                .map(|(name, _)| name.clone()),
        );
        names
    }

    pub fn secret_literals(&self, script: &Script, vars: &Map<String, Value>) -> Vec<String> {
        let mut values = BTreeSet::new();
        for name in self.secret_variable_names(script) {
            if let Some(value) = vars
                .get(&name)
                .or_else(|| self.fixtures.get(&name).map(|fixture| &fixture.value))
            {
                collect_string_literals(value, &mut values);
            }
        }
        let mut values = values
            .into_iter()
            .filter(|value| !value.is_empty() && value != REDACTED)
            .collect::<Vec<_>>();
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        values
    }
}

pub fn script_variable_references(script: &Script) -> BTreeSet<String> {
    let mut references = BTreeSet::new();
    for step in script
        .setup
        .iter()
        .chain(script.steps.iter())
        .chain(script.teardown.iter())
    {
        for value in step.arguments.values() {
            collect_references(value, &mut references);
        }
    }
    references
}

fn produced_variables(script: &Script) -> BTreeSet<String> {
    script
        .setup
        .iter()
        .chain(script.steps.iter())
        .chain(script.teardown.iter())
        .flat_map(|step| {
            let saved = step
                .save_as
                .iter()
                .chain(step.save_session_as.iter())
                .cloned();
            let extracted = step
                .arguments
                .get("extract")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|specification| specification.keys().cloned());
            saved.chain(extracted)
        })
        .collect()
}

fn collect_references(value: &Value, references: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            if let Some(name) = exact_reference(text) {
                references.insert(name.to_owned());
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_references(value, references);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_references(value, references);
            }
        }
        _ => {}
    }
}

pub fn exact_reference(text: &str) -> Option<&str> {
    text.strip_prefix("${")?.strip_suffix('}')
}

fn collect_string_literals(value: &Value, values: &mut BTreeSet<String>) {
    match value {
        Value::String(value) => {
            values.insert(value.clone());
        }
        Value::Array(items) => {
            for item in items {
                collect_string_literals(item, values);
            }
        }
        Value::Object(items) => {
            for item in items.values() {
                collect_string_literals(item, values);
            }
        }
        _ => {}
    }
}

pub fn contains_secret(value: &Value, literals: &[String]) -> bool {
    match value {
        Value::String(value) => literals
            .iter()
            .any(|literal| !literal.is_empty() && value.contains(literal)),
        Value::Array(values) => values.iter().any(|value| contains_secret(value, literals)),
        Value::Object(values) => values
            .values()
            .any(|value| contains_secret(value, literals)),
        _ => false,
    }
}

pub fn secret_argument_fields(arguments: &Map<String, Value>, literals: &[String]) -> Vec<String> {
    arguments
        .iter()
        .filter(|(_, value)| contains_secret(value, literals))
        .map(|(name, _)| name.clone())
        .collect()
}

pub fn redact_value(value: &Value, literals: &[String], sensitive_fields: bool) -> Value {
    match value {
        Value::String(text) => {
            let mut redacted = text.clone();
            for literal in literals {
                if !literal.is_empty() {
                    redacted = redacted.replace(literal, REDACTED);
                }
            }
            Value::String(redacted)
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| redact_value(value, literals, sensitive_fields))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    let value = if sensitive_fields && is_sensitive_key(key) {
                        redact_leaf(value)
                    } else {
                        redact_value(value, literals, sensitive_fields)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn redact_leaf(value: &Value) -> Value {
    match value {
        Value::Null => Value::Null,
        Value::Array(values) => Value::Array(values.iter().map(redact_leaf).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), redact_leaf(value)))
                .collect(),
        ),
        _ => Value::String(REDACTED.into()),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "value" | "text" | "password" | "token" | "secret" | "input"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{Application, ScriptOptions};
    use serde_json::{Map, json};

    fn script() -> Script {
        Script {
            version: 1,
            name: "fixture".into(),
            application: Application::default(),
            options: ScriptOptions::default(),
            vars: Map::new(),
            setup: vec![],
            steps: vec![Step {
                op: "set_value".into(),
                arguments: Map::from_iter([
                    ("id".into(), json!("login.password")),
                    ("value".into(), json!("${credentials.password}")),
                ]),
                save_as: None,
                save_session_as: None,
                session: None,
                depends_on: vec![],
                expect_error: None,
                step_id: None,
                redact: true,
            }],
            teardown: vec![],
            cases: vec![],
            tags: vec![],
            generated_by: None,
            generated: true,
            generator_version: Some("1".into()),
            inventory_version: Some(1),
            generated_from: Some("inventory.json".into()),
            intent: None,
            regression: None,
            secret_vars: vec!["credentials.password".into()],
        }
    }

    #[test]
    fn resolves_only_referenced_fixture_and_redacts_nested_output() {
        let fixtures = FixtureStore {
            version: 1,
            fixtures: BTreeMap::from_iter([
                (
                    "credentials.password".into(),
                    FixtureValue {
                        value: json!("s3cr3t"),
                        secret: true,
                        description: None,
                    },
                ),
                (
                    "unused".into(),
                    FixtureValue {
                        value: json!("do not merge"),
                        secret: false,
                        description: None,
                    },
                ),
            ]),
        };
        let script = script();
        let mut vars = script.vars.clone();
        fixtures.merge_for_script(&script, &mut vars).unwrap();
        assert_eq!(vars.len(), 1);
        let literals = fixtures.secret_literals(&script, &vars);
        let redacted = redact_value(
            &json!({"result":{"value":"prefix-s3cr3t-suffix"}}),
            &literals,
            false,
        );
        assert_eq!(redacted["result"]["value"], "prefix-[REDACTED]-suffix");
        assert!(!serde_json::to_string(&redacted).unwrap().contains("s3cr3t"));
    }

    #[test]
    fn saved_result_property_is_not_treated_as_missing_fixture() {
        let mut script = script();
        script.steps = vec![
            Step {
                op: "acquire_input_lock".into(),
                arguments: Map::new(),
                save_as: Some("lock".into()),
                save_session_as: None,
                session: None,
                depends_on: vec![],
                expect_error: None,
                step_id: None,
                redact: false,
            },
            Step {
                op: "release_input_lock".into(),
                arguments: Map::from_iter([("token".into(), json!("${lock.token}"))]),
                save_as: None,
                save_session_as: None,
                session: None,
                depends_on: vec![],
                expect_error: None,
                step_id: None,
                redact: false,
            },
        ];
        script.secret_vars.clear();
        let mut vars = script.vars.clone();

        FixtureStore::default()
            .merge_for_script(&script, &mut vars)
            .unwrap();
        assert!(vars.is_empty());
    }

    #[test]
    fn extracted_value_is_not_treated_as_missing_fixture() {
        let mut script = script();
        script.steps = vec![
            Step {
                op: "screenshot_window".into(),
                arguments: Map::from_iter([("extract".into(), json!({"baseline_path":"$.path"}))]),
                save_as: None,
                save_session_as: None,
                session: None,
                depends_on: vec![],
                expect_error: None,
                step_id: None,
                redact: false,
            },
            Step {
                op: "assert_region_changed".into(),
                arguments: Map::from_iter([("baseline_path".into(), json!("${baseline_path}"))]),
                save_as: None,
                save_session_as: None,
                session: None,
                depends_on: vec![],
                expect_error: None,
                step_id: None,
                redact: false,
            },
        ];
        script.secret_vars.clear();
        let mut vars = script.vars.clone();

        FixtureStore::default()
            .merge_for_script(&script, &mut vars)
            .unwrap();
        assert!(vars.is_empty());
    }
}
