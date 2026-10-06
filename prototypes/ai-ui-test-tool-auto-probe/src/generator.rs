//! Deterministic capability- and safety-driven test planning and DSL compilation.

use crate::{
    catalog,
    coverage::UiCoverage,
    fixtures::FixtureStore,
    inventory::{ElementHint, InventoryOptions, SafetyClass, UiElement, UiInventory},
    script::{Application, Script, ScriptOptions, Step},
    templates::{self, TestTemplate},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

pub const GENERATOR_VERSION: &str = "1.1.0";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeneratorMode {
    Smoke,
    #[default]
    Standard,
    Exhaustive,
}

impl FromStr for GeneratorMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "smoke" => Ok(Self::Smoke),
            "standard" => Ok(Self::Standard),
            "exhaustive" => Ok(Self::Exhaustive),
            _ => Err(format!("invalid generator mode: {value}")),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeneratorConfig {
    #[serde(default = "default_true")]
    pub allow_stateful: bool,
    #[serde(default)]
    pub allow_destructive: bool,
    #[serde(default)]
    pub allow_unknown_mutation: bool,
    #[serde(default)]
    pub allow_stateful_spatial: bool,
    /// Explicit opt-in: generated scripts may acquire the interactive desktop
    /// for real title/frame/state gestures.
    #[serde(default)]
    pub generate_window_physical: bool,
    #[serde(default = "default_true")]
    pub generate_negative: bool,
    #[serde(default = "default_true")]
    pub generate_spatial: bool,
    #[serde(default = "default_cases")]
    pub max_cases_per_control: usize,
    #[serde(default = "default_tests")]
    pub max_generated_tests: usize,
    #[serde(default = "default_per_element")]
    pub max_tests_per_element: usize,
    #[serde(default)]
    pub regenerate_all: bool,
    #[serde(default)]
    pub element_hints: BTreeMap<String, GeneratorElementHint>,
    /// Opt-in trusted metadata for deterministic async observation fixtures.
    #[serde(default)]
    pub observation_workflow: Option<ObservationWorkflowHint>,
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        Self {
            allow_stateful: true,
            allow_destructive: false,
            allow_unknown_mutation: false,
            allow_stateful_spatial: false,
            generate_window_physical: false,
            generate_negative: true,
            generate_spatial: true,
            max_cases_per_control: 4,
            max_generated_tests: 100,
            max_tests_per_element: 4,
            regenerate_all: false,
            element_hints: BTreeMap::new(),
            observation_workflow: None,
        }
    }
}

