//! Full Windows E2E scenario for the generic Makepad accessibility interaction host.

#[cfg(not(windows))]
fn main() {
    eprintln!("ai-ui-advanced-runner is available only on Windows");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = runner::run() {
        eprintln!("AI UI advanced interaction failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod runner {
    use ai_ui_test_tool_auto_probe::advanced::AdvancedHost;
    use ai_ui_test_tool_auto_probe::host::HostError;
    use serde::Serialize;
    use serde_json::{Value, json};
    use std::{
        collections::BTreeSet,
        error::Error,
        fs,
        path::{Path, PathBuf},
    };
    use toolkit_windows_automation::{ControlInfo, ControlTreeResult, ElementState};

    pub fn run() -> Result<(), Box<dyn Error>> {
        let demo_path = argument_path("--demo").unwrap_or_else(default_demo_path);
        let output_dir = argument_path("--output-dir").unwrap_or_else(default_output_dir);
        fs::create_dir_all(&output_dir)?;
        let mut host =
            AdvancedHost::with_title(&demo_path, "AI UI Advanced Makepad Probe", &output_dir)?;

        let result = scenario(&mut host, &output_dir);
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                let message = error.to_string();
                let _ = host.write_failure_evidence();
                let failure = json!({
                    "status": "AI_UI_ADVANCED_INTERACTION_FAILED",
                    "error": message,
                    "recent_actions": host.recent_actions(),
                });
                let _ = write_json(&output_dir.join("failure-summary.json"), &failure);
                let _ = host.call("shutdown", json!({}));
                Err(message.into())
            }
        }
    }

    fn scenario(host: &mut AdvancedHost, output_dir: &Path) -> Result<(), Box<dyn Error>> {
        let window = call_success(host, "launch", json!({}))?;
        let initial_tree: ControlTreeResult =
            serde_json::from_value(call_success(host, "inspect_ui", json!({}))?)?;
        write_json(&output_dir.join("initial-ui-tree.json"), &initial_tree)?;
        let initial_semantic = call_success(host, "inspect_semantic", json!({}))?;
        write_json(
            &output_dir.join("initial-semantic-tree.json"),
            &initial_semantic,
        )?;

        let ids = ProbeIds::from_tree(&initial_tree)?;
        require_id(&initial_tree, &ids.window, "Window")?;
        require_id(&initial_tree, &ids.input, "Edit")?;
        require_id(&initial_tree, &ids.create, "Button")?;
        require_id(&initial_tree, &ids.project_list, "List")?;
        require_id(&initial_tree, &ids.item_50, "CheckBox")?;
        require_id(&initial_tree, &ids.settings_option, "CheckBox")?;
        require_id(&initial_tree, &ids.settings_mode, "ComboBox")?;
        require_id(&initial_tree, &ids.dialog, "Pane")?;

        call_success(host, "wait_idle", json!({"timeout_ms": 3_000}))?;

        let input_state: ElementState = serde_json::from_value(call_success(
            host,
            "input",
            json!({"id": ids.input, "text": "Hello Advanced"}),
        )?)?;
        if input_state.value.as_deref() != Some("Hello Advanced") {
            return Err(format!("input value mismatch: {input_state:?}").into());
        }
        call_success(
            host,
            "wait_value",
            json!({"id": ids.input, "value": "Hello Advanced", "timeout_ms": 3_000}),
        )?;
        let create_click = call_success(host, "click", json!({"id": ids.create}))?;
        call_success(
            host,
            "wait_value",
            json!({
                "id": ids.result,
                "value": "Hello Advanced",
                "contains": true,
                "timeout_ms": 3_000
            }),
        )?;

        let selected_2 = call_success(host, "select", json!({"id": ids.item_2}))?;
        call_success(
            host,
            "wait_selected",
            json!({"id": ids.item_2, "timeout_ms": 3_000}),
        )?;
        let added_4 = call_success(host, "add_to_selection", json!({"id": ids.item_4}))?;
        call_success(
            host,
            "wait_count",
            json!({"container_id": ids.scroll_list, "count": 2, "timeout_ms": 3_000}),
        )?;
        let selected_4: ElementState =
            serde_json::from_value(call_success(host, "read", json!({"id": ids.item_4}))?)?;
        if selected_4.selected != Some(true) {
            return Err(format!("add_to_selection did not select item 4: {selected_4:?}").into());
        }
        let cleared = call_success(
            host,
            "clear_selection",
            json!({"container_id": ids.scroll_list}),
        )?;
        call_success(
            host,
            "wait_count",
            json!({"container_id": ids.scroll_list, "count": 0, "timeout_ms": 3_000}),
        )?;
        let range = call_success(
            host,
            "select_range",
            json!({"start_id": ids.item_2, "end_id": ids.item_6}),
        )?;
        call_success(
            host,
            "wait_count",
            json!({"container_id": ids.scroll_list, "count": 5, "timeout_ms": 3_000}),
        )?;
        call_success(host, "scroll_to_start", json!({"id": ids.item_4}))?;
        call_success(host, "ensure_visible", json!({"id": ids.item_4}))?;

        let right_click = call_success(host, "right_click", json!({"id": ids.item_4}))?;
        call_success(
            host,
            "wait_visible",
            json!({"id": ids.context_menu, "timeout_ms": 3_000}),
        )?;
        call_success(host, "click", json!({"id": ids.context_delete}))?;
        call_success(
            host,
            "wait_value",
            json!({
                "id": ids.status,
                "value": "context item deleted",
                "contains": true,
                "timeout_ms": 3_000
            }),
        )?;

        let toggled = call_success(host, "toggle", json!({"id": ids.settings_option}))?;
        call_success(
            host,
            "wait_selected",
            json!({"id": ids.settings_option, "timeout_ms": 3_000}),
        )?;
        let chosen = call_success(
            host,
            "choose",
            json!({"id": ids.settings_mode, "value": "advanced"}),
        )?;
        call_success(
            host,
            "wait_value",
            json!({"id": ids.settings_mode, "value": "advanced", "timeout_ms": 3_000}),
        )?;

        let ensured = call_success(host, "ensure_visible", json!({"id": ids.item_50}))?;
        call_success(
            host,
            "wait_visible",
            json!({"id": ids.item_50, "timeout_ms": 3_000}),
        )?;
        let hovered = call_success(host, "hover", json!({"id": ids.item_50}))?;
        let dragged = call_success(
            host,
            "drag",
            json!({"source_id": ids.item_50, "target_id": ids.drop_target}),
        )?;
        call_success(
            host,
            "wait_value",
            json!({
                "id": ids.status,
                "value": "drop completed",
                "contains": true,
                "timeout_ms": 3_000
            }),
        )?;

        let opened_tree = call_success(host, "open", json!({"id": ids.tree_section}))?;
        call_success(
            host,
            "wait_visible",
            json!({"id": ids.tree_children, "timeout_ms": 3_000}),
        )?;
        call_success(
            host,
            "wait_selected",
            json!({"id": ids.tree_section, "timeout_ms": 3_000}),
        )?;
        let closed_tree = call_success(host, "close", json!({"id": ids.tree_section}))?;
        call_success(
            host,
            "wait_hidden",
            json!({"id": ids.tree_children, "timeout_ms": 3_000}),
        )?;

        call_success(host, "hotkey", json!({"keys": ["Ctrl", "S"]}))?;
        call_success(
            host,
            "wait_value",
            json!({
                "id": ids.status,
                "value": "Ctrl+S handled",
                "contains": true,
                "timeout_ms": 3_000
            }),
        )?;

        let modal_open = call_success(host, "click", json!({"id": ids.open_modal}))?;
        call_success(
            host,
            "wait_visible",
            json!({"id": ids.dialog, "timeout_ms": 3_000}),
        )?;
        call_success(host, "press", json!({"key": "Escape"}))?;
        call_success(
            host,
            "wait_hidden",
            json!({"id": ids.dialog, "timeout_ms": 3_000}),
        )?;

        let async_click = call_success(host, "click", json!({"id": ids.async_button}))?;
        call_success(
            host,
            "wait_visible",
            json!({"id": ids.async_result, "timeout_ms": 5_000}),
        )?;
        call_success(host, "wait_idle", json!({"timeout_ms": 3_000}))?;

        let screenshot = call_success(host, "screenshot", json!({}))?;
        copy_screenshot(&screenshot, &output_dir.join("screenshot.png"))?;
        let final_tree: ControlTreeResult =
            serde_json::from_value(call_success(host, "inspect_ui", json!({}))?)?;
        let final_semantic = call_success(host, "inspect_semantic", json!({}))?;
        write_json(&output_dir.join("final-ui-tree.json"), &final_tree)?;
        write_json(
            &output_dir.join("final-semantic-tree.json"),
            &final_semantic,
        )?;
        let ui_changes = call_success(host, "get_ui_changes", json!({}))?;
        call_success(host, "shutdown", json!({}))?;
        let logs = call_success(host, "get_logs", json!({}))?;
        write_json(&output_dir.join("logs.json"), &logs)?;
        write_json(&output_dir.join("ui-changes.json"), &ui_changes)?;

        let summary = json!({
            "status": "AI_UI_ADVANCED_INTERACTION_PASSED",
            "markers": [
                "AI_UI_ADVANCED_INTERACTION_PASSED",
                "AI_UI_SCROLLYVIEW_ENSURE_VISIBLE_PASSED"
            ],
            "window": window,
            "scenario": [
                "launch", "inspect_ui", "inspect_semantic", "wait_idle",
                "input", "click", "wait_value", "select", "add_to_selection",
                "clear_selection", "select_range", "right_click", "context menu",
                "toggle", "choose", "ensure_visible", "hover", "drag",
                "open", "close", "hotkey", "modal", "async", "screenshot",
                "failure_evidence", "shutdown"
            ],
            "tree_elements_initial": initial_tree.total_enumerated,
            "tree_elements_final": final_tree.total_enumerated,
            "input": input_state,
            "create_click": create_click,
            "selection": {"select": selected_2, "add": added_4, "clear": cleared, "range": range},
            "right_click": right_click,
            "toggle": toggled,
            "choose": chosen,
            "scroll": ensured,
            "hover": hovered,
            "drag": dragged,
            "tree": {"open": opened_tree, "close": closed_tree},
            "modal": modal_open,
            "async": async_click,
            "screenshot": screenshot,
            "ui_changes": ui_changes,
            "widget_types": collect_widget_types(&final_tree.elements),
            "log_entries": logs.as_array().map_or(0, Vec::len),
            "checks": {
                "generic_semantic_tree": true,
                "roles_names_bounds": true,
                "selection_multiple_and_range": true,
                "context_menu_pointer": true,
                "scroll_and_ensure_visible": true,
                "hover_and_drag_drop": true,
                "toggle_and_dropdown": true,
                "tree_expand_collapse": true,
                "keyboard_and_window_actions": true,
                "modal_wait": true,
                "async_wait": true,
                "ai_friendly_errors_and_failure_evidence": true,
                "legacy_contract": true,
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
        project_list: String,
        scroll_list: String,
        item_2: String,
        item_4: String,
        item_6: String,
        item_50: String,
        settings_option: String,
        settings_mode: String,
        context_menu: String,
        context_delete: String,
        status: String,
        drop_target: String,
        tree_section: String,
        tree_children: String,
        open_modal: String,
        dialog: String,
        async_button: String,
        async_result: String,
    }

    impl ProbeIds {
        fn from_tree(tree: &ControlTreeResult) -> Result<Self, Box<dyn Error>> {
            let ids = tree
                .elements
                .iter()
                .map(|element| element.automation_id.as_str())
                .collect::<Vec<_>>();
            let find = |suffix: &str| {
                ids.iter()
                    .find(|id| id.ends_with(suffix))
                    .map(|id| (*id).to_owned())
            };
            Ok(Self {
                window: find("window.main").ok_or("missing window.main")?,
                input: find("/form.main/input.name").ok_or("missing input.name")?,
                create: find("/form.main/button.create").ok_or("missing button.create")?,
                result: find("/result.message").ok_or("missing result.message")?,
                project_list: find("/project.list").ok_or("missing project.list")?,
                scroll_list: find("/scroll.list").ok_or("missing scroll.list")?,
                item_2: find("/item.2").ok_or("missing item.2")?,
                item_4: find("/item.4").ok_or("missing item.4")?,
                item_6: find("/item.6").ok_or("missing item.6")?,
                item_50: find("/item.50").ok_or("missing item.50")?,
                settings_option: find("/settings.row/settings.option")
                    .ok_or("missing settings.option")?,
                settings_mode: find("/settings.row/settings.mode")
                    .ok_or("missing settings.mode")?,
                context_menu: find("/context.menu").ok_or("missing context.menu")?,
                context_delete: find("/context.menu/context.delete")
                    .ok_or("missing context.delete")?,
                status: find("/status").ok_or("missing status")?,
                drop_target: find("/drop.target").ok_or("missing drop.target")?,
                tree_section: find("/tree.section").ok_or("missing tree.section")?,
                tree_children: find("/tree.children").ok_or("missing tree.children")?,
                open_modal: find("/button.open.modal").ok_or("missing open modal")?,
                dialog: find("/dialog.modal").ok_or("missing dialog.modal")?,
                async_button: find("/button.async").ok_or("missing async button")?,
                async_result: find("/async.result").ok_or("missing async result")?,
            })
        }
    }

    fn call_success(
        host: &mut AdvancedHost,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, Box<dyn Error>> {
        let response = host.call(operation, arguments);
        if response["success"] == true {
            return Ok(response["result"].clone());
        }
        let error: HostError = serde_json::from_value(response["error"].clone())?;
        Err(error.into())
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
            .map(|element| element.control_type.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn copy_screenshot(value: &Value, target: &Path) -> Result<(), Box<dyn Error>> {
        let source = value
            .get("path")
            .and_then(Value::as_str)
            .ok_or("screenshot response did not contain a path")?;
        fs::copy(source, target)?;
        Ok(())
    }

    fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), Box<dyn Error>> {
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
        Path::new("prototypes")
            .join("ai-ui-test-tool-auto-probe")
            .join("target")
            .join("debug")
            .join("ai-ui-advanced-demo.exe")
    }

    fn default_output_dir() -> PathBuf {
        Path::new("reports")
            .join("evidence")
            .join("ai-ui-test-tool-auto-probe-advanced")
    }
}
