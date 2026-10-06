//! Windows E2E audit for generic top-level window interactions.
//!
//! Programmatic bounds/state calls are setup and recovery only. Every real
//! gesture below goes through ReliabilityManager's single physical-input lock.

#[cfg(not(windows))]
fn main() {
    eprintln!("window-frame-runner is available only on Windows");
    std::process::exit(1);
}

#[cfg(windows)]
use std::{
    collections::BTreeSet,
    fs,
    mem::size_of,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[cfg(windows)]
use ai_ui_test_tool_auto_probe::reliability::ReliabilityManager;
#[cfg(windows)]
use serde_json::{Value, json};
#[cfg(windows)]
use toolkit_windows_automation::{
    ErrorCode, WindowFrameTargetKind, WindowQuery, WindowsAutomation,
};
#[cfg(windows)]
use windows_uia::{
    Win32::{
        Foundation::{LPARAM, RECT, TRUE},
        Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO},
        UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS},
    },
    core::BOOL,
};

#[cfg(windows)]
#[derive(Clone, Copy)]
struct Counts {
    moves: usize,
    edge: usize,
    corner: usize,
    state: usize,
    semantic: usize,
}

#[cfg(windows)]
impl Counts {
    fn from_args() -> (Self, bool) {
        if std::env::args().any(|arg| arg == "--smoke") {
            (
                Self {
                    moves: 1,
                    edge: 1,
                    corner: 1,
                    state: 1,
                    semantic: 1,
                },
                true,
            )
        } else {
            (
                Self {
                    moves: 30,
                    edge: 20,
                    corner: 20,
                    state: 20,
                    semantic: 30,
                },
                false,
            )
        }
    }
}

#[cfg(windows)]
#[derive(Default)]
struct Audit {
    records: Vec<Value>,
    assertions: usize,
    failures: usize,
    warnings: Vec<String>,
}

#[cfg(windows)]
impl Audit {
    fn call(
        &mut self,
        manager: &mut ReliabilityManager,
        name: &str,
        operation: &str,
        arguments: Value,
    ) -> Option<Value> {
        let started = Instant::now();
        let response = manager.call(operation, arguments);
        let result = unpack(&response);
        let success = result.is_ok();
        self.records.push(json!({
            "kind":"operation",
            "name":name,
            "operation":operation,
            "success":success,
            "duration_ms":started.elapsed().as_millis(),
            "response":response,
        }));
        if !success {
            self.failures += 1;
        }
        result.ok()
    }

    fn assert(&mut self, name: impl Into<String>, passed: bool, evidence: Value) {
        self.assertions += 1;
        if !passed {
            self.failures += 1;
        }
        self.records.push(json!({
            "kind":"assertion",
            "name":name.into(),
            "success":passed,
            "evidence":evidence,
        }));
    }

