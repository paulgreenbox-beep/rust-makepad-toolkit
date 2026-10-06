//! Reusable generation and audit workflow built only on Public Tool API.

use crate::{
    api::{AiUiClient, Transport},
    contract::{CONTRACT_VERSION, SCHEMA_VERSION, TOOL_VERSION},
    coverage::{self, UiCoverage},
    fixtures::FixtureStore,
    generator::{
        self, AiTestIntent, GeneratedScenario, GenerationReport, GeneratorConfig, GeneratorMode,
        TestPlan,
    },
    inventory::{self, UiInventory},
    runner::{self, ScriptReport},
    script::Script,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedGeneration {
    pub inventory: UiInventory,
    pub coverage_before: UiCoverage,
    pub plan: TestPlan,
    pub scenarios: Vec<GeneratedScenario>,
    pub generation_report: GenerationReport,
    pub coverage_generated: UiCoverage,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExecutionSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub failed_tests: Vec<String>,
    pub reports: Vec<Value>,
    pub markers: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditReport {
    pub version: u32,
    pub status: String,
    pub application: String,
    pub generated_files: Vec<String>,
    pub execution: ExecutionSummary,
    pub coverage_before_score: usize,
    pub coverage_after_score: usize,
    pub coverage_improved: bool,
    pub evidence_path: Option<String>,
    pub markers: Vec<String>,
}

pub fn prepare_generation(
    inventory: &UiInventory,
    existing_scripts: &[Script],
    existing_reports: &[Value],
    config: &GeneratorConfig,
    fixtures: &FixtureStore,
    mode: GeneratorMode,
    ai_intents: &[AiTestIntent],
) -> Result<PreparedGeneration, String> {
    let coverage_before = coverage::analyze(inventory, existing_scripts, existing_reports);
    let mut plan = generator::build_plan(inventory, &coverage_before, config, mode)
        .map_err(|errors| errors.join("; "))?;
    generator::resolve_fixtures(inventory, &mut plan, config, fixtures)
        .map_err(|errors| errors.join("; "))?;
    if !ai_intents.is_empty() {
        generator::append_ai_intents(&mut plan, inventory, config, ai_intents).map_err(
            |errors| {
                errors
                    .into_iter()
                    .map(|error| format!("{}[{}]: {}", error.code, error.index, error.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            },
        )?;
    }
    let (scenarios, generation_report) =
        generator::compile_plan(inventory, &plan).map_err(|errors| errors.join("; "))?;
    generator::validate_compiled_scenarios(inventory, &scenarios)
        .map_err(|errors| errors.join("; "))?;
    for scenario in &scenarios {
        let value = serde_json::to_value(&scenario.script).map_err(|error| error.to_string())?;
        crate::script::validate_schema_value(&value).map_err(|errors| errors.join("; "))?;
    }
    let mut all_scripts = existing_scripts.to_vec();
    all_scripts.extend(scenarios.iter().map(|scenario| scenario.script.clone()));
    let coverage_generated =
        coverage::analyze_with_plan(inventory, &all_scripts, existing_reports, Some(&plan));
    Ok(PreparedGeneration {
        inventory: inventory.clone(),
        coverage_before,
        plan,
        scenarios,
        generation_report,
        coverage_generated,
    })
}

pub fn write_prepared(
    output: impl AsRef<Path>,
    prepared: &PreparedGeneration,
    plan_only: bool,
) -> Result<Vec<PathBuf>, String> {
    let output = output.as_ref();
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    write_json(output.join("inventory.json"), &prepared.inventory)?;
    write_json(output.join("test-plan.json"), &prepared.plan)?;
    write_json(
        output.join("generation-report.json"),
        &prepared.generation_report,
    )?;
    if plan_only {
        return Ok(Vec::new());
    }
    let paths = generator::write_generation(
        output,
        &prepared.inventory,
        &prepared.plan,
        &prepared.scenarios,
        &prepared.generation_report,
    )?;
    coverage::write_reports(output, &prepared.coverage_generated)?;
    Ok(paths)
}

pub fn execute_scenarios<T: Transport>(
    client: &mut AiUiClient<T>,
    scenarios: &[GeneratedScenario],
    fixtures: &FixtureStore,
    reports_root: impl AsRef<Path>,
) -> Result<ExecutionSummary, String> {
    let reports_root = reports_root.as_ref();
    let mut summary = ExecutionSummary {
        total: scenarios.len(),
        ..Default::default()
    };
    for scenario in scenarios {
        let report_dir = reports_root.join(report_directory(&scenario.relative_path));
        let report = runner::run_with_fixtures(client, &scenario.script, &report_dir, fixtures)?;
        if report.status == "PASSED" {
            summary.passed += 1;
        } else {
            summary.failed += 1;
            summary.failed_tests.push(scenario.script.name.clone());
        }
        summary
            .reports
            .push(serde_json::to_value(&report).map_err(|error| error.to_string())?);
    }
    summary.failed_tests.sort();
    if summary.failed == 0 && summary.total > 0 {
        summary
            .markers
            .push("AI_UI_GENERATED_SCRIPT_EXECUTION_PASSED".into());
    }
    Ok(summary)
}

fn report_directory(relative_path: &Path) -> PathBuf {
    let mut path = relative_path.to_path_buf();
    path.set_extension("");
    path
}

pub fn audit<T: Transport>(
    client: &mut AiUiClient<T>,
    application: &str,
    window_title: Option<&str>,
    output: impl AsRef<Path>,
    config: &GeneratorConfig,
    fixtures: &FixtureStore,
    mode: GeneratorMode,
    existing_scripts: &[Script],
    existing_reports: &[Value],
    ai_intents: &[AiTestIntent],
    validate_evidence: bool,
) -> Result<AuditReport, String> {
    let output = output.as_ref();
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    let inventory = inventory::collect(
        client,
        application,
        window_title,
        &config.inventory_options(),
    )?;
    let prepared = prepare_generation(
        &inventory,
        existing_scripts,
        existing_reports,
        config,
        fixtures,
        mode,
        ai_intents,
    )?;
    write_json(output.join("inventory.json"), &prepared.inventory)?;
    write_json(output.join("test-plan.json"), &prepared.plan)?;
    write_json(
        output.join("coverage-before.json"),
        &prepared.coverage_before,
    )?;
    let generated_root = output.join("generated");
    generator::write_generation(
        &generated_root,
        &prepared.inventory,
        &prepared.plan,
        &prepared.scenarios,
        &prepared.generation_report,
    )?;
    let execution = execute_scenarios(
        client,
        &prepared.scenarios,
        fixtures,
        output.join("reports"),
    )?;
    let mut scripts = existing_scripts.to_vec();
    scripts.extend(
        prepared
            .scenarios
            .iter()
            .map(|scenario| scenario.script.clone()),
    );
    let mut reports = existing_reports.to_vec();
    reports.extend(execution.reports.clone());
    let mut coverage_after = coverage::analyze_with_plan(
        &prepared.inventory,
        &scripts,
        &reports,
        Some(&prepared.plan),
    );
    let improved = coverage::improved(&prepared.coverage_before, &coverage_after);
    if improved {
        coverage_after
            .markers
            .push("AI_UI_COVERAGE_IMPROVEMENT_PASSED".into());
    }
    write_json(output.join("coverage-after.json"), &coverage_after)?;
    coverage::write_reports(output, &coverage_after)?;

    let (evidence_path, evidence_valid) = if validate_evidence {
        validate_audit_evidence(client, application, window_title)?
    } else {
        (None, true)
    };
    let mut generation_report = prepared.generation_report.clone();
    if execution.failed == 0 && execution.total > 0 {
        generation_report = generator::generation_report_with_execution_marker(&generation_report);
    }
    write_json(output.join("generation-report.json"), &generation_report)?;
    write_json(
        generated_root.join("generation-report.json"),
        &generation_report,
    )?;

    let passed = execution.failed == 0 && execution.total > 0 && improved && evidence_valid;
    let mut markers = prepared.inventory.markers.clone();
    markers.extend(prepared.plan.markers.clone());
    markers.extend(generation_report.markers.clone());
    markers.extend(coverage_after.markers.clone());
    markers.extend(execution.markers.clone());
    if evidence_valid && validate_evidence {
        markers.push("AI_UI_EVIDENCE_CONSISTENCY_PASSED".into());
    }
    if passed {
        markers.push("AI_UI_AUDIT_PIPELINE_PASSED".into());
    }
    markers.sort();
    markers.dedup();
    let report = AuditReport {
        version: 1,
        status: if passed { "PASSED" } else { "FAILED" }.into(),
        application: application.into(),
        generated_files: generation_report.generated_files.clone(),
        execution,
        coverage_before_score: coverage::meaningful_score(&prepared.coverage_before),
        coverage_after_score: coverage::meaningful_score(&coverage_after),
        coverage_improved: improved,
        evidence_path,
        markers,
    };
    write_json(output.join("ai-report.json"), &report)?;
    Ok(report)
}

fn validate_audit_evidence<T: Transport>(
    client: &mut AiUiClient<T>,
    application: &str,
    window_title: Option<&str>,
) -> Result<(Option<String>, bool), String> {
    let open = client.call(
        "session.open",
        json!({"application":application,"window_title":window_title}),
    );
    let session_id = open
        .pointer("/result/session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "evidence session.open returned no session_id".to_string())?
        .to_owned();
    let _ = client.call("get_runtime_health", json!({"session_id":session_id}));
    let export = client.call(
        "windows_export_evidence",
        json!({"session_id":session_id,"reason":"generator-audit"}),
    );
    let path = export
        .pointer("/result/path")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let validation = if let Some(path) = path.as_deref() {
        let actions_path = Path::new(path).join("actions.json");
        let actions: Value = serde_json::from_str(
            &fs::read_to_string(&actions_path).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        client.call(
            "windows_validate_evidence",
            json!({
                "session_id":session_id,
                "bundle":{
                    "schema_version":SCHEMA_VERSION,
                    "tool_version":TOOL_VERSION,
                    "contract_version":CONTRACT_VERSION,
                    "actions":actions
                }
            }),
        )
    } else {
        export
    };
    let _ = client.call("session.close", json!({"session_id":session_id}));
    Ok((
        path,
        validation.get("success").and_then(Value::as_bool) == Some(true),
    ))
}

pub fn write_json(path: impl AsRef<Path>, value: &impl Serialize) -> Result<(), String> {
    if let Some(parent) = path.as_ref().parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

pub fn reports_from_script_reports(reports: &[ScriptReport]) -> Result<Vec<Value>, String> {
    reports
        .iter()
        .map(|report| serde_json::to_value(report).map_err(|error| error.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{AiUiClient, InProcessTransport};
    use serde_json::json;

    #[test]
    fn public_api_workflow_generates_executes_and_improves_coverage() {
        let transport = InProcessTransport::new(|operation: &str, arguments: Value| {
            let result = match operation {
                "session.open" => json!({
                    "session_id":"s",
                    "windows":[{
                        "window_id":"w",
                        "name":"Demo",
                        "automation_id":"window.main",
                        "bounds":{"x":0,"y":0,"width":800,"height":600},
                        "visible":true
                    }]
                }),
                "inspect_ui" => json!({"elements":[{
                    "element_id":"e1",
                    "automation_id":"window.main/body/button.create",
                    "control_type":"Button",
                    "name":"Create",
                    "bounds":{"x":20,"y":20,"width":120,"height":32},
                    "visible":true,
                    "offscreen":false,
                    "enabled":true,
                    "interactable":true,
                    "capabilities":{
                        "invoke":true,
                        "select":false,
                        "value":false,
                        "expand_collapse":false,
                        "native_action":true,
                        "synthesized_pointer":true
                    }
                }]}),
                "acquire_input_lock" => json!({"token":"lock"}),
                "release_input_lock" => json!(true),
                "get_runtime_health" => json!({"healthy":true}),
                _ => json!({"accepted":true,"arguments":arguments}),
            };
            json!({"operation":operation,"success":true,"result":result})
        });
        let mut client = AiUiClient::new(transport);
        let output =
            std::env::temp_dir().join(format!("ai-ui-workflow-audit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&output);
        let report = audit(
            &mut client,
            "demo.exe",
            Some("Demo"),
            &output,
            &GeneratorConfig::default(),
            &FixtureStore::default(),
            GeneratorMode::Standard,
            &[],
            &[],
            &[],
            false,
        )
        .unwrap();
        assert_eq!(report.status, "PASSED");
        assert!(report.coverage_improved);
        assert!(report.execution.total > 0);
        assert_eq!(report.execution.failed, 0);
        assert!(
            report
                .markers
                .iter()
                .any(|marker| marker == "AI_UI_COVERAGE_IMPROVEMENT_PASSED")
        );
        assert!(
            !report
                .markers
                .iter()
                .any(|marker| marker == "AI_UI_TEST_GENERATOR_PASSED")
        );
        assert!(output.join("coverage-before.json").is_file());
        assert!(output.join("coverage-after.json").is_file());
        assert!(output.join("ai-report.json").is_file());
        let _ = fs::remove_dir_all(output);
    }
}
