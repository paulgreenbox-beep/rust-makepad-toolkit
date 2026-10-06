#![cfg(windows)]

use serde_json::Value;
use std::process::Command;
use toolkit_windows_automation::now_unix_ms;

#[test]
#[ignore = "requires an interactive Windows desktop session"]
fn fixture_proves_id_addressed_gui_automation_chain() {
    let runner = env!("CARGO_BIN_EXE_windows-gui-self-test");
    let fixture = env!("CARGO_BIN_EXE_windows-gui-self-test-fixture");
    let output_dir = std::env::temp_dir()
        .join("rust-makepad-toolkit")
        .join(format!("windows-gui-integration-{}", now_unix_ms()));
    let output = Command::new(runner)
        .arg("--fixture")
        .arg(fixture)
        .arg("--output-dir")
        .arg(&output_dir)
        .output()
        .expect("self-test runner should start");
    assert!(
        output.status.success(),
        "self-test failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: Value =
        serde_json::from_slice(&output.stdout).expect("runner stdout should be one JSON summary");
    assert_eq!(summary["status"], "WINDOWS_GUI_AUTOMATION_SELF_TEST_PASSED");
    assert!(output_dir.join("self-test-large.png").is_file());
    assert!(output_dir.join("self-test-medium.png").is_file());
    assert!(output_dir.join("self-test-narrow.png").is_file());
    assert!(output_dir.join("control-tree.json").is_file());
    assert!(output_dir.join("evidence.json").is_file());
}