    fn expected_constraint(
        &mut self,
        manager: &mut ReliabilityManager,
        session: &str,
        token: &str,
    ) {
        let started = Instant::now();
        let response = manager.call(
            "windows_resize_frame",
            json!({
                "session_id":session,
                "input_lock_token":token,
                "target":"right",
                "delta_x":-1600,
                "delta_y":0,
                "steps":8,
                "duration_ms":260,
            }),
        );
        let result = unpack(&response).ok();
        let typed = response["error"]["code"] == "WINDOW_RESIZE_CONSTRAINT";
        let observed = result
            .as_ref()
            .is_some_and(|value| value["constraint_observed"] == true);
        let classified = typed || observed;
        self.records.push(json!({
            "kind":"operation",
            "name":"constraints.minimum-size",
            "operation":"windows_resize_frame",
            "success":classified,
            "expected_constraint":true,
            "duration_ms":started.elapsed().as_millis(),
            "response":response,
        }));
        self.assert(
            "constraints.minimum-size.classified",
            classified,
            json!({"typed":typed,"observed":observed,"result":result}),
        );
        if !observed {
            self.warnings.push(
                "Minimum-size drag was attempted without a conclusive clamp observation."
                    .to_owned(),
            );
        }
    }
}

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("AI UI window-frame audit failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (counts, smoke) = Counts::from_args();
    let demo = argument_path("--demo")
        .unwrap_or_else(|| Path::new("target").join("debug").join("spatial-demo.exe"));
    let output = argument_path("--output-dir").unwrap_or_else(|| {
        Path::new("reports")
            .join("evidence")
            .join("ai-ui-window-frame")
    });
    let fixture = argument_path("--fixed-fixture").unwrap_or_else(|| {
        Path::new("..")
            .join("..")
            .join("target")
            .join("debug")
            .join("windows-gui-self-test-fixture.exe")
    });
    fs::create_dir_all(&output)?;

    let mut manager = ReliabilityManager::new(&demo, "AI UI Spatial Demo", &output);
    let mut audit = Audit::default();
    let open = audit
        .call(&mut manager, "session.open", "session.open", json!({}))
        .ok_or("session.open failed")?;
    let session = open["session_id"]
        .as_str()
        .ok_or("session.open did not return session_id")?
        .to_owned();
    let lock = audit
        .call(
            &mut manager,
            "input-lock.acquire",
            "acquire_input_lock",
            json!({"session_id":session}),
        )
        .ok_or("acquire_input_lock failed")?;
    let token = lock["token"]
        .as_str()
        .ok_or("input lock did not return token")?
        .to_owned();

    let scenario = scenario(&mut manager, &mut audit, &session, &token, counts, &fixture);
    let released = audit.call(
        &mut manager,
        "input-lock.release",
        "release_input_lock",
        json!({"session_id":session,"token":token}),
    );
    audit.assert(
        "cleanup.global-input-lock-released",
        released.as_ref() == Some(&Value::Bool(true)),
        json!({"release":released}),
    );
    let closed = audit.call(
        &mut manager,
        "session.close",
        "session.close",
        json!({"session_id":session}),
    );
    audit.assert(
        "cleanup.session-closed",
        closed.is_some(),
        json!({"close":closed}),
    );
    if let Err(error) = &scenario {
        audit.assert("scenario.bootstrap", false, json!({"error":error}));
    }

    let smoke_pass = smoke && scenario.is_ok() && audit.failures == 0;
    let full_pass = !smoke && scenario.is_ok() && audit.failures == 0;
    let markers = if full_pass {
        vec![
            "AI_UI_WINDOW_FRAME_AUDIT_PASSED",
            "AI_UI_WINDOW_PHYSICAL_MOVE_PASSED",
            "AI_UI_WINDOW_EDGE_RESIZE_PASSED",
            "AI_UI_WINDOW_CORNER_RESIZE_PASSED",
            "AI_UI_WINDOW_STATE_TRANSITIONS_PASSED",
            "AI_UI_WINDOW_POST_RESIZE_LAYOUT_PASSED",
            "AI_UI_WINDOW_POST_MOVE_HITTEST_PASSED",
            "AI_UI_WINDOW_MULTIWINDOW_ISOLATION_PASSED",
            "AI_UI_WINDOW_POINTER_CLEANUP_PASSED",
            "AI_UI_WINDOW_FIXED_SIZE_REFUSAL_PASSED",
            "AI_UI_WINDOW_INTERACTION_V1_PASSED",
        ]
    } else {
        Vec::new()
    };
    let summary = json!({
        "status":if full_pass {"AI_UI_WINDOW_INTERACTION_V1_PASSED"} else if smoke_pass {"AI_UI_WINDOW_INTERACTION_SMOKE_COMPLETE"} else {"AI_UI_WINDOW_INTERACTION_INCOMPLETE"},
        "mode":if smoke {"smoke"} else {"full"},
        "requested_cycles":{"title_moves":counts.moves,"each_edge":counts.edge,"each_corner":counts.corner,"state_cycles":counts.state,"post_move_and_resize_semantic_clicks":counts.semantic},
        "scenario":scenario.as_ref().ok(),
        "scenario_error":scenario.as_ref().err(),
        "failure_count":audit.failures,
        "assertion_count":audit.assertions,
        "warnings":audit.warnings,
        "markers":markers,
        "records":audit.records,
        "single_global_physical_input_lock":true,
    });
    fs::write(
        output.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    println!(
        "window-frame-runner status={} summary={}",
        summary["status"].as_str().unwrap_or("UNKNOWN"),
        output.join("summary.json").display()
    );
    if full_pass || smoke_pass {
        Ok(())
    } else {
        Err("window-frame audit gate was not met".into())
    }
}

#[cfg(windows)]
fn scenario(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    session: &str,
    token: &str,
    counts: Counts,
    fixture: &Path,
) -> Result<Value, String> {
    let initial = audit
        .call(
            manager,
            "inspection.initial",
            "inspect_window_interaction",
            json!({"session_id":session}),
        )
        .ok_or("initial window inspection failed")?;
    audit.assert(
        "inspection.coordinate-model",
        initial["coordinate_model"] == "screen_physical_pixels",
        initial.clone(),
    );
    audit.assert(
        "inspection.makepad-custom-caption",
        initial["frame_kind"] == "custom_caption",
        initial.clone(),
    );
    audit.assert(
        "inspection.title-and-eight-frame-targets",
        initial["movable"] == true
            && initial["resizable"] == true
            && initial["minimizable"] == true
            && initial["maximizable"] == true
            && initial["title_target"].is_object()
            && initial["frame_targets"]
                .as_array()
                .is_some_and(|items| items.len() == 8),
        initial.clone(),
    );
    let tree = audit
        .call(
            manager,
            "inspection.tree",
            "inspect_ui",
            json!({"session_id":session}),
        )
        .ok_or("initial semantic tree failed")?;
    let canvas = find_id(&tree, "/canvas.main").ok_or("canvas.main absent")?;
    let pan = find_id(&tree, "/canvas.pan.right").ok_or("canvas.pan.right absent")?;
    let zoom = find_id(&tree, "/canvas.zoom.in").ok_or("canvas.zoom.in absent")?;
    let context = find_id(&tree, "/context.menu").ok_or("context.menu absent")?;
    let context_delete = find_id(&tree, "/context.delete").ok_or("context.delete absent")?;

    size_matrix(manager, audit, session, &initial, &canvas);
    let move_directions = [(34, 0), (-34, 0), (0, 24), (0, -24)];
    for index in 0..counts.moves {
        let (delta_x, delta_y) = move_directions[index % move_directions.len()];
        title_drag(
            manager,
            audit,
            session,
            token,
            &format!("move.{index}"),
            delta_x,
            delta_y,
        );
    }
    for (target, dx, dy) in [
        ("left", -32, 0),
        ("right", 32, 0),
        ("top", 0, -26),
        ("bottom", 0, 26),
    ] {
        for index in 0..counts.edge {
            let direction = if index % 2 == 0 { 1 } else { -1 };
            resize_drag(
                manager,
                audit,
                session,
                token,
                &format!("resize.edge.{target}.{index}"),
                target,
                dx * direction,
                dy * direction,
            );
        }
    }
    for (target, dx, dy) in [
        ("top_left", -30, -24),
        ("top_right", 30, -24),
        ("bottom_left", -30, 24),
        ("bottom_right", 30, 24),
    ] {
        for index in 0..counts.corner {
            let direction = if index % 2 == 0 { 1 } else { -1 };
            resize_drag(
                manager,
                audit,
                session,
                token,
                &format!("resize.corner.{target}.{index}"),
                target,
                dx * direction,
                dy * direction,
            );
        }
    }
    state_cycles(manager, audit, session, token, counts.state);
    semantic_after_window_actions(manager, audit, session, token, counts.semantic, &pan, &zoom);
    desktop_boundary_probe(manager, audit, session, token);
    popup_and_canvas(
        manager,
        audit,
        session,
        token,
        &canvas,
        &context,
        &context_delete,
    );
    if prepare(manager, audit, session, "constraints.minimum-size") {
        audit.expected_constraint(manager, session, token);
    }
    resize_drag(
        manager,
        audit,
        session,
        token,
        "constraints.large-size",
        "bottom_right",
        520,
        360,
    );

    let process_id = initial["window"]["process_id"]
        .as_u64()
        .ok_or("window process id absent")? as u32;
    let primary_window = initial["window"]["window_id"]
        .as_str()
        .ok_or("primary window id absent")?;
    same_pid_isolation(manager, audit, session, token, primary_window);
    cross_process_isolation(manager, audit, session, token, process_id, primary_window);
    fixed_size_refusal(audit, fixture);
    monitor_metadata(manager, audit, session, token, &initial, &pan);
    Ok(json!({"initial":initial,"canvas_id":canvas,"process_id":process_id}))
}

#[cfg(windows)]
fn size_matrix(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    session: &str,
    initial: &Value,
    canvas: &str,
) {
    let work = &initial["monitor"]["work_area"];
    let work_x = work["x"].as_i64().unwrap_or(0) as i32;
    let work_y = work["y"].as_i64().unwrap_or(0) as i32;
    let work_width = work["width"].as_i64().unwrap_or(1280).max(640) as i32;
    let work_height = work["height"].as_i64().unwrap_or(720).max(480) as i32;
    for (index, (name, percent)) in [
        ("small", 40_i32),
        ("medium", 65_i32),
        ("large", 85_i32),
        ("small-again", 40_i32),
        ("restored-medium", 65_i32),
    ]
    .into_iter()
    .enumerate()
    {
        let width = ((work_width.saturating_mul(percent)) / 100).clamp(480, work_width - 24);
        let height = ((work_height.saturating_mul(percent)) / 100).clamp(360, work_height - 24);
        let x = work_x + ((work_width - width) / 2).max(8);
        let y = work_y + ((work_height - height) / 2).max(8);
        let prefix = format!("size-matrix.{index}.{name}");
        let set = audit.call(
            manager,
            &format!("{prefix}.set-bounds"),
            "windows_set_window_bounds",
            json!({"session_id":session,"x":x,"y":y,"width":width,"height":height}),
        );
        let inspected = audit.call(
            manager,
            &format!("{prefix}.inspect"),
            "inspect_window_interaction",
            json!({"session_id":session}),
        );
        let tree = audit.call(
            manager,
            &format!("{prefix}.tree"),
            "inspect_ui",
            json!({"session_id":session}),
        );
        let spatial = audit.call(
            manager,
            &format!("{prefix}.canvas"),
            "inspect_spatial_target",
            json!({"session_id":session,"id":canvas}),
        );
        let screenshot = audit.call(
            manager,
            &format!("{prefix}.screenshot"),
            "screenshot_window",
            json!({"session_id":session}),
        );
        let health = audit.call(
            manager,
            &format!("{prefix}.runtime"),
            "get_runtime_health",
            json!({"session_id":session}),
        );
        let bounds_match = inspected.as_ref().is_some_and(|value| {
            value["window"]["bounds"]["width"].as_i64() == Some(width as i64)
                && value["window"]["bounds"]["height"].as_i64() == Some(height as i64)
        });
        let layout_valid = tree
            .as_ref()
            .is_some_and(|value| find_id(value, "/canvas.main").is_some())
            && spatial.as_ref().is_some_and(|value| {
                value["canvas_health"] == "Healthy" && value["interactable"] == true
            });
        audit.assert(
            format!("{prefix}.valid"),
            set.as_ref().is_some_and(|value| value["success"] == true)
                && bounds_match
                && layout_valid
                && screenshot.is_some()
                && health
                    .as_ref()
                    .is_some_and(|value| value["healthy"] == true),
            json!({
                "requested":{"x":x,"y":y,"width":width,"height":height},
                "set":set,
                "inspection":inspected,
                "tree":tree,
                "spatial":spatial,
                "screenshot":screenshot,
                "runtime":health
            }),
        );
    }
}
#[cfg(windows)]
fn title_drag(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    session: &str,
    token: &str,
    name: &str,
    delta_x: i32,
    delta_y: i32,
) -> bool {
    if !prepare(manager, audit, session, name) {
        return false;
    }
    let result = physical(
        manager,
        audit,
        name,
        session,
        token,
        "windows_drag_title",
        json!({"delta_x":delta_x,"delta_y":delta_y,"steps":6,"duration_ms":220}),
    );
    audit.assert(
        format!("{name}.operation"),
        result.is_some(),
        json!({"result":result}),
    );
    if let Some(result) = result {
        verify_gesture(audit, name, &result);
        audit.assert(
            format!("{name}.bounds-changed"),
            position_changed(&result["pre_bounds"], &result["post_bounds"]),
            json!({"pre":result["pre_bounds"],"post":result["post_bounds"]}),
        );
        true
    } else {
        false
    }
}

#[cfg(windows)]
fn resize_drag(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    session: &str,
    token: &str,
    name: &str,
    target: &str,
    delta_x: i32,
    delta_y: i32,
) -> bool {
    if !prepare(manager, audit, session, name) {
        return false;
    }
    let result = physical(
        manager,
        audit,
        name,
        session,
        token,
        "windows_resize_frame",
        json!({"target":target,"delta_x":delta_x,"delta_y":delta_y,"steps":6,"duration_ms":220}),
    );
    audit.assert(
        format!("{name}.operation"),
        result.is_some(),
        json!({"result":result}),
    );
    if let Some(result) = result {
        verify_gesture(audit, name, &result);
        audit.assert(
            format!("{name}.target"),
            result["frame_target"]["target"] == target
                && result["frame_target"]["source"] == "wm_nchittest"
                && changed(&result["pre_bounds"], &result["post_bounds"]),
            json!({"frame_target":result["frame_target"],"pre":result["pre_bounds"],"post":result["post_bounds"]}),
        );
        true
    } else {
        false
    }
}

#[cfg(windows)]
fn state_cycles(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    session: &str,
    token: &str,
    count: usize,
) {
    for index in 0..count {
        for state in ["maximized", "minimized", "normal"] {
            let name = format!("state.programmatic.{index}.{state}");
            let result = audit.call(
                manager,
                &name,
                "windows_set_window_state",
                json!({"session_id":session,"state":state}),
            );
            let inspected = audit.call(
                manager,
                &format!("{name}.inspect"),
                "inspect_window_interaction",
                json!({"session_id":session}),
            );
            let layout = (state != "minimized").then(|| {
                audit.call(
                    manager,
                    &format!("{name}.layout"),
                    "inspect_ui",
                    json!({"session_id":session}),
                )
            });
            audit.assert(
                format!("{name}.readback"),
                result.as_ref().is_some_and(|value| {
                    value["after"]["state"] == state && value["runtime_health"]["healthy"] == true
                }) && inspected.as_ref().is_some_and(|value| {
                    value["state"] == state
                        && value["window"]["bounds"]["width"].as_i64().unwrap_or(0) > 0
                        && value["window"]["bounds"]["height"].as_i64().unwrap_or(0) > 0
                }) && layout.as_ref().is_none_or(|value| {
                    value
                        .as_ref()
                        .is_some_and(|tree| find_id(tree, "/canvas.main").is_some())
                }),
                json!({"result":result,"inspection":inspected,"layout":layout}),
            );
        }
        let name = format!("state.physical.{index}");
        if prepare(manager, audit, session, &name) {
            for expected in ["maximized", "normal"] {
                let result = physical(
                    manager,
                    audit,
                    &format!("{name}.{expected}"),
                    session,
                    token,
                    "windows_double_click_title",
                    json!({}),
                );
                audit.assert(
                    format!("{name}.{expected}.readback"),
                    result
                        .as_ref()
                        .is_some_and(|value| value["post"]["state"] == expected),
                    json!({"result":result}),
                );
                if let Some(result) = result {
                    verify_gesture(audit, &format!("{name}.{expected}"), &result);
                }
            }
        }
    }
}

#[cfg(windows)]
fn semantic_after_window_actions(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    session: &str,
    token: &str,
    count: usize,
    pan: &str,
    zoom: &str,
) {
    for index in 0..count {
        let after_move = format!("semantic.after-move.{index}");
        if title_drag(manager, audit, session, token, &after_move, 26, 14) {
            semantic_click(manager, audit, session, token, &after_move, pan);
        }
        let after_resize = format!("semantic.after-resize.{index}");
        if resize_drag(
            manager,
            audit,
            session,
            token,
            &after_resize,
            "bottom_right",
            24,
            18,
        ) {
            semantic_click(manager, audit, session, token, &after_resize, zoom);
        }
    }
}

#[cfg(windows)]
fn semantic_click(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    session: &str,
    token: &str,
    name: &str,
    id: &str,
) {
    let click = physical(
        manager,
        audit,
        &format!("{name}.element-relative-click"),
        session,
        token,
        "click_relative",
        json!({"id":id,"position":{"x":0.5,"y":0.5}}),
    );
    if let Some(click) = click.as_ref() {
        verify_relative_cleanup(audit, &format!("{name}.element-relative-click"), click);
    }
    let tree = audit.call(
        manager,
        &format!("{name}.readback"),
        "inspect_ui",
        json!({"session_id":session}),
    );
    let health = audit.call(
        manager,
        &format!("{name}.runtime"),
        "get_runtime_health",
        json!({"session_id":session}),
    );
    audit.assert(
        format!("{name}.post-window-hit-test"),
        click.is_some()
            && tree.as_ref().is_some_and(|value| {
                find_id(value, "/canvas.main").is_some()
                    && find_text(value, "/canvas.status")
                        .is_some_and(|text| text.contains("last.action="))
            })
            && health
                .as_ref()
                .is_some_and(|value| value["healthy"] == true),
        json!({"physical_click":click,"tree":tree,"runtime":health}),
    );
}

#[cfg(windows)]
fn desktop_boundary_probe(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    session: &str,
    token: &str,
) {
    let initial = audit.call(
        manager,
        "desktop-boundary.inspect",
        "inspect_window_interaction",
        json!({"session_id":session}),
    );
    let Some(initial) = initial else {
        audit.assert("desktop-boundary.initial", false, json!({}));
        return;
    };
    let work = &initial["monitor"]["work_area"];
    let work_x = work["x"].as_i64().unwrap_or(0) as i32;
    let work_y = work["y"].as_i64().unwrap_or(0) as i32;
    let work_width = work["width"].as_i64().unwrap_or(1280).max(640) as i32;
    let work_height = work["height"].as_i64().unwrap_or(720).max(480) as i32;
    let width = initial["window"]["bounds"]["width"]
        .as_i64()
        .unwrap_or(800)
        .clamp(480, i64::from((work_width - 32).max(480))) as i32;
    let height = initial["window"]["bounds"]["height"]
        .as_i64()
        .unwrap_or(600)
        .clamp(360, i64::from((work_height - 32).max(360))) as i32;
    let cases = [
        ("near-left-top", work_x + 48, work_y + 48, -24, -24),
        (
            "near-right-bottom",
            work_x + work_width - width - 48,
            work_y + work_height - height - 48,
            24,
            24,
        ),
    ];
    for (name, x, y, delta_x, delta_y) in cases {
        let prefix = format!("desktop-boundary.{name}");
        let reset = audit.call(
            manager,
            &format!("{prefix}.normal"),
            "windows_set_window_state",
            json!({"session_id":session,"state":"normal"}),
        );
        let set = audit.call(
            manager,
            &format!("{prefix}.set-bounds"),
            "windows_set_window_bounds",
            json!({"session_id":session,"x":x,"y":y,"width":width,"height":height}),
        );
        let before = audit.call(
            manager,
            &format!("{prefix}.before"),
            "inspect_window_interaction",
            json!({"session_id":session}),
        );
        let moved = physical(
            manager,
            audit,
            &format!("{prefix}.drag"),
            session,
            token,
            "windows_drag_title",
            json!({"delta_x":delta_x,"delta_y":delta_y,"steps":6,"duration_ms":220}),
        );
        if let Some(moved) = moved.as_ref() {
            verify_gesture(audit, &format!("{prefix}.drag"), moved);
        }
        let after = audit.call(
            manager,
            &format!("{prefix}.after"),
            "inspect_window_interaction",
            json!({"session_id":session}),
        );
        let tree = audit.call(
            manager,
            &format!("{prefix}.tree"),
            "inspect_ui",
            json!({"session_id":session}),
        );
        audit.assert(
            format!("{prefix}.fresh"),
            reset.is_some()
                && set.as_ref().is_some_and(|value| value["success"] == true)
                && moved.is_some()
                && interaction_bounds_changed(&before, &after)
                && before
                    .as_ref()
                    .zip(after.as_ref())
                    .is_some_and(|(left, right)| {
                        left["window"]["window_id"] == right["window"]["window_id"]
                    })
                && tree
                    .as_ref()
                    .is_some_and(|value| find_id(value, "/canvas.main").is_some()),
            json!({"reset":reset,"set":set,"before":before,"after":after,"drag":moved,"tree":tree}),
        );
    }
}
#[cfg(windows)]
fn popup_and_canvas(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    session: &str,
    token: &str,
    canvas: &str,
    context: &str,
    context_delete: &str,
) {
    if title_drag(manager, audit, session, token, "popup.after-move", 30, 18) {
        let click = physical(
            manager,
            audit,
            "popup.right-click",
            session,
            token,
            "right_click_relative",
            json!({"id":canvas,"position":{"x":0.5,"y":0.5}}),
        );
        if let Some(click) = click.as_ref() {
            verify_relative_cleanup(audit, "popup.right-click", click);
        }
        let visible = audit.call(
            manager,
            "popup.wait-visible",
            "wait_visible",
            json!({"session_id":session,"id":context,"timeout_ms":3000}),
        );
        let screenshot = audit.call(
            manager,
            "popup.after-move.screenshot-window",
            "screenshot_window",
            json!({"session_id":session}),
        );
        let dismissed = audit.call(
            manager,
            "popup.dismiss",
            "click",
            json!({"session_id":session,"id":context_delete}),
        );
        audit.assert(
            "popup.after-move.lifecycle",
            click.is_some()
                && visible.is_some()
                && screenshot.is_some()
                && dismissed.is_some(),
            json!({"right_click":click,"visible":visible,"screenshot":screenshot,"dismissed":dismissed}),
        );
    }
    if resize_drag(
        manager,
        audit,
        session,
        token,
        "canvas.after-resize",
        "bottom_right",
        30,
        20,
    ) {
        let before = audit.call(
            manager,
            "canvas.after-resize.inspect-before",
            "inspect_spatial_target",
            json!({"session_id":session,"id":canvas}),
        );
        let drag = physical(
            manager,
            audit,
            "canvas.after-resize.drag-relative",
            session,
            token,
            "drag_relative",
            json!({
                "id":canvas,
                "from":{"x":0.25,"y":0.50},
                "to":{"x":0.75,"y":0.50},
                "steps":6,
                "duration_ms":220
            }),
        );
        if let Some(drag) = drag.as_ref() {
            verify_relative_cleanup(audit, "canvas.after-resize.drag-relative", drag);
        }
        let after = audit.call(
            manager,
            "canvas.after-resize.inspect-after",
            "inspect_spatial_target",
            json!({"session_id":session,"id":canvas}),
        );
        let window_shot = audit.call(
            manager,
            "canvas.after-resize.screenshot-window",
            "screenshot_window",
            json!({"session_id":session}),
        );
        let element_shot = audit.call(
            manager,
            "canvas.after-resize.screenshot-element",
            "screenshot_element",
            json!({"session_id":session,"id":canvas}),
        );
        audit.assert(
            "canvas.after-resize.spatial-readback",
            before
                .as_ref()
                .is_some_and(|value| value["canvas_health"] == "Healthy")
                && drag.is_some()
                && after.as_ref().is_some_and(|value| {
                    value["canvas_health"] == "Healthy" && value["interactable"] == true
                })
                && window_shot.is_some()
                && element_shot.is_some(),
            json!({
                "before":before,
                "drag":drag,
                "after":after,
                "window_screenshot":window_shot,
                "element_screenshot":element_shot
            }),
        );
    }
}

#[cfg(windows)]
fn prepare(manager: &mut ReliabilityManager, audit: &mut Audit, session: &str, name: &str) -> bool {
    let normal = audit.call(
        manager,
        &format!("{name}.setup-normal"),
        "windows_set_window_state",
        json!({"session_id":session,"state":"normal"}),
    );
    let inspect = audit.call(
        manager,
        &format!("{name}.setup-inspect"),
        "inspect_window_interaction",
        json!({"session_id":session}),
    );
    let Some(inspect) = inspect else {
        audit.assert(format!("{name}.setup"), false, json!({"normal":normal}));
        return false;
    };
    let (x, y, width, height) = setup_bounds(&inspect);
    let bounds = audit.call(
        manager,
        &format!("{name}.setup-bounds"),
        "windows_set_window_bounds",
        json!({"session_id":session,"x":x,"y":y,"width":width,"height":height}),
    );
    let ok = normal.is_some()
        && bounds
            .as_ref()
            .is_some_and(|value| value["success"] == true);
    audit.assert(
        format!("{name}.setup"),
        ok,
        json!({"normal":normal,"bounds":bounds,"requested":{"x":x,"y":y,"width":width,"height":height}}),
    );
    ok
}

#[cfg(windows)]
fn physical(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    name: &str,
    session: &str,
    token: &str,
    operation: &str,
    mut arguments: Value,
) -> Option<Value> {
    arguments["session_id"] = json!(session);
    arguments["input_lock_token"] = json!(token);
    audit.call(manager, name, operation, arguments)
}

#[cfg(windows)]
fn verify_gesture(audit: &mut Audit, name: &str, result: &Value) {
    audit.assert(
        format!("{name}.cleanup"),
        result["pointer_cleanup"]["button_released"] == true
            && result["pointer_cleanup"]["modifiers_released"] == true
            && result["pointer_cleanup"]["host_input_armed"] == false,
        json!({"pointer_cleanup":result["pointer_cleanup"]}),
    );
    audit.assert(
        format!("{name}.healthy-layout"),
        result["runtime_health"]["healthy"] == true
            && result["layout"]["invalid_visible_bounds"]
                .as_array()
                .is_some_and(|items| items.is_empty()),
        json!({"runtime":result["runtime_health"],"layout":result["layout"]}),
    );
}

#[cfg(windows)]
fn verify_relative_cleanup(audit: &mut Audit, name: &str, result: &Value) {
    audit.assert(
        format!("{name}.cleanup"),
        result["pointer_cleanup"]["button_released"] == true
            && result["pointer_cleanup"]["modifiers_released"] == true
            && result["pointer_cleanup"]["host_input_armed"] == false,
        json!({"pointer_cleanup":result["pointer_cleanup"]}),
    );
}

#[cfg(windows)]
fn interaction_bounds_changed(before: &Option<Value>, after: &Option<Value>) -> bool {
    before
        .as_ref()
        .zip(after.as_ref())
        .is_some_and(|(before, after)| before["window"]["bounds"] != after["window"]["bounds"])
}
#[cfg(windows)]
fn same_pid_isolation(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    session: &str,
    token: &str,
    primary_window: &str,
) {
    let listed = audit.call(
        manager,
        "isolation.same-pid.list",
        "list_test_windows",
        json!({"session_id":session}),
    );
    let Some(items) = listed.as_ref().and_then(Value::as_array) else {
        audit.assert(
            "isolation.same-pid.identities",
            false,
            json!({"windows":listed}),
        );
        return;
    };
    let candidates = items
        .iter()
        .filter(|window| window["window_id"].is_string() && window["window_handle"].is_string())
        .cloned()
        .collect::<Vec<_>>();
    let handles = candidates
        .iter()
        .filter_map(|window| window["window_handle"].as_str())
        .collect::<BTreeSet<_>>();
    let ids = candidates
        .iter()
        .filter_map(|window| window["window_id"].as_str())
        .collect::<BTreeSet<_>>();
    audit.assert(
        "isolation.same-pid.identities",
        candidates.len() >= 2 && handles.len() == candidates.len() && ids.len() == candidates.len(),
        json!({"windows":candidates}),
    );
    let Some(primary_id) = candidates
        .iter()
        .find(|window| window["window_id"].as_str() == Some(primary_window))
        .and_then(|window| window["window_id"].as_str())
        .map(str::to_owned)
    else {
        audit.assert(
            "isolation.same-pid.primary-present",
            false,
            json!({"primary_window":primary_window,"windows":items}),
        );
        return;
    };
    let Some(secondary_id) = candidates
        .iter()
        .filter_map(|window| window["window_id"].as_str())
        .find(|window_id| *window_id != primary_id)
        .map(str::to_owned)
    else {
        audit.assert(
            "isolation.same-pid.secondary-present",
            false,
            json!({"primary_window":primary_id,"windows":items}),
        );
        return;
    };

    let primary_before = audit.call(
        manager,
        "isolation.same-pid.primary-before",
        "inspect_window_interaction",
        json!({"session_id":session,"window_id":primary_id}),
    );
    let secondary_before = audit.call(
        manager,
        "isolation.same-pid.secondary-before",
        "inspect_window_interaction",
        json!({"session_id":session,"window_id":secondary_id}),
    );
    let primary_drag = physical(
        manager,
        audit,
        "isolation.same-pid.primary-drag",
        session,
        token,
        "windows_drag_title",
        json!({"window_id":primary_id,"delta_x":36,"delta_y":18,"steps":6,"duration_ms":220}),
    );
    if let Some(result) = primary_drag.as_ref() {
        verify_gesture(audit, "isolation.same-pid.primary-drag", result);
    }
    let primary_after = audit.call(
        manager,
        "isolation.same-pid.primary-after",
        "inspect_window_interaction",
        json!({"session_id":session,"window_id":primary_id}),
    );
    let secondary_after = audit.call(
        manager,
        "isolation.same-pid.secondary-after-primary",
        "inspect_window_interaction",
        json!({"session_id":session,"window_id":secondary_id}),
    );
    audit.assert(
        "isolation.same-pid.primary-moves-only-primary",
        primary_drag.is_some()
            && interaction_bounds_changed(&primary_before, &primary_after)
            && !interaction_bounds_changed(&secondary_before, &secondary_after),
        json!({
            "primary_before":primary_before,
            "primary_after":primary_after,
            "secondary_before":secondary_before,
            "secondary_after":secondary_after,
            "drag":primary_drag
        }),
    );

    let secondary_drag = physical(
        manager,
        audit,
        "isolation.same-pid.secondary-drag",
        session,
        token,
        "windows_drag_title",
        json!({"window_id":secondary_id,"delta_x":-36,"delta_y":18,"steps":6,"duration_ms":220}),
    );
    if let Some(result) = secondary_drag.as_ref() {
        verify_gesture(audit, "isolation.same-pid.secondary-drag", result);
    }
    let primary_after_secondary = audit.call(
        manager,
        "isolation.same-pid.primary-after-secondary",
        "inspect_window_interaction",
        json!({"session_id":session,"window_id":primary_id}),
    );
    let secondary_after_secondary = audit.call(
        manager,
        "isolation.same-pid.secondary-after",
        "inspect_window_interaction",
        json!({"session_id":session,"window_id":secondary_id}),
    );
    audit.assert(
        "isolation.same-pid.secondary-moves-only-secondary",
        secondary_drag.is_some()
            && !interaction_bounds_changed(&primary_after, &primary_after_secondary)
            && interaction_bounds_changed(&secondary_after, &secondary_after_secondary),
        json!({
            "primary_before":primary_after,
            "primary_after":primary_after_secondary,
            "secondary_before":secondary_after,
            "secondary_after":secondary_after_secondary,
            "drag":secondary_drag
        }),
    );
    let _ = audit.call(
        manager,
        "isolation.same-pid.restore-primary-selection",
        "inspect_window_interaction",
        json!({"session_id":session,"window_id":primary_id}),
    );
}

#[cfg(windows)]
fn cross_process_isolation(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    primary_session: &str,
    token: &str,
    primary_process: u32,
    primary_window: &str,
) {
    let second = audit.call(
        manager,
        "isolation.cross-process.open",
        "session.open",
        json!({}),
    );
    let Some(second) = second else {
        audit.assert("isolation.cross-process", false, json!({}));
        return;
    };
    let second_session = second["session_id"].as_str().map(str::to_owned);
    let second_process = second["processes"]
        .as_array()
        .and_then(|processes| processes.first())
        .and_then(|process| process["process_id"].as_u64());
    audit.assert(
        "isolation.cross-process.pid",
        second_process.is_some_and(|process| process != primary_process as u64),
        json!({"primary_session":primary_session,"primary_process":primary_process,"second":second}),
    );
    let Some(second_session) = second_session else {
        audit.assert(
            "isolation.cross-process.session",
            false,
            json!({"second":second}),
        );
        return;
    };
    let secondary_before = audit.call(
        manager,
        "isolation.cross-process.secondary-before",
        "inspect_window_interaction",
        json!({"session_id":second_session}),
    );
    let primary_before = audit.call(
        manager,
        "isolation.cross-process.primary-before",
        "inspect_window_interaction",
        json!({"session_id":primary_session,"window_id":primary_window}),
    );
    let primary_drag = physical(
        manager,
        audit,
        "isolation.cross-process.primary-drag",
        primary_session,
        token,
        "windows_drag_title",
        json!({"window_id":primary_window,"delta_x":30,"delta_y":16,"steps":6,"duration_ms":220}),
    );
    if let Some(result) = primary_drag.as_ref() {
        verify_gesture(audit, "isolation.cross-process.primary-drag", result);
    }
    let primary_after = audit.call(
        manager,
        "isolation.cross-process.primary-after",
        "inspect_window_interaction",
        json!({"session_id":primary_session,"window_id":primary_window}),
    );
    let secondary_after = audit.call(
        manager,
        "isolation.cross-process.secondary-after",
        "inspect_window_interaction",
        json!({"session_id":second_session}),
    );
    let closed = audit.call(
        manager,
        "isolation.cross-process.close",
        "session.close",
        json!({"session_id":second_session}),
    );
    audit.assert(
        "isolation.cross-process.primary-moves-only-primary",
        primary_drag.is_some()
            && interaction_bounds_changed(&primary_before, &primary_after)
            && !interaction_bounds_changed(&secondary_before, &secondary_after)
            && closed.is_some(),
        json!({
            "primary_before":primary_before,
            "primary_after":primary_after,
            "secondary_before":secondary_before,
            "secondary_after":secondary_after,
            "drag":primary_drag,
            "close":closed
        }),
    );
}

#[cfg(windows)]
fn fixed_size_refusal(audit: &mut Audit, fixture: &Path) {
    if !fixture.is_file() {
        audit.assert("fixed-size.fixture-present", false, json!({"path":fixture}));
        return;
    }
    let title = format!("AI UI fixed-size {}", std::process::id());
    let mut child = match Command::new(fixture)
        .args(["--fixed-size", "--title", &title])
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            audit.assert(
                "fixed-size.launch",
                false,
                json!({"error":error.to_string()}),
            );
            return;
        }
    };
    let result = (|| {
        let mut automation = WindowsAutomation::new().map_err(|error| error.to_string())?;
        let mut found = None;
        for _ in 0..40 {
            if let Ok(window) = automation.find_window(&WindowQuery {
                process_name: None,
                process_id: Some(child.id()),
                title: Some(title.clone()),
                title_contains: None,
            }) {
                found = Some(window);
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let window = found.ok_or("fixed-size fixture did not open")?;
        let refusal = automation
            .resolve_window_frame_target(&window.window_id, WindowFrameTargetKind::Right)
            .err()
            .is_some_and(|error| error.code == ErrorCode::WindowNotResizable);
        Ok::<_, String>(json!({"window":window,"typed_refusal":refusal}))
    })();
    let _ = child.kill();
    let _ = child.wait();
    match result {
        Ok(evidence) => audit.assert(
            "fixed-size.typed-refusal",
            evidence["typed_refusal"] == true,
            evidence,
        ),
        Err(error) => audit.assert("fixed-size.typed-refusal", false, json!({"error":error})),
    }
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug)]
struct DesktopRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[cfg(windows)]
impl DesktopRect {
    fn from_win32(rect: RECT) -> Self {
        Self {
            x: rect.left,
            y: rect.top,
            width: rect.right.saturating_sub(rect.left),
            height: rect.bottom.saturating_sub(rect.top),
        }
    }

