#[cfg(not(windows))]
fn main() {
    eprintln!("ai-ui-test-runner is available only on Windows");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = runner::run() {
        eprintln!("AI UI Test Tool end-to-end smoke failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod runner {
    use ai_ui_test_tool::host::{Condition, HostError, TestHost};
    use serde_json::{Value, json};
    use std::{
        error::Error,
        fs,
        path::{Path, PathBuf},
    };
    use toolkit_windows_automation::{ControlTreeResult, ElementState};

    pub fn run() -> Result<(), Box<dyn Error>> {
        let demo_path = argument_path("--demo").unwrap_or_else(default_demo_path);
        let output_dir = argument_path("--output-dir").unwrap_or_else(default_output_dir);
        fs::create_dir_all(&output_dir)?;

        let mut host = TestHost::new(&demo_path, &output_dir)?;
        let mut calls = Vec::new();

        let window = call_success(&mut host, &mut calls, "launch", json!({}))?;
        let tree: ControlTreeResult = serde_json::from_value(call_success(
            &mut host,
            &mut calls,
            "inspect_ui",
            json!({}),
        )?)?;
        require_id(&tree, "input.name", "Edit")?;
        require_id(&tree, "button.create", "Button")?;
        require_id(&tree, "result.message", "Text")?;

        let input_state: ElementState = serde_json::from_value(call_success(
            &mut host,
            &mut calls,
            "input",
            json!({"id": "input.name", "text": "Hello Runa"}),
        )?)?;
        if input_state.value.as_deref() != Some("Hello Runa") {
            return Err(format!("input did not update: {input_state:?}").into());
        }

        let input_condition = serde_json::to_value(Condition::ValueEquals("Hello Runa".into()))?;
        call_success(
            &mut host,
            &mut calls,
            "wait",
            json!({
                "id": "input.name",
                "condition": input_condition,
                "timeout_ms": 5_000,
            }),
        )?;

        let click = call_success(
            &mut host,
            &mut calls,
            "click",
            json!({"id": "button.create"}),
        )?;
        if click["action_method"] != "InvokePattern" {
            return Err(format!("button used unexpected action: {click}").into());
        }

        let result_condition = serde_json::to_value(Condition::ValueContains("Hello Runa".into()))?;
        let result_state: ElementState = serde_json::from_value(call_success(
            &mut host,
            &mut calls,
            "wait",
            json!({
                "id": "result.message",
                "condition": result_condition.clone(),
                "timeout_ms": 5_000,
            }),
        )?)?;
        call_success(
            &mut host,
            &mut calls,
            "assert",
            json!({
                "id": "result.message",
                "condition": result_condition,
            }),
        )?;

        let screenshot = call_success(&mut host, &mut calls, "screenshot", json!({}))?;
        let final_tree: ControlTreeResult = serde_json::from_value(call_success(
            &mut host,
            &mut calls,
            "inspect_ui",
            json!({}),
        )?)?;
        let ui_changes = call_success(&mut host, &mut calls, "get_ui_changes", json!({}))?;
        call_success(&mut host, &mut calls, "shutdown", json!({}))?;
        let logs = call_success(&mut host, &mut calls, "get_logs", json!({}))?;

        fs::write(
            output_dir.join("control-tree.json"),
            serde_json::to_vec_pretty(&final_tree)?,
        )?;
        fs::write(
            output_dir.join("logs.json"),
            serde_json::to_vec_pretty(&logs)?,
        )?;
        fs::write(
            output_dir.join("ui-changes.json"),
            serde_json::to_vec_pretty(&ui_changes)?,
        )?;

        let summary = json!({
            "status": "AI_UI_TEST_TOOL_PROTOTYPE_PASSED",
            "window": window,
            "scenario": [
                "launch",
                "inspect_ui",
                "input(input.name, Hello Runa)",
                "wait(input.name, value_equals)",
                "click(button.create)",
                "wait(result.message, value_contains)",
                "assert(result.message contains Hello Runa)",
                "screenshot",
                "get_logs",
                "shutdown",
            ],
            "tree_elements": final_tree.total_enumerated,
            "input": input_state,
            "click": click,
            "result": result_state,
            "screenshot": screenshot,
            "ui_changes": ui_changes,
            "log_entries": logs.as_array().map_or(0, Vec::len),
            "checks": {
                "app_launch": true,
                "ui_probe_tree": true,
                "semantic_ids": true,
                "value_pattern_input": true,
                "invoke_pattern_click": true,
                "wait_condition": true,
                "assertion": true,
                "screenshot": true,
                "action_log": true,
                "clean_shutdown": true,
            },
        });
        fs::write(
            output_dir.join("summary.json"),
            serde_json::to_vec_pretty(&summary)?,
        )?;
        println!("{}", serde_json::to_string(&summary)?);
        Ok(())
    }

    fn call_success(
        host: &mut TestHost,
        calls: &mut Vec<Value>,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, Box<dyn Error>> {
        let response = host.call(operation, arguments.clone());
        calls.push(json!({
            "operation": operation,
            "arguments": arguments,
            "response": response,
        }));
        if response["success"] != true {
            let error: HostError = serde_json::from_value(response["error"].clone())?;
            return Err(error.into());
        }
        Ok(response["result"].clone())
    }

    fn require_id(
        tree: &ControlTreeResult,
        automation_id: &str,
        control_type: &str,
    ) -> Result<(), Box<dyn Error>> {
        if tree.elements.iter().any(|control| {
            control.automation_id == automation_id && control.control_type == control_type
        }) {
            Ok(())
        } else {
            Err(format!("missing {control_type} with semantic id {automation_id}").into())
        }
    }

    fn argument_path(name: &str) -> Option<PathBuf> {
        let mut arguments = std::env::args_os();
        while let Some(argument) = arguments.next() {
            if argument == name {
                return arguments.next().map(PathBuf::from);
            }
        }
        None
    }

    fn default_demo_path() -> PathBuf {
        Path::new("target")
            .join("debug")
            .join("ai-ui-test-demo.exe")
    }

    fn default_output_dir() -> PathBuf {
        Path::new("reports")
            .join("evidence")
            .join("ai-ui-test-tool-prototype")
    }
}
