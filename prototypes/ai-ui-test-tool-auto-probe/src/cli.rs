//! Command-line surface for inventory, coverage, generation and audit.

use crate::{
    coverage,
    fixtures::FixtureStore,
    generator::{self, AiTestIntent, GeneratorConfig, GeneratorMode, TestPlan},
    inventory::{self, UiInventory},
    script::{self, Script},
    workflow,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    str::FromStr,
};

const ASYNC_FAILURE_MODE_ENV: &str = "AI_UI_TEST_ASYNC_FAILURE_MODE";

#[derive(Clone, Debug, Default)]
struct CliArgs {
    positionals: Vec<String>,
    options: BTreeMap<String, String>,
    switches: BTreeSet<String>,
}

impl CliArgs {
    fn option(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(String::as_str)
    }

    fn switch(&self, name: &str) -> bool {
        self.switches.contains(name)
    }

    fn path(&self, name: &str) -> Option<PathBuf> {
        self.option(name).map(PathBuf::from)
    }
}

pub fn main_entry() {
    let mut raw = env::args().skip(1);
    let command = raw.next().unwrap_or_else(|| "help".into());
    let args = parse_args(raw.collect());
    if let Err(error) = dispatch(&command, &args) {
        eprintln!("{error}");
        std::process::exit(
            if error.starts_with("invalid") || error.starts_with("missing") {
                2
            } else {
                3
            },
        );
    }
}

fn parse_args(raw: Vec<String>) -> CliArgs {
    let switches = [
        "plan-only",
        "run",
        "coverage-gaps",
        "regenerate-all",
        "no-evidence",
        "help",
    ];
    let mut parsed = CliArgs::default();
    let mut index = 0;
    while index < raw.len() {
        let value = &raw[index];
        if let Some(flag) = value.strip_prefix("--") {
            if let Some((name, value)) = flag.split_once('=') {
                parsed.options.insert(name.into(), value.into());
            } else if switches.contains(&flag) {
                parsed.switches.insert(flag.into());
            } else if raw
                .get(index + 1)
                .is_some_and(|next| !next.starts_with("--"))
            {
                parsed.options.insert(flag.into(), raw[index + 1].clone());
                index += 1;
            } else {
                parsed.switches.insert(flag.into());
            }
        } else {
            parsed.positionals.push(value.clone());
        }
        index += 1;
    }
    parsed
}

fn dispatch(command: &str, args: &CliArgs) -> Result<(), String> {
    match command {
        "validate" => validate_command(required_path(args, 0, "script path")?),
        "normalize" => normalize_command(required_path(args, 0, "script path")?),
        "dry-run" => dry_run_command(required_path(args, 0, "script path")?),
        "list" => list_command(
            args.positionals
                .first()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".")),
        ),
        "run" => run_command(required_path(args, 0, "script or directory path")?, args),
        "inventory" => inventory_command(args),
        "coverage" => coverage_command(args),
        "generate" => generate_command(args),
        "audit" => audit_command(args),
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        other => Err(format!("invalid command: {other}")),
    }
}

fn print_usage() {
    println!(
        "ai-ui-test inventory <app.exe> [--output DIR] [--window-title TITLE]\n\
         ai-ui-test coverage <tests/> [--inventory inventory.json] [--reports DIR]\n\
         ai-ui-test generate <app.exe> [--inventory FILE] [--mode smoke|standard|exhaustive]\n\
             [--output generated-tests/] [--plan-only] [--run]\n\
         ai-ui-test audit <app.exe> [--output audit/] [--mode standard]\n\
         ai-ui-test validate|normalize|dry-run <script.aiui.json>\n\
         ai-ui-test list|run <path> [--fixtures ai-ui-fixtures.json]"
    );
}

fn required_path(args: &CliArgs, index: usize, what: &str) -> Result<PathBuf, String> {
    args.positionals
        .get(index)
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {what}"))
}

