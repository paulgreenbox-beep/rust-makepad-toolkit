# AI UI Test Tool Quickstart

Status: CURRENT  
Updated: 2026-08-22  
Contract/API version: AI UI Test Tool Script API v1

This is the five-minute entry point. For operation fields, always consult
prototypes/ai-ui-test-tool-auto-probe/operations.json rather than copying a
legacy direct-host alias.

## 1. Build and validate an included UI test

From the repository root on Windows:

~~~powershell
cargo build --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-test --bin spatial-demo
cargo run --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-test -- validate prototypes/ai-ui-test-tool-auto-probe/examples/spatial-smoke.aiui.json
cargo run --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-test -- run prototypes/ai-ui-test-tool-auto-probe/examples/spatial-smoke.aiui.json
~~~

The runner writes report.json, ai-report.json and evidence below the selected
report location. Generated evidence stays local; release verification is in
[the publication audit](docs/PUBLICATION_AUDIT.md).

## 2. Write .aiui.json

Use the schema and a semantic session flow:

~~~json
{
  "version": 1,
  "name": "smoke",
  "application": {"command": "target/debug/my-app.exe"},
  "options": {"stop_on_error": true, "evidence_on_failure": true},
  "steps": [
    {"op": "session.open", "save_session_as": "app"},
    {"op": "wait_for_automation_id", "session": "app", "id": "login.submit"},
    {"op": "click", "session": "app", "id": "login.submit"},
    {"op": "assert_visible", "session": "app", "id": "result.message"},
    {"op": "windows_export_evidence", "session": "app"}
  ],
  "teardown": [{"op": "session.close", "session": "app"}]
}
~~~

Use IDs, not guessed coordinates. The current .aiui operation list includes
semantic actions and real Windows physical/spatial actions such as
click_relative, wheel_relative, drag_relative, select_region, hotkey and
press. Physical input is desktop-global; it is unavailable in Container Level
0. See the [Agent Guide](AI_UI_TEST_TOOL_AGENT_GUIDE.md).

## Observation workflow
After an action that starts background work, prove the outcome in layers:

~~~text
action
  -> wait_for_task_complete / assert_task_success
  -> wait_for_api_response + wait_for_api_state(Completed)
  -> assert_api_success
  -> wait_for_file_stable
  -> verify the semantic UI result
  -> visual assertion when useful
  -> get_runtime_health
  -> windows_export_evidence
~~~

The canonical verified script is
[async-observation-success.aiui.json](prototypes/ai-ui-test-tool-auto-probe/examples/async-observation-success.aiui.json).
wait_for_file proves existence only; use wait_for_file_stable for a quiet write
window. Prefer semantic assertions before visual assertions; OCR is not a
default replacement for the semantic layer. Trusted API observation is not
arbitrary HTTP, and HTTP 200 alone is not PASS. The complete vocabulary is in
[OPERATION_REFERENCE.md](OPERATION_REFERENCE.md).

## 3. Generate tests

~~~powershell
cargo build --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-advanced-demo --bin ai-ui-test
cargo run --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-test -- generate prototypes/ai-ui-test-tool-auto-probe/target/debug/ai-ui-advanced-demo.exe --window-title "AI UI Advanced Makepad Probe" --mode standard --output prototypes/ai-ui-test-tool-auto-probe/reports/local/generated --plan-only
~~~

Remove --plan-only only after reviewing safety classifications and fixtures.
The complete flow is in [GENERATOR_QUICKSTART.md](prototypes/ai-ui-test-tool-auto-probe/GENERATOR_QUICKSTART.md).

## 4. Run the Scheduler

~~~powershell
Push-Location prototypes/ai-ui-test-tool-auto-probe
cargo build --locked --bin ai-ui-test --bin spatial-demo
cargo build --locked --manifest-path scheduler/Cargo.toml --bin ai-ui-scheduler
.\target\debug\ai-ui-scheduler.exe run scheduler/examples/windows-e2e/pass-spatial.aiui.json --applications scheduler/examples/windows-e2e/trusted-apps.json --workers 1 --working-dir . --output reports/scheduler/local/quickstart
Pop-Location
~~~

LocalWorker runs on the Windows desktop. ContainerWorker runs the same public
.aiui command inside a disposable Docker/Linux Container Level 0 environment,
but it has no Makepad GUI, UIA, physical input or independent desktop. Use
[CAPABILITIES.md](CAPABILITIES.md) and [LIMITATIONS.md](LIMITATIONS.md) before
selecting it.
