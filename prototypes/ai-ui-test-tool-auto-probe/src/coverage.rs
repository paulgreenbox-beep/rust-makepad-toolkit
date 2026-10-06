//! UI capability coverage derived from actual scripts and execution reports.

use crate::{
    catalog,
    generator::TestPlan,
    inventory::{UiElement, UiInventory},
    script::{Script, Step},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum CoverageState {
    Uncovered,
    Generated,
    ExecutedPassed,
    ExecutedFailed,
    Blocked,
    RequiresFixture,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageCounts {
    pub available: usize,
    pub covered: usize,
    pub attempted: usize,
    pub verified: usize,
}

impl CoverageCounts {
    pub fn percent(&self) -> u32 {
        if self.available == 0 {
            100
        } else {
            ((self.covered * 100) / self.available) as u32
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityCoverage {
    pub capability: String,
    pub state: CoverageState,
    pub planned: bool,
    pub generated: bool,
    pub executed: bool,
    pub passed: bool,
    pub attempted: bool,
    pub verified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementCoverage {
    pub automation_id: String,
    pub capabilities: Vec<CapabilityCoverage>,
    pub covered: Vec<String>,
    pub missing: Vec<String>,
    pub discovery: bool,
    pub interaction: bool,
    pub verification: bool,
    pub spatial: bool,
    pub negative: bool,
    pub regression: bool,
    pub attempted: bool,
    pub verified: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UntestedCapability {
    pub available_targets: usize,
    pub tested_targets: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaleGeneratedTest {
    pub code: String,
    pub test: String,
    pub automation_id: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionCoverage {
    pub planned: usize,
    pub generated: usize,
    pub executed: usize,
    pub passed: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionCoverage {
    pub pattern: String,
    pub representatives: usize,
    pub covered_representatives: usize,
    pub interaction_covered: bool,
    pub verification_covered: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiCoverage {
    pub version: u32,
    pub total_semantic_elements: usize,
    pub interactive_elements: usize,
    pub covered_elements: usize,
    pub uncovered_elements: usize,
    pub partially_covered_elements: usize,
    pub discovery: CoverageCounts,
    pub interaction: CoverageCounts,
    pub state_verification: CoverageCounts,
    pub spatial: CoverageCounts,
    pub negative: CoverageCounts,
    pub regression: CoverageCounts,
    pub execution: ExecutionCoverage,
    #[serde(default)]
    pub global_capabilities: Vec<CapabilityCoverage>,
    pub elements: Vec<ElementCoverage>,
    pub collections: Vec<CollectionCoverage>,
    pub critical_uncovered: Vec<String>,
    pub untested_capabilities: BTreeMap<String, UntestedCapability>,
    pub stale_generated_tests: Vec<StaleGeneratedTest>,
    pub markers: Vec<String>,
}

#[derive(Clone, Debug)]
struct Observation {
    capability: String,
    generated: bool,
    executed: bool,
    passed: bool,
    attempted: bool,
    verified: bool,
    discovery: bool,
    interaction: bool,
    spatial: bool,
    negative: bool,
    regression: bool,
}

pub fn analyze(inventory: &UiInventory, scripts: &[Script], reports: &[Value]) -> UiCoverage {
    analyze_with_plan(inventory, scripts, reports, None)
}

pub fn analyze_with_plan(
    inventory: &UiInventory,
    scripts: &[Script],
    reports: &[Value],
    plan: Option<&TestPlan>,
) -> UiCoverage {
    let report_by_test = reports
        .iter()
        .filter_map(|report| {
            report
                .get("test")
                .and_then(Value::as_str)
                .map(|name| (name, report))
        })
        .collect::<BTreeMap<_, _>>();
    let inventory_ids = inventory
        .elements
        .iter()
        .filter(|element| !element.automation_id.is_empty())
        .map(|element| element.automation_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut observed: BTreeMap<String, Vec<Observation>> = BTreeMap::new();
    let mut global_observed: BTreeMap<String, Vec<Observation>> = BTreeMap::new();
    let mut stale = Vec::new();
    let mut execution = ExecutionCoverage::default();
    for script in scripts {
        let report = report_by_test.get(script.name.as_str()).copied();
        let steps = all_steps(script);
        execution.planned += 1;
        if script.generated || script.generated_by.is_some() {
            execution.generated += 1;
        }
        if report.is_some() {
            execution.executed += 1;
        }
        if report
            .and_then(|report| report.get("status"))
            .and_then(Value::as_str)
            == Some("PASSED")
        {
            execution.passed += 1;
        }
        for (index, step) in steps.iter().enumerate() {
            let status = report
                .and_then(|report| report.get("steps"))
                .and_then(Value::as_array)
                .and_then(|steps| steps.get(index))
                .and_then(|step| step.get("status"))
                .and_then(Value::as_str);
            let executed = status.is_some_and(|status| !status.starts_with("SKIPPED"));
            let passed = status.is_some_and(|status| status.starts_with("PASS"));
            if let Some(capability) = global_capability(&step.op) {
                let operation = catalog::operation(&step.op);
                global_observed
                    .entry(capability.into())
                    .or_default()
                    .push(Observation {
                        capability: capability.into(),
                        generated: script.generated || script.generated_by.is_some(),
                        executed,
                        passed,
                        attempted: executed,
                        verified: passed
                            && operation.is_some_and(|operation| operation.verification),
                        discovery: false,
                        interaction: operation.is_some_and(|operation| operation.mutation),
                        spatial: false,
                        negative: false,
                        regression: false,
                    });
            }
            for target in step_targets(step) {
                if !inventory_ids.contains(target.as_str()) {
                    if script.generated || script.generated_by.is_some() {
                        stale.push(StaleGeneratedTest {
                            code: "STALE_GENERATED_TEST".into(),
                            test: script.name.clone(),
                            automation_id: target,
                            message: "test target removed from inventory".into(),
                        });
                    }
                    continue;
                }
                let operation = catalog::operation(&step.op);
                let capability = operation
                    .and_then(|operation| operation.capability.clone())
                    .unwrap_or_else(|| inferred_capability(&step.op));
                let interaction = operation.is_some_and(|operation| operation.mutation)
                    || is_interaction_operation(&step.op);
                let spatial = operation.is_some_and(|operation| operation.spatial);
                let explicit_verification = operation
                    .is_some_and(|operation| operation.verification || operation.assertion)
                    || step.arguments.contains_key("postcondition");
                observed.entry(target).or_default().push(Observation {
                    capability,
                    generated: script.generated || script.generated_by.is_some(),
                    executed,
                    passed,
                    attempted: executed && interaction,
                    verified: passed && explicit_verification,
                    discovery: !interaction || explicit_verification,
                    interaction,
                    spatial,
                    negative: step.expect_error.is_some()
                        || script.tags.iter().any(|tag| tag == "negative"),
                    regression: script.regression.is_some()
                        || script.tags.iter().any(|tag| tag == "regression"),
                });
            }
        }
        if steps.iter().any(|step| step.op == "inspect_ui") {
            for id in &inventory_ids {
                observed
                    .entry((*id).to_owned())
                    .or_default()
                    .push(Observation {
                        capability: "discovery".into(),
                        generated: script.generated || script.generated_by.is_some(),
                        executed: report.is_some(),
                        passed: report
                            .and_then(|report| report.get("status"))
                            .and_then(Value::as_str)
                            == Some("PASSED"),
                        attempted: false,
                        verified: false,
                        discovery: true,
                        interaction: false,
                        spatial: false,
                        negative: false,
                        regression: false,
                    });
            }
        }
    }
    build_coverage(inventory, observed, global_observed, stale, execution, plan)
}

fn build_coverage(
    inventory: &UiInventory,
    observed: BTreeMap<String, Vec<Observation>>,
    global_observed: BTreeMap<String, Vec<Observation>>,
    mut stale: Vec<StaleGeneratedTest>,
    execution: ExecutionCoverage,
    plan: Option<&TestPlan>,
) -> UiCoverage {
    let mut elements = Vec::new();
    let mut discovery = CoverageCounts {
        available: inventory
            .elements
            .iter()
            .filter(|element| !element.automation_id.is_empty())
            .count(),
        ..Default::default()
    };
    let mut interaction = CoverageCounts::default();
    let mut state_verification = CoverageCounts::default();
    let mut spatial = CoverageCounts::default();
    let mut negative = CoverageCounts {
        available: inventory
            .elements
            .iter()
            .filter(|element| element.is_interactive())
            .count(),
        ..Default::default()
    };
    let mut regression = CoverageCounts {
        available: inventory
            .elements
            .iter()
            .filter(|element| element.is_interactive())
            .count(),
        ..Default::default()
    };
    let mut critical_uncovered = Vec::new();
    let mut untested_capabilities = BTreeMap::<String, UntestedCapability>::new();

    for element in inventory
        .elements
        .iter()
        .filter(|element| !element.automation_id.is_empty())
    {
        let observations = observed
            .get(&element.automation_id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let discovery_covered = observations.iter().any(|item| item.discovery);
        let interaction_covered = observations.iter().any(|item| item.interaction);
        let verification_covered = observations.iter().any(|item| item.verified);
        let spatial_covered = observations.iter().any(|item| item.spatial);
        let negative_covered = observations.iter().any(|item| item.negative);
        let regression_covered = observations.iter().any(|item| item.regression);
        let attempted = observations.iter().any(|item| item.attempted);
        let verified = observations.iter().any(|item| item.verified);
        discovery.covered += usize::from(discovery_covered);
        discovery.attempted += usize::from(discovery_covered);
        discovery.verified += usize::from(discovery_covered);
        negative.covered += usize::from(negative_covered);
        negative.attempted += usize::from(
            observations
                .iter()
                .any(|item| item.negative && item.executed),
        );
        negative.verified +=
            usize::from(observations.iter().any(|item| item.negative && item.passed));
        regression.covered += usize::from(regression_covered);
        regression.attempted += usize::from(
            observations
                .iter()
                .any(|item| item.regression && item.executed),
        );
        regression.verified += usize::from(
            observations
                .iter()
                .any(|item| item.regression && item.passed),
        );

        let available = available_capabilities(element);
        let mut capability_coverage = Vec::new();
        let mut covered = Vec::new();
        let mut missing = Vec::new();
        for (name, is_available, is_spatial) in available {
            let matches = observations
                .iter()
                .filter(|item| capability_matches(&item.capability, &name))
                .collect::<Vec<_>>();
            let planned = !matches.is_empty();
            let generated = matches.iter().any(|item| item.generated);
            let executed = matches.iter().any(|item| item.executed);
            let passed = matches.iter().any(|item| item.passed);
            let attempted_capability = matches.iter().any(|item| item.attempted);
            let verified_capability = matches.iter().any(|item| item.verified);
            let requires_fixture = plan.is_some_and(|plan| {
                name == "value"
                    && plan
                        .fixture_requirements
                        .iter()
                        .any(|requirement| requirement.element == element.automation_id)
            });
            let explicitly_blocked = plan.is_some_and(|plan| {
                plan.blocked.iter().any(|blocked| {
                    blocked.target == element.automation_id
                        && template_covers_capability(&blocked.template, &name)
                })
            });
            let state = if requires_fixture {
                CoverageState::RequiresFixture
            } else if explicitly_blocked || !is_available {
                CoverageState::Blocked
            } else if executed && passed {
                CoverageState::ExecutedPassed
            } else if executed {
                CoverageState::ExecutedFailed
            } else if planned {
                CoverageState::Generated
            } else {
                CoverageState::Uncovered
            };
            if is_available {
                let entry = untested_capabilities.entry(name.clone()).or_default();
                entry.available_targets += 1;
                entry.tested_targets += usize::from(planned);
                if is_spatial {
                    spatial.available += 1;
                    spatial.covered += usize::from(planned);
                    spatial.attempted += usize::from(attempted_capability);
                    spatial.verified += usize::from(verified_capability);
                } else {
                    interaction.available += 1;
                    state_verification.available += 1;
                    interaction.covered += usize::from(planned);
                    interaction.attempted += usize::from(attempted_capability);
                    interaction.verified += usize::from(passed);
                    state_verification.covered += usize::from(verified_capability);
                    state_verification.attempted += usize::from(attempted_capability);
                    state_verification.verified += usize::from(verified_capability);
                }
                if planned {
                    covered.push(name.clone());
                } else {
                    missing.push(name.clone());
                }
            }
            capability_coverage.push(CapabilityCoverage {
                capability: name,
                state,
                planned,
                generated,
                executed,
                passed,
                attempted: attempted_capability,
                verified: verified_capability,
            });
        }
        covered.sort();
        missing.sort();
        if element.visible && element.is_interactive() && !interaction_covered {
            critical_uncovered.push(element.automation_id.clone());
        }
        elements.push(ElementCoverage {
            automation_id: element.automation_id.clone(),
            capabilities: capability_coverage,
            covered,
            missing,
            discovery: discovery_covered,
            interaction: interaction_covered,
            verification: verification_covered,
            spatial: spatial_covered,
            negative: negative_covered,
            regression: regression_covered,
            attempted,
            verified,
        });
    }
    stale.sort_by(|left, right| {
        left.test
            .cmp(&right.test)
            .then(left.automation_id.cmp(&right.automation_id))
    });
    stale.dedup_by(|left, right| {
        left.test == right.test && left.automation_id == right.automation_id
    });
    critical_uncovered.sort();
    let covered_elements = elements
        .iter()
        .filter(|element| element.discovery || !element.covered.is_empty())
        .count();
    let uncovered_elements = elements
        .iter()
        .filter(|element| !element.discovery && element.covered.is_empty())
        .count();
    let partially_covered_elements = elements
        .iter()
        .filter(|element| !element.covered.is_empty() && !element.missing.is_empty())
        .count();
    let collections = inventory
        .collections
        .iter()
        .map(|family| {
            let representative_coverage = family
                .representatives
                .iter()
                .filter_map(|id| elements.iter().find(|element| element.automation_id == *id))
                .collect::<Vec<_>>();
            CollectionCoverage {
                pattern: family.pattern.clone(),
                representatives: family.representatives.len(),
                covered_representatives: representative_coverage
                    .iter()
                    .filter(|coverage| coverage.interaction)
                    .count(),
                interaction_covered: representative_coverage
                    .iter()
                    .any(|coverage| coverage.interaction),
                verification_covered: representative_coverage
                    .iter()
                    .any(|coverage| coverage.verification),
            }
        })
        .collect::<Vec<_>>();
    let global_capabilities = [
        "runtime.health",
        "window.interaction",
        "window.move",
        "window.resize",
        "window.state",
        "runtime.task",
        "external.request",
        "file.output",
        "trace.milestone",
        "visual.region",
    ]
    .into_iter()
    .filter(|capability| {
        !matches!(
            *capability,
            "window.interaction" | "window.move" | "window.resize" | "window.state"
        ) || !inventory.windows.is_empty()
    })
    .map(|capability| {
        let matches = global_observed
            .get(capability)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let planned = !matches.is_empty();
        let generated = matches.iter().any(|item| item.generated);
        let executed = matches.iter().any(|item| item.executed);
        let passed = matches.iter().any(|item| item.passed);
        CapabilityCoverage {
            capability: capability.into(),
            state: if executed && passed {
                CoverageState::ExecutedPassed
            } else if executed {
                CoverageState::ExecutedFailed
            } else if planned {
                CoverageState::Generated
            } else {
                CoverageState::Uncovered
            },
            planned,
            generated,
            executed,
            passed,
            attempted: matches.iter().any(|item| item.attempted),
            verified: matches.iter().any(|item| item.verified),
        }
    })
    .collect();
    UiCoverage {
        version: 1,
        total_semantic_elements: elements.len(),
        interactive_elements: inventory
            .elements
            .iter()
            .filter(|element| element.is_interactive())
            .count(),
        covered_elements,
        uncovered_elements,
        partially_covered_elements,
        discovery,
        interaction,
        state_verification,
        spatial,
        negative,
        regression,
        execution,
        global_capabilities,
        elements,
        collections,
        critical_uncovered,
        untested_capabilities,
        stale_generated_tests: stale,
        markers: vec!["AI_UI_COVERAGE_ANALYSIS_PASSED".into()],
    }
}

fn global_capability(operation: &str) -> Option<&'static str> {
    match operation {
        "get_runtime_health" => Some("runtime.health"),
        "inspect_window_interaction" => Some("window.interaction"),
        "windows_drag_title" => Some("window.move"),
        "windows_resize_frame" | "windows_set_window_bounds" => Some("window.resize"),
        "windows_double_click_title" | "windows_set_window_state" => Some("window.state"),
        "get_task_state"
        | "wait_for_task"
        | "wait_for_task_state"
        | "wait_for_task_complete"
        | "assert_task_success"
        | "assert_task_failure"
        | "assert_no_pending_task" => Some("runtime.task"),
        "get_api_request_state"
        | "wait_for_api_state"
        | "wait_for_api_response"
        | "assert_api_success"
        | "assert_api_failure" => Some("external.request"),
        "list_files"
        | "wait_for_file"
        | "assert_file_exists"
        | "assert_file_not_exists"
        | "read_file_metadata"
        | "assert_file_count"
        | "assert_file_sha256"
        | "wait_for_file_stable" => Some("file.output"),
        "get_runtime_milestones"
        | "wait_for_runtime_milestone"
        | "assert_runtime_milestone"
        | "get_runtime_timeline"
        | "get_application_logs"
        | "wait_for_log_event"
        | "assert_log_event"
        | "assert_trace_event"
        | "assert_no_fatal_log" => Some("trace.milestone"),
        "capture_screenshot"
        | "assert_screenshot_matches"
        | "assert_region_changed"
        | "assert_not_blank"
        | "assert_visual_mask"
        | "get_makepad_visual_diagnostics" => Some("visual.region"),
        _ => None,
    }
}

fn available_capabilities(element: &UiElement) -> Vec<(String, bool, bool)> {
    let mut capabilities = element
        .capabilities
        .iter()
        .filter(|capability| {
            capability.supported
                && matches!(
                    capability.name.as_str(),
                    "invoke"
                        | "select"
                        | "select.multiple"
                        | "select.range"
                        | "value"
                        | "expand_collapse"
                )
        })
        .map(|capability| (capability.name.clone(), capability.available, false))
        .chain(
            element
                .spatial_capabilities
                .iter()
                .filter(|capability| capability.supported)
                .map(|capability| (capability.name.clone(), capability.available, true)),
        )
        .collect::<Vec<_>>();
    if element.offscreen {
        capabilities.push(("scroll".into(), true, false));
    }
    capabilities.sort_by(|left, right| left.0.cmp(&right.0));
    capabilities.dedup_by(|left, right| left.0 == right.0);
    capabilities
}

fn capability_matches(observed: &str, expected: &str) -> bool {
    observed == expected
        || (expected == "invoke" && observed == "native_action")
        || (expected == "select" && observed.starts_with("select."))
        || (expected == "spatial.capture" && observed == "spatial.health")
}

fn template_covers_capability(template: &str, capability: &str) -> bool {
    matches!(
        (template, capability),
        ("button.invoke", "invoke" | "native_action")
            | ("modal.lifecycle", "invoke" | "native_action")
            | ("context_menu.lifecycle", "context")
            | ("edit.value" | "dropdown.choose", "value")
            | ("checkbox.toggle" | "selection.single", "select")
            | ("selection.multiple", "select.multiple")
            | ("selection.range", "select.range")
            | ("scroll.ensure_visible", "scroll")
            | ("spatial.health", "spatial.capture" | "spatial.health")
            | (
                "spatial.click" | "negative.relative.invalid",
                "spatial.click"
            )
            | ("spatial.drag", "spatial.drag")
            | ("spatial.select_region", "spatial.select_region")
    )
}
fn inferred_capability(operation: &str) -> String {
    match operation {
        "set_value" | "input" | "choose" | "wait_for_value" => "value",
        "select" | "toggle" | "wait_selected" => "select",
        value if value.contains("relative") => "spatial.unknown",
        "click" | "double_click" => "invoke",
        _ => "discovery",
    }
    .into()
}

fn is_interaction_operation(operation: &str) -> bool {
    matches!(
        operation,
        "click"
            | "double_click"
            | "right_click"
            | "set_value"
            | "input"
            | "toggle"
            | "choose"
            | "select"
            | "select_many"
            | "select_range"
            | "drag"
            | "drag_to"
            | "click_relative"
            | "drag_relative"
            | "select_region"
            | "wheel_relative"
    )
}

fn all_steps(script: &Script) -> Vec<&Step> {
    script
        .setup
        .iter()
        .chain(script.steps.iter())
        .chain(script.teardown.iter())
        .collect()
}

fn step_targets(step: &Step) -> Vec<String> {
    let mut targets = BTreeSet::new();
    for key in [
        "id",
        "automation_id",
        "target_id",
        "container_id",
        "start_id",
        "end_id",
    ] {
        if let Some(id) = step.arguments.get(key).and_then(Value::as_str) {
            targets.insert(id.to_owned());
        }
    }
    for key in ["ids", "automation_ids"] {
        if let Some(ids) = step.arguments.get(key).and_then(Value::as_array) {
            targets.extend(ids.iter().filter_map(Value::as_str).map(str::to_owned));
        }
    }
    if let Some(condition) = step.arguments.get("postcondition") {
        collect_condition_targets(condition, &mut targets);
    }
    targets.into_iter().collect()
}

fn collect_condition_targets(value: &Value, targets: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if (key.contains("automation_id") || key == "id") && value.is_string() {
                    if let Some(id) = value.as_str() {
                        targets.insert(id.to_owned());
                    }
                } else {
                    collect_condition_targets(value, targets);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_condition_targets(value, targets);
            }
        }
        _ => {}
    }
}

pub fn load_scripts(root: impl AsRef<Path>) -> Result<Vec<Script>, String> {
    let mut paths = Vec::new();
    collect_files(root.as_ref(), ".aiui.json", &mut paths)?;
    paths.sort();
    paths
        .iter()
        .map(|path| {
            crate::script::load(path)
                .map_err(|errors| format!("{}: {}", path.display(), errors.join("; ")))
        })
        .collect()
}

pub fn load_reports(root: impl AsRef<Path>) -> Result<Vec<Value>, String> {
    let mut paths = Vec::new();
    collect_named_files(root.as_ref(), "report.json", &mut paths)?;
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
            serde_json::from_str(&raw).map_err(|error| format!("{}: {error}", path.display()))
        })
        .collect()
}

fn collect_files(root: &Path, suffix: &str, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    if root.is_file() {
        if root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(suffix))
        {
            paths.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            collect_files(&path, suffix, paths)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(suffix))
        {
            paths.push(path);
        }
    }
    Ok(())
}

fn collect_named_files(root: &Path, name: &str, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    if root.is_file() {
        if root.file_name().and_then(|value| value.to_str()) == Some(name) {
            paths.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            collect_named_files(&path, name, paths)?;
        } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            paths.push(path);
        }
    }
    Ok(())
}

pub fn meaningful_score(coverage: &UiCoverage) -> usize {
    coverage.discovery.covered
        + coverage.interaction.covered
        + coverage.state_verification.covered
        + coverage.spatial.covered
        + coverage.negative.covered
        + coverage.regression.covered
}

pub fn improved(before: &UiCoverage, after: &UiCoverage) -> bool {
    meaningful_score(after) > meaningful_score(before)
}

pub fn write_reports(output: impl AsRef<Path>, coverage: &UiCoverage) -> Result<(), String> {
    let output = output.as_ref();
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    fs::write(
        output.join("coverage.json"),
        serde_json::to_vec_pretty(coverage).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let ai = serde_json::json!({
        "interactive_elements":coverage.interactive_elements,
        "covered":coverage.covered_elements,
        "uncovered":coverage.uncovered_elements,
        "critical_uncovered":coverage.critical_uncovered,
        "interaction":coverage.interaction,
        "verification":coverage.state_verification,
        "spatial":coverage.spatial,
        "execution":coverage.execution,
        "global_capabilities":coverage.global_capabilities,
        "stale_generated_tests":coverage.stale_generated_tests,
        "markers":coverage.markers,
    });
    fs::write(
        output.join("ai-coverage.json"),
        serde_json::to_vec_pretty(&ai).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(output.join("coverage.md"), coverage_markdown(coverage))
        .map_err(|error| error.to_string())
}

pub fn coverage_markdown(coverage: &UiCoverage) -> String {
    let critical = coverage
        .critical_uncovered
        .iter()
        .map(|id| format!("- {id}"))
        .collect::<Vec<_>>()
        .join("\n");
    let capabilities = coverage
        .untested_capabilities
        .iter()
        .map(|(name, counts)| {
            format!(
                "- {name}: tested {}/{} targets",
                counts.tested_targets, counts.available_targets
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let collections = coverage
        .collections
        .iter()
        .map(|collection| {
            format!(
                "- {}: {}/{} representatives, interaction={}, verification={}",
                collection.pattern,
                collection.covered_representatives,
                collection.representatives,
                collection.interaction_covered,
                collection.verification_covered
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let stale = coverage
        .stale_generated_tests
        .iter()
        .map(|test| format!("- {}: {}", test.test, test.message))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "# UI coverage\n\n\
         - Semantic discovery: {}% ({}/{})\n\
         - Interaction: {}% ({}/{}) — attempted {}, verified {}\n\
         - Verified interactions: {}% ({}/{})\n\
         - Spatial: {}% ({}/{})\n\
         - Negative: {}% ({}/{})\n\
         - Regression: {}% ({}/{})\n\
         - Execution: planned {}, generated {}, executed {}, passed {}\n\n\
         ## Critical uncovered\n\n{}\n\n\
         ## Capability targets\n\n{}\n\n\
         ## Collection coverage\n\n{}\n\n\
         ## Stale generated tests\n\n{}\n",
        coverage.discovery.percent(),
        coverage.discovery.covered,
        coverage.discovery.available,
        coverage.interaction.percent(),
        coverage.interaction.covered,
        coverage.interaction.available,
        coverage.interaction.attempted,
        coverage.state_verification.verified,
        coverage.state_verification.percent(),
        coverage.state_verification.covered,
        coverage.state_verification.available,
        coverage.spatial.percent(),
        coverage.spatial.covered,
        coverage.spatial.available,
        coverage.negative.percent(),
        coverage.negative.covered,
        coverage.negative.available,
        coverage.regression.percent(),
        coverage.regression.covered,
        coverage.regression.available,
        coverage.execution.planned,
        coverage.execution.generated,
        coverage.execution.executed,
        coverage.execution.passed,
        critical,
        capabilities,
        collections,
        stale,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        inventory::{InventoryOptions, from_public_responses},
        script::{Application, ScriptOptions},
    };
    use serde_json::{Map, json};

    fn inventory() -> UiInventory {
        from_public_responses(
            "demo.exe",
            &json!({"success":true,"result":{"session_id":"s","windows":[]}}),
            &json!({"success":true,"result":{"elements":[
                {
                    "element_id":"e1",
                    "parent_element_id":null,
                    "automation_id":"button.save",
                    "control_type":"Button",
                    "name":"Save",
                    "bounds":{"x":0,"y":0,"width":100,"height":30},
                    "visible":true,
                    "enabled":true,
                    "interactable":true,
                    "capabilities":{"invoke":true,"select":false,"value":false,"expand_collapse":false,"native_action":true,"synthesized_pointer":true}
                }
            ]}}),
            &BTreeMap::new(),
            &InventoryOptions::default(),
        )
        .unwrap()
    }

    fn step(op: &str, id: &str) -> Step {
        Step {
            op: op.into(),
            arguments: Map::from_iter([("id".into(), Value::String(id.into()))]),
            save_as: None,
            save_session_as: None,
            session: None,
            depends_on: vec![],
            expect_error: None,
            step_id: None,
            redact: false,
        }
    }

    fn script(name: &str, steps: Vec<Step>) -> Script {
        Script {
            version: 1,
            name: name.into(),
            application: Application::default(),
            options: ScriptOptions::default(),
            vars: Map::new(),
            setup: vec![],
            steps,
            teardown: vec![],
            cases: vec![],
            tags: vec![],
            generated_by: Some("test-generator".into()),
            generated: true,
            generator_version: Some("1".into()),
            inventory_version: Some(1),
            generated_from: Some("inventory.json".into()),
            intent: Some("test".into()),
            regression: None,
            secret_vars: vec![],
        }
    }

    #[test]
    fn action_without_postcondition_is_not_verification_coverage() {
        let coverage = analyze(
            &inventory(),
            &[script("click", vec![step("click", "button.save")])],
            &[],
        );
        assert_eq!(coverage.interaction.covered, 1);
        assert_eq!(coverage.state_verification.covered, 0);
        assert!(!coverage.elements[0].verification);
    }

    #[test]
    fn postcondition_and_passed_report_prove_verified_coverage() {
        let mut click = step("click", "button.save");
        click.arguments.insert(
            "postcondition".into(),
            json!({"automation_id_visible":"button.save"}),
        );
        let script = script("verified", vec![click]);
        let report = json!({
            "test":"verified",
            "status":"PASSED",
            "steps":[{"status":"PASS","operation":"click"}]
        });
        let coverage = analyze(&inventory(), &[script], &[report]);
        assert_eq!(coverage.interaction.attempted, 1);
        assert_eq!(coverage.state_verification.covered, 1);
        assert_eq!(
            coverage.elements[0].capabilities[0].state,
            CoverageState::ExecutedPassed
        );
    }

    #[test]
    fn failed_action_counts_attempt_but_not_verified() {
        let mut click = step("click", "button.save");
        click.arguments.insert(
            "postcondition".into(),
            json!({"automation_id_visible":"button.save"}),
        );
        let script = script("failed", vec![click]);
        let report = json!({
            "test":"failed",
            "status":"FAILED",
            "steps":[{"status":"FAIL","operation":"click"}]
        });
        let coverage = analyze(&inventory(), &[script], &[report]);
        assert_eq!(coverage.interaction.attempted, 1);
        assert_eq!(coverage.state_verification.verified, 0);
        assert_eq!(
            coverage.elements[0].capabilities[0].state,
            CoverageState::ExecutedFailed
        );
    }

    #[test]
    fn spatial_health_assertion_verifies_capture_capability() {
        let mut inventory = inventory();
        inventory.elements[0].spatial_capabilities = vec![crate::inventory::CapabilityState {
            name: "spatial.capture".into(),
            supported: true,
            available: true,
        }];
        let scenario = script(
            "spatial-health",
            vec![
                step("capture_region", "button.save"),
                step("assert_canvas_healthy", "button.save"),
            ],
        );
        let report = json!({
            "test":"spatial-health",
            "status":"PASSED",
            "steps":[
                {"status":"PASS","operation":"capture_region"},
                {"status":"PASS","operation":"assert_canvas_healthy"}
            ]
        });
        let coverage = analyze(&inventory, &[scenario], &[report]);
        assert_eq!(coverage.spatial.covered, 1);
        assert_eq!(coverage.spatial.verified, 1);
    }

    #[test]
    fn removed_generated_target_is_stale_not_deleted() {
        let coverage = analyze(
            &inventory(),
            &[script("stale", vec![step("click", "button.removed")])],
            &[],
        );
        assert_eq!(coverage.stale_generated_tests.len(), 1);
        assert_eq!(
            coverage.stale_generated_tests[0].code,
            "STALE_GENERATED_TEST"
        );
    }

    #[test]
    fn generated_script_increases_meaningful_coverage() {
        let inventory = inventory();
        let before = analyze(&inventory, &[], &[]);
        let after = analyze(
            &inventory,
            &[script("click", vec![step("click", "button.save")])],
            &[],
        );
        assert!(improved(&before, &after));
    }

    #[test]
    fn execution_counts_scenarios_and_plan_states_are_explicit() {
        use crate::{
            generator::{BlockedIntent, FixtureRequirement, GeneratorMode, TestPlan},
            inventory::SafetyClass,
        };

        let mut click = step("click", "button.save");
        click.arguments.insert(
            "postcondition".into(),
            json!({"automation_id_visible":"button.save"}),
        );
        let scenario = script(
            "two-steps",
            vec![click, step("assert_visible", "button.save")],
        );
        let report = json!({
            "test":"two-steps",
            "status":"PASSED",
            "steps":[
                {"status":"PASS","operation":"click"},
                {"status":"PASS","operation":"assert_visible"}
            ]
        });
        let executed = analyze(&inventory(), &[scenario], &[report]);
        assert_eq!(executed.execution.planned, 1);
        assert_eq!(executed.execution.generated, 1);
        assert_eq!(executed.execution.executed, 1);
        assert_eq!(executed.execution.passed, 1);

        let mut value_inventory = inventory();
        value_inventory.elements[0].capabilities[0].name = "value".into();
        let base_plan = TestPlan {
            version: 1,
            generator_version: "1".into(),
            inventory_version: 1,
            mode: GeneratorMode::Standard,
            planned: 1,
            safe: 0,
            requires_fixture: 1,
            blocked_destructive: 0,
            intents: vec![],
            blocked: vec![],
            fixture_requirements: vec![FixtureRequirement {
                element: "button.save".into(),
                fixture: "domain-value".into(),
                secret: false,
                reason: "fixture required".into(),
            }],
            markers: vec![],
        };
        let waiting = analyze_with_plan(&value_inventory, &[], &[], Some(&base_plan));
        assert_eq!(
            waiting.elements[0].capabilities[0].state,
            CoverageState::RequiresFixture
        );

        let mut blocked_plan = base_plan;
        blocked_plan.fixture_requirements.clear();
        blocked_plan.blocked.push(BlockedIntent {
            target: "button.save".into(),
            template: "edit.value".into(),
            safety: SafetyClass::Unknown,
            code: "UNSAFE_OR_UNKNOWN_MUTATION".into(),
            reason: "blocked".into(),
        });
        let blocked = analyze_with_plan(&value_inventory, &[], &[], Some(&blocked_plan));
        assert_eq!(
            blocked.elements[0].capabilities[0].state,
            CoverageState::Blocked
        );
    }
}