fn validate_command(path: PathBuf) -> Result<(), String> {
    let value = read_json(&path)?;
    if value.get("tests").is_some() {
        let suite = crate::suite::load(&path)?;
        crate::suite::validate(&suite, path.parent().unwrap_or_else(|| Path::new(".")))
            .map_err(|errors| format!("invalid suite: {}", errors.join("; ")))?;
        println!("VALID SUITE: {} ({} tests)", suite.name, suite.tests.len());
    } else {
        let script = script::from_value(value)
            .map_err(|errors| format!("invalid script: {}", errors.join("; ")))?;
        println!(
            "VALID SCHEMA + SEMANTICS: {} ({} steps)",
            script.name,
            script.steps.len()
        );
    }
    Ok(())
}

fn normalize_command(path: PathBuf) -> Result<(), String> {
    let script =
        script::load(&path).map_err(|errors| format!("invalid script: {}", errors.join("; ")))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&script).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn dry_run_command(path: PathBuf) -> Result<(), String> {
    let script =
        script::load(&path).map_err(|errors| format!("invalid script: {}", errors.join("; ")))?;
    println!("DRY RUN: {}", script.name);
    for (index, step) in script
        .setup
        .iter()
        .chain(script.steps.iter())
        .chain(script.teardown.iter())
        .enumerate()
    {
        let lock =
            crate::catalog::operation(&step.op).is_some_and(|operation| operation.physical_input);
        println!(
            "{index}: {}{}{}",
            step.op,
            if lock { " [physical-lock]" } else { "" },
            if step.redact { " [redacted]" } else { "" }
        );
    }
    println!("AI_UI_GENERATED_SCRIPT_VALIDATION_PASSED");
    Ok(())
}

fn list_command(path: PathBuf) -> Result<(), String> {
    for path in script_paths(&path)? {
        println!("{}", path.display());
    }
    Ok(())
}

fn inventory_command(args: &CliArgs) -> Result<(), String> {
    let application = required_path(args, 0, "application path")?;
    let config = load_config(args)?;
    let output = args
        .path("output")
        .unwrap_or_else(|| PathBuf::from("inventory.json"));
    let inventory = collect_live_inventory(
        &application,
        args.option("window-title"),
        output_parent(&output).join("runtime"),
        &config,
    )?;
    let inventory_path = if output.extension().and_then(|value| value.to_str()) == Some("json") {
        output.clone()
    } else {
        output.join("inventory.json")
    };
    workflow::write_json(&inventory_path, &inventory)?;
    if let Some(previous) = args.path("compare") {
        let before = generator::load_inventory(previous)?;
        let diff = inventory::diff(&before, &inventory);
        let root = inventory_path.parent().unwrap_or_else(|| Path::new("."));
        workflow::write_json(root.join("inventory-diff.json"), &diff)?;
        fs::write(
            root.join("ui-change-report.md"),
            inventory::diff_markdown(&diff),
        )
        .map_err(|error| error.to_string())?;
    }
    println!(
        "AI_UI_INVENTORY_PASSED elements={} issues={} path={}",
        inventory.elements.len(),
        inventory.quality_issues.len(),
        inventory_path.display()
    );
    Ok(())
}

fn coverage_command(args: &CliArgs) -> Result<(), String> {
    let tests = args
        .positionals
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let inventory_path = args
        .path("inventory")
        .or_else(|| find_inventory(&tests))
        .ok_or_else(|| "missing inventory; pass --inventory inventory.json".to_string())?;
    let inventory = generator::load_inventory(&inventory_path)?;
    let scripts = if tests.exists() {
        coverage::load_scripts(&tests)?
    } else {
        Vec::new()
    };
    let reports_root = args
        .path("reports")
        .unwrap_or_else(|| PathBuf::from("reports"));
    let reports = if reports_root.exists() {
        coverage::load_reports(&reports_root)?
    } else {
        Vec::new()
    };
    let plan_path = args
        .path("plan")
        .or_else(|| {
            inventory_path
                .parent()
                .map(|root| root.join("test-plan.json"))
        })
        .filter(|path| path.is_file());
    let plan = plan_path.as_deref().map(load_plan).transpose()?;
    let report = coverage::analyze_with_plan(&inventory, &scripts, &reports, plan.as_ref());
    let output = args
        .path("output")
        .unwrap_or_else(|| tests.parent().unwrap_or(&tests).to_path_buf());
    coverage::write_reports(&output, &report)?;
    println!(
        "AI_UI_COVERAGE_ANALYSIS_PASSED covered={}/{} interaction={} verification={} spatial={}",
        report.covered_elements,
        report.total_semantic_elements,
        report.interaction.percent(),
        report.state_verification.percent(),
        report.spatial.percent()
    );
    Ok(())
}