    fn from_json(value: &Value) -> Option<Self> {
        let rect = Self {
            x: value["x"].as_i64()? as i32,
            y: value["y"].as_i64()? as i32,
            width: value["width"].as_i64()? as i32,
            height: value["height"].as_i64()? as i32,
        };
        (rect.width > 0 && rect.height > 0).then_some(rect)
    }

    fn centered_position(self, width: i32, height: i32) -> (i32, i32) {
        let free_width = self.width.saturating_sub(width).max(0);
        let free_height = self.height.saturating_sub(height).max(0);
        (
            self.x.saturating_add(free_width / 2),
            self.y.saturating_add(free_height / 2),
        )
    }

    fn as_json(self) -> Value {
        json!({"x":self.x,"y":self.y,"width":self.width,"height":self.height})
    }
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct DesktopMonitor {
    handle: String,
    bounds: DesktopRect,
    work_area: DesktopRect,
    primary: bool,
}

#[cfg(windows)]
impl DesktopMonitor {
    fn as_json(&self) -> Value {
        json!({
            "handle":self.handle,
            "bounds":self.bounds.as_json(),
            "work_area":self.work_area.as_json(),
            "primary":self.primary,
        })
    }
}

#[cfg(windows)]
unsafe extern "system" fn collect_desktop_monitor(
    monitor: HMONITOR,
    _dc: HDC,
    _clip: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let monitors = unsafe { &mut *(data.0 as *mut Vec<DesktopMonitor>) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info as *mut MONITORINFO).as_bool() } {
        return BOOL(0);
    }
    monitors.push(DesktopMonitor {
        handle: format!("{monitor:?}"),
        bounds: DesktopRect::from_win32(info.rcMonitor),
        work_area: DesktopRect::from_win32(info.rcWork),
        primary: info.dwFlags & 1 != 0,
    });
    TRUE
}

