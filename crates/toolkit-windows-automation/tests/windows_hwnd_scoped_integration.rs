#![cfg(windows)]

use toolkit_windows_automation::{WindowQuery, WindowsAutomation};

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

#[test]
#[ignore = "requires a logged-in Forge Makepad window on an interactive Windows desktop"]
fn forge_admin_shell_invocation_uses_the_top_level_owner_hwnd() {
    let process_name = env_or("FORGE_UIA_PROCESS_NAME", "forge-workspace-ui.exe");
    let title_contains = std::env::var("FORGE_UIA_TITLE_CONTAINS").ok();
    let automation_id = env_or("FORGE_UIA_AUTOMATION_ID", "forge.admin.shell");
    let mut automation = WindowsAutomation::new().expect("UIA COM should initialize");
    let window = automation
        .find_window(&WindowQuery {
            process_name: Some(process_name),
            title_contains,
            ..Default::default()
        })
        .expect("Forge top-level window should be discoverable");
    assert!(
        !window.hwnd.is_empty(),
        "top-level window must expose an HWND"
    );

    let control = automation
        .find_automation_id(&window.window_id, &automation_id)
        .expect("target AutomationId should be discoverable");
    let state = automation
        .get_automation_id_state(&window.window_id, &automation_id)
        .expect("target state should be readable before action");

    let result = automation
        .click_automation_id(&window.window_id, &automation_id)
        .expect("target should be activated through the window-scoped resolver");
    assert!(result.success);
    assert_ne!(
        result.action_method, "SynthesizedPointerFromElementBounds",
        "coordinate/mouse fallback must not be used"
    );

    println!(
        "{}",
        serde_json::json!({
            "window_id": window.window_id,
            "owner_hwnd": window.hwnd,
            "automation_id": automation_id,
            "element_id": control.element_id,
            "child_native_hwnd": control.hwnd,
            "child_hwnd_inherited": control.hwnd.is_none(),
            "before_state": state,
            "action_method": result.action_method,
            "after_state": result.after_state,
            "after_error": result.after_error,
        })
    );
}

#[cfg(not(windows))]
#[test]
fn forge_admin_shell_invocation_uses_the_top_level_owner_hwnd() {}