fn generate_command(args: &CliArgs) -> Result<(), String> {
    let output = args
        .path("output")
        .unwrap_or_else(|| PathBuf::from("generated-tests"));
    let mut config = load_config(args)?;
    if args.switch("regenerate-all") {
        config.regenerate_all = true;
    }
    if args.switch("coverage-gaps") {
        config.regenerate_all = false;
    }
    let mode = GeneratorMode::from_str(args.option("mode").unwrap_or("standard"))?;
    let fixtures = load_fixtures(args)?;
    let inventory = if let Some(path) = args.path("inventory") {
        generator::load_inventory(path)?
    } else {
        let application = required_path(args, 0, "application path or --inventory")?;
        collect_live_inventory(
            &application,
            args.option("window-title"),
            output.join("runtime"),
            &config,
        )?
    };
    let existing_root = args.path("tests").unwrap_or_else(|| output.clone());
    let existing_scripts = if existing_root.exists() {
        coverage::load_scripts(&existing_root)?
    } else {
        Vec::new()
    };
    let reports_root = args
        .path("reports")
        .unwrap_or_else(|| output.join("reports"));
    let existing_reports = if reports_root.exists() {
        coverage::load_reports(&reports_root)?
    } else {
        Vec::new()
    };
    let ai_intents = load_ai_intents(args)?;
    let prepared = workflow::prepare_generation(
        &inventory,
        &existing_scripts,
        &existing_reports,
        &config,
        &fixtures,
        mode,
        &ai_intents,
    )?;
    let plan_only = args.switch("plan-only");
    let paths = workflow::write_prepared(&output, &prepared, plan_only)?;
    if plan_only {
        println!(
            "AI_UI_TEST_PLAN_PASSED planned={} safe={} fixtures={} blocked={}",
            prepared.plan.planned,
            prepared.plan.safe,
            prepared.plan.requires_fixture,
            prepared.plan.blocked_destructive
        );
        return Ok(());
    }
    if args.switch("run") {
        let execution = execute_generated_live(
            &inventory,
            &prepared.scenarios,
            &fixtures,
            &reports_root,
            args.option("window-title"),
            output.join("runtime"),
        )?;
        let mut scripts = existing_scripts;
        scripts.extend(
            prepared
                .scenarios
                .iter()
                .map(|scenario| scenario.script.clone()),
        );
        let mut reports = existing_reports;
        reports.extend(execution.reports.clone());
        let mut after =
            coverage::analyze_with_plan(&inventory, &scripts, &reports, Some(&prepared.plan));
        if coverage::improved(&prepared.coverage_before, &after) {
            after
                .markers
                .push("AI_UI_COVERAGE_IMPROVEMENT_PASSED".into());
        }
        coverage::write_reports(&output, &after)?;
        let report = if execution.failed == 0 {
            generator::generation_report_with_execution_marker(&prepared.generation_report)
        } else {
            prepared.generation_report.clone()
        };
        workflow::write_json(output.join("generation-report.json"), &report)?;
        if execution.failed != 0 {
            return Err(format!(
                "generated tests failed: {}",
                execution.failed_tests.join(", ")
            ));
        }
    }
    println!(
        "AI_UI_GENERATED_SCRIPT_VALIDATION_PASSED files={} output={}",
        paths.len(),
        output.display()
    );
    Ok(())
}