#[cfg(windows)]
fn desktop_monitors() -> Result<Vec<DesktopMonitor>, String> {
    let mut monitors = Vec::new();
    let data = LPARAM((&mut monitors as *mut Vec<DesktopMonitor>) as isize);
    if !unsafe { EnumDisplayMonitors(None, None, Some(collect_desktop_monitor), data).as_bool() } {
        return Err("EnumDisplayMonitors/GetMonitorInfoW failed".to_owned());
    }
    if monitors.is_empty() {
        return Err("EnumDisplayMonitors returned no desktop monitors".to_owned());
    }
    Ok(monitors)
}

#[cfg(windows)]
fn migration_dimension(current: i32, shared_work_area: i32, floor: i32, ceiling: i32) -> i32 {
    let available = shared_work_area.saturating_sub(96).max(1);
    let lower = floor.min(available);
    let upper = available.min(ceiling).max(lower);
    current.clamp(lower, upper)
}

#[cfg(windows)]
fn monitor_metadata(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    session: &str,
    token: &str,
    initial: &Value,
    pan: &str,
) {
    let monitor_count = unsafe { GetSystemMetrics(SM_CMONITORS) };
    let enumerated = desktop_monitors();
    let enumeration = match &enumerated {
        Ok(items) => {
            json!({"monitors":items.iter().map(DesktopMonitor::as_json).collect::<Vec<_>>() })
        }
        Err(error) => json!({"error":error}),
    };
    let enumeration_matches =
        matches!(&enumerated, Ok(items) if items.len() as i32 == monitor_count);
    audit.assert(
        "monitor.metadata",
        initial["dpi"].as_u64().unwrap_or(0) > 0
            && initial["monitor"]["work_area"]["width"]
                .as_i64()
                .unwrap_or(0)
                > 0
            && enumeration_matches,
        json!({
            "monitor_count":monitor_count,
            "initial_monitor":initial["monitor"],
            "dpi":initial["dpi"],
            "enumeration":enumeration,
        }),
    );
    if monitor_count > 1 {
        match enumerated {
            Ok(monitors) => cross_monitor_migration(manager, audit, session, token, monitors, pan),
            Err(error) => audit.assert(
                "monitor.cross-migration",
                false,
                json!({"monitor_count":monitor_count,"error":error}),
            ),
        }
    } else {
        audit.assert(
            "monitor.NOT_TESTED_NO_SECOND_MONITOR",
            true,
            json!({"monitor_count": monitor_count}),
        );
        audit
            .warnings
            .push("Cross-monitor coverage is unavailable on this one-monitor desktop.".to_owned());
    }
}

