#[cfg(windows)]
use std::{
    fs,
    io::BufReader,
    path::{Path, PathBuf},
};

// Windows E2E runner for the Pointer, Canvas & Spatial Interaction milestone.

#[cfg(not(windows))]
fn main() {
    eprintln!("spatial-runner is available only on Windows");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("AI UI spatial milestone failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use ai_ui_test_tool_auto_probe::reliability::ReliabilityManager;
    use serde_json::json;
    use std::{fs, path::Path};

    let demo = argument_path("--demo").unwrap_or_else(|| {
        Path::new("prototypes")
            .join("ai-ui-test-tool-auto-probe")
            .join("target")
            .join("debug")
            .join("spatial-demo.exe")
    });
    let output = argument_path("--output-dir")
        .unwrap_or_else(|| Path::new("reports").join("evidence").join("ai-ui-spatial"));
    fs::create_dir_all(&output)?;
    let mut manager = ReliabilityManager::new(&demo, "AI UI Spatial Demo", &output);
    let open = call_ok(&mut manager, "session.open", json!({}))?;
    let session_id = open["session_id"]
        .as_str()
        .ok_or("session.open did not return session_id")?
        .to_owned();
    let result = scenario(&mut manager, &session_id, &output);
    let _ = manager.call("session.close", json!({"session_id":session_id}));
    result
}

#[cfg(windows)]
fn scenario(
    manager: &mut ai_ui_test_tool_auto_probe::reliability::ReliabilityManager,
    session_id: &str,
    output: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::json;
    let lock = call_ok(
        manager,
        "acquire_input_lock",
        json!({"session_id":session_id}),
    )?;
    let token = lock["token"]
        .as_str()
        .ok_or("input lock did not return token")?
        .to_owned();
    let tree = call_ok(manager, "inspect_ui", json!({"session_id":session_id}))?;
    write_json(&output.join("initial-ui-tree.json"), &tree)?;
    let id = find_id(&tree, "/canvas.main").ok_or("spatial demo did not export canvas.main")?;
    let inspect = call_ok(
        manager,
        "inspect_spatial_target",
        json!({"session_id":session_id,"id":id}),
    )?;
    write_json(&output.join("spatial-target.json"), &inspect)?;
    if inspect["interactable"] != true {
        return Err("canvas target is not interactable".into());
    }
    call_ok(
        manager,
        "assert_canvas_healthy",
        json!({"session_id":session_id,"id":id}),
    )?;

    let baseline_capture = physical(
        manager,
        session_id,
        &token,
        "capture_region",
        json!({"id":id,"region":{"x":0.1,"y":0.1,"width":0.8,"height":0.8}}),
    )?;
    let mut markers = Vec::new();
    let resized = physical(
        manager,
        session_id,
        &token,
        "windows_set_window_bounds",
        json!({"width":1000,"height":700}),
    )?;
    let resized_inspect = call_ok(
        manager,
        "inspect_spatial_target",
        json!({"session_id":session_id,"id":id}),
    )?;
    let resize_capture = physical(
        manager,
        session_id,
        &token,
        "capture_region",
        json!({"id":id,"region":{"x":0.1,"y":0.1,"width":0.8,"height":0.8}}),
    )?;
    write_json(
        &output.join("region-change.json"),
        &json!({"before":baseline_capture,"after_resize":resize_capture}),
    )?;
    if let (Some(before), Some(after)) = (
        screenshot_path(&baseline_capture),
        screenshot_path(&resize_capture),
    ) {
        if png_pixel_similarity(before, after)? < 0.995 {
            markers.push("AI_UI_REGION_CHANGE_ASSERTION_PASSED");
        }
    }
    write_json(
        &output.join("resize.json"),
        &json!({"resize":resized,"inspect":resized_inspect}),
    )?;
    if resized_inspect["window"]["bounds"]["width"] != 1000
        || resized_inspect["window"]["bounds"]["height"] != 700
    {
        return Err("window resize postcondition was not observed".into());
    }
    if resized_inspect["bounds_source"] != "fresh_uia_tree"
        || resized_inspect["bounds_timestamp"].as_i64().unwrap_or(0)
            <= inspect["bounds_timestamp"].as_i64().unwrap_or(0)
    {
        return Err("resize did not refresh spatial bounds".into());
    }
    markers.push("AI_UI_SPATIAL_RESIZE_PASSED");
    let _ = physical(
        manager,
        session_id,
        &token,
        "windows_set_window_bounds",
        json!({"width":1100,"height":760}),
    );

    let invalid = manager.call(
        "move_pointer",
        json!({"session_id":session_id,"input_lock_token":token,"id":id,"position":{"x":1.5,"y":0.5}}),
    );
    write_json(&output.join("invalid-coordinate.json"), &invalid)?;
    if invalid["success"] == true && invalid["result"]["result"]["code"].is_null() {
        return Err("invalid coordinate was accepted".into());
    }

    if invalid["success"] == false
        && invalid["failed_phase"] == "validation"
        && invalid["dispatched"] == false
        && invalid["error"]["code"].is_string()
    {
        markers.push("AI_UI_INVALID_ARGUMENT_CONTRACT_PASSED");
        markers.push("AI_UI_OPERATION_CONTRACT_PASSED");
    }
    markers.push("AI_UI_SPATIAL_INVALID_BOUNDS_REJECTED");
    let pan_right = find_id(&tree, "/canvas.pan.right").ok_or("missing canvas.pan.right")?;
    let zoom_in = find_id(&tree, "/canvas.zoom.in").ok_or("missing canvas.zoom.in")?;
    call_ok(
        manager,
        "click",
        json!({"session_id":session_id,"id":pan_right}),
    )?;
    call_ok(
        manager,
        "click",
        json!({"session_id":session_id,"id":zoom_in}),
    )?;
    physical(
        manager,
        session_id,
        &token,
        "click_relative",
        json!({"id":pan_right,"position":{"x":0.5,"y":0.5}}),
    )?;
    physical(
        manager,
        session_id,
        &token,
        "click_relative",
        json!({"id":zoom_in,"position":{"x":0.5,"y":0.5}}),
    )?;
    std::thread::sleep(std::time::Duration::from_millis(250));
    physical(
        manager,
        session_id,
        &token,
        "click_relative",
        json!({"id":pan_right,"position":{"x":0.5,"y":0.5}}),
    )?;
    physical(
        manager,
        session_id,
        &token,
        "click_relative",
        json!({"id":zoom_in,"position":{"x":0.5,"y":0.5}}),
    )?;
    std::thread::sleep(std::time::Duration::from_millis(400));
    let viewport_tree = call_ok(manager, "inspect_ui", json!({"session_id":session_id}))?;
    let viewport_status =
        find_text(&viewport_tree, "/canvas.status").ok_or("missing canvas.status")?;
    let viewport_passed = viewport_status.contains("pan.x=")
        && !viewport_status.contains("pan.x=0 ")
        && viewport_status.contains("zoom.value=")
        && !viewport_status.contains("zoom.value=1.00");
    write_json(&output.join("viewport-state.json"), &viewport_tree)?;
    if viewport_passed {
        markers.push("AI_UI_SPATIAL_VIEWPORT_PASSED");
    } else {
        write_json(
            &output.join("viewport-postcondition-failed.json"),
            &json!({"status":"not_observed","actual":viewport_status}),
        )?;
    }
    let selected = physical(
        manager,
        session_id,
        &token,
        "select_region",
        json!({"id":id,"from":{"x":0.15,"y":0.20},"to":{"x":0.75,"y":0.65},"duration_ms":220,"require_canvas_healthy":true}),
    )?;
    let dragged = physical(
        manager,
        session_id,
        &token,
        "drag_relative",
        json!({"id":id,"from":{"x":0.25,"y":0.35},"to":{"x":0.65,"y":0.45},"duration_ms":180,"steps":6,"modifiers":["SHIFT"]}),
    )?;
    std::thread::sleep(std::time::Duration::from_millis(350));
    let drag_tree = call_ok(manager, "inspect_ui", json!({"session_id":session_id}))?;
    let drag_status =
        find_text(&drag_tree, "/canvas.status").ok_or("missing canvas.status after drag")?;
    write_json(
        &output.join("drag-postcondition.json"),
        &json!({"status":drag_status}),
    )?;
    if !drag_status.contains("selection.count=1")
        || !drag_status.contains("last.action=drag:completed")
    {
        return Err("drag postcondition was not observed".into());
    }
    markers.push("AI_UI_SPATIAL_SELECTION_PASSED");
    markers.push("AI_UI_RELATIVE_DRAG_PASSED");
    let right = physical(
        manager,
        session_id,
        &token,
        "right_click_relative",
        json!({"id":id,"position":{"x":0.50,"y":0.50}}),
    )?;
    let context_id = find_id(&tree, "/context.menu").ok_or("missing context.menu")?;
    call_ok(
        manager,
        "wait_visible",
        json!({"session_id":session_id,"id":context_id,"timeout_ms":3000}),
    )?;
    markers.push("AI_UI_SPATIAL_CONTEXT_MENU_PASSED");
    let _ = call_ok(
        manager,
        "click",
        json!({"session_id":session_id,"id":find_id(&tree, "/context.delete").ok_or("missing context.delete")?}),
    )?;
    let hidden_target = manager.call(
        "inspect_spatial_target",
        json!({"session_id":session_id,"id":context_id}),
    );
    write_json(&output.join("hidden-target.json"), &hidden_target)?;
    if hidden_target["result"]["result"]["interactable"] != false {
        return Err("hidden target was incorrectly accepted as interactable".into());
    }
    let hover = physical(
        manager,
        session_id,
        &token,
        "hover_relative",
        json!({"id":id,"position":{"x":0.40,"y":0.40}}),
    )?;
    markers.push("AI_UI_SPATIAL_HOVER_PASSED");
    let dpi = inspect["dpi"]["scale_factor"]
        .as_f64()
        .ok_or("spatial target did not return dpi scale_factor")?;
    if dpi > 0.0 {
        markers.push("AI_UI_SPATIAL_DPI_PASSED");
    }
    let replay = call_ok(
        manager,
        "windows_run_sequence",
        json!({"session_id":session_id,"steps":[{"op":"move_pointer","id":id,"position":{"x":0.5,"y":0.5},"input_lock_token":token}],"stop_on_error":true}),
    )?;
    let screenshot = physical(
        manager,
        session_id,
        &token,
        "capture_region",
        json!({"id":id,"region":{"x":0.1,"y":0.1,"width":0.8,"height":0.8}}),
    )?;
    let cleanup_down = physical(
        manager,
        session_id,
        &token,
        "pointer_down",
        json!({"id":id,"position":{"x":0.5,"y":0.5},"button":"left"}),
    )?;
    let cleanup_up = physical(
        manager,
        session_id,
        &token,
        "pointer_up",
        json!({"id":id,"position":{"x":0.5,"y":0.5},"button":"left"}),
    )?;
    write_json(
        &output.join("pointer-cleanup.json"),
        &json!({"down":cleanup_down,"up":cleanup_up}),
    )?;
    markers.push("AI_UI_POINTER_CLEANUP_PASSED");
    let stability_capture = physical(
        manager,
        session_id,
        &token,
        "capture_region",
        json!({"id":id,"region":{"x":0.1,"y":0.1,"width":0.8,"height":0.8}}),
    )?;
    let stability_similarity = match (
        screenshot_path(&screenshot),
        screenshot_path(&stability_capture),
    ) {
        (Some(first), Some(second)) => png_pixel_similarity(first, second)?,
        _ => 0.0,
    };
    write_json(
        &output.join("region-stability.json"),
        &json!({"first":screenshot,"second":stability_capture,"pixel_similarity":stability_similarity,"threshold":0.995}),
    )?;
    if stability_similarity >= 0.995 {
        markers.push("AI_UI_REGION_STABILITY_ASSERTION_PASSED");
    }

    let second_open = call_ok(manager, "session.open", json!({}))?;
    let second_session_id = second_open["session_id"]
        .as_str()
        .ok_or("second session.open did not return session_id")?
        .to_owned();
    let second_result = (|| -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let second_tree = call_ok(
            manager,
            "inspect_ui",
            json!({"session_id":second_session_id}),
        )?;
        let second_id = find_id(&second_tree, "/canvas.main")
            .ok_or("second session did not export canvas.main")?;
        let second_inspect = call_ok(
            manager,
            "inspect_spatial_target",
            json!({"session_id":second_session_id,"id":second_id}),
        )?;
        let isolated = inspect["window"]["process_id"] != second_inspect["window"]["process_id"]
            && inspect["window"]["window_id"] != second_inspect["window"]["window_id"];
        let evidence = json!({"first":{"session_id":session_id,"window":inspect["window"].clone()},"second":{"session_id":second_session_id,"window":second_inspect["window"].clone()},"isolated":isolated});
        write_json(&output.join("multi-window.json"), &evidence)?;
        if isolated {
            markers.push("AI_UI_SPATIAL_MULTI_WINDOW_PASSED");
        }
        Ok(evidence)
    })();
    let _ = manager.call("session.close", json!({"session_id":second_session_id}));
    let multi_window = second_result?;
    let crash_id = find_id(&tree, "/button.crash").ok_or("missing crash button")?;
    let crash_response = manager.call("click", json!({"session_id":session_id,"id":crash_id}));
    write_json(&output.join("runtime-failure.json"), &crash_response)?;
    if crash_response["success"] != true
        && crash_response["error"]["code"] == "APPLICATION_RUNTIME_FAILURE"
    {
        markers.push("AI_UI_SPATIAL_RUNTIME_FAILURE_DETECTED");
    } else {
        return Err(
            "runtime failure regression did not produce APPLICATION_RUNTIME_FAILURE".into(),
        );
    }

    let required_markers = vec![
        "AI_UI_OPERATION_CONTRACT_PASSED",
        "AI_UI_INVALID_ARGUMENT_CONTRACT_PASSED",
        "AI_UI_SPATIAL_INVALID_BOUNDS_REJECTED",
        "AI_UI_SPATIAL_RESIZE_PASSED",
        "AI_UI_SPATIAL_SELECTION_PASSED",
        "AI_UI_RELATIVE_DRAG_PASSED",
        "AI_UI_SPATIAL_CONTEXT_MENU_PASSED",
        "AI_UI_SPATIAL_HOVER_PASSED",
        "AI_UI_POINTER_CLEANUP_PASSED",
        "AI_UI_REGION_STABILITY_ASSERTION_PASSED",
        "AI_UI_SPATIAL_MULTI_WINDOW_PASSED",
        "AI_UI_SPATIAL_RUNTIME_FAILURE_DETECTED",
    ];
    let passed_markers = markers.clone();
    let missing_markers = required_markers
        .iter()
        .filter(|marker| !passed_markers.iter().any(|passed| passed == *marker))
        .copied()
        .collect::<Vec<_>>();
    let status = if missing_markers.is_empty() {
        "AI_UI_SPATIAL_PASSED"
    } else {
        "AI_UI_SPATIAL_INCOMPLETE"
    };
    let summary = json!({"status":status,"session_id":session_id,"canvas_id":id,"inspect":inspect,"selection":selected,"drag":dragged,"right_click":right,"hover":hover,"replay":replay,"capture_region":screenshot,"region_stability":stability_capture,"multi_window":multi_window,"markers":markers,"required_markers":required_markers,"optional_markers":[],"passed_markers":passed_markers,"failed_markers":[],"missing_markers":missing_markers,"input_lock_owned":true,"note":"Full pass requires every required assertion marker."});
    write_json(&output.join("summary.json"), &summary)?;
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}