#[cfg(windows)]
fn run_command(path: PathBuf, args: &CliArgs) -> Result<(), String> {
    let fixtures = load_fixtures(args)?;
    let output = args
        .path("output")
        .unwrap_or_else(|| PathBuf::from("reports").join("script"));
    if path.is_dir() {
        let scripts = coverage::load_scripts(&path)?;
        let mut reports = Vec::new();
        let mut failed = Vec::new();
        for script in &scripts {
            let report = run_script_live(script, &fixtures, &output)?;
            if report.status != "PASSED" {
                failed.push(script.name.clone());
            }
            reports.push(report);
        }
        let summary = serde_json::json!({
            "status":if failed.is_empty() {"PASSED"} else {"FAILED"},
            "total":reports.len(),
            "failed":failed,
            "reports":reports,
        });
        workflow::write_json(output.join("directory-summary.json"), &summary)?;
        println!(
            "{}",
            serde_json::to_string(&summary).map_err(|error| error.to_string())?
        );
        return if summary["status"] == "PASSED" {
            Ok(())
        } else {
            Err("script assertions failed".into())
        };
    }
    let value = read_json(&path)?;
    if value.get("tests").is_some() {
        let suite = crate::suite::load(&path)?;
        let root = path.parent().unwrap_or_else(|| Path::new("."));
        let paths = crate::suite::validate(&suite, root).map_err(|errors| errors.join("; "))?;
        let scripts = paths
            .iter()
            .map(|path| {
                script::load(path)
                    .map_err(|errors| format!("invalid script: {}", errors.join("; ")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let report = crate::suite::aggregate(&suite, &scripts, |script| {
            run_script_live(script, &fixtures, &output)
        })?;
        workflow::write_json(output.join("suite-summary.json"), &report)?;
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|error| error.to_string())?
        );
        return if report.status == "PASSED" {
            Ok(())
        } else {
            Err("suite assertions failed".into())
        };
    }
    let script = script::from_value(value)
        .map_err(|errors| format!("invalid script: {}", errors.join("; ")))?;
    let report = run_script_live(&script, &fixtures, &output)?;
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|error| error.to_string())?
    );
    if report.status == "PASSED" {
        Ok(())
    } else {
        Err("script assertions failed".into())
    }
}

#[cfg(not(windows))]
fn run_command(path: PathBuf, args: &CliArgs) -> Result<(), String> {
    if env::var("AI_UI_TRANSPORT").as_deref() != Ok("environment_probe") {
        return Err("runner infrastructure failure: real script execution requires Windows".into());
    }
    let environment_id = env::var("AI_UI_ENVIRONMENT_ID").map_err(|_| {
        "runner infrastructure failure: environment probe has no trusted environment identity"
            .to_string()
    })?;
    let probe_mode = env::var("AI_UI_ENVIRONMENT_PROBE_MODE").unwrap_or_else(|_| "pass".into());
    if !matches!(
        probe_mode.as_str(),
        "pass" | "test_failed" | "infrastructure_failed" | "wait"
    ) {
        return Err("runner infrastructure failure: invalid trusted probe mode".into());
    }
    let probe_state = environment_probe_state(&environment_id)?;
    if path.is_dir() {
        return Err("runner infrastructure failure: environment probe accepts one script".into());
    }
    let value = read_json(&path)?;
    if value.get("tests").is_some() {
        return Err(
            "runner infrastructure failure: environment probe does not execute suites".into(),
        );
    }
    let script = script::from_value(value)
        .map_err(|errors| format!("invalid script: {}", errors.join("; ")))?;
    if script
        .setup
        .iter()
        .chain(script.steps.iter())
        .chain(script.teardown.iter())
        .any(|step| step.op != "get_runtime_health")
    {
        return Err(
            "runner infrastructure failure: environment probe permits only get_runtime_health"
                .into(),
        );
    }
    let fixtures = load_fixtures(args)?;
    let output = args
        .path("output")
        .unwrap_or_else(|| PathBuf::from("reports").join("script"));
    use crate::api::{AiUiClient, InProcessTransport};
    let transport = InProcessTransport::new(move |operation: &str, _arguments: Value| {
        if operation != "get_runtime_health" {
            return serde_json::json!({
                "operation": operation,
                "success": false,
                "error": {
                    "code": "ENVIRONMENT_PROBE_OPERATION_DENIED",
                    "message": "environment probe permits only get_runtime_health"
                }
            });
        }
        if probe_mode == "wait" {
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
        if probe_mode == "test_failed" {
            return serde_json::json!({
                "operation": operation,
                "success": false,
                "error": {
                    "code": "POSTCONDITION_FAILED",
                    "message": "trusted environment probe test failure"
                }
            });
        }
        if probe_mode == "infrastructure_failed" {
            return serde_json::json!({
                "operation": operation,
                "success": false,
                "error": {
                    "code": "RUNNER_ENVIRONMENT_PROBE_FAILED",
                    "message": "trusted environment probe infrastructure failure"
                }
            });
        }
        serde_json::json!({
            "operation": operation,
            "success": true,
            "result": {
                "runtime_healthy": true,
                "environment_id": environment_id,
                "attempt_id": env::var("AI_UI_ATTEMPT_ID").ok(),
                "capability_level": "level0",
                "transport": "environment_probe",
                "filesystem_probe": probe_state.clone()
            }
        })
    });
    let mut client = AiUiClient::new(transport);
    let mut report = crate::runner::run_with_fixtures(&mut client, &script, &output, &fixtures)?;
    if report.status == "PASSED" {
        report.markers.push("AI_UI_ENVIRONMENT_PROBE_PASSED".into());
        report.markers.sort();
        report.markers.dedup();
        workflow::write_json(output.join("report.json"), &report)?;
    }
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|error| error.to_string())?
    );
    if report.status == "PASSED" {
        Ok(())
    } else {
        Err("environment probe assertions failed".into())
    }
}