#[cfg(windows)]
fn cross_monitor_migration(
    manager: &mut ReliabilityManager,
    audit: &mut Audit,
    session: &str,
    token: &str,
    monitors: Vec<DesktopMonitor>,
    pan: &str,
) {
    let normal = audit.call(
        manager,
        "monitor.cross-migration.setup-normal",
        "windows_set_window_state",
        json!({"session_id":session,"state":"normal"}),
    );
    let current = audit.call(
        manager,
        "monitor.cross-migration.source-inspect",
        "inspect_window_interaction",
        json!({"session_id":session}),
    );
    let Some(current) = current else {
        audit.assert(
            "monitor.cross-migration.setup",
            false,
            json!({"normal":normal}),
        );
        return;
    };
    let Some(source_handle) = current["monitor"]["handle"].as_str() else {
        audit.assert(
            "monitor.cross-migration.setup",
            false,
            json!({"current":current}),
        );
        return;
    };
    let Some(source) = monitors
        .iter()
        .find(|monitor| monitor.handle == source_handle)
    else {
        audit.assert(
            "monitor.cross-migration.setup",
            false,
            json!({"current":current,"enumerated":monitors.iter().map(DesktopMonitor::as_json).collect::<Vec<_>>() }),
        );
        return;
    };
    let Some(target) = monitors
        .iter()
        .find(|monitor| monitor.handle != source.handle)
    else {
        audit.assert(
            "monitor.cross-migration.setup",
            false,
            json!({"source":source.as_json()}),
        );
        return;
    };
    let Some(current_bounds) = DesktopRect::from_json(&current["window"]["bounds"]) else {
        audit.assert(
            "monitor.cross-migration.setup",
            false,
            json!({"current":current}),
        );
        return;
    };
    let width = migration_dimension(
        current_bounds.width,
        source.work_area.width.min(target.work_area.width),
        320,
        960,
    );
    let height = migration_dimension(
        current_bounds.height,
        source.work_area.height.min(target.work_area.height),
        240,
        700,
    );
    let (source_x, source_y) = source.work_area.centered_position(width, height);
    let setup = audit.call(
        manager,
        "monitor.cross-migration.setup-bounds",
        "windows_set_window_bounds",
        json!({"session_id":session,"x":source_x,"y":source_y,"width":width,"height":height}),
    );
    let source_ready = audit.call(
        manager,
        "monitor.cross-migration.source-ready",
        "inspect_window_interaction",
        json!({"session_id":session}),
    );
    let setup_ok = normal.is_some()
        && setup.is_some()
        && source_ready.as_ref().is_some_and(|value| {
            value["state"] == "normal"
                && value["monitor"]["handle"].as_str() == Some(source.handle.as_str())
                && DesktopRect::from_json(&value["window"]["bounds"]).is_some()
        });
    audit.assert(
        "monitor.cross-migration.setup",
        setup_ok,
        json!({"normal":normal,"setup":setup,"source":source.as_json(),"target":target.as_json(),"source_ready":source_ready}),
    );
    if !setup_ok {
        return;
    }
    let source_ready = source_ready.expect("setup checked source_ready");
    let source_before = DesktopRect::from_json(&source_ready["window"]["bounds"])
        .expect("setup checked positive source bounds");
    let (target_x, target_y) = target
        .work_area
        .centered_position(source_before.width, source_before.height);
    let outbound = physical(
        manager,
        audit,
        "monitor.cross-migration.outbound-title-drag",
        session,
        token,
        "windows_drag_title",
        json!({
            "delta_x":target_x.saturating_sub(source_before.x),
            "delta_y":target_y.saturating_sub(source_before.y),
            "steps":32,
            "duration_ms":900,
        }),
    );
    if let Some(outbound) = outbound.as_ref() {
        verify_gesture(
            audit,
            "monitor.cross-migration.outbound-title-drag",
            outbound,
        );
    }
    let destination = audit.call(
        manager,
        "monitor.cross-migration.destination-inspect",
        "inspect_window_interaction",
        json!({"session_id":session}),
    );
    let destination_ok = outbound.is_some()
        && destination.as_ref().is_some_and(|value| {
            value["monitor"]["handle"].as_str() == Some(target.handle.as_str())
                && value["dpi"].as_u64().unwrap_or(0) > 0
                && value["scale_factor"].as_f64().unwrap_or(0.0) > 0.0
                && DesktopRect::from_json(&value["window"]["bounds"]).is_some()
        });
    if destination_ok {
        semantic_click(
            manager,
            audit,
            session,
            token,
            "monitor.cross-migration.semantic",
            pan,
        );
    } else {
        audit.assert(
            "monitor.cross-migration.semantic.post-window-hit-test",
            false,
            json!({"destination":destination,"pan":pan}),
        );
    }
    let screenshot = audit.call(
        manager,
        "monitor.cross-migration.screenshot-window",
        "screenshot_window",
        json!({"session_id":session}),
    );
    let screenshot_ok =
        screenshot
            .as_ref()
            .zip(destination.as_ref())
            .is_some_and(|(capture, destination)| {
                capture["capture"]["window_id"] == destination["window"]["window_id"]
                    && capture["image"]["blank"] == false
            });
    audit.assert(
        "monitor.cross-migration.destination",
        destination_ok && screenshot_ok,
        json!({
            "source":source.as_json(),
            "target":target.as_json(),
            "outbound":outbound,
            "destination":destination,
            "screenshot":screenshot,
            "screenshot_ok":screenshot_ok,
        }),
    );

    let destination_bounds = destination
        .as_ref()
        .and_then(|value| DesktopRect::from_json(&value["window"]["bounds"]));
    let return_needed = destination_bounds
        .is_some_and(|bounds| bounds.x != source_before.x || bounds.y != source_before.y);
    let returned_by_drag = if let Some(bounds) = destination_bounds {
        if return_needed {
            physical(
                manager,
                audit,
                "monitor.cross-migration.return-title-drag",
                session,
                token,
                "windows_drag_title",
                json!({
                    "delta_x":source_before.x.saturating_sub(bounds.x),
                    "delta_y":source_before.y.saturating_sub(bounds.y),
                    "steps":32,
                    "duration_ms":900,
                }),
            )
        } else {
            Some(json!({"not_needed":true}))
        }
    } else {
        None
    };
    if return_needed {
        if let Some(returned_by_drag) = returned_by_drag.as_ref() {
            verify_gesture(
                audit,
                "monitor.cross-migration.return-title-drag",
                returned_by_drag,
            );
        }
    }
    let returned = audit.call(
        manager,
        "monitor.cross-migration.returned-inspect",
        "inspect_window_interaction",
        json!({"session_id":session}),
    );
    let return_ok = returned.as_ref().is_some_and(|value| {
        value["monitor"]["handle"].as_str() == Some(source.handle.as_str())
            && value["state"] == "normal"
            && DesktopRect::from_json(&value["window"]["bounds"]).is_some()
    }) && (!return_needed || returned_by_drag.is_some());
    audit.assert(
        "monitor.cross-migration.returned",
        return_ok,
        json!({
            "source":source.as_json(),
            "return_needed":return_needed,
            "return_drag":returned_by_drag,
            "returned":returned,
        }),
    );
}