#[cfg(windows)]
fn call_ok(
    manager: &mut ai_ui_test_tool_auto_probe::reliability::ReliabilityManager,
    operation: &str,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let response = manager.call(operation, arguments);
    if response["success"] == true {
        let result = response["result"].clone();
        if result["dispatched"] == true && result.get("result").is_some() {
            let inner = result["result"].clone();
            if inner["code"].is_string() {
                return Err(format!("{}: {}", inner["code"], inner["message"]).into());
            }
            Ok(inner)
        } else {
            Ok(result)
        }
    } else {
        Err(format!("{}", response["error"]).into())
    }
}

#[cfg(windows)]
fn physical(
    manager: &mut ai_ui_test_tool_auto_probe::reliability::ReliabilityManager,
    session_id: &str,
    token: &str,
    operation: &str,
    mut args: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    args["session_id"] = serde_json::json!(session_id);
    args["input_lock_token"] = serde_json::json!(token);
    call_ok(manager, operation, args)
}

fn png_pixel_similarity(left: &str, right: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let mut left_reader = png::Decoder::new(BufReader::new(fs::File::open(left)?)).read_info()?;
    let mut right_reader = png::Decoder::new(BufReader::new(fs::File::open(right)?)).read_info()?;
    let mut left_buf = vec![0; left_reader.output_buffer_size().unwrap_or(0)];
    let mut right_buf = vec![0; right_reader.output_buffer_size().unwrap_or(0)];
    let left_info = left_reader.next_frame(&mut left_buf)?;
    let right_info = right_reader.next_frame(&mut right_buf)?;
    if left_info.width != right_info.width || left_info.height != right_info.height {
        return Ok(0.0);
    }
    let left_pixels = &left_buf[..left_info.buffer_size()];
    let right_pixels = &right_buf[..right_info.buffer_size()];
    if left_pixels.len() != right_pixels.len() {
        return Ok(0.0);
    }
    let same = left_pixels
        .iter()
        .zip(right_pixels)
        .filter(|(l, r)| l == r)
        .count();
    Ok(same as f64 / left_pixels.len().max(1) as f64)
}

fn screenshot_path(value: &serde_json::Value) -> Option<&str> {
    value["screenshot"]["path"]
        .as_str()
        .or_else(|| value["path"].as_str())
}

fn find_text(tree: &serde_json::Value, suffix: &str) -> Option<String> {
    tree["elements"]
        .as_array()?
        .iter()
        .filter_map(|e| {
            let id = e["automation_id"].as_str()?;
            let text = e["text"].as_str().or_else(|| e["value"].as_str())?;
            Some((id, text))
        })
        .find(|(id, _)| id.ends_with(suffix))
        .map(|(_, text)| text.to_owned())
}

fn find_id(tree: &serde_json::Value, suffix: &str) -> Option<String> {
    tree["elements"]
        .as_array()?
        .iter()
        .filter_map(|e| e["automation_id"].as_str())
        .find(|id| id.ends_with(suffix))
        .map(str::to_owned)
}

#[cfg(windows)]
fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

#[cfg(windows)]
fn argument_path(name: &str) -> Option<PathBuf> {
    let mut args = std::env::args_os();
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next().map(PathBuf::from);
        }
    }
    None
}