#[cfg(not(windows))]
fn environment_probe_state(environment_id: &str) -> Result<Value, String> {
    let root = env::temp_dir().join("ai-ui-environment-probe");
    let state_path = root.join("state.json");
    let preexisting_state = fs::read_to_string(&state_path).ok();
    fs::create_dir_all(&root).map_err(|error| {
        format!("runner infrastructure failure: create isolated probe state: {error}")
    })?;
    fs::write(&state_path, environment_id).map_err(|error| {
        format!("runner infrastructure failure: write isolated probe state: {error}")
    })?;
    let state_roundtrip = fs::read_to_string(&state_path).map_err(|error| {
        format!("runner infrastructure failure: read isolated probe state: {error}")
    })?;
    let roundtrip_matches_environment = state_roundtrip == environment_id;
    Ok(serde_json::json!({
        "preexisting_state": preexisting_state,
        "state_roundtrip": state_roundtrip,
        "roundtrip_matches_environment": roundtrip_matches_environment,
        "state_path": state_path,
        "profile_roots": {
            "home": env::var("HOME").ok(),
            "temp": env::var("TEMP").ok(),
            "tmp": env::var("TMP").ok(),
            "appdata": env::var("APPDATA").ok(),
            "localappdata": env::var("LOCALAPPDATA").ok(),
            "xdg_cache_home": env::var("XDG_CACHE_HOME").ok(),
            "xdg_config_home": env::var("XDG_CONFIG_HOME").ok(),
            "xdg_data_home": env::var("XDG_DATA_HOME").ok()
        }
    }))
}

#[cfg(windows)]
fn audit_command(args: &CliArgs) -> Result<(), String> {
    use crate::{
        api::{AiUiClient, InProcessTransport},
        reliability::ReliabilityManager,
    };

    let application = required_path(args, 0, "application path")?;
    let output = args
        .path("output")
        .unwrap_or_else(|| PathBuf::from("audit-report"));
    let config = load_config(args)?;
    let fixtures = load_fixtures(args)?;
    let mode = GeneratorMode::from_str(args.option("mode").unwrap_or("standard"))?;
    let ai_intents = load_ai_intents(args)?;
    let existing_scripts = args
        .path("tests")
        .filter(|path| path.exists())
        .map(coverage::load_scripts)
        .transpose()?
        .unwrap_or_default();
    let existing_reports = args
        .path("reports")
        .filter(|path| path.exists())
        .map(coverage::load_reports)
        .transpose()?
        .unwrap_or_default();
    let title = effective_title(args.option("window-title"));
    let mut manager = ReliabilityManager::new(&application, &title, output.join("runtime"));
    let transport = InProcessTransport::new(move |operation: &str, arguments: Value| {
        manager.call(operation, arguments)
    });
    let mut client = AiUiClient::new(transport);
    let mut report = workflow::audit(
        &mut client,
        &application.to_string_lossy(),
        Some(&title),
        &output,
        &config,
        &fixtures,
        mode,
        &existing_scripts,
        &existing_reports,
        &ai_intents,
        !args.switch("no-evidence"),
    )?;
    if report.status == "PASSED" {
        report.markers.push("AI_UI_TEST_GENERATOR_PASSED".into());
        report.markers.sort();
        report.markers.dedup();
        workflow::write_json(output.join("ai-report.json"), &report)?;
        workflow::write_json(output.join("final-report.json"), &report)?;
        println!("AI_UI_TEST_GENERATOR_PASSED");
        Ok(())
    } else {
        workflow::write_json(output.join("final-report.json"), &report)?;
        Err("audit did not satisfy execution/coverage/evidence gates".into())
    }
}