#[cfg(windows)]
fn setup_bounds(info: &Value) -> (i32, i32, i32, i32) {
    let work = &info["monitor"]["work_area"];
    let work_x = work["x"].as_i64().unwrap_or(0) as i32;
    let work_y = work["y"].as_i64().unwrap_or(0) as i32;
    let work_width = work["width"].as_i64().unwrap_or(1920) as i32;
    let work_height = work["height"].as_i64().unwrap_or(1080) as i32;
    let width = (work_width - 160).clamp(760, 1000);
    let height = (work_height - 160).clamp(540, 700);
    (
        work_x + ((work_width - width) / 2).max(16),
        work_y + ((work_height - height) / 2).max(16),
        width,
        height,
    )
}

#[cfg(windows)]
fn unpack(response: &Value) -> Result<Value, String> {
    if response["success"] != true {
        return Err(response["error"].to_string());
    }
    let result = response["result"].clone();
    if result["dispatched"] == true && result.get("result").is_some() {
        let inner = result["result"].clone();
        if inner["code"].is_string() {
            Err(format!("{}: {}", inner["code"], inner["message"]))
        } else {
            Ok(inner)
        }
    } else {
        Ok(result)
    }
}

#[cfg(windows)]
fn changed(before: &Value, after: &Value) -> bool {
    before["x"] != after["x"]
        || before["y"] != after["y"]
        || before["width"] != after["width"]
        || before["height"] != after["height"]
}

#[cfg(windows)]
fn position_changed(before: &Value, after: &Value) -> bool {
    before["x"] != after["x"] || before["y"] != after["y"]
}
#[cfg(windows)]
fn find_id(tree: &Value, suffix: &str) -> Option<String> {
    tree["elements"]
        .as_array()?
        .iter()
        .filter_map(|element| element["automation_id"].as_str())
        .find(|id| id.ends_with(suffix))
        .map(str::to_owned)
}

#[cfg(windows)]
fn find_text(tree: &Value, suffix: &str) -> Option<String> {
    tree["elements"]
        .as_array()?
        .iter()
        .filter_map(|element| {
            let id = element["automation_id"].as_str()?;
            let text = element["text"]
                .as_str()
                .or_else(|| element["value"].as_str())?;
            Some((id, text))
        })
        .find(|(id, _)| id.ends_with(suffix))
        .map(|(_, text)| text.to_owned())
}

#[cfg(windows)]
fn argument_path(name: &str) -> Option<PathBuf> {
    let mut arguments = std::env::args_os();
    while let Some(argument) = arguments.next() {
        if argument == name {
            return arguments.next().map(PathBuf::from);
        }
    }
    None
}
