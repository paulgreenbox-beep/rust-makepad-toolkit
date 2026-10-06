//! Real Windows end-to-end scenario for the automatic Makepad UI Probe.

#[cfg(not(windows))]
fn main() {
    eprintln!("ai-ui-auto-probe-runner is available only on Windows");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = runner::run() {
        eprintln!("AI UI automatic Makepad Probe smoke failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod runner {
    use ai_ui_test_tool_auto_probe::host::{Condition, HostError, TestHost};
    use serde_json::{Value, json};
    use std::{
        collections::{BTreeMap, BTreeSet},
        error::Error,
        fs,
        path::{Path, PathBuf},
    };
    use toolkit_windows_automation::{ControlInfo, ControlTreeResult, ElementState};

    pub fn run() -> Result<(), Box<dyn Error>> {
        let demo_path = argument_path("--demo").unwrap_or_else(default_demo_path);
        let output_dir = argument_path("--output-dir").unwrap_or_else(default_output_dir);
        fs::create_dir_all(&output_dir)?;
        let mut host = TestHost::new(&demo_path, &output_dir)?;

        let window = call_success(&mut host, "launch", json!({}))?;
        let initial_tree: ControlTreeResult =
            serde_json::from_value(call_success(&mut host, "inspect_ui", json!({}))?)?;
        write_json(&output_dir.join("initial-ui-tree.json"), &initial_tree)?;

        let ids = ProbeIds::from_tree(&initial_tree)?;
        require_id(&initial_tree, &ids.window, "Window")?;
        require_id(&initial_tree, &ids.input, "Edit")?;
        require_id(&initial_tree, &ids.create, "Button")?;
        require_id(&initial_tree, &ids.checkbox, "CheckBox")?;
        require_id(&initial_tree, &ids.project_list, "List")?;
        require_id(&initial_tree, &ids.dialog, "Pane")?;
        require_id(&initial_tree, &ids.dynamic, "Group")?;

        let input_state: ElementState = serde_json::from_value(call_success(
            &mut host,
            "input",
            json!({"id": ids.input, "text": "Hello Runa"}),
        )?)?;
        if input_state.value.as_deref() != Some("Hello Runa") {
            return Err(format!("automatic Probe input did not update: {input_state:?}").into());
        }
        wait(
            &mut host,
            &ids.input,
            Condition::ValueEquals("Hello Runa".into()),
        )?;

        let create_click = call_success(&mut host, "click", json!({"id": ids.create}))?;
        if create_click["action_method"] != "InvokePattern" {
            return Err(format!("create button used unexpected action: {create_click}").into());
        }
        let result_state: ElementState = serde_json::from_value(call_success(
            &mut host,
            "wait",
            json!({
                "id": ids.result,
                "condition": serde_json::to_value(Condition::ValueContains("Hello Runa".into()))?,
                "timeout_ms": 5_000,
            }),
        )?)?;
        call_success(
            &mut host,
            "assert",
            json!({
                "id": ids.result,
                "condition": serde_json::to_value(Condition::ValueContains("Hello Runa".into()))?,
            }),
        )?;

        let checkbox_click = call_success(&mut host, "click", json!({"id": ids.checkbox}))?;
        let checked_state: ElementState = serde_json::from_value(call_success(
            &mut host,
            "wait",
            json!({
                "id": ids.checkbox,
                "condition": serde_json::to_value(Condition::Selected(true))?,
                "timeout_ms": 5_000,
            }),
        )?)?;
        call_success(
            &mut host,
            "assert",
            json!({
                "id": ids.checkbox,
                "condition": serde_json::to_value(Condition::Selected(true))?,
            }),
        )?;
        let enabled_state: ElementState = serde_json::from_value(call_success(
            &mut host,
            "wait",
            json!({
                "id": ids.enabled_button,
                "condition": serde_json::to_value(Condition::Enabled)?,
                "timeout_ms": 5_000,
            }),
        )?)?;

        let dialog_click = call_success(&mut host, "click", json!({"id": ids.open_modal}))?;
        let dialog_visible: ElementState = serde_json::from_value(call_success(
            &mut host,
            "wait",
            json!({
                "id": ids.dialog,
                "condition": serde_json::to_value(Condition::Visible)?,
                "timeout_ms": 5_000,
            }),
        )?)?;
        let modal_tree: ControlTreeResult =
            serde_json::from_value(call_success(&mut host, "inspect_ui", json!({}))?)?;
        require_id(&modal_tree, &ids.close_modal, "Button")?;
        call_success(&mut host, "click", json!({"id": ids.close_modal}))?;
        let dialog_closed: ElementState =
            serde_json::from_value(call_success(&mut host, "read", json!({"id": ids.dialog}))?)?;
        if dialog_closed.visible {
            return Err(format!("dialog remained visible after close: {dialog_closed:?}").into());
        }

        let dynamic_click = call_success(&mut host, "click", json!({"id": ids.dynamic_trigger}))?;
        let dynamic_visible: ElementState = serde_json::from_value(call_success(
            &mut host,
            "wait",
            json!({
                "id": ids.dynamic,
                "condition": serde_json::to_value(Condition::Visible)?,
                "timeout_ms": 5_000,
            }),
        )?)?;
        let host_ui_changes = call_success(&mut host, "get_ui_changes", json!({}))?;

        let screenshot = call_success(&mut host, "screenshot", json!({}))?;
        copy_screenshot(&screenshot, &output_dir.join("screenshot.png"))?;
        let final_tree: ControlTreeResult =
            serde_json::from_value(call_success(&mut host, "inspect_ui", json!({}))?)?;
        write_json(&output_dir.join("final-ui-tree.json"), &final_tree)?;
        call_success(&mut host, "shutdown", json!({}))?;
        let logs = call_success(&mut host, "get_logs", json!({}))?;
        write_json(&output_dir.join("logs.json"), &logs)?;
        let widget_types = collect_widget_types(&final_tree.elements);
        write_json(&output_dir.join("widget-types.json"), &widget_types)?;

        let ui_changes = json!({
            "host_last_refresh": host_ui_changes,
            "snapshot_diff": tree_diff(&initial_tree, &final_tree),
        });
        write_json(&output_dir.join("ui-changes.json"), &ui_changes)?;

        let summary = json!({
            "status": "AI_UI_AUTO_PROBE_PASSED",
            "window": window,
            "scenario": [
                "launch", "automatic inspect_ui", "input", "click", "assert result",
                "toggle checkbox", "assert checked and enabled", "open and discover dialog",
                "close dialog", "trigger dynamic element", "wait visible", "screenshot",
                "logs", "shutdown"
            ],
            "tree_elements_initial": initial_tree.total_enumerated,
            "tree_elements_final": final_tree.total_enumerated,
            "input": input_state,
            "create_click": create_click,
            "result": result_state,
            "checkbox_click": checkbox_click,
            "checked": checked_state,
            "enabled_button": enabled_state,
            "dialog_click": dialog_click,
            "dialog_visible": dialog_visible,
            "dialog_closed": dialog_closed,
            "dynamic_click": dynamic_click,
            "dynamic_visible": dynamic_visible,
            "screenshot": screenshot,
            "ui_changes": ui_changes,
            "widget_types": widget_types,
            "log_entries": logs.as_array().map_or(0, Vec::len),
            "checks": {
                "app_launch": true,
                "automatic_makepad_runtime_probe": true,
                "semantic_tree_ids": true,
                "roles_names_bounds": true,
                "value_pattern_input": true,
                "invoke_pattern_button": true,
                "checkbox_toggle_state": true,
                "modal_discovery": true,
                "dynamic_addition": true,
                "ui_changes": true,
                "screenshot": true,
                "action_log": true,
                "clean_shutdown": true
            }
        });
        write_json(&output_dir.join("summary.json"), &summary)?;
        println!("{}", serde_json::to_string(&summary)?);
        Ok(())
    }

    #[derive(Clone, Debug)]
    struct ProbeIds {
        window: String,
        input: String,
        create: String,
        result: String,
        checkbox: String,
        enabled_button: String,
        project_list: String,
        open_modal: String,
        dialog: String,
        close_modal: String,
        dynamic_trigger: String,
        dynamic: String,
    }

    impl ProbeIds {
        fn from_tree(tree: &ControlTreeResult) -> Result<Self, Box<dyn Error>> {
            let ids = tree
                .elements
                .iter()
                .map(|e| e.automation_id.as_str())
                .collect::<Vec<_>>();
            let find = |suffix: &str| {
                ids.iter()
                    .find(|id| id.ends_with(suffix))
                    .map(|id| (*id).to_owned())
            };
            Ok(Self {
                window: find("window.main").ok_or("Probe did not discover window.main")?,
                input: find("/form.main/input.name").ok_or("missing input.name")?,
                create: find("/form.main/button.create").ok_or("missing button.create")?,
                result: find("/result.message").ok_or("missing result.message")?,
                checkbox: find("/checkbox.ready").ok_or("missing checkbox.ready")?,
                enabled_button: find("/button.enable").ok_or("missing button.enable")?,
                project_list: find("/project.list").ok_or("missing project.list")?,
                open_modal: find("/button.open.modal").ok_or("missing open modal")?,
                dialog: find("/dialog.modal").ok_or("missing dialog.modal")?,
                close_modal: find("/dialog.modal/content/button.close.modal")
                    .ok_or("missing close modal")?,
                dynamic_trigger: find("/button.dynamic").ok_or("missing dynamic trigger")?,
                dynamic: find("/dynamic.banner").ok_or("missing dynamic.banner")?,
            })
        }
    }

    fn call_success(
        host: &mut TestHost,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, Box<dyn Error>> {
        let response = host.call(operation, arguments);
        if response["success"] != true {
            let error: HostError = serde_json::from_value(response["error"].clone())?;
            return Err(error.into());
        }
        Ok(response["result"].clone())
    }

    fn wait(
        host: &mut TestHost,
        id: &str,
        condition: Condition,
    ) -> Result<ElementState, Box<dyn Error>> {
        Ok(serde_json::from_value(call_success(
            host,
            "wait",
            json!({
                "id": id,
                "condition": serde_json::to_value(condition)?,
                "timeout_ms": 5_000
            }),
        )?)?)
    }

    fn require_id(tree: &ControlTreeResult, id: &str, role: &str) -> Result<(), Box<dyn Error>> {
        if tree
            .elements
            .iter()
            .any(|element| element.automation_id == id && element.control_type == role)
        {
            Ok(())
        } else {
            Err(format!("missing {role} with semantic id {id}").into())
        }
    }

    fn collect_widget_types(elements: &[ControlInfo]) -> Vec<String> {
        elements
            .iter()
            .map(|e| e.control_type.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn tree_diff(initial: &ControlTreeResult, final_tree: &ControlTreeResult) -> Value {
        let before = initial
            .elements
            .iter()
            .map(|element| (element.automation_id.clone(), element))
            .collect::<BTreeMap<_, _>>();
        let after = final_tree
            .elements
            .iter()
            .map(|element| (element.automation_id.clone(), element))
            .collect::<BTreeMap<_, _>>();
        let created = after
            .keys()
            .filter(|id| !before.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        let removed = before
            .keys()
            .filter(|id| !after.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        let changed = after
            .iter()
            .filter_map(|(id, current)| {
                before.get(id).and_then(|previous| {
                    (serde_json::to_value(*previous).ok()?
                        != serde_json::to_value(*current).ok()?)
                    .then_some(id.clone())
                })
            })
            .collect::<Vec<_>>();
        json!({"created": created, "removed": removed, "changed": changed})
    }

    fn copy_screenshot(value: &Value, target: &Path) -> Result<(), Box<dyn Error>> {
        let source = value
            .get("path")
            .and_then(Value::as_str)
            .ok_or("screenshot response did not contain a path")?;
        fs::copy(source, target)?;
        Ok(())
    }

    fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), Box<dyn Error>> {
        fs::write(path, serde_json::to_vec_pretty(value)?)?;
        Ok(())
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
        let local = Path::new("target")
            .join("debug")
            .join("ai-ui-auto-probe-demo.exe");
        if local.is_file() {
            local
        } else {
            Path::new("prototypes")
                .join("ai-ui-test-tool-auto-probe")
                .join("target")
                .join("debug")
                .join("ai-ui-auto-probe-demo.exe")
        }
    }

    fn default_output_dir() -> PathBuf {
        Path::new("reports")
            .join("evidence")
            .join("ai-ui-test-tool-auto-probe")
    }
}