#[cfg(not(windows))]
fn audit_command(_args: &CliArgs) -> Result<(), String> {
    Err("audit requires the real Windows Public Tool API adapter".into())
}

#[cfg(windows)]
fn collect_live_inventory(
    application: &Path,
    window_title: Option<&str>,
    runtime_output: PathBuf,
    config: &GeneratorConfig,
) -> Result<UiInventory, String> {
    use crate::{
        api::{AiUiClient, InProcessTransport},
        reliability::ReliabilityManager,
    };

    let title = window_title
        .map(str::to_owned)
        .unwrap_or_else(effective_title_default);
    let mut manager = ReliabilityManager::new(application, &title, runtime_output);
    let transport = InProcessTransport::new(move |operation: &str, arguments: Value| {
        manager.call(operation, arguments)
    });
    let mut client = AiUiClient::new(transport);
    inventory::collect(
        &mut client,
        &application.to_string_lossy(),
        Some(&title),
        &config.inventory_options(),
    )
}

#[cfg(not(windows))]
fn collect_live_inventory(
    _application: &Path,
    _window_title: Option<&str>,
    _runtime_output: PathBuf,
    _config: &GeneratorConfig,
) -> Result<UiInventory, String> {
    Err("inventory requires the real Windows Public Tool API adapter".into())
}

#[cfg(windows)]
fn execute_generated_live(
    inventory: &UiInventory,
    scenarios: &[generator::GeneratedScenario],
    fixtures: &FixtureStore,
    reports_root: &Path,
    requested_title: Option<&str>,
    runtime_output: PathBuf,
) -> Result<workflow::ExecutionSummary, String> {
    use crate::{
        api::{AiUiClient, InProcessTransport},
        reliability::ReliabilityManager,
    };

    let title = requested_title
        .map(str::to_owned)
        .or_else(|| inventory.windows.first().map(|window| window.name.clone()))
        .unwrap_or_else(effective_title_default);
    let mut manager = ReliabilityManager::new(&inventory.application, &title, runtime_output);
    let transport = InProcessTransport::new(move |operation: &str, arguments: Value| {
        manager.call(operation, arguments)
    });
    let mut client = AiUiClient::new(transport);
    workflow::execute_scenarios(&mut client, scenarios, fixtures, reports_root)
}

#[cfg(not(windows))]
fn execute_generated_live(
    _inventory: &UiInventory,
    _scenarios: &[generator::GeneratedScenario],
    _fixtures: &FixtureStore,
    _reports_root: &Path,
    _requested_title: Option<&str>,
    _runtime_output: PathBuf,
) -> Result<workflow::ExecutionSummary, String> {
    Err("generated execution requires the real Windows adapter".into())
}

#[cfg(windows)]
fn run_script_live(
    script: &Script,
    fixtures: &FixtureStore,
    output: &Path,
) -> Result<crate::runner::ScriptReport, String> {
    use crate::{
        api::{AiUiClient, InProcessTransport},
        reliability::ReliabilityManager,
    };

    let application = script
        .application
        .command
        .as_deref()
        .ok_or_else(|| "invalid script: application.command is required for run".to_string())?;
    let title = script
        .application
        .window_title
        .clone()
        .unwrap_or_else(effective_title_default);
    let report_dir = output.join(sanitize(&script.name));
    let mut manager = ReliabilityManager::with_launch_environment_and_arguments(
        application,
        title,
        &report_dir,
        fixture_launch_environment(script),
        script.application.arguments.clone(),
    );
    let transport = InProcessTransport::new(move |operation: &str, arguments: Value| {
        manager.call(operation, arguments)
    });
    let mut client = AiUiClient::new(transport);
    let mut report = crate::runner::run_with_fixtures(&mut client, script, &report_dir, fixtures)
        .map_err(|error| format!("runner infrastructure failure: {error}"))?;
    if report.status == "PASSED" {
        report.markers.push("AI_UI_SCRIPT_API_PASSED".into());
        report.markers.sort();
        report.markers.dedup();
        workflow::write_json(report_dir.join("final-report.json"), &report)?;
    }
    Ok(report)
}

