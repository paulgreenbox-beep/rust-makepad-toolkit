//! Windows E2E for reliability, multi-window isolation and runtime failure evidence.

#[cfg(not(windows))]
fn main() {
    eprintln!("ai-ui-reliability-runner is available only on Windows");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = runner::run() {
        eprintln!("AI UI reliability regression failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod runner {
    use ai_ui_test_tool_auto_probe::{
        host::{HostError, RuntimeHealth},
        reliability::{AutomationSession, ReliabilityManager},
    };
    use serde::Serialize;
    use serde_json::{Value, json};
    use std::{
        collections::hash_map::DefaultHasher,
        error::Error,
        fs,
        hash::{Hash, Hasher},
        path::{Path, PathBuf},
    };
    use toolkit_windows_automation::ElementState;

    const WINDOW_TITLE: &str = "AI UI Advanced Makepad Probe";

    pub fn run() -> Result<(), Box<dyn Error>> {
        let demo_path = argument_path("--demo").unwrap_or_else(default_demo_path);
        let output_dir = argument_path("--output-dir").unwrap_or_else(default_output_dir);
        fs::create_dir_all(&output_dir)?;
        let mut manager = ReliabilityManager::new(&demo_path, WINDOW_TITLE, &output_dir);
        let scenario_result = scenario(&mut manager, &output_dir);
        if let Err(error) = &scenario_result {
            let snapshot = manager.call("session.list", json!({}));
            write_json(
                &output_dir.join("failure-summary.json"),
                &json!({
                    "status":"AI_UI_RELIABILITY_FAILED",
                    "error":error.to_string(),
                    "sessions":snapshot,
                }),
            )?;
        }
        scenario_result
    }

    fn scenario(manager: &mut ReliabilityManager, output_dir: &Path) -> Result<(), Box<dyn Error>> {
        let session_a: AutomationSession =
            serde_json::from_value(call_ok(manager, "session.open", json!({}))?)?;
        let session_b: AutomationSession =
            serde_json::from_value(call_ok(manager, "session.open", json!({}))?)?;
        require(
            session_a.session_id != session_b.session_id,
            "session IDs must be unique",
        )?;
        require(
            session_a.processes[0].process_id != session_b.processes[0].process_id,
            "process IDs must be unique",
        )?;
        require(
            session_a.windows[0].window_id != session_b.windows[0].window_id,
            "window IDs must be owner-scoped",
        )?;

        let ambiguous = expect_error(
            manager,
            "windows_find_window",
            json!({"title":WINDOW_TITLE}),
            "AMBIGUOUS_WINDOW",
        )?;
        let mismatch = expect_error(
            manager,
            "session.resolve_window",
            json!({
                "session_id":session_a.session_id,
                "window_id":session_b.windows[0].window_id
            }),
            "WINDOW_SESSION_MISMATCH",
        )?;

        let compact_a = call_ok(
            manager,
            "inspect_ui",
            json!({
                "session_id":session_a.session_id,
                "compact":true,
                "structured_only":true,
                "max_depth":8,
                "visible_only":false
            }),
        )?;
        let compact_b = call_ok(
            manager,
            "inspect_ui",
            json!({
                "session_id":session_b.session_id,
                "compact":true,
                "automation_ids_only":true,
                "max_depth":8
            }),
        )?;
        let ids_a = ProbeIds::from_compact(&compact_a)?;
        let ids_b = ProbeIds::from_compact(&compact_b)?;

        let filtered_buttons = call_ok(
            manager,
            "get_control_tree",
            json!({
                "session_id":session_a.session_id,
                "compact":true,
                "automation_ids_only":true,
                "visible_only":true,
                "control_type":"Button",
                "automation_id_prefix":"window.main/body"
            }),
        )?;
        require(
            filtered_buttons["matched"].as_u64().unwrap_or(0) > 0,
            "compact Button filter must return controls",
        )?;

        let input_a = call_ok(
            manager,
            "input",
            json!({
                "session_id":session_a.session_id,
                "id":ids_a.input,
                "value":"Session A"
            }),
        )?;
        require(
            input_a["verified"] == true,
            "session A input read-back failed",
        )?;
        let untouched_b: ElementState = serde_json::from_value(call_ok(
            manager,
            "read",
            json!({"session_id":session_b.session_id,"id":ids_b.input}),
        )?)?;
        require(
            untouched_b.value.as_deref() != Some("Session A"),
            "input A leaked into session B",
        )?;

        let input_b = call_ok(
            manager,
            "set_value",
            json!({
                "session_id":session_b.session_id,
                "id":ids_b.input,
                "value":"Session B"
            }),
        )?;
        require(
            input_b["verified"] == true,
            "session B input read-back failed",
        )?;
        assert_value(manager, &session_a.session_id, &ids_a.input, "Session A")?;
        assert_value(manager, &session_b.session_id, &ids_b.input, "Session B")?;

        let click_a = call_ok(
            manager,
            "click",
            json!({
                "session_id":session_a.session_id,
                "id":ids_a.create,
                "postcondition":{
                    "value_changes":{"id":ids_a.result,"to":"Result: Session A"}
                },
                "timeout_ms":3000
            }),
        )?;
        require(
            click_a["postcondition_verified"] == true,
            "click postcondition was not verified",
        )?;
        call_ok(
            manager,
            "click",
            json!({
                "session_id":session_a.session_id,
                "id":ids_a.async_button,
                "postcondition":{"automation_id_visible":ids_a.async_result},
                "timeout_ms":3000
            }),
        )?;
        call_ok(
            manager,
            "wait_for_state",
            json!({
                "session_id":session_a.session_id,
                "id":ids_a.async_result,
                "state":{"visible":true,"enabled":true},
                "timeout_ms":3000
            }),
        )?;
        call_ok(
            manager,
            "assert_interactable",
            json!({"session_id":session_a.session_id,"id":ids_a.create}),
        )?;
        call_ok(
            manager,
            "assert_valid_bounds",
            json!({"session_id":session_a.session_id,"id":ids_a.async_result}),
        )?;

        let coverage = call_ok(
            manager,
            "windows_assert_automation_ids",
            json!({
                "session_id":session_a.session_id,
                "expected":["input.name","button.create","button.crash","result.message","async.result"]
            }),
        )?;
        let canvas = call_ok(
            manager,
            "assert_canvas_healthy",
            json!({"session_id":session_a.session_id,"id":ids_a.async_result}),
        )?;

        let screenshot_a = call_ok(
            manager,
            "screenshot_window",
            json!({"session_id":session_a.session_id}),
        )?;
        let screenshot_b = call_ok(
            manager,
            "screenshot_window",
            json!({"session_id":session_b.session_id}),
        )?;
        require_capture_owner(&screenshot_a, &session_a.windows[0].window_id)?;
        require_capture_owner(&screenshot_b, &session_b.windows[0].window_id)?;
        let path_a = required_path(&screenshot_a)?;
        let path_b = required_path(&screenshot_b)?;
        let hash_a = hash_file(&path_a)?;
        let hash_b = hash_file(&path_b)?;

        let element_capture = call_ok(
            manager,
            "screenshot_element",
            json!({"session_id":session_a.session_id,"id":ids_a.create}),
        )?;
        let bounds = session_a.windows[0].bounds;
        let region_capture = call_ok(
            manager,
            "windows_capture_region",
            json!({
                "session_id":session_a.session_id,
                "x":bounds.x + 8,
                "y":bounds.y + 8,
                "width":160,
                "height":80,
                "coordinate_space":"screen"
            }),
        )?;

        call_ok(
            manager,
            "focus",
            json!({"session_id":session_b.session_id,"id":ids_b.create}),
        )?;
        let lock_b = call_ok(
            manager,
            "acquire_input_lock",
            json!({"session_id":session_b.session_id}),
        )?;
        let token_b = required_string(&lock_b, "token")?;
        call_ok(
            manager,
            "activate_window",
            json!({
                "session_id":session_b.session_id,
                "input_lock_token":token_b
            }),
        )?;

        let overlay_b_before: ElementState = serde_json::from_value(call_ok(
            manager,
            "read",
            json!({"session_id":session_b.session_id,"id":ids_b.input}),
        )?)?;
        let semantic_under_overlay = call_ok(
            manager,
            "input",
            json!({
                "session_id":session_a.session_id,
                "id":ids_a.input,
                "value":"Session A under B"
            }),
        )?;
        require(
            semantic_under_overlay["verified"] == true,
            "semantic action failed under foreign foreground",
        )?;
        let overlay_b_after: ElementState = serde_json::from_value(call_ok(
            manager,
            "read",
            json!({"session_id":session_b.session_id,"id":ids_b.input}),
        )?)?;
        require(
            overlay_b_after.value == overlay_b_before.value,
            format!(
                "semantic action in A changed B: before {:?}, after {:?}",
                overlay_b_before.value, overlay_b_after.value
            ),
        )?;
        let focus_conflict = expect_error(
            manager,
            "hover",
            json!({"session_id":session_a.session_id,"id":ids_a.create}),
            "WINDOW_FOCUS_CONFLICT",
        )?;
        call_ok(
            manager,
            "release_input_lock",
            json!({"session_id":session_b.session_id,"token":token_b}),
        )?;
        let lock_a = call_ok(
            manager,
            "acquire_input_lock",
            json!({"session_id":session_a.session_id}),
        )?;
        let token_a = required_string(&lock_a, "token")?;
        let physical_after_activation = call_ok(
            manager,
            "hover",
            json!({
                "session_id":session_a.session_id,
                "id":ids_a.create,
                "input_lock_token":token_a
            }),
        )?;
        call_ok(
            manager,
            "release_input_lock",
            json!({"session_id":session_a.session_id,"token":token_a}),
        )?;
        let screenshot_b_under_a = call_ok(
            manager,
            "screenshot_window",
            json!({"session_id":session_b.session_id}),
        )?;
        require_capture_owner(&screenshot_b_under_a, &session_b.windows[0].window_id)?;

        let sequence = call_ok(
            manager,
            "windows_run_sequence",
            json!({
                "session_id":session_a.session_id,
                "stop_on_error":true,
                "retry":1,
                "timeout_ms":3000,
                "wait_after_action":1,
                "evidence_on_failure":true,
                "steps":[
                    {"op":"wait_for_value","id":ids_a.input,"value":"Session A under B"},
                    {"op":"assert_visible","id":ids_a.result},
                    {"op":"screenshot_window"}
                ]
            }),
        )?;
        let health_a: RuntimeHealth = serde_json::from_value(call_ok(
            manager,
            "get_runtime_health",
            json!({"session_id":session_a.session_id}),
        )?)?;
        require(health_a.healthy, "session A must remain healthy")?;

        let evidence_a = call_ok(
            manager,
            "windows_export_evidence",
            json!({"session_id":session_a.session_id,"reason":"reliability-pass"}),
        )?;
        let evidence_a_path = PathBuf::from(required_string(&evidence_a, "path")?);
        verify_evidence_bundle(&evidence_a_path)?;

        call_ok(
            manager,
            "session.close",
            json!({"session_id":session_a.session_id}),
        )?;
        call_ok(
            manager,
            "session.close",
            json!({"session_id":session_b.session_id}),
        )?;

        let crash_session: AutomationSession =
            serde_json::from_value(call_ok(manager, "session.open", json!({}))?)?;
        let crash_tree = call_ok(
            manager,
            "inspect_ui",
            json!({
                "session_id":crash_session.session_id,
                "compact":true,
                "automation_ids_only":true
            }),
        )?;
        let crash_id = find_suffix(&crash_tree, "/button.crash")?;
        let crash_error = expect_error(
            manager,
            "click",
            json!({"session_id":crash_session.session_id,"id":crash_id}),
            "APPLICATION_RUNTIME_FAILURE",
        )?;
        let crash_health: RuntimeHealth =
            serde_json::from_value(crash_error["details"]["runtime_health"].clone())?;
        require(
            !crash_health.process_alive,
            "crash process must be terminated",
        )?;
        require(
            crash_health.stderr_tail.iter().any(|line| {
                line.contains("AI_UI_INTENTIONAL_RUNTIME_FAILURE")
                    || line.to_lowercase().contains("panicked at")
            }),
            "crash stderr marker was not captured",
        )?;
        let crash_evidence =
            PathBuf::from(required_string(&crash_error["details"], "evidence_path")?);
        verify_evidence_bundle(&crash_evidence)?;
        let crash_errors: Value =
            serde_json::from_slice(&fs::read(crash_evidence.join("errors.json"))?)?;
        require(
            crash_errors
                .as_array()
                .is_some_and(|errors| errors.iter().any(|entry| entry["action"] == "click")),
            "crash evidence does not contain failed click action",
        )?;
        call_ok(
            manager,
            "session.close",
            json!({"session_id":crash_session.session_id}),
        )?;

        let summary = json!({
            "status":"AI_UI_RELIABILITY_PASSED",
            "markers":[
                "AI_UI_RELIABILITY_PASSED",
                "AI_UI_MULTI_WINDOW_PASSED",
                "AI_UI_RUNTIME_FAILURE_DETECTED",
                "AI_UI_EVIDENCE_EXPORT_PASSED"
            ],
            "sessions":{
                "a":session_a,
                "b":session_b,
                "crash":crash_session
            },
            "identity":{
                "ambiguous_window":ambiguous,
                "window_session_mismatch":mismatch
            },
            "verified_actions":{
                "input_a":input_a,
                "input_b":input_b,
                "click_a":click_a,
                "semantic_under_overlay":semantic_under_overlay
            },
            "compact_tree":{
                "a_matched":compact_a["matched"],
                "b_matched":compact_b["matched"],
                "filtered_buttons":filtered_buttons["matched"]
            },
            "multi_window":{
                "focus_conflict":focus_conflict,
                "physical_after_activation":physical_after_activation,
                "screenshot_a_hash":hash_a,
                "screenshot_b_hash":hash_b,
                "capture_window_ids_verified":true,
                "semantic_without_foreground":true,
                "physical_requires_lock":true
            },
            "screenshots":{
                "window_a":screenshot_a,
                "window_b":screenshot_b,
                "window_b_under_a":screenshot_b_under_a,
                "element":element_capture,
                "region":region_capture
            },
            "coverage":coverage,
            "canvas_health":canvas,
            "sequence":sequence,
            "runtime_health":health_a,
            "runtime_failure":{
                "error":crash_error,
                "stderr_captured":true,
                "evidence":crash_evidence
            },
            "evidence":evidence_a,
            "checks":{
                "session_process_window_identity":true,
                "ambiguous_window_is_structured":true,
                "wrong_session_rejected":true,
                "verified_set_value_read_back":true,
                "click_postcondition":true,
                "wait_api":true,
                "interactability_sanity":true,
                "compact_tree_filters":true,
                "multi_window_isolation":true,
                "input_lock_and_activation":true,
                "window_element_region_screenshots":true,
                "canvas_health":true,
                "semantic_coverage":true,
                "declarative_sequence":true,
                "runtime_failure_and_stderr":true,
                "automatic_failure_evidence":true
            }
        });
        write_json(&output_dir.join("summary.json"), &summary)?;
        println!("{}", serde_json::to_string(&summary)?);
        Ok(())
    }

    #[derive(Clone, Debug)]
    struct ProbeIds {
        input: String,
        create: String,
        result: String,
        async_button: String,
        async_result: String,
    }

    impl ProbeIds {
        fn from_compact(tree: &Value) -> Result<Self, Box<dyn Error>> {
            Ok(Self {
                input: find_suffix(tree, "/input.name")?,
                create: find_suffix(tree, "/button.create")?,
                result: find_suffix(tree, "/result.message")?,
                async_button: find_suffix(tree, "/button.async")?,
                async_result: find_suffix(tree, "/async.result")?,
            })
        }
    }

    fn find_suffix(tree: &Value, suffix: &str) -> Result<String, Box<dyn Error>> {
        tree.get("elements")
            .and_then(Value::as_array)
            .and_then(|elements| {
                elements.iter().find_map(|element| {
                    element
                        .get("automation_id")
                        .and_then(Value::as_str)
                        .filter(|id| id.ends_with(suffix))
                        .map(str::to_owned)
                })
            })
            .ok_or_else(|| format!("compact tree is missing {suffix}").into())
    }

    fn assert_value(
        manager: &mut ReliabilityManager,
        session_id: &str,
        id: &str,
        expected: &str,
    ) -> Result<(), Box<dyn Error>> {
        let state: ElementState = serde_json::from_value(call_ok(
            manager,
            "read",
            json!({"session_id":session_id,"id":id}),
        )?)?;
        require(
            state.value.as_deref() == Some(expected),
            format!("{session_id} expected {expected:?}, got {:?}", state.value),
        )
    }

    fn require_capture_owner(value: &Value, expected: &str) -> Result<(), Box<dyn Error>> {
        require(
            value["capture"]["window_id"].as_str() == Some(expected),
            format!(
                "capture window mismatch: expected {expected}, got {:?}",
                value["capture"]["window_id"]
            ),
        )
    }

    fn required_path(value: &Value) -> Result<PathBuf, Box<dyn Error>> {
        Ok(PathBuf::from(required_string(value, "path")?))
    }

    fn required_string(value: &Value, key: &str) -> Result<String, Box<dyn Error>> {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("response has no string {key}: {value}").into())
    }

    fn hash_file(path: &Path) -> Result<u64, Box<dyn Error>> {
        let bytes = fs::read(path)?;
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        Ok(hasher.finish())
    }

    fn verify_evidence_bundle(path: &Path) -> Result<(), Box<dyn Error>> {
        for required in [
            "summary.json",
            "actions.json",
            "results.json",
            "errors.json",
            "ui-changes.json",
            "last-ui-tree.json",
            "runtime.json",
            "screenshots",
        ] {
            require(
                path.join(required).exists(),
                format!("evidence bundle is missing {required}: {}", path.display()),
            )?;
        }
        let screenshot_count = fs::read_dir(path.join("screenshots"))?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
            })
            .count();
        require(
            screenshot_count > 0,
            format!("evidence bundle has no captured PNG: {}", path.display()),
        )?;
        Ok(())
    }

    fn call_ok(
        manager: &mut ReliabilityManager,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, Box<dyn Error>> {
        let response = manager.call(operation, arguments);
        if response["success"] == true {
            Ok(response["result"].clone())
        } else {
            let error: HostError = serde_json::from_value(response["error"].clone())?;
            Err(error.into())
        }
    }

    fn expect_error(
        manager: &mut ReliabilityManager,
        operation: &str,
        arguments: Value,
        expected_code: &str,
    ) -> Result<Value, Box<dyn Error>> {
        let response = manager.call(operation, arguments);
        require(
            response["success"] == false,
            format!("{operation} unexpectedly succeeded: {response}"),
        )?;
        require(
            response["error"]["code"].as_str() == Some(expected_code),
            format!(
                "{operation} expected {expected_code}, got {}",
                response["error"]
            ),
        )?;
        Ok(response["error"].clone())
    }

    fn require(condition: bool, message: impl Into<String>) -> Result<(), Box<dyn Error>> {
        if condition {
            Ok(())
        } else {
            Err(message.into().into())
        }
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
            .join("ai-ui-test-tool-reliability")
    }
}
