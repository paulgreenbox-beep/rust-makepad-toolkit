# AI UI Test Tool — Agent Guide

Status: CURRENT  
Updated: 2026-08-24  
Contract/API version: AI UI Test Tool Script API v1

Use this guide when operating or testing a Makepad application through the
current Tool. It is intentionally shorter than the subsystem documents; begin
with [docs/INDEX.md](docs/INDEX.md) for links.

## Contract boundary

For .aiui.json, generated tests and Scheduler jobs, the only operation
vocabulary is prototypes/ai-ui-test-tool-auto-probe/operations.json.
ai-ui-test-script.schema.json describes JSON shape and src/script.rs performs
additional semantic validation.

ReliabilityManager and AdvancedHost expose additional direct-host aliases for
compatibility/debugging. Do not put those aliases in a .aiui script and do not
use them to bypass ai-ui-test run or the Scheduler.

## Decision order

1. Inspect the semantic tree and address an element by stable AutomationId.
2. Use a semantic operation (read, set_value, click, select, toggle, choose,
   ensure_visible) when it is supported.
3. Use a physical/spatial operation only when semantic interaction is not the
   right abstraction and the run is on Local Windows.
4. Wait for observable state, assert it, check runtime health, and export
   evidence. A dispatch response alone is never a UI PASS.


### Popup interaction priority

For a popup, modal, dropdown, or context menu that is already discoverable in
the session-owned semantic tree, use this order:

1. resolve the popup root/item through the session/window and stable
   AutomationId;
2. invoke the item semantically with click/InvokePattern;
3. use semantic focus plus keyboard activation only when the target exposes no
   usable invoke action;
4. use a physical item click only in an explicit physical-selection UX test.

After a physical right_click, wait for the popup root and item, refresh the
semantic tree, assert visible/interactable state, and invoke the item. Do not
click the popup background, move the pointer to manufacture focus, call
activate_window after the popup appears, or emit another mouse down/up when
the semantic target is available. The right-button cleanup still releases the
button and input lock, but must not add an outside-pointer event.

Popup children may be windowless. The Windows automation backend inherits the
top-level owner HWND for HWND-scoped resolution; a missing child HWND is not a
reason to fall back to a mouse click.
Do not start with guessed screen coordinates. A screenshot is evidence next to
a semantic assertion, not an identity mechanism.

## Action versus observation
An ACTION changes or queries the UI interaction surface: click, input, select,
drag, scroll, hotkey and related semantic/spatial operations. An OBSERVATION
proves what happened after the action: task state, runtime milestone, trusted
API state, file postcondition, visual result, log/trace event, cleanup and
runtime health.

Recommended asynchronous workflow:

~~~text
action
  -> wait_for_task_complete / assert_task_success
  -> wait_for_runtime_milestone when the app publishes milestone telemetry
  -> wait_for_api_response + wait_for_api_state(Completed)
  -> assert_api_success
  -> wait_for_file_stable
  -> wait_visible or another semantic postcondition
  -> assert_region_changed / compare_screenshot when useful
  -> get_runtime_health
  -> windows_export_evidence
~~~

API observation uses trusted telemetry or a configured observer alias; it is
not an arbitrary HTTP client. HTTP 200 or response_received alone does not
mean application_accepted_response or PASS.

## Stable Script API groups

| Intent | Version-1 operations |
| --- | --- |
| Session | session.open, session.list, session.close |
| Inspect | inspect_ui, inspect_semantic, inspect_spatial_target, read |
| Semantic interaction | set_value, input, click, double_click, right_click, toggle, choose, selection operations, scroll, ensure_visible |
| Wait/assert | wait_for_automation_id, wait_for_value, wait_for_state, wait_until, wait_visible, wait_hidden, wait_enabled, wait_disabled, wait_selected, wait_count, wait_idle, assert_* |
| Spatial/physical (Windows only) | move_pointer, pointer_down, pointer_up, click_relative, double_click_relative, right_click_relative, hover_relative, wheel_relative, drag, drag_to, drag_relative, select_region, hotkey, press, press_repeat, type_text |
| Evidence | screenshot_window, screenshot_element, capture_region, windows_capture_region, get_runtime_health, get_runtime_stderr, windows_export_evidence, windows_validate_evidence |