fn fixture_launch_environment(script: &Script) -> BTreeMap<String, String> {
    script
        .application
        .fixture_async_failure_mode
        .as_deref()
        .map(|mode| BTreeMap::from([(ASYNC_FAILURE_MODE_ENV.into(), mode.into())]))
        .unwrap_or_default()
}

#[cfg(windows)]
fn effective_title(requested: Option<&str>) -> String {
    requested
        .map(str::to_owned)
        .unwrap_or_else(effective_title_default)
}

#[cfg(windows)]
fn effective_title_default() -> String {
    crate::host::DEFAULT_WINDOW_TITLE.into()
}

fn load_config(args: &CliArgs) -> Result<GeneratorConfig, String> {
    let default = PathBuf::from("ai-ui-generator.json");
    let path = args
        .path("config")
        .or_else(|| default.is_file().then_some(default));
    path.map(generator::load_config)
        .transpose()
        .map(|config| config.unwrap_or_default())
}

fn load_fixtures(args: &CliArgs) -> Result<FixtureStore, String> {
    let default = PathBuf::from("ai-ui-fixtures.json");
    let path = args
        .path("fixtures")
        .or_else(|| default.is_file().then_some(default));
    path.map(FixtureStore::load)
        .transpose()
        .map(|fixtures| fixtures.unwrap_or_default())
}

fn load_ai_intents(args: &CliArgs) -> Result<Vec<AiTestIntent>, String> {
    args.path("ai-intents")
        .map(generator::load_ai_intents)
        .transpose()
        .map(|intents| intents.unwrap_or_default())
}

fn load_plan(path: &Path) -> Result<TestPlan, String> {
    serde_json::from_value(read_json(path)?).map_err(|error| format!("invalid test plan: {error}"))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&raw).map_err(|error| format!("invalid JSON: {error}"))
}

fn script_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    collect_script_paths(root, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_script_paths(root: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    if root.is_file() {
        if root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".aiui.json"))
        {
            paths.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            collect_script_paths(&path, paths)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".aiui.json"))
        {
            paths.push(path);
        }
    }
    Ok(())
}

