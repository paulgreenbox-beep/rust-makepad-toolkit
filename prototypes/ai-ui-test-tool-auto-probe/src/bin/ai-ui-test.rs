use ai_ui_test_tool_auto_probe::script::{self, Script};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    ai_ui_test_tool_auto_probe::cli::main_entry();
}

#[allow(dead_code)]
fn legacy_main() {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    let result = match command.as_str() {
        "validate" => args
            .next()
            .ok_or_else(|| "missing script path".to_string())
            .and_then(|p| validate(Path::new(&p))),
        "normalize" => args
            .next()
            .ok_or_else(|| "missing script path".to_string())
            .and_then(|p| normalize(Path::new(&p))),
        "dry-run" => args
            .next()
            .ok_or_else(|| "missing script path".to_string())
            .and_then(|p| dry_run(Path::new(&p))),
        "list" => list(
            args.next()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".")),
        ),
        "run" => args
            .next()
            .ok_or_else(|| "missing script path".to_string())
            .and_then(|p| run(Path::new(&p))),
        _ => {
            println!("ai-ui-test validate|normalize|dry-run|list|run <path>");
            Ok(())
        }
    };
    if let Err(error) = result {
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

fn read(path: &Path) -> Result<Script, String> {
    script::load(path).map_err(|errors| format!("invalid script: {}", errors.join("; ")))
}
fn validate(path: &Path) -> Result<(), String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("invalid JSON: {e}"))?;
    if value.get("tests").is_some() {
        let suite = ai_ui_test_tool_auto_probe::suite::load(path)?;
        ai_ui_test_tool_auto_probe::suite::validate(
            &suite,
            path.parent().unwrap_or_else(|| Path::new(".")),
        )
        .map_err(|e| e.join("; "))?;
        println!("VALID SUITE: {} ({} tests)", suite.name, suite.tests.len());
    } else {
        let script: Script =
            serde_json::from_value(value).map_err(|e| format!("invalid script: {e}"))?;
        script::validate(&script).map_err(|e| e.join("; "))?;
        println!("VALID: {} ({} steps)", script.name, script.steps.len());
    }
    Ok(())
}
fn dry_run(path: &Path) -> Result<(), String> {
    let script = read(path)?;
    println!("DRY RUN: {}", script.name);
    for (index, step) in script.setup.iter().chain(script.steps.iter()).enumerate() {
        println!(
            "{}: {}{}",
            index,
            step.op,
            if step.op.contains("relative")
                || matches!(
                    step.op.as_str(),
                    "click" | "drag" | "select" | "scroll" | "input"
                )
            {
                " [physical-lock]"
            } else {
                ""
            }
        );
    }
    for (index, step) in script.teardown.iter().enumerate() {
        println!("teardown-{}: {}", index, step.op);
    }
    Ok(())
}
fn normalize(path: &Path) -> Result<(), String> {
    let script = read(path)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&script).map_err(|e| e.to_string())?
    );
    Ok(())
}
fn list(path: PathBuf) -> Result<(), String> {
    let entries = fs::read_dir(path).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|v| v.to_str()) == Some("json")
            && p.file_name()
                .and_then(|v| v.to_str())
                .is_some_and(|n| n.ends_with(".aiui.json"))
        {
            println!("{}", p.display());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn run(path: &Path) -> Result<(), String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("invalid JSON: {e}"))?;
    if value.get("tests").is_some() {
        let suite = ai_ui_test_tool_auto_probe::suite::load(path)?;
        let root = path.parent().unwrap_or_else(|| Path::new("."));
        let paths =
            ai_ui_test_tool_auto_probe::suite::validate(&suite, root).map_err(|e| e.join("; "))?;
        let scripts = paths
            .iter()
            .map(|p| read(p))
            .collect::<Result<Vec<_>, _>>()?;
        let mut next = paths.iter();
        let report = ai_ui_test_tool_auto_probe::suite::aggregate(&suite, &scripts, |_| {
            run_one(next.next().unwrap())
        })?;
        let out = PathBuf::from("reports")
            .join("script")
            .join(suite.name.replace(" ", "-"));
        fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        fs::write(
            out.join("suite-summary.json"),
            serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|e| e.to_string())?
        );
        return if report.status == "PASSED" {
            Ok(())
        } else {
            Err("suite assertions failed".into())
        };
    }
    let report = run_one(path)?;
    if report.status == "PASSED" {
        Ok(())
    } else {
        Err("script assertions failed".into())
    }
}
#[cfg(windows)]
fn run_one(path: &Path) -> Result<ai_ui_test_tool_auto_probe::runner::ScriptReport, String> {
    use ai_ui_test_tool_auto_probe::{
        api::{AiUiClient, InProcessTransport},
        reliability::ReliabilityManager,
        runner,
    };
    let script = read(path)?;
    let command = script
        .application
        .command
        .clone()
        .ok_or_else(|| "invalid script: application.command is required for run".to_string())?;
    let title = script
        .application
        .window_title
        .clone()
        .unwrap_or_else(|| script.name.clone());
    let output = PathBuf::from("reports")
        .join("script")
        .join(script.name.replace(' ', "-"));
    let mut manager = ReliabilityManager::new(command, title, &output);
    let transport =
        InProcessTransport::new(move |op: &str, args: serde_json::Value| manager.call(op, args));
    let mut client = AiUiClient::new(transport);
    let mut report = runner::run(&mut client, &script, &output)
        .map_err(|e| format!("runner infrastructure failure: {e}"))?;
    if report.status == "PASSED" {
        report.markers.push("AI_UI_SCRIPT_API_PASSED".into());
        fs::write(
            output.join("final-report.json"),
            serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|e| e.to_string())?
    );
    if report.status == "PASSED" {
        Ok(report)
    } else {
        Err("script assertions failed".into())
    }
}

#[cfg(not(windows))]
fn run(_path: &Path) -> Result<(), String> {
    Err("runner infrastructure failure: real script execution requires Windows transport".into())
}