operations.json declares required fields, capability tags and whether an
operation uses physical input. Do not invent aliases or omit required fields.

## Minimal .aiui flow

~~~json
{
  "version": 1,
  "name": "my-semantic-smoke",
  "application": {"command": "target/debug/my-app.exe"},
  "options": {"stop_on_error": true, "evidence_on_failure": true},
  "steps": [
    {"op": "session.open", "save_session_as": "app"},
    {"op": "inspect_ui", "session": "app", "save_as": "tree"},
    {"op": "wait_for_automation_id", "session": "app", "id": "login.submit"},
    {"op": "click", "session": "app", "id": "login.submit"},
    {"op": "get_runtime_health", "session": "app"},
    {"op": "windows_export_evidence", "session": "app"}
  ],
  "teardown": [{"op": "session.close", "session": "app"}]
}
~~~

Validate before running:

~~~powershell
cargo run --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-test -- validate path/to/test.aiui.json
cargo run --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-test -- run path/to/test.aiui.json
~~~

## Spatial and physical interaction

For top-level window UX, call `inspect_window_interaction` immediately before a
title drag, edge/corner resize, or title double-click. Use
`windows_set_window_bounds` and `windows_set_window_state` only for
programmatic setup/recovery; they are not proof of physical window behavior.
The physical window operations use the same desktop input lease as other
pointer actions and must include their fresh postcondition/evidence.
`press_repeat` is for navigation keys only: it accepts one named key and a
positive bounded `count`, then injects one foreground-verified Windows input
batch. It must not be used to enter text; use `type_text` for literal content.
Physical operations must target a known semantic element plus relative
coordinates, never an unverified absolute coordinate. Typical canvas flow:

~~~text
inspect_spatial_target -> click_relative / drag_relative / wheel_relative
-> wait/assert -> capture_region -> export evidence
~~~

On direct Windows host use the input lock, foreground verification and release
flow. The Scheduler analyzes catalog operations and serializes the shared
desktop resource. Container Level 0 always rejects these GUI/physical paths.
Container Level 0 has no Makepad GUI, Windows UIA, physical input, screenshots
or independent desktop.

## Generator, Scheduler and evidence

- Generator and coverage consume public inventory/results only; use
  [GENERATOR_QUICKSTART.md](prototypes/ai-ui-test-tool-auto-probe/GENERATOR_QUICKSTART.md).
- Scheduler accepts scripts, suites and generated directories only through the
  public runner; use [scheduler/README.md](prototypes/ai-ui-test-tool-auto-probe/scheduler/README.md).
- ContainerWorker is run-only Level 0. Read [LIMITATIONS.md](LIMITATIONS.md)
  before choosing it.
- A real PASS requires correct session/window, discovered controls, verified
  actions/postconditions, healthy runtime and saved evidence. See
  [RELIABILITY.md](prototypes/ai-ui-test-tool-auto-probe/RELIABILITY.md).


## Trusted observations and bounded async workflows

The public catalog includes non-physical observation operations for runtime
tasks/milestones/timeline, external-request state, trusted files, visual
assertions, application logs, trace events, and cleanup checks. Use them after
action dispatch to prove an outcome; a transport response alone is not an
application-success proof.

Observation reads are constrained to a resolved Makepad telemetry AutomationId,
fixed observer aliases, named trusted roots, and session-owned captures. Public
scripts cannot evaluate code, call arbitrary endpoints, or use the backend-only
screenshot alias. extract, when, and repeat_until are bounded declarative
controls, not executable workflow hooks.

There is no public session.attach or session.resume capability. Recover from a lost run
with a fresh session.open, inspect/rediscovery, and a new input lease. See
[Trusted Observation & Async Workflows](prototypes/ai-ui-test-tool-auto-probe/OBSERVATION_WORKFLOWS.md),
[Backend API Classification](BACKEND_API_CLASSIFICATION.md), and
[Session Attach / Resume Feasibility](SESSION_ATTACH_RESUME_FEASIBILITY.md).