fn find_inventory(tests: &Path) -> Option<PathBuf> {
    let root = if tests.is_dir() {
        tests
    } else {
        tests.parent().unwrap_or_else(|| Path::new("."))
    };
    [
        root.join("inventory.json"),
        root.parent()
            .unwrap_or_else(|| Path::new("."))
            .join("inventory.json"),
        PathBuf::from("inventory.json"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn output_parent(output: &Path) -> PathBuf {
    if output.extension().is_some() {
        output
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    } else {
        output.to_path_buf()
    }
}

fn sanitize(value: &str) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    value.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_accepts_positionals_switches_and_values() {
        let args = parse_args(vec![
            "demo.exe".into(),
            "--mode".into(),
            "smoke".into(),
            "--plan-only".into(),
            "--output=generated".into(),
        ]);
        assert_eq!(args.positionals, vec!["demo.exe"]);
        assert_eq!(args.option("mode"), Some("smoke"));
        assert_eq!(args.option("output"), Some("generated"));
        assert!(args.switch("plan-only"));
    }

    #[test]
    fn fixture_mode_is_scoped_to_the_child_environment() {
        use serde_json::json;

        let configured = script::from_value(json!({
            "version": 1,
            "name": "fixture-mode",
            "application": {"fixture_async_failure_mode": "file_never_stable"},
            "options": {},
            "steps": [{"op": "session.open", "save_session_as": "app"}]
        }))
        .unwrap();
        assert_eq!(
            fixture_launch_environment(&configured).get(ASYNC_FAILURE_MODE_ENV),
            Some(&"file_never_stable".to_owned())
        );
        let neutral = script::from_value(json!({
            "version": 1,
            "name": "neutral",
            "application": {},
            "options": {},
            "steps": [{"op": "session.open", "save_session_as": "app"}]
        }))
        .unwrap();
        assert!(fixture_launch_environment(&neutral).is_empty());
    }

    #[test]
    fn file_failure_scripts_wait_for_terminal_api_before_asserting_success() {
        for source in [
            include_str!("../examples/async-observation-file-missing.aiui.json"),
            include_str!("../examples/async-observation-file-unstable.aiui.json"),
        ] {
            let raw: Value = serde_json::from_str(source).unwrap();
            let script = script::from_value(raw).unwrap();
            let completed = script
                .steps
                .iter()
                .position(|step| {
                    step.op == "wait_for_api_state"
                        && step.arguments.get("state").and_then(Value::as_str) == Some("Completed")
                })
                .expect("file failure script must wait for accepted API completion");
            let assertion = script
                .steps
                .iter()
                .position(|step| step.op == "assert_api_success")
                .unwrap();
            assert!(completed < assertion, "{}", script.name);
        }
    }

    #[test]
    fn file_unstable_script_requires_a_real_change_window() {
        let raw: Value = serde_json::from_str(include_str!(
            "../examples/async-observation-file-unstable.aiui.json"
        ))
        .unwrap();
        let script = script::from_value(raw).unwrap();
        let stability_wait = script
            .steps
            .iter()
            .find(|step| step.op == "wait_for_file_stable")
            .unwrap();
        assert_eq!(
            stability_wait
                .arguments
                .get("unchanged_for_ms")
                .and_then(Value::as_u64),
            Some(1500)
        );
        assert_eq!(
            stability_wait
                .arguments
                .get("timeout_ms")
                .and_then(Value::as_u64),
            Some(2500)
        );
        assert_eq!(
            stability_wait.expect_error.as_deref(),
            Some("FILE_NOT_STABLE")
        );
        assert!(script.steps.iter().any(|step| {
            step.op == "assert_trace_event"
                && step.arguments.get("event").and_then(Value::as_str) == Some("file.not_stable")
        }));
    }

    #[test]
    fn offline_generate_writes_valid_ordinary_aiui_scripts() {
        use crate::inventory::{InventoryOptions, from_public_responses};
        use serde_json::json;

        let inventory = from_public_responses(
            "demo.exe",
            &json!({"success":true,"result":{
                "session_id":"s",
                "windows":[{
                    "window_id":"w",
                    "name":"Demo",
                    "automation_id":"window.main",
                    "bounds":{"x":0,"y":0,"width":800,"height":600},
                    "visible":true
                }]
            }}),
            &json!({"success":true,"result":{"elements":[{
                "element_id":"e1",
                "automation_id":"window.main/button.safe",
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
            }]}}),
            &BTreeMap::new(),
            &InventoryOptions::default(),
        )
        .unwrap();
        let root = std::env::temp_dir().join(format!("ai-ui-cli-generate-{}", std::process::id()));
        let inventory_path = root.join("source-inventory.json");
        let output = root.join("generated");
        let _ = fs::remove_dir_all(&root);
        workflow::write_json(&inventory_path, &inventory).unwrap();
        let args = parse_args(vec![
            "--inventory".into(),
            inventory_path.to_string_lossy().into_owned(),
            "--output".into(),
            output.to_string_lossy().into_owned(),
            "--mode".into(),
            "smoke".into(),
        ]);
        generate_command(&args).unwrap();
        let paths = script_paths(&output).unwrap();
        assert!(!paths.is_empty());
        for path in paths {
            let script = script::load(path).unwrap();
            assert!(script.generated);
        }
        assert!(output.join("inventory.json").is_file());
        assert!(output.join("test-plan.json").is_file());
        assert!(output.join("generation-report.json").is_file());
        assert!(output.join("coverage.json").is_file());
        let _ = fs::remove_dir_all(root);
    }
}