impl GeneratorConfig {
    pub fn inventory_options(&self) -> InventoryOptions {
        InventoryOptions {
            element_hints: self
                .element_hints
                .iter()
                .map(|(id, hint)| {
                    (
                        id.clone(),
                        ElementHint {
                            safety: hint.safety.clone(),
                            dynamic: hint.dynamic,
                            transient: hint.transient,
                        },
                    )
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GeneratorElementHint {
    #[serde(default)]
    pub safety: Option<SafetyClass>,
    #[serde(default)]
    pub dynamic: Option<bool>,
    #[serde(default)]
    pub transient: Option<bool>,
    #[serde(default)]
    pub postcondition: Option<Value>,
    #[serde(default)]
    pub fixture: Option<String>,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub approved_destructive: bool,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub failure_fixture: bool,
    #[serde(default)]
    pub modal_id: Option<String>,
    #[serde(default)]
    pub close_id: Option<String>,
    #[serde(default)]
    pub context_menu_id: Option<String>,
}

/// Trusted fixture metadata used to generate one bounded async observation
/// workflow. Values are data only; they cannot configure arbitrary transport
/// or filesystem access.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ObservationWorkflowHint {
    pub start_id: String,
    pub task_id: String,
    pub request_id: String,
    pub output_root: String,
    pub output_path: String,
    pub milestone: String,
    pub trace_event: String,
}

impl ObservationWorkflowHint {
    fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("start_id", &self.start_id),
            ("task_id", &self.task_id),
            ("request_id", &self.request_id),
            ("output_root", &self.output_root),
            ("output_path", &self.output_path),
            ("milestone", &self.milestone),
            ("trace_event", &self.trace_event),
        ] {
            if value.trim().is_empty() {
                return Err(format!("observation_workflow.{name} must be non-empty"));
            }
        }
        if !matches!(
            self.output_root.as_str(),
            "workspace" | "fixture" | "output" | "temp"
        ) {
            return Err(
                "observation_workflow.output_root must be a trusted file root alias".into(),
            );
        }
        if self.output_path.contains("..")
            || self.output_path.starts_with(['/', '\\'])
            || self.output_path.contains(':')
        {
            return Err("observation_workflow.output_path must stay below its trusted root".into());
        }
        Ok(())
    }

    fn parameters(&self) -> Map<String, Value> {
        Map::from_iter([
            ("start_id".into(), Value::String(self.start_id.clone())),
            ("task_id".into(), Value::String(self.task_id.clone())),
            ("request_id".into(), Value::String(self.request_id.clone())),
            (
                "output_root".into(),
                Value::String(self.output_root.clone()),
            ),
            (
                "output_path".into(),
                Value::String(self.output_path.clone()),
            ),
            ("milestone".into(), Value::String(self.milestone.clone())),
            (
                "trace_event".into(),
                Value::String(self.trace_event.clone()),
            ),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestIntent {
    pub id: String,
    pub intent: String,
    pub target: String,
    pub template: String,
    pub class: String,
    pub safety: SafetyClass,
    pub reason: String,
    #[serde(default)]
    pub parameters: Map<String, Value>,
    #[serde(default)]
    pub dynamic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedIntent {
    pub target: String,
    pub template: String,
    pub safety: SafetyClass,
    pub code: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureRequirement {
    pub element: String,
    pub fixture: String,
    pub secret: bool,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestPlan {
    pub version: u32,
    pub generator_version: String,
    pub inventory_version: u32,
    pub mode: GeneratorMode,
    pub planned: usize,
    pub safe: usize,
    pub requires_fixture: usize,
    pub blocked_destructive: usize,
    pub intents: Vec<TestIntent>,
    pub blocked: Vec<BlockedIntent>,
    pub fixture_requirements: Vec<FixtureRequirement>,
    pub markers: Vec<String>,
}

fn default_true() -> bool {
    true
}
fn default_cases() -> usize {
    4
}
fn default_tests() -> usize {
    100
}
fn default_per_element() -> usize {
    4
}

pub fn load_config(path: impl AsRef<Path>) -> Result<GeneratorConfig, String> {
    let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&raw).map_err(|error| format!("invalid generator config: {error}"))
}

pub fn build_plan(
    inventory: &UiInventory,
    coverage: &UiCoverage,
    config: &GeneratorConfig,
    mode: GeneratorMode,
) -> Result<TestPlan, Vec<String>> {
    templates::validate_registry()?;
    let representatives = inventory
        .collections
        .iter()
        .flat_map(|family| family.representatives.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut plan = TestPlan {
        version: 1,
        generator_version: GENERATOR_VERSION.into(),
        inventory_version: inventory.version,
        mode,
        planned: 0,
        safe: 0,
        requires_fixture: 0,
        blocked_destructive: 0,
        intents: Vec::new(),
        blocked: Vec::new(),
        fixture_requirements: Vec::new(),
        markers: vec![],
    };
    for element in &inventory.elements {
        if element.automation_id.is_empty() {
            continue;
        }
        if element.collection_family.is_some() && !representatives.contains(&element.automation_id)
        {
            continue;
        }
        plan_element(&mut plan, inventory, coverage, config, mode, element);
        if plan.intents.len() >= config.max_generated_tests {
            break;
        }
    }
    if mode != GeneratorMode::Smoke
        && !inventory.windows.is_empty()
        && (config.regenerate_all || !global_is_planned(coverage, "window.resize"))
    {
        consider_global(&mut plan, config, "window.resize", "stability");
    }
    if mode != GeneratorMode::Smoke
        && !inventory.windows.is_empty()
        && (config.regenerate_all || !global_is_planned(coverage, "window.interaction"))
    {
        consider_global(&mut plan, config, "window.interaction", "stability");
    }
    if mode != GeneratorMode::Smoke
        && !inventory.windows.is_empty()
        && config.generate_window_physical
    {
        for template in ["window.move", "window.frame_resize", "window.state"] {
            consider_global(&mut plan, config, template, "stability");
        }
    }
    if config.regenerate_all || !global_is_planned(coverage, "runtime.health") {
        consider_global(&mut plan, config, "runtime.health", "smoke");
    }
    if let Some(workflow) = &config.observation_workflow {
        workflow.validate().map_err(|error| vec![error])?;
        for (template, capability) in [
            ("runtime.task", "runtime.task"),
            ("external.request", "external.request"),
            ("file.output", "file.output"),
            ("trace.milestone", "trace.milestone"),
            ("visual.region", "visual.region"),
        ] {
            if config.regenerate_all || !global_is_planned(coverage, capability) {
                consider_global_with_parameters(
                    &mut plan,
                    config,
                    template,
                    "observation",
                    workflow.parameters(),
                );
            }
        }
    }
    plan.intents.sort_by(|left, right| {
        left.class
            .cmp(&right.class)
            .then(left.target.cmp(&right.target))
            .then(left.template.cmp(&right.template))
    });
    plan.blocked.sort_by(|left, right| {
        left.target
            .cmp(&right.target)
            .then(left.template.cmp(&right.template))
    });
    plan.planned = plan.intents.len() + plan.blocked.len() + plan.fixture_requirements.len();
    plan.safe = plan.intents.len();
    plan.requires_fixture = plan.fixture_requirements.len();
    plan.blocked_destructive = plan
        .blocked
        .iter()
        .filter(|blocked| blocked.safety == SafetyClass::PotentiallyDestructive)
        .count();
    plan.markers.push("AI_UI_TEST_PLAN_PASSED".into());
    plan.markers.push("AI_UI_GENERATOR_SAFETY_PASSED".into());
    Ok(plan)
}

pub fn resolve_fixtures(
    inventory: &UiInventory,
    plan: &mut TestPlan,
    config: &GeneratorConfig,
    fixtures: &FixtureStore,
) -> Result<(), Vec<String>> {
    let requirements = std::mem::take(&mut plan.fixture_requirements);
    let mut remaining = Vec::new();
    let mut errors = Vec::new();
    for requirement in requirements {
        let Some(fixture) = fixtures.get(&requirement.fixture) else {
            remaining.push(requirement);
            continue;
        };
        let Some(element) = inventory
            .elements
            .iter()
            .find(|element| element.automation_id == requirement.element)
        else {
            errors.push(format!(
                "UNKNOWN_AUTOMATION_ID for fixture {}: {}",
                requirement.fixture, requirement.element
            ));
            continue;
        };
        if !config.allow_stateful {
            plan.blocked.push(BlockedIntent {
                target: element.automation_id.clone(),
                template: template_for_capability(element, "value").into(),
                safety: SafetyClass::StatefulRecoverable,
                code: "STATEFUL_DISABLED".into(),
                reason: requirement.reason,
            });
            continue;
        }
        if !fixture.value.is_string() {
            errors.push(format!(
                "fixture {} for {} must be a string",
                requirement.fixture, requirement.element
            ));
            remaining.push(requirement);
            continue;
        }
        let reference = format!("${{{}}}", requirement.fixture);
        let template = if element.control_type.eq_ignore_ascii_case("ComboBox") {
            "dropdown.choose"
        } else {
            "edit.value"
        };
        let mut parameters = Map::new();
        parameters.insert("fixture".into(), Value::String(requirement.fixture.clone()));
        parameters.insert(
            "secret".into(),
            Value::Bool(requirement.secret || fixture.secret),
        );
        if template == "dropdown.choose" {
            parameters.insert("values".into(), json!([reference]));
        } else {
            parameters.insert("cases".into(), json!([reference]));
            parameters.insert(
                "original_value".into(),
                Value::String(element.value.clone().unwrap_or_default()),
            );
        }
        let hint = config.element_hints.get(&element.automation_id);
        let failure_fixture = hint.is_some_and(|hint| hint.failure_fixture);
        if failure_fixture {
            parameters.insert("failure_fixture".into(), Value::Bool(true));
        }
        let class = if failure_fixture {
            "regression"
        } else if template == "dropdown.choose" {
            "state"
        } else {
            "interaction"
        };
        let intent = TestIntent {
            id: format!("{template}:{}", element.automation_id),
            intent: format!(
                "Verify {template} for {} using application fixture {}",
                element.automation_id, requirement.fixture
            ),
            target: element.automation_id.clone(),
            template: template.into(),
            class: class.into(),
            safety: SafetyClass::StatefulRecoverable,
            reason: format!(
                "{} is now testable because fixture {} was supplied",
                element.automation_id, requirement.fixture
            ),
            parameters,
            dynamic: element.dynamic,
        };
        if !plan.intents.iter().any(|candidate| {
            candidate.target == intent.target && candidate.template == intent.template
        }) {
            plan.intents.push(intent);
        }
    }
    plan.fixture_requirements = remaining;
    plan.intents.sort_by(|left, right| {
        left.class
            .cmp(&right.class)
            .then(left.target.cmp(&right.target))
            .then(left.template.cmp(&right.template))
    });
    plan.requires_fixture = plan.fixture_requirements.len();
    plan.safe = plan.intents.len();
    plan.planned = plan.intents.len() + plan.blocked.len() + plan.fixture_requirements.len();
    if !plan
        .markers
        .iter()
        .any(|marker| marker == "AI_UI_FIXTURES_RESOLVED")
    {
        plan.markers.push("AI_UI_FIXTURES_RESOLVED".into());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
fn plan_element(
    plan: &mut TestPlan,
    inventory: &UiInventory,
    coverage: &UiCoverage,
    config: &GeneratorConfig,
    mode: GeneratorMode,
    element: &UiElement,
) {
    let mut count = 0usize;
    if !coverage_for(coverage, &element.automation_id).is_some_and(|coverage| coverage.discovery) {
        consider(
            plan,
            config,
            element,
            "discovery.semantic",
            "discovery",
            SafetyClass::Safe,
            Map::new(),
            format!(
                "{} is a stable semantic element with no discovery coverage",
                element.automation_id
            ),
        );
        count += 1;
    }
    if count >= config.max_tests_per_element {
        return;
    }
    if element.visible && element.offscreen && mode != GeneratorMode::Smoke {
        consider(
            plan,
            config,
            element,
            "scroll.ensure_visible",
            "interaction",
            SafetyClass::Safe,
            Map::new(),
            format!(
                "{} is outside the viewport and needs safe scroll coverage",
                element.automation_id
            ),
        );
        count += 1;
    }
    if count >= config.max_tests_per_element || !element.visible {
        return;
    }
    let hint = config.element_hints.get(&element.automation_id);
    for capability in element
        .capabilities
        .iter()
        .chain(element.spatial_capabilities.iter())
        .filter(|capability| capability.supported)
    {
        if count >= config.max_tests_per_element || plan.intents.len() >= config.max_generated_tests
        {
            break;
        }
        if !config.regenerate_all
            && coverage_for(coverage, &element.automation_id).is_some_and(|coverage| {
                coverage
                    .capabilities
                    .iter()
                    .any(|item| item.capability == capability.name && item.planned)
            })
        {
            continue;
        }
        if !capability.available {
            plan.blocked.push(BlockedIntent {
                target: element.automation_id.clone(),
                template: template_for_capability(element, &capability.name).into(),
                safety: element.safety.clone(),
                code: "CAPABILITY_CURRENTLY_UNAVAILABLE".into(),
                reason: format!(
                    "{} is supported but not currently available",
                    capability.name
                ),
            });
            continue;
        }
        let mut parameters = Map::new();
        parameters.insert("capability".into(), Value::String(capability.name.clone()));
        let candidate = match capability.name.as_str() {
            "invoke" | "native_action" => {
                let explicit_modal =
                    hint.and_then(|hint| hint.modal_id.clone().zip(hint.close_id.clone()));
                if let Some((modal, close)) =
                    explicit_modal.or_else(|| modal_relationship(inventory, element))
                {
                    parameters.insert("modal_id".into(), Value::String(modal));
                    parameters.insert("close_id".into(), Value::String(close));
                    Some((
                        "modal.lifecycle",
                        "interaction",
                        SafetyClass::StatefulRecoverable,
                    ))
                } else if let Some(menu) = hint.and_then(|hint| hint.context_menu_id.clone()) {
                    parameters.insert("menu_id".into(), Value::String(menu));
                    Some((
                        "context_menu.lifecycle",
                        "interaction",
                        SafetyClass::StatefulRecoverable,
                    ))
                } else {
                    let postcondition = hint.and_then(|hint| hint.postcondition.clone());
                    let failure_fixture = hint.is_some_and(|hint| hint.failure_fixture);
                    if failure_fixture && postcondition.is_none() {
                        plan.blocked.push(BlockedIntent {
                            target: element.automation_id.clone(),
                            template: "button.invoke".into(),
                            safety: element.safety.clone(),
                            code: "FAILURE_FIXTURE_REQUIRES_POSTCONDITION".into(),
                            reason: "failure fixtures must declare an observable postcondition"
                                .into(),
                        });
                        None
                    } else {
                        if let Some(postcondition) = postcondition {
                            parameters.insert("postcondition".into(), postcondition);
                        }
                        if failure_fixture {
                            parameters.insert("failure_fixture".into(), Value::Bool(true));
                        }
                        Some((
                            "button.invoke",
                            if failure_fixture {
                                "regression"
                            } else {
                                "interaction"
                            },
                            element.safety.clone(),
                        ))
                    }
                }
            }
            "value" if element.control_type.eq_ignore_ascii_case("ComboBox") => {
                let values = hint.map(|hint| hint.values.clone()).unwrap_or_default();
                if values.is_empty() {
                    require_fixture(
                        plan,
                        element,
                        hint.and_then(|hint| hint.fixture.as_deref())
                            .unwrap_or("dropdown_value"),
                        hint.is_some_and(|hint| hint.secret),
                        "available values are unknown; generator will not guess",
                    );
                    None
                } else {
                    parameters.insert("values".into(), json!(values));
                    Some(("dropdown.choose", "state", SafetyClass::StatefulRecoverable))
                }
            }
            "value" if element.classification == crate::inventory::InteractiveClass::Editable => {
                if let Some(fixture) = required_fixture_name(element, hint) {
                    require_fixture(
                        plan,
                        element,
                        &fixture,
                        hint.is_some_and(|hint| hint.secret) || is_secret_element(element),
                        "editable control requires an application-provided fixture",
                    );
                    None
                } else {
                    parameters.insert(
                        "original_value".into(),
                        Value::String(element.value.clone().unwrap_or_default()),
                    );
                    parameters.insert("cases".into(), json!(edit_cases(mode, config)));
                    Some((
                        "edit.value",
                        "interaction",
                        SafetyClass::StatefulRecoverable,
                    ))
                }
            }
            "select" if element.control_type.eq_ignore_ascii_case("CheckBox") => {
                if let Some(selected) = element.selected {
                    parameters.insert("selected".into(), Value::Bool(selected));
                    Some(("checkbox.toggle", "state", SafetyClass::StatefulRecoverable))
                } else {
                    Some((
                        "selection.single",
                        "selection",
                        SafetyClass::StatefulRecoverable,
                    ))
                }
            }
            "select" if element.control_type.eq_ignore_ascii_case("ComboBox") => None,
            "select" => Some((
                "selection.single",
                "selection",
                SafetyClass::StatefulRecoverable,
            )),
            "select.multiple" => {
                let representatives = collection_representatives(inventory, element);
                if representatives.len() >= 2
                    && representatives.first() == Some(&element.automation_id)
                {
                    let selected_ids = inventory
                        .elements
                        .iter()
                        .filter(|candidate| {
                            representatives.contains(&candidate.automation_id)
                                && candidate.selected == Some(true)
                        })
                        .map(|candidate| candidate.automation_id.clone())
                        .collect::<Vec<_>>();
                    parameters.insert("ids".into(), json!(representatives));
                    parameters.insert("selected_ids".into(), json!(selected_ids));
                    Some((
                        "selection.multiple",
                        "selection",
                        SafetyClass::StatefulRecoverable,
                    ))
                } else {
                    None
                }
            }
            "select.range" => {
                let representatives = collection_representatives(inventory, element);
                if representatives.len() >= 2
                    && representatives.first() == Some(&element.automation_id)
                {
                    parameters.insert(
                        "start_id".into(),
                        Value::String(representatives.first().cloned().unwrap_or_default()),
                    );
                    parameters.insert(
                        "end_id".into(),
                        Value::String(representatives.last().cloned().unwrap_or_default()),
                    );
                    let selected_ids = inventory
                        .elements
                        .iter()
                        .filter(|candidate| {
                            representatives.contains(&candidate.automation_id)
                                && candidate.selected == Some(true)
                        })
                        .map(|candidate| candidate.automation_id.clone())
                        .collect::<Vec<_>>();
                    parameters.insert("selected_ids".into(), json!(selected_ids));
                    Some((
                        "selection.range",
                        "selection",
                        SafetyClass::StatefulRecoverable,
                    ))
                } else {
                    None
                }
            }
            "spatial.capture" if config.generate_spatial => {
                Some(("spatial.health", "spatial", SafetyClass::Safe))
            }
            "spatial.click" if config.generate_spatial && config.allow_stateful_spatial => {
                parameters.insert("position".into(), json!({"x":0.5,"y":0.5}));
                Some(("spatial.click", "spatial", SafetyClass::StatefulRecoverable))
            }
            "spatial.drag" if config.generate_spatial && config.allow_stateful_spatial => {
                parameters.insert("from".into(), json!({"x":0.25,"y":0.25}));
                parameters.insert("to".into(), json!({"x":0.75,"y":0.75}));
                Some(("spatial.drag", "spatial", SafetyClass::StatefulRecoverable))
            }
            "spatial.select_region" if config.generate_spatial && config.allow_stateful_spatial => {
                parameters.insert("from".into(), json!({"x":0.25,"y":0.25}));
                parameters.insert("to".into(), json!({"x":0.75,"y":0.75}));
                Some((
                    "spatial.select_region",
                    "spatial",
                    SafetyClass::StatefulRecoverable,
                ))
            }
            _ => None,
        };
        if let Some((template, class, safety)) = candidate {
            consider(
                plan,
                config,
                element,
                template,
                class,
                safety,
                parameters,
                format!(
                    "{} exposes available {} capability with no existing coverage",
                    element.automation_id, capability.name
                ),
            );
            count += 1;
        }
        if config.generate_negative
            && capability.name == "spatial.click"
            && mode != GeneratorMode::Smoke
            && count < config.max_tests_per_element
        {
            consider(
                plan,
                config,
                element,
                "negative.relative.invalid",
                "negative",
                SafetyClass::Safe,
                Map::new(),
                format!(
                    "{} supports relative coordinates and needs boundary rejection coverage",
                    element.automation_id
                ),
            );
            count += 1;
        }
    }
}

fn consider(
    plan: &mut TestPlan,
    config: &GeneratorConfig,
    element: &UiElement,
    template_id: &str,
    class: &str,
    proposed_safety: SafetyClass,
    parameters: Map<String, Value>,
    reason: String,
) {
    if plan
        .intents
        .iter()
        .any(|intent| intent.target == element.automation_id && intent.template == template_id)
    {
        return;
    }
    let Some(template) = templates::template(template_id) else {
        plan.blocked.push(BlockedIntent {
            target: element.automation_id.clone(),
            template: template_id.into(),
            safety: SafetyClass::Unknown,
            code: "UNKNOWN_TEMPLATE".into(),
            reason: "template is not in the declarative registry".into(),
        });
        return;
    };
    let safety = effective_safety(&element.safety, &proposed_safety, template);
    let hint = config.element_hints.get(&element.automation_id);
    let allowed = match safety {
        SafetyClass::Safe => true,
        SafetyClass::StatefulRecoverable => config.allow_stateful,
        SafetyClass::PotentiallyDestructive => {
            config.allow_destructive && hint.is_some_and(|hint| hint.approved_destructive)
        }
        SafetyClass::Unknown => config.allow_unknown_mutation,
    };
    if !allowed {
        plan.blocked.push(BlockedIntent {
            target: element.automation_id.clone(),
            template: template_id.into(),
            safety: safety.clone(),
            code: if safety == SafetyClass::PotentiallyDestructive {
                "REQUIRES_EXPLICIT_APPROVAL_FIXTURE".into()
            } else {
                "UNSAFE_OR_UNKNOWN_MUTATION".into()
            },
            reason,
        });
        return;
    }
    plan.intents.push(TestIntent {
        id: format!("{template_id}:{}", element.automation_id),
        intent: format!("Verify {template_id} for {}", element.automation_id),
        target: element.automation_id.clone(),
        template: template_id.into(),
        class: class.into(),
        safety,
        reason,
        parameters,
        dynamic: element.dynamic,
    });
}

fn consider_global(plan: &mut TestPlan, config: &GeneratorConfig, template_id: &str, class: &str) {
    let Some(template) = templates::template(template_id) else {
        return;
    };
    if template.safety == SafetyClass::StatefulRecoverable && !config.allow_stateful {
        return;
    }
    plan.intents.push(TestIntent {
        id: template_id.into(),
        intent: format!("Verify {template_id}"),
        target: String::new(),
        template: template_id.into(),
        class: class.into(),
        safety: template.safety.clone(),
        reason: "global application capability is part of the selected generator mode".into(),
        parameters: Map::new(),
        dynamic: false,
    });
}

fn consider_global_with_parameters(
    plan: &mut TestPlan,
    config: &GeneratorConfig,
    template_id: &str,
    class: &str,
    parameters: Map<String, Value>,
) {
    let Some(template) = templates::template(template_id) else {
        return;
    };
    if template.safety == SafetyClass::StatefulRecoverable && !config.allow_stateful {
        return;
    }
    plan.intents.push(TestIntent {
        id: template_id.into(),
        intent: format!("Verify {template_id}"),
        target: String::new(),
        template: template_id.into(),
        class: class.into(),
        safety: template.safety.clone(),
        reason: "trusted observation workflow metadata is available for this application".into(),
        parameters,
        dynamic: false,
    });
}

fn global_is_planned(coverage: &UiCoverage, capability: &str) -> bool {
    coverage
        .global_capabilities
        .iter()
        .any(|item| item.capability == capability && item.planned)
}

fn effective_safety(
    element: &SafetyClass,
    proposed: &SafetyClass,
    template: &TestTemplate,
) -> SafetyClass {
    if proposed == &SafetyClass::Safe && template.safety == SafetyClass::Safe {
        return SafetyClass::Safe;
    }
    if element == &SafetyClass::PotentiallyDestructive {
        SafetyClass::PotentiallyDestructive
    } else if element == &SafetyClass::Unknown && template.safety != SafetyClass::Safe {
        SafetyClass::Unknown
    } else {
        proposed.clone()
    }
}

fn template_for_capability(element: &UiElement, capability: &str) -> &'static str {
    match capability {
        "invoke" | "native_action" => "button.invoke",
        "value" if element.control_type.eq_ignore_ascii_case("ComboBox") => "dropdown.choose",
        "value" if element.classification == crate::inventory::InteractiveClass::Editable => {
            "edit.value"
        }
        "value" => "discovery.semantic",
        "select" if element.control_type.eq_ignore_ascii_case("CheckBox") => "checkbox.toggle",
        "select" if element.control_type.eq_ignore_ascii_case("ComboBox") => "dropdown.choose",
        "select" => "selection.single",
        "select.multiple" => "selection.multiple",
        "select.range" => "selection.range",
        "spatial.capture" => "spatial.health",
        "spatial.click" => "spatial.click",
        "spatial.drag" => "spatial.drag",
        "spatial.select_region" => "spatial.select_region",
        _ => "discovery.semantic",
    }
}

fn coverage_for<'a>(
    coverage: &'a UiCoverage,
    automation_id: &str,
) -> Option<&'a crate::coverage::ElementCoverage> {
    coverage
        .elements
        .iter()
        .find(|coverage| coverage.automation_id == automation_id)
}

fn collection_representatives(inventory: &UiInventory, element: &UiElement) -> Vec<String> {
    let Some(family) = element.collection_family.as_deref() else {
        return Vec::new();
    };
    inventory
        .collections
        .iter()
        .find(|collection| collection.pattern == family)
        .map(|collection| collection.representatives.clone())
        .unwrap_or_default()
}
fn modal_relationship(inventory: &UiInventory, element: &UiElement) -> Option<(String, String)> {
    let signal = format!("{} {}", element.automation_id, element.name).to_ascii_lowercase();
    if !signal.contains("open") || !signal.contains("modal") {
        return None;
    }
    let modal = inventory.elements.iter().find(|candidate| {
        let signal = format!("{} {}", candidate.automation_id, candidate.name).to_ascii_lowercase();
        signal.contains("modal") && candidate.transient
    })?;
    let close = inventory.elements.iter().find(|candidate| {
        let signal = format!("{} {}", candidate.automation_id, candidate.name).to_ascii_lowercase();
        signal.contains("close") && signal.contains("modal")
    })?;
    Some((modal.automation_id.clone(), close.automation_id.clone()))
}

fn required_fixture_name(
    element: &UiElement,
    hint: Option<&GeneratorElementHint>,
) -> Option<String> {
    if let Some(fixture) = hint.and_then(|hint| hint.fixture.clone()) {
        return Some(fixture);
    }
    let signal = format!("{} {}", element.automation_id, element.name).to_ascii_lowercase();
    if signal.contains("password") || signal.contains("token") {
        Some("credentials".into())
    } else {
        None
    }
}

fn is_secret_element(element: &UiElement) -> bool {
    let signal = format!("{} {}", element.automation_id, element.name).to_ascii_lowercase();
    signal.contains("password") || signal.contains("token") || signal.contains("secret")
}

fn require_fixture(
    plan: &mut TestPlan,
    element: &UiElement,
    fixture: &str,
    secret: bool,
    reason: &str,
) {
    if !plan
        .fixture_requirements
        .iter()
        .any(|requirement| requirement.element == element.automation_id)
    {
        plan.fixture_requirements.push(FixtureRequirement {
            element: element.automation_id.clone(),
            fixture: fixture.into(),
            secret,
            reason: reason.into(),
        });
    }
}

fn edit_cases(mode: GeneratorMode, config: &GeneratorConfig) -> Vec<String> {
    let desired = match mode {
        GeneratorMode::Smoke => vec!["AI UI smoke"],
        GeneratorMode::Standard | GeneratorMode::Exhaustive => {
            vec![
                "AI UI test",
                "",
                "\u{041f}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442} \u{4e16}\u{754c}",
            ]
        }
    };
    desired
        .into_iter()
        .take(config.max_cases_per_control)
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AiTestIntent {
    pub intent: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub operation: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub dynamic: bool,
    #[serde(default)]
    pub approval_fixture: bool,
    #[serde(default)]
    pub parameters: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentValidationError {
    pub index: usize,
    pub code: String,
    pub message: String,
}

pub fn validate_ai_intents(
    inventory: &UiInventory,
    config: &GeneratorConfig,
    proposals: &[AiTestIntent],
) -> Result<Vec<TestIntent>, Vec<IntentValidationError>> {
    let mut accepted = Vec::new();
    let mut errors = Vec::new();
    for (index, proposal) in proposals.iter().enumerate() {
        match validate_ai_intent(inventory, config, proposal) {
            Ok(intent) => accepted.push(intent),
            Err((code, message)) => errors.push(IntentValidationError {
                index,
                code,
                message,
            }),
        }
    }
    if errors.is_empty() {
        accepted.sort_by(|left, right| {
            left.target
                .cmp(&right.target)
                .then(left.template.cmp(&right.template))
        });
        Ok(accepted)
    } else {
        Err(errors)
    }
}

fn validate_ai_intent(
    inventory: &UiInventory,
    config: &GeneratorConfig,
    proposal: &AiTestIntent,
) -> Result<TestIntent, (String, String)> {
    if proposal.intent.trim().is_empty() {
        return Err(("INVALID_INTENT".into(), "intent must not be empty".into()));
    }
    if let Some(operation) = proposal.operation.as_deref() {
        if catalog::operation(operation).is_none() {
            return Err((
                "UNKNOWN_OPERATION".into(),
                format!("operation is not in operations.json: {operation}"),
            ));
        }
    }
    let template_id = proposal.template.as_deref().ok_or_else(|| {
        (
            "UNKNOWN_TEMPLATE".into(),
            "AI intent must select a declarative template".into(),
        )
    })?;
    let template = templates::template(template_id).ok_or_else(|| {
        (
            "UNKNOWN_TEMPLATE".into(),
            format!("template is not registered: {template_id}"),
        )
    })?;
    if matches!(
        template_id,
        "window.move" | "window.frame_resize" | "window.state"
    ) && !config.generate_window_physical
    {
        return Err((
            "PHYSICAL_WINDOW_GENERATION_DISABLED".into(),
            "physical window templates require generate_window_physical=true".into(),
        ));
    }
    if let Some(operation) = proposal.operation.as_deref() {
        if !template.operations.iter().any(|known| known == operation) {
            return Err((
                "UNSUPPORTED_ACTION".into(),
                format!("{operation} is not part of template {template_id}"),
            ));
        }
    }
    validate_ai_parameters(inventory, proposal, template_id)?;
    let element = inventory
        .elements
        .iter()
        .find(|element| element.automation_id == proposal.target);
    if element.is_none()
        && !proposal.dynamic
        && !(proposal.target.is_empty() && template.requirements.is_empty())
    {
        return Err((
            "UNKNOWN_AUTOMATION_ID".into(),
            format!("{} is absent from inventory", proposal.target),
        ));
    }
    let safety = if let Some(element) = element {
        for requirement in &template.requirements {
            let supported = match requirement.as_str() {
                "context" => proposal.parameters.contains_key("menu_id") && element.interactable,
                "scroll" => element.offscreen,
                requirement => element
                    .capability(requirement)
                    .is_some_and(|capability| capability.supported && capability.available),
            };
            if !supported {
                return Err((
                    "UNSUPPORTED_ACTION".into(),
                    format!(
                        "{} does not expose available {requirement} capability",
                        proposal.target
                    ),
                ));
            }
        }
        effective_safety(&element.safety, &template.safety, template)
    } else {
        template.safety.clone()
    };
    let allowed = match safety {
        SafetyClass::Safe => true,
        SafetyClass::StatefulRecoverable => config.allow_stateful,
        SafetyClass::PotentiallyDestructive => {
            config.allow_destructive && proposal.approval_fixture
        }
        SafetyClass::Unknown => config.allow_unknown_mutation,
    };
    if !allowed {
        return Err((
            "SAFETY_POLICY_REJECTED".into(),
            format!("{template_id} is {safety:?} and lacks explicit permission"),
        ));
    }
    Ok(TestIntent {
        id: format!("ai:{template_id}:{}", proposal.target),
        intent: proposal.intent.clone(),
        target: proposal.target.clone(),
        template: template_id.into(),
        class: template.class.clone(),
        safety,
        reason: if proposal.reason.is_empty() {
            "AI proposal validated against inventory, capabilities and safety".into()
        } else {
            proposal.reason.clone()
        },
        parameters: proposal.parameters.clone(),
        dynamic: proposal.dynamic,
    })
}

fn validate_ai_parameters(
    inventory: &UiInventory,
    proposal: &AiTestIntent,
    template_id: &str,
) -> Result<(), (String, String)> {
    let required = match template_id {
        "modal.lifecycle" => &["modal_id", "close_id"][..],
        "context_menu.lifecycle" => &["menu_id"][..],
        "dropdown.choose" => &["values"][..],
        "edit.value" => &["cases"][..],
        "selection.multiple" => &["ids"][..],
        "selection.range" => &["start_id", "end_id"][..],
        "runtime.task" | "visual.region" => &["start_id", "task_id"][..],
        "external.request" => &["start_id", "request_id"][..],
        "file.output" => &["start_id", "output_root", "output_path"][..],
        "trace.milestone" => &["start_id", "milestone", "trace_event"][..],
        _ => &[][..],
    };
    for field in required {
        if !proposal.parameters.contains_key(*field) {
            return Err((
                "INVALID_TEMPLATE_PARAMETERS".into(),
                format!("{template_id} requires parameter {field}"),
            ));
        }
    }
    if proposal.parameters.get("secret").and_then(Value::as_bool) == Some(true)
        && proposal
            .parameters
            .values()
            .any(|value| value.is_string() && !value.as_str().unwrap_or_default().starts_with("${"))
    {
        return Err((
            "SECRET_VALUE_NOT_ALLOWED".into(),
            "AI intents may reference secret fixtures but may not contain raw secrets".into(),
        ));
    }
    let known = inventory
        .elements
        .iter()
        .map(|element| element.automation_id.as_str())
        .collect::<BTreeSet<_>>();
    if let Some(postcondition) = proposal.parameters.get("postcondition") {
        validate_ai_postcondition(postcondition, &known, proposal.dynamic)?;
    }
    if proposal.dynamic {
        return Ok(());
    }
    for key in [
        "modal_id",
        "close_id",
        "menu_id",
        "start_id",
        "end_id",
        "target_id",
    ] {
        if let Some(id) = proposal.parameters.get(key).and_then(Value::as_str) {
            if !known.contains(id) {
                return Err((
                    "UNKNOWN_AUTOMATION_ID".into(),
                    format!("parameter {key} references absent AutomationId {id}"),
                ));
            }
        }
    }
    for key in ["ids", "automation_ids"] {
        if let Some(ids) = proposal.parameters.get(key).and_then(Value::as_array) {
            if let Some(id) = ids
                .iter()
                .filter_map(Value::as_str)
                .find(|id| !known.contains(id))
            {
                return Err((
                    "UNKNOWN_AUTOMATION_ID".into(),
                    format!("parameter {key} references absent AutomationId {id}"),
                ));
            }
        }
    }
    Ok(())
}
fn validate_ai_postcondition(
    postcondition: &Value,
    known: &BTreeSet<&str>,
    dynamic: bool,
) -> Result<(), (String, String)> {
    let object = postcondition.as_object().ok_or_else(|| {
        (
            "INVALID_TEMPLATE_PARAMETERS".into(),
            "postcondition must be an object".into(),
        )
    })?;
    let mut ids = Vec::new();
    let mut supported = false;
    for key in [
        "automation_id_visible",
        "element_appears",
        "automation_id_hidden",
        "element_disappears",
    ] {
        if let Some(value) = object.get(key) {
            supported = true;
            let id = value.as_str().filter(|id| !id.is_empty()).ok_or_else(|| {
                (
                    "INVALID_TEMPLATE_PARAMETERS".into(),
                    format!("postcondition.{key} must be a non-empty AutomationId"),
                )
            })?;
            ids.push(id);
        }
    }
    for key in ["value_changes", "enabled_changes", "selected_changes"] {
        if let Some(value) = object.get(key) {
            supported = true;
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    (
                        "INVALID_TEMPLATE_PARAMETERS".into(),
                        format!("postcondition.{key}.id must be a non-empty AutomationId"),
                    )
                })?;
            ids.push(id);
        }
    }
    if object.contains_key("window_appears") || object.contains_key("window_closes") {
        supported = true;
    }
    if !supported {
        return Err((
            "INVALID_TEMPLATE_PARAMETERS".into(),
            "postcondition does not use a supported observable condition".into(),
        ));
    }
    if !dynamic {
        if let Some(id) = ids.into_iter().find(|id| !known.contains(id)) {
            return Err((
                "UNKNOWN_AUTOMATION_ID".into(),
                format!("postcondition references absent AutomationId {id}"),
            ));
        }
    }
    Ok(())
}
pub fn append_ai_intents(
    plan: &mut TestPlan,
    inventory: &UiInventory,
    config: &GeneratorConfig,
    proposals: &[AiTestIntent],
) -> Result<(), Vec<IntentValidationError>> {
    let accepted = validate_ai_intents(inventory, config, proposals)?;
    for intent in accepted {
        if !plan.intents.iter().any(|existing| {
            existing.target == intent.target && existing.template == intent.template
        }) {
            plan.intents.push(intent);
        }
    }
    plan.intents.sort_by(|left, right| {
        left.class
            .cmp(&right.class)
            .then(left.target.cmp(&right.target))
            .then(left.template.cmp(&right.template))
    });
    plan.safe = plan.intents.len();
    plan.planned = plan.intents.len() + plan.blocked.len() + plan.fixture_requirements.len();
    if !plan
        .markers
        .iter()
        .any(|marker| marker == "AI_UI_AI_PLAN_VALIDATION_PASSED")
    {
        plan.markers.push("AI_UI_AI_PLAN_VALIDATION_PASSED".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeneratedScenario {
    pub relative_path: PathBuf,
    pub intent_ids: Vec<String>,
    pub script: Script,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenerationReason {
    pub target: String,
    pub template: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenerationReport {
    pub version: u32,
    pub generator_version: String,
    pub inventory_version: u32,
    pub templates_selected: Vec<String>,
    pub templates_skipped: Vec<BlockedIntent>,
    pub safety_blocks: Vec<BlockedIntent>,
    pub fixture_requirements: Vec<FixtureRequirement>,
    pub generated_files: Vec<String>,
    pub reasons: Vec<GenerationReason>,
    pub markers: Vec<String>,
}

pub fn compile_plan(
    inventory: &UiInventory,
    plan: &TestPlan,
) -> Result<(Vec<GeneratedScenario>, GenerationReport), Vec<String>> {
    let mut errors = Vec::new();
    let mut grouped: BTreeMap<(String, String), Vec<&TestIntent>> = BTreeMap::new();
    for intent in &plan.intents {
        if let Err(error) = validate_compilable_intent(inventory, intent) {
            errors.push(error);
            continue;
        }
        grouped
            .entry((intent.class.clone(), functional_group(&intent.target)))
            .or_default()
            .push(intent);
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut scenarios = Vec::new();
    for ((class, group), intents) in grouped {
        for (chunk_index, chunk) in intents.chunks(8).enumerate() {
            let suffix = if chunk_index == 0 {
                String::new()
            } else {
                format!("-{}", chunk_index + 1)
            };
            let name = format!("generated-{class}-{group}{suffix}");
            let mut steps = vec![Step {
                op: "session.open".into(),
                arguments: Map::new(),
                save_as: None,
                save_session_as: Some("app".into()),
                session: None,
                depends_on: vec![],
                expect_error: None,
                step_id: Some("session-open".into()),
                redact: false,
            }];
            for intent in chunk {
                let mut compiled = compile_intent(inventory, intent);
                if intent.parameters.get("secret").and_then(Value::as_bool) == Some(true) {
                    for step in &mut compiled {
                        step.redact = true;
                    }
                }
                steps.extend(compiled);
            }
            if !steps.iter().any(|step| step.op == "get_runtime_health") {
                steps.push(session_step("get_runtime_health", Map::new()));
            }
            let mut tags = vec!["generated".into(), class.clone()];
            if chunk.iter().any(|intent| intent.dynamic) {
                tags.push("dynamic-discovery".into());
            }
            if chunk.iter().any(|intent| {
                intent
                    .parameters
                    .get("failure_fixture")
                    .and_then(Value::as_bool)
                    == Some(true)
            }) {
                tags.push("regression".into());
            }
            let mut secret_vars = chunk
                .iter()
                .filter(|intent| {
                    intent.parameters.get("secret").and_then(Value::as_bool) == Some(true)
                })
                .filter_map(|intent| intent.parameters.get("fixture").and_then(Value::as_str))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            secret_vars.sort();
            secret_vars.dedup();
            let script = Script {
                version: 1,
                name: name.clone(),
                application: Application {
                    command: Some(inventory.application.clone()),
                    arguments: Vec::new(),
                    window_title: inventory.windows.first().map(|window| window.name.clone()),
                    fixture_async_failure_mode: None,
                },
                options: ScriptOptions {
                    stop_on_error: true,
                    collect_failures: false,
                    evidence_on_failure: true,
                    ..Default::default()
                },
                vars: Map::new(),
                setup: vec![],
                steps,
                teardown: vec![],
                cases: vec![],
                tags,
                generated_by: Some(format!("ai-ui-generator/{GENERATOR_VERSION}")),
                generated: true,
                generator_version: Some(GENERATOR_VERSION.into()),
                inventory_version: Some(inventory.version),
                generated_from: Some("inventory.json".into()),
                intent: Some(
                    chunk
                        .iter()
                        .map(|intent| intent.intent.as_str())
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
                regression: None,
                secret_vars,
            };
            if let Err(validation) = crate::script::validate(&script) {
                errors.extend(
                    validation
                        .into_iter()
                        .map(|error| format!("{name}: {error}")),
                );
            }
            scenarios.push(GeneratedScenario {
                relative_path: PathBuf::from(&class).join(format!("{group}{suffix}.aiui.json")),
                intent_ids: chunk.iter().map(|intent| intent.id.clone()).collect(),
                script,
            });
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut templates_selected = plan
        .intents
        .iter()
        .map(|intent| intent.template.clone())
        .collect::<Vec<_>>();
    templates_selected.sort();
    templates_selected.dedup();
    let mut generated_files = scenarios
        .iter()
        .map(|scenario| scenario.relative_path.to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>();
    generated_files.sort();
    let report = GenerationReport {
        version: 1,
        generator_version: GENERATOR_VERSION.into(),
        inventory_version: inventory.version,
        templates_selected,
        templates_skipped: plan.blocked.clone(),
        safety_blocks: plan
            .blocked
            .iter()
            .filter(|blocked| {
                matches!(
                    blocked.safety,
                    SafetyClass::PotentiallyDestructive | SafetyClass::Unknown
                )
            })
            .cloned()
            .collect(),
        fixture_requirements: plan.fixture_requirements.clone(),
        generated_files,
        reasons: plan
            .intents
            .iter()
            .map(|intent| GenerationReason {
                target: intent.target.clone(),
                template: intent.template.clone(),
                reason: intent.reason.clone(),
            })
            .collect(),
        markers: vec!["AI_UI_GENERATED_SCRIPT_VALIDATION_PASSED".into()],
    };
    Ok((scenarios, report))
}

fn validate_compilable_intent(inventory: &UiInventory, intent: &TestIntent) -> Result<(), String> {
    let template = templates::template(&intent.template)
        .ok_or_else(|| format!("{}: UNKNOWN_TEMPLATE", intent.template))?;
    for operation in &template.operations {
        if catalog::operation(operation).is_none() {
            return Err(format!(
                "{}: UNKNOWN_OPERATION {operation}",
                intent.template
            ));
        }
    }
    if !intent.target.is_empty()
        && !intent.dynamic
        && !inventory
            .elements
            .iter()
            .any(|element| element.automation_id == intent.target)
    {
        return Err(format!("UNKNOWN_AUTOMATION_ID: {}", intent.target));
    }
    Ok(())
}

fn functional_group(target: &str) -> String {
    if target.is_empty() {
        return "application".into();
    }
    let segment = target
        .split('/')
        .rev()
        .nth(1)
        .or_else(|| target.split('/').next_back())
        .unwrap_or("ui");
    sanitize(segment.split('.').next().unwrap_or(segment))
}

fn sanitize(value: &str) -> String {
    let normalized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    normalized.trim_matches('-').to_owned().if_empty("ui")
}

trait EmptyFallback {
    fn if_empty(self, fallback: &str) -> String;
}

impl EmptyFallback for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.into()
        } else {
            self
        }
    }
}

fn compile_intent(inventory: &UiInventory, intent: &TestIntent) -> Vec<Step> {
    let element = inventory
        .elements
        .iter()
        .find(|element| element.automation_id == intent.target);
    let id = intent.target.as_str();
    match intent.template.as_str() {
        "discovery.semantic" => {
            let mut steps = vec![target_step("wait_for_automation_id", id)];
            if element.is_some_and(|element| element.visible) {
                steps.push(target_step("assert_visible", id));
                steps.push(target_step("assert_valid_bounds", id));
            }
            steps
        }
        "button.invoke" => {
            let mut click = target_step("click", id);
            if let Some(postcondition) = intent.parameters.get("postcondition") {
                click
                    .arguments
                    .insert("postcondition".into(), postcondition.clone());
            }
            vec![
                target_step("wait_for_automation_id", id),
                target_step("assert_interactable", id),
                click,
                session_step("get_runtime_health", Map::new()),
            ]
        }
        "edit.value" => {
            let mut steps = Vec::new();
            let mut read = target_step("read", id);
            read.save_as = Some(format!("original-{}", sanitize(id)));
            steps.push(read);
            let cases = intent
                .parameters
                .get("cases")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str);
            for case in cases {
                steps.push(value_step("set_value", id, case));
                steps.push(value_step("wait_for_value", id, case));
            }
            let original = intent
                .parameters
                .get("original_value")
                .and_then(Value::as_str)
                .unwrap_or_default();
            steps.push(value_step("set_value", id, original));
            steps.push(value_step("wait_for_value", id, original));
            steps
        }
        "checkbox.toggle" => {
            let selected = intent
                .parameters
                .get("selected")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            vec![
                target_step("read", id),
                target_step("toggle", id),
                state_step(id, !selected),
                target_step("toggle", id),
                state_step(id, selected),
            ]
        }
        "dropdown.choose" => intent
            .parameters
            .get("values")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .flat_map(|value| {
                [
                    value_step("choose", id, value),
                    value_step("wait_for_value", id, value),
                ]
            })
            .collect(),
        "selection.single" => {
            let mut steps = vec![target_step("select", id), state_step(id, true)];
            if element.and_then(|element| element.selected) == Some(false) {
                steps.push(target_step("remove_from_selection", id));
                steps.push(state_step(id, false));
            }
            steps
        }
        "selection.multiple" => {
            let ids = intent
                .parameters
                .get("ids")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let original = intent
                .parameters
                .get("selected_ids")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut steps = vec![session_step(
                "select_many",
                Map::from_iter([("ids".into(), Value::Array(ids.clone()))]),
            )];
            for id in ids.iter().filter_map(Value::as_str) {
                steps.push(target_step("wait_selected", id));
            }
            steps.push(session_step("clear_selection", Map::new()));
            if !original.is_empty() {
                steps.push(session_step(
                    "select_many",
                    Map::from_iter([("ids".into(), Value::Array(original))]),
                ));
            }
            steps
        }
        "selection.range" => {
            let start = intent.parameters["start_id"].as_str().unwrap_or_default();
            let end = intent.parameters["end_id"].as_str().unwrap_or_default();
            let original = intent
                .parameters
                .get("selected_ids")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut steps = vec![session_step(
                "select_range",
                Map::from_iter([
                    ("start_id".into(), Value::String(start.into())),
                    ("end_id".into(), Value::String(end.into())),
                ]),
            )];
            steps.push(target_step("wait_selected", start));
            steps.push(target_step("wait_selected", end));
            steps.push(session_step("clear_selection", Map::new()));
            if !original.is_empty() {
                steps.push(session_step(
                    "select_many",
                    Map::from_iter([("ids".into(), Value::Array(original))]),
                ));
            }
            steps
        }
        "modal.lifecycle" => {
            let modal = intent.parameters["modal_id"].as_str().unwrap_or_default();
            let close = intent.parameters["close_id"].as_str().unwrap_or_default();
            let mut open = target_step("click", id);
            open.arguments.insert(
                "postcondition".into(),
                json!({"automation_id_visible":modal}),
            );
            let mut close_step = target_step("click", close);
            close_step.arguments.insert(
                "postcondition".into(),
                json!({"automation_id_hidden":modal}),
            );
            vec![
                open,
                target_step("wait_visible", modal),
                target_step("assert_visible", modal),
                close_step,
                target_step("wait_hidden", modal),
            ]
        }
        "context_menu.lifecycle" => {
            let menu = intent.parameters["menu_id"].as_str().unwrap_or_default();
            vec![
                target_step("right_click", id),
                target_step("wait_visible", menu),
                session_step("inspect_ui", Map::new()),
                session_step(
                    "press",
                    Map::from_iter([("key".into(), Value::String("ESCAPE".into()))]),
                ),
                target_step("wait_hidden", menu),
            ]
        }
        "scroll.ensure_visible" => vec![
            target_step("ensure_visible", id),
            target_step("assert_interactable", id),
            target_step("assert_valid_bounds", id),
        ],
        "spatial.health" => vec![
            target_step("inspect_spatial_target", id),
            target_step("assert_canvas_healthy", id),
            spatial_region_step("capture_region", id),
            point_step("hover_relative", id, json!({"x":0.5,"y":0.5})),
        ],
        "spatial.click" => vec![
            point_step(
                "click_relative",
                id,
                intent
                    .parameters
                    .get("position")
                    .cloned()
                    .unwrap_or_else(|| json!({"x":0.5,"y":0.5})),
            ),
            target_step("assert_canvas_healthy", id),
        ],
        "spatial.drag" => vec![
            drag_step("drag_relative", id, intent),
            target_step("assert_canvas_healthy", id),
        ],
        "spatial.select_region" => vec![
            drag_step("select_region", id, intent),
            target_step("assert_canvas_healthy", id),
        ],
        "negative.relative.invalid" => {
            let mut step = point_step("click_relative", id, json!({"x":1.5,"y":0.5}));
            step.expect_error = Some("INVALID_ARGUMENT".into());
            vec![step]
        }
        "window.resize" => {
            let bounds = inventory.windows.first().map(|window| &window.bounds);
            let width = bounds
                .map(|bounds| bounds.width)
                .filter(|value| *value > 0)
                .unwrap_or(1100);
            let height = bounds
                .map(|bounds| bounds.height)
                .filter(|value| *value > 0)
                .unwrap_or(760);
            vec![
                session_step(
                    "windows_set_window_bounds",
                    Map::from_iter([
                        ("width".into(), json!(width + 40)),
                        ("height".into(), json!(height + 30)),
                    ]),
                ),
                session_step("inspect_ui", Map::new()),
                session_step("get_runtime_health", Map::new()),
                session_step(
                    "windows_set_window_bounds",
                    Map::from_iter([
                        ("width".into(), json!(width)),
                        ("height".into(), json!(height)),
                    ]),
                ),
            ]
        }
        "window.interaction" => vec![
            session_step("inspect_window_interaction", Map::new()),
            session_step("inspect_ui", Map::new()),
            session_step("get_runtime_health", Map::new()),
        ],
        "window.move" => vec![
            session_step("inspect_window_interaction", Map::new()),
            session_step(
                "windows_drag_title",
                Map::from_iter([("delta_x".into(), json!(28)), ("delta_y".into(), json!(16))]),
            ),
            session_step("inspect_window_interaction", Map::new()),
            session_step("get_runtime_health", Map::new()),
        ],
        "window.frame_resize" => vec![
            session_step("inspect_window_interaction", Map::new()),
            session_step(
                "windows_resize_frame",
                Map::from_iter([
                    ("target".into(), json!("bottom_right")),
                    ("delta_x".into(), json!(24)),
                    ("delta_y".into(), json!(18)),
                ]),
            ),
            session_step("inspect_window_interaction", Map::new()),
            session_step("get_runtime_health", Map::new()),
        ],
        "window.state" => vec![
            session_step("inspect_window_interaction", Map::new()),
            session_step("windows_double_click_title", Map::new()),
            session_step("inspect_window_interaction", Map::new()),
            session_step("windows_double_click_title", Map::new()),
            session_step("get_runtime_health", Map::new()),
        ],
        "runtime.task" => {
            let start = observation_parameter(intent, "start_id");
            let task_id = observation_parameter(intent, "task_id");
            vec![
                target_step("click", &start),
                session_step(
                    "wait_for_task_complete",
                    Map::from_iter([("operation_id".into(), Value::String(task_id.clone()))]),
                ),
                session_step(
                    "assert_task_success",
                    Map::from_iter([("operation_id".into(), Value::String(task_id))]),
                ),
                session_step("assert_no_pending_task", Map::new()),
            ]
        }
        "external.request" => {
            let start = observation_parameter(intent, "start_id");
            let request_id = observation_parameter(intent, "request_id");
            vec![
                target_step("click", &start),
                session_step(
                    "wait_for_api_response",
                    Map::from_iter([("request_id".into(), Value::String(request_id.clone()))]),
                ),
                session_step(
                    "assert_api_success",
                    Map::from_iter([("request_id".into(), Value::String(request_id))]),
                ),
            ]
        }
        "file.output" => {
            let start = observation_parameter(intent, "start_id");
            let root = observation_parameter(intent, "output_root");
            let path = observation_parameter(intent, "output_path");
            vec![
                target_step("click", &start),
                session_step(
                    "wait_for_file",
                    Map::from_iter([
                        ("root".into(), Value::String(root.clone())),
                        ("path".into(), Value::String(path.clone())),
                    ]),
                ),
                session_step(
                    "wait_for_file_stable",
                    Map::from_iter([
                        ("root".into(), Value::String(root.clone())),
                        ("path".into(), Value::String(path.clone())),
                        ("unchanged_for_ms".into(), json!(150)),
                    ]),
                ),
                session_step(
                    "assert_file_exists",
                    Map::from_iter([
                        ("root".into(), Value::String(root)),
                        ("path".into(), Value::String(path)),
                    ]),
                ),
            ]
        }
        "trace.milestone" => {
            let start = observation_parameter(intent, "start_id");
            let milestone = observation_parameter(intent, "milestone");
            let trace_event = observation_parameter(intent, "trace_event");
            vec![
                target_step("click", &start),
                session_step(
                    "wait_for_runtime_milestone",
                    Map::from_iter([("name".into(), Value::String(milestone.clone()))]),
                ),
                session_step(
                    "assert_runtime_milestone",
                    Map::from_iter([("name".into(), Value::String(milestone))]),
                ),
                session_step(
                    "assert_trace_event",
                    Map::from_iter([("event".into(), Value::String(trace_event))]),
                ),
            ]
        }
        "visual.region" => {
            let start = observation_parameter(intent, "start_id");
            let task_id = observation_parameter(intent, "task_id");
            vec![
                target_step("click", &start),
                session_step(
                    "wait_for_task_complete",
                    Map::from_iter([("operation_id".into(), Value::String(task_id))]),
                ),
                session_step("assert_not_blank", Map::new()),
                session_step("get_makepad_visual_diagnostics", Map::new()),
            ]
        }
        "runtime.health" => vec![session_step("get_runtime_health", Map::new())],
        _ => Vec::new(),
    }
}

fn observation_parameter(intent: &TestIntent, key: &str) -> String {
    intent
        .parameters
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn target_step(operation: &str, id: &str) -> Step {
    session_step(
        operation,
        Map::from_iter([("id".into(), Value::String(id.into()))]),
    )
}

fn value_step(operation: &str, id: &str, value: &str) -> Step {
    session_step(
        operation,
        Map::from_iter([
            ("id".into(), Value::String(id.into())),
            ("value".into(), Value::String(value.into())),
        ]),
    )
}

fn state_step(id: &str, selected: bool) -> Step {
    session_step(
        "wait_for_state",
        Map::from_iter([
            ("id".into(), Value::String(id.into())),
            ("state".into(), json!({"selected":selected})),
        ]),
    )
}

fn point_step(operation: &str, id: &str, position: Value) -> Step {
    session_step(
        operation,
        Map::from_iter([
            ("id".into(), Value::String(id.into())),
            ("position".into(), position),
        ]),
    )
}

fn drag_step(operation: &str, id: &str, intent: &TestIntent) -> Step {
    session_step(
        operation,
        Map::from_iter([
            ("id".into(), Value::String(id.into())),
            (
                "from".into(),
                intent
                    .parameters
                    .get("from")
                    .cloned()
                    .unwrap_or_else(|| json!({"x":0.25,"y":0.25})),
            ),
            (
                "to".into(),
                intent
                    .parameters
                    .get("to")
                    .cloned()
                    .unwrap_or_else(|| json!({"x":0.75,"y":0.75})),
            ),
        ]),
    )
}

fn spatial_region_step(operation: &str, id: &str) -> Step {
    session_step(
        operation,
        Map::from_iter([
            ("id".into(), Value::String(id.into())),
            (
                "region".into(),
                json!({"x":0.1,"y":0.1,"width":0.8,"height":0.8}),
            ),
        ]),
    )
}

fn session_step(operation: &str, arguments: Map<String, Value>) -> Step {
    Step {
        op: operation.into(),
        arguments,
        save_as: None,
        save_session_as: None,
        session: Some("app".into()),
        depends_on: vec![],
        expect_error: None,
        step_id: None,
        redact: false,
    }
}

pub fn validate_compiled_scenarios(
    inventory: &UiInventory,
    scenarios: &[GeneratedScenario],
) -> Result<(), Vec<String>> {
    let known_ids = inventory
        .elements
        .iter()
        .map(|element| element.automation_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut errors = Vec::new();
    for scenario in scenarios {
        if let Err(script_errors) = crate::script::validate(&scenario.script) {
            errors.extend(
                script_errors
                    .into_iter()
                    .map(|error| format!("{}: {error}", scenario.script.name)),
            );
        }
        let dynamic = scenario
            .script
            .tags
            .iter()
            .any(|tag| tag == "dynamic-discovery");
        for (index, step) in scenario.script.steps.iter().enumerate() {
            let Some(operation) = catalog::operation(&step.op) else {
                errors.push(format!(
                    "{}[{index}]: UNKNOWN_OPERATION {}",
                    scenario.script.name, step.op
                ));
                continue;
            };
            for required in &operation.required {
                if matches!(required.as_str(), "session_id" | "input_lock_token") {
                    continue;
                }
                if !step.arguments.contains_key(required) {
                    errors.push(format!(
                        "{}[{index}]: {} requires {required}",
                        scenario.script.name, step.op
                    ));
                }
            }
            if operation.target {
                if let Some(id) = step.arguments.get("id").and_then(Value::as_str) {
                    if !dynamic && !known_ids.contains(id) {
                        errors.push(format!(
                            "{}[{index}]: UNKNOWN_AUTOMATION_ID {id}",
                            scenario.script.name
                        ));
                    }
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn write_generation(
    output: impl AsRef<Path>,
    inventory: &UiInventory,
    plan: &TestPlan,
    scenarios: &[GeneratedScenario],
    report: &GenerationReport,
) -> Result<Vec<PathBuf>, String> {
    validate_compiled_scenarios(inventory, scenarios).map_err(|errors| errors.join("; "))?;
    let output = output.as_ref();
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    write_json(output.join("inventory.json"), inventory)?;
    write_json(output.join("test-plan.json"), plan)?;
    write_json(output.join("generation-report.json"), report)?;
    fs::write(
        output.join("test-templates.json"),
        include_str!("../test-templates.json"),
    )
    .map_err(|error| error.to_string())?;
    let mut paths = Vec::new();
    for scenario in scenarios {
        let path = output.join(&scenario.relative_path);
        if path.exists() {
            let raw = fs::read_to_string(&path).map_err(|error| error.to_string())?;
            let existing: Value = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
            if existing.get("generated").and_then(Value::as_bool) != Some(true) {
                return Err(format!(
                    "refusing to overwrite manual/regression test: {}",
                    path.display()
                ));
            }
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        write_json(&path, &scenario.script)?;
        paths.push(path);
    }
    paths.sort();
    Ok(paths)
}

fn write_json(path: impl AsRef<Path>, value: &impl Serialize) -> Result<(), String> {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

pub fn load_ai_intents(path: impl AsRef<Path>) -> Result<Vec<AiTestIntent>, String> {
    let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&raw).map_err(|error| format!("invalid AI intents: {error}"))
}

pub fn load_inventory(path: impl AsRef<Path>) -> Result<UiInventory, String> {
    let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let inventory: UiInventory =
        serde_json::from_str(&raw).map_err(|error| format!("invalid inventory: {error}"))?;
    if inventory.version != crate::inventory::INVENTORY_VERSION {
        return Err(format!(
            "unsupported inventory version: {}",
            inventory.version
        ));
    }
    Ok(inventory)
}

pub fn generation_report_with_execution_marker(report: &GenerationReport) -> GenerationReport {
    let mut report = report.clone();
    if !report
        .markers
        .iter()
        .any(|marker| marker == "AI_UI_GENERATED_SCRIPT_EXECUTION_PASSED")
    {
        report
            .markers
            .push("AI_UI_GENERATED_SCRIPT_EXECUTION_PASSED".into());
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        coverage,
        inventory::{InventoryOptions, from_public_responses},
    };

    fn raw_control(
        id: &str,
        role: &str,
        name: &str,
        invoke: bool,
        select: bool,
        value: bool,
    ) -> Value {
        json!({
            "element_id":format!("e:{id}"),
            "parent_element_id":null,
            "automation_id":id,
            "control_type":role,
            "name":name,
            "bounds":{"x":10,"y":10,"width":100,"height":30},
            "visible":true,
            "enabled":true,
            "interactable":true,
            "selected":false,
            "value":"",
            "capabilities":{
                "invoke":invoke,
                "select":select,
                "value":value,
                "expand_collapse":false,
                "native_action":invoke,
                "synthesized_pointer":true
            }
        })
    }

    fn inventory() -> UiInventory {
        let open = json!({"success":true,"result":{
            "session_id":"s",
            "windows":[{
                "window_id":"w",
                "name":"Demo",
                "automation_id":"window.main",
                "bounds":{"x":0,"y":0,"width":1100,"height":760},
                "visible":true
            }]
        }});
        let inspect = json!({"success":true,"result":{"elements":[
            raw_control("window.main/body/button.create","Button","Create greeting",true,false,false),
            raw_control("window.main/body/button.delete","Button","Delete",true,false,false),
            raw_control("window.main/body/input.name","Edit","Name",false,false,true),
            raw_control("window.main/body/text.help","Text","Help",false,false,true),
            raw_control("window.main/body/canvas.main","Pane","Canvas",false,false,false)
        ]}});
        let spatial = BTreeMap::from_iter([(
            "window.main/body/canvas.main".into(),
            json!({"success":true,"result":{
                "interactable":true,
                "supported_spatial_actions":[
                    "hover_relative",
                    "click_relative",
                    "drag_relative",
                    "select_region",
                    "capture_region"
                ]
            }}),
        )]);
        from_public_responses(
            "demo.exe",
            &open,
            &inspect,
            &spatial,
            &InventoryOptions::default(),
        )
        .unwrap()
    }

    #[test]
    fn plan_is_deterministic_capability_driven_and_conservative() {
        let mut inventory = inventory();
        let combo = inventory
            .elements
            .iter_mut()
            .find(|element| element.automation_id.ends_with("input.name"))
            .unwrap();
        combo.role = "ComboBox".into();
        combo.control_type = "ComboBox".into();
        combo.capabilities.push(crate::inventory::CapabilityState {
            name: "select".into(),
            supported: true,
            available: true,
        });
        let coverage = coverage::analyze(&inventory, &[], &[]);
        let config = GeneratorConfig::default();
        let first = build_plan(&inventory, &coverage, &config, GeneratorMode::Standard).unwrap();
        let second = build_plan(&inventory, &coverage, &config, GeneratorMode::Standard).unwrap();
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&second).unwrap()
        );
        assert!(first.intents.iter().any(|intent| {
            intent.target.ends_with("button.create") && intent.template == "button.invoke"
        }));
        assert!(first.blocked.iter().any(|intent| {
            intent.target.ends_with("button.delete")
                && intent.code == "REQUIRES_EXPLICIT_APPROVAL_FIXTURE"
        }));
        assert!(!first.intents.iter().any(|intent| {
            intent.target.ends_with("text.help") && intent.template != "discovery.semantic"
        }));
        assert!(!first.intents.iter().any(|intent| {
            intent.target.ends_with("input.name") && intent.template == "selection.single"
        }));
        assert!(first.fixture_requirements.iter().any(|requirement| {
            requirement.element.ends_with("input.name") && requirement.fixture == "dropdown_value"
        }));
        assert!(
            first
                .intents
                .iter()
                .any(|intent| intent.template == "spatial.health")
        );
        assert!(
            !first
                .intents
                .iter()
                .any(|intent| intent.template == "spatial.drag")
        );
        assert!(
            first
                .markers
                .iter()
                .any(|marker| marker == "AI_UI_GENERATOR_SAFETY_PASSED")
        );
    }

    #[test]
    fn compiler_emits_normal_valid_aiui_clients() {
        let inventory = inventory();
        let coverage = coverage::analyze(&inventory, &[], &[]);
        let plan = build_plan(
            &inventory,
            &coverage,
            &GeneratorConfig::default(),
            GeneratorMode::Standard,
        )
        .unwrap();
        let (scenarios, report) = compile_plan(&inventory, &plan).unwrap();
        validate_compiled_scenarios(&inventory, &scenarios).unwrap();
        assert!(!scenarios.is_empty());
        assert!(scenarios.iter().all(|scenario| scenario.script.generated));
        let generator_identity = format!("ai-ui-generator/{GENERATOR_VERSION}");
        assert!(scenarios.iter().all(|scenario| {
            scenario.script.generated_by.as_deref() == Some(generator_identity.as_str())
        }));
        assert!(
            report
                .markers
                .iter()
                .any(|marker| marker == "AI_UI_GENERATED_SCRIPT_VALIDATION_PASSED")
        );
    }

    #[test]
    fn gap_plan_does_not_repeat_covered_global_capabilities() {
        let inventory = inventory();
        let empty = coverage::analyze(&inventory, &[], &[]);
        let config = GeneratorConfig::default();
        let first = build_plan(&inventory, &empty, &config, GeneratorMode::Standard).unwrap();
        let (scenarios, _) = compile_plan(&inventory, &first).unwrap();
        let scripts = scenarios
            .into_iter()
            .map(|scenario| scenario.script)
            .collect::<Vec<_>>();
        let covered = coverage::analyze(&inventory, &scripts, &[]);
        assert!(
            covered.global_capabilities.iter().any(|item| {
                item.capability == "runtime.health" && item.planned && item.generated
            })
        );
        assert!(
            covered.global_capabilities.iter().any(|item| {
                item.capability == "window.resize" && item.planned && item.generated
            })
        );

        let gap = build_plan(&inventory, &covered, &config, GeneratorMode::Standard).unwrap();
        assert!(!gap.intents.iter().any(|intent| {
            matches!(intent.template.as_str(), "runtime.health" | "window.resize")
        }));

        let regenerate = GeneratorConfig {
            regenerate_all: true,
            ..Default::default()
        };
        let full = build_plan(&inventory, &covered, &regenerate, GeneratorMode::Standard).unwrap();
        assert!(
            full.intents
                .iter()
                .any(|intent| intent.template == "runtime.health")
        );
        assert!(
            full.intents
                .iter()
                .any(|intent| intent.template == "window.resize")
        );
    }

    #[test]
    fn observation_workflow_is_opt_in_and_compiles_trusted_scenarios() {
        let inventory = inventory();
        let initial = coverage::analyze(&inventory, &[], &[]);
        let default_plan = build_plan(
            &inventory,
            &initial,
            &GeneratorConfig::default(),
            GeneratorMode::Standard,
        )
        .unwrap();
        assert!(!default_plan.intents.iter().any(|intent| {
            matches!(
                intent.template.as_str(),
                "runtime.task"
                    | "external.request"
                    | "file.output"
                    | "trace.milestone"
                    | "visual.region"
            )
        }));

        let workflow = ObservationWorkflowHint {
            start_id: "window.main/body/button.create".into(),
            task_id: "fixture.async.workflow".into(),
            request_id: "fixture.request.workflow".into(),
            output_root: "fixture".into(),
            output_path: "async-output.json".into(),
            milestone: "fixture.output.ready".into(),
            trace_event: "fixture.async.completed".into(),
        };
        let config = GeneratorConfig {
            observation_workflow: Some(workflow.clone()),
            ..Default::default()
        };
        let plan = build_plan(&inventory, &initial, &config, GeneratorMode::Standard).unwrap();
        let expected_templates = [
            "runtime.task",
            "external.request",
            "file.output",
            "trace.milestone",
            "visual.region",
        ];
        for template in expected_templates {
            assert!(
                plan.intents
                    .iter()
                    .any(|intent| intent.template == template),
                "missing {template}"
            );
        }
        let (scenarios, _) = compile_plan(&inventory, &plan).unwrap();
        validate_compiled_scenarios(&inventory, &scenarios).unwrap();
        let scripts = scenarios
            .into_iter()
            .map(|scenario| scenario.script)
            .collect::<Vec<_>>();
        let covered = coverage::analyze(&inventory, &scripts, &[]);
        for capability in [
            "runtime.task",
            "external.request",
            "file.output",
            "trace.milestone",
            "visual.region",
        ] {
            assert!(
                covered.global_capabilities.iter().any(|item| {
                    item.capability == capability && item.planned && item.generated
                }),
                "missing coverage for {capability}"
            );
        }

        let invalid = GeneratorConfig {
            observation_workflow: Some(ObservationWorkflowHint {
                output_path: "../escape.json".into(),
                ..workflow
            }),
            ..Default::default()
        };
        let error =
            build_plan(&inventory, &initial, &invalid, GeneratorMode::Standard).unwrap_err();
        assert!(error.iter().any(|message| message.contains("trusted root")));
    }
    #[test]
    fn ai_validator_rejects_hallucination_unknown_id_and_destructive_action() {
        let inventory = inventory();
        let proposals = vec![
            AiTestIntent {
                intent: "invent operation".into(),
                target: "window.main/body/button.create".into(),
                template: Some("button.invoke".into()),
                operation: Some("super_click".into()),
                ..Default::default()
            },
            AiTestIntent {
                intent: "missing target".into(),
                target: "missing.id".into(),
                template: Some("button.invoke".into()),
                ..Default::default()
            },
            AiTestIntent {
                intent: "delete".into(),
                target: "window.main/body/button.delete".into(),
                template: Some("button.invoke".into()),
                ..Default::default()
            },
            AiTestIntent {
                intent: "hallucinated postcondition".into(),
                target: "window.main/body/button.create".into(),
                template: Some("button.invoke".into()),
                operation: Some("click".into()),
                parameters: Map::from_iter([(
                    "postcondition".into(),
                    json!({"automation_id_visible":"missing.result"}),
                )]),
                ..Default::default()
            },
        ];
        let errors =
            validate_ai_intents(&inventory, &GeneratorConfig::default(), &proposals).unwrap_err();
        assert!(errors.iter().any(|error| error.code == "UNKNOWN_OPERATION"));
        assert!(
            errors
                .iter()
                .any(|error| error.code == "UNKNOWN_AUTOMATION_ID")
        );
        assert!(errors.iter().any(|error| {
            error.code == "UNKNOWN_AUTOMATION_ID" && error.message.contains("missing.result")
        }));
        assert!(
            errors
                .iter()
                .any(|error| error.code == "SAFETY_POLICY_REJECTED")
        );
    }

    #[test]
    fn writer_never_overwrites_manual_test() {
        let inventory = inventory();
        let coverage = coverage::analyze(&inventory, &[], &[]);
        let plan = build_plan(
            &inventory,
            &coverage,
            &GeneratorConfig::default(),
            GeneratorMode::Smoke,
        )
        .unwrap();
        let (scenarios, report) = compile_plan(&inventory, &plan).unwrap();
        let root =
            std::env::temp_dir().join(format!("ai-ui-generator-manual-{}", std::process::id()));
        let first = root.join(&scenarios[0].relative_path);
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::write(
            &first,
            br#"{"version":1,"name":"manual","generated":false}"#,
        )
        .unwrap();
        let error = write_generation(&root, &inventory, &plan, &scenarios, &report).unwrap_err();
        assert!(error.contains("refusing to overwrite manual"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fixture_resolution_keeps_secret_out_of_generated_script() {
        use crate::fixtures::{FixtureStore, FixtureValue};

        let inventory = inventory();
        let coverage = coverage::analyze(&inventory, &[], &[]);
        let mut config = GeneratorConfig::default();
        config.element_hints.insert(
            "window.main/body/input.name".into(),
            GeneratorElementHint {
                fixture: Some("credentials.password".into()),
                secret: true,
                ..Default::default()
            },
        );
        let mut plan = build_plan(&inventory, &coverage, &config, GeneratorMode::Standard).unwrap();
        assert!(
            plan.fixture_requirements
                .iter()
                .any(|requirement| { requirement.element == "window.main/body/input.name" })
        );
        let fixtures = FixtureStore {
            version: 1,
            fixtures: BTreeMap::from_iter([(
                "credentials.password".into(),
                FixtureValue {
                    value: json!("never-serialize-this-secret"),
                    secret: true,
                    description: None,
                },
            )]),
        };
        resolve_fixtures(&inventory, &mut plan, &config, &fixtures).unwrap();
        assert!(plan.fixture_requirements.is_empty());
        let (scenarios, _) = compile_plan(&inventory, &plan).unwrap();
        let script = scenarios
            .iter()
            .map(|scenario| &scenario.script)
            .find(|script| {
                script
                    .secret_vars
                    .iter()
                    .any(|name| name == "credentials.password")
            })
            .unwrap();
        let serialized = serde_json::to_string(script).unwrap();
        assert!(serialized.contains("${credentials.password}"));
        assert!(!serialized.contains("never-serialize-this-secret"));
        assert!(script.steps.iter().any(|step| step.redact));
    }

    #[test]
    fn offscreen_dynamic_and_failure_fixture_rules_are_explicit() {
        let mut inventory = inventory();
        let input = inventory
            .elements
            .iter_mut()
            .find(|element| element.automation_id.ends_with("input.name"))
            .unwrap();
        input.offscreen = true;
        input.visible = false;
        input.interactable = false;
        input.dynamic = true;
        for capability in &mut input.capabilities {
            capability.available = false;
        }
        let coverage = coverage::analyze(&inventory, &[], &[]);
        let mut config = GeneratorConfig::default();
        config.element_hints.insert(
            "window.main/body/button.create".into(),
            GeneratorElementHint {
                failure_fixture: true,
                postcondition: Some(json!({
                    "automation_id_visible":"window.main/body/never.appears"
                })),
                ..Default::default()
            },
        );
        let plan = build_plan(&inventory, &coverage, &config, GeneratorMode::Standard).unwrap();
        assert!(!plan.intents.iter().any(|intent| {
            intent.target.ends_with("input.name") && intent.template == "scroll.ensure_visible"
        }));
        assert!(plan.intents.iter().any(|intent| {
            intent.target.ends_with("button.create")
                && intent.class == "regression"
                && intent.parameters["failure_fixture"] == true
        }));
        let (scenarios, _) = compile_plan(&inventory, &plan).unwrap();
        assert!(scenarios.iter().any(|scenario| {
            scenario
                .script
                .tags
                .iter()
                .any(|tag| tag == "dynamic-discovery")
        }));
        assert!(
            scenarios
                .iter()
                .any(|scenario| { scenario.script.tags.iter().any(|tag| tag == "regression") })
        );
    }
    #[test]
    fn physical_window_templates_are_explicit_opt_in_and_compile() {
        let inventory = inventory();
        let coverage = coverage::analyze(&inventory, &[], &[]);
        let ordinary = build_plan(
            &inventory,
            &coverage,
            &GeneratorConfig::default(),
            GeneratorMode::Standard,
        )
        .unwrap();
        assert!(
            ordinary
                .intents
                .iter()
                .any(|intent| intent.template == "window.interaction")
        );
        assert!(!ordinary.intents.iter().any(|intent| {
            matches!(
                intent.template.as_str(),
                "window.move" | "window.frame_resize" | "window.state"
            )
        }));

        let mut config = GeneratorConfig::default();
        config.generate_window_physical = true;
        let opted_in = build_plan(&inventory, &coverage, &config, GeneratorMode::Standard).unwrap();
        for template in ["window.move", "window.frame_resize", "window.state"] {
            assert!(
                opted_in
                    .intents
                    .iter()
                    .any(|intent| intent.template == template),
                "missing {template}"
            );
        }
        let (scenarios, _) = compile_plan(&inventory, &opted_in).unwrap();
        let operations = scenarios
            .iter()
            .flat_map(|scenario| scenario.script.steps.iter())
            .map(|step| step.op.as_str())
            .collect::<BTreeSet<_>>();
        for operation in [
            "inspect_window_interaction",
            "windows_drag_title",
            "windows_resize_frame",
            "windows_double_click_title",
        ] {
            assert!(operations.contains(operation), "missing {operation}");
        }
    }
}
