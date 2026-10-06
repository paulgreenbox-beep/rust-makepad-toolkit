# Reliability and critical-feature contract

Status: CURRENT
Updated: 2026-08-22
Contract/API version: Reliability / Evidence v1

This document describes the verified Windows behavior of the automatic
Makepad probe. The implementation is in `src/reliability.rs`; the executable
proof is `src/bin/reliability-runner.rs`.

## Success invariant

An action is successful only when all applicable checks pass:

1. the requested action was dispatched to the resolved semantic target;
2. the target state was read back and verified;
3. an explicit postcondition, when supplied, was observed before timeout;
4. the application process was still healthy after the settle interval.

The successful `set_value` and `click` responses expose
`dispatched`, `target_state_verified`, `postcondition_verified`, and
`application_healthy`. Dispatch without an observed outcome is not reported
as success. A crash or recognized Makepad/Script runtime error is returned as
`APPLICATION_RUNTIME_FAILURE`, even when the native action call itself had
already returned successfully.

## Session and window identity

`ReliabilityManager` owns explicit `AutomationSession` objects. A session
contains its `session_id`, process identities, registered top-level windows,
active window, action log, and evidence references. A window identity carries
the owning `session_id`, `process_id`, native HWND, owner-scoped `window_id`,
root automation ID/type/name, bounds, and visibility.

Top-level windows are refreshed from Windows by process ID. New windows are
registered and stale/closed HWNDs disappear from the session view. The
reusable Windows primitive is `WindowsAutomation::list_windows`; stable
backend IDs are retained while an HWND remains valid.

Window queries can use `session_id`, `process_id`, `automation_id`, `title`,
`name`, `control_type`, and `root_type`. A query matching more than one window
returns `AMBIGUOUS_WINDOW`. Passing a window ID owned by another session
returns `WINDOW_SESSION_MISMATCH`; it is never silently redirected.

Identity operations:

- `session.open`, `session.list`, `session.resolve_window`, `session.close`;
- `windows_find_window`, `list_test_windows`, `wait_for_window`;
- `get_active_window`, `activate_window`, `ensure_active_window`.

## Multi-window input isolation

UIA/semantic actions are addressed through the session and can run when the
target is not foreground. Pointer and keyboard fallbacks are desktop-global,
so they require an explicit process-wide lease:

- `acquire_input_lock` returns a token owned by one session;
- `activate_window`/`ensure_active_window` verifies the foreground HWND and
  PID while holding that token;
- physical-input operations reject a missing, foreign, or stale token with
  `WINDOW_FOCUS_CONFLICT`;
- `release_input_lock` releases the lease, and closing a session releases its
  lease automatically.

This serializes physical fallback work and prevents a foreground window from
another test session from receiving input accidentally.

## Verified actions and postconditions

`input` and `set_value` resolve an automation ID, dispatch a semantic value
action, read the value back, wait for the settle interval, and read it back a
second time. A mismatch returns `VALUE_VERIFICATION_FAILED` with requested
and observed state.

`click` captures pre-action evidence, verifies the target is interactable,
dispatches Invoke/click, and optionally polls one of these postconditions:

- `automation_id_visible` or `element_appears`;
- `automation_id_hidden` or `element_disappears`;
- `value_changes` with optional `to`/`value`;
- `enabled_changes` with optional `to`;
- `selected_changes` with optional `to`;
- `window_appears` with a window query;
- `window_closes` with a window ID or query.

An unmet condition returns `ACTION_POSTCONDITION_FAILED`. The error includes
the last observation, timeout, runtime health, and pre-action screenshot.

## Waits, assertions, and compact trees

Reliability waits use explicit `timeout_ms` and `poll_ms`, include the last
observation in `WAIT_TIMEOUT`, and check runtime health while polling:

- `wait_for_window`, `wait_for_element`, `wait_for_automation_id`;
- `wait_for_value`, `wait_for_state`, `wait_until`, `wait_idle`.

Assertions are `assert_visible`, `assert_interactable`, and
`assert_valid_bounds`. Interactability combines visibility, enabled state,
offscreen state, window visibility, and configurable minimum width/height.
Custom widgets are not rejected merely because they use small bounds; the
threshold belongs to `ReliabilityConfig`.

`inspect_ui`, `get_control_tree`, and `inspect_semantic` accept compact-tree
filters: `compact`, `structured_only`, `max_depth`, `automation_ids_only`,
`visible_only`, `interactable_only`, exact `automation_id`,
`automation_id_prefix`, and `control_type`.

Semantic coverage can be checked with `windows_list_automation_ids` and
`windows_assert_automation_ids`. The latter reports expected, actual, and
missing IDs rather than reducing the result to a boolean.

## Runtime health and evidence

The launched child process has piped, bounded stderr capture. Critical panic
and runtime-error lines are retained even when a long backtrace fills the
normal tail. `get_runtime_health` reports PID, liveness, exit code/success,
Makepad/Script error classification, health, stderr tail, and timestamp.
`get_runtime_stderr` returns a caller-limited tail.

Every failed reliability operation records the failed action before
automatically exporting evidence. `windows_export_evidence` can also be called
explicitly. A bundle contains exactly these JSON files plus a `screenshots/`
directory when screenshots exist:

- `summary.json`;
- `actions.json`;
- `results.json`;
- `errors.json`;
- `ui-changes.json`;
- `last-ui-tree.json`;
- `runtime.json`.

Window, element, and region capture operations are `screenshot_window`,
`screenshot_element`, and `windows_capture_region`. PNG dimensions and
non-blank sampled content are validated. Native capture first tries
`PrintWindow(PW_RENDERFULLCONTENT)`; a completely black result is retried with
`BitBlt(WindowDC)`. The returned metadata states the capture method and
whether it was occlusion-independent.

## Canvas health

`get_canvas_health` and `assert_canvas_healthy` use the states `Healthy`,
`ZeroViewport`, `EmptyClip`, `Hidden`, `DrawNotCalled`,
`ContentOutsideViewport`, `StaleLayout`, and `Unknown`.

Bounds, visibility, offscreen state, and viewport relations are observable
through the accessibility bridge. `draw_called` and authoritative stale
layout/clip generation require optional application telemetry; without that
telemetry the report keeps `draw_called: null` and does not invent a value.

## Declarative sequences

`windows_run_sequence` executes JSON steps with a shared `session_id`. It
supports default `timeout_ms`, `retry`, `stop_on_error`,
`wait_after_action_ms` (or `wait_after_action`), and `evidence_on_failure`. Each result contains every attempt and its
complete structured response.

## Makepad 2.0 Script integration note

A top-level Splash/Script function is not assumed to be directly callable
from an arbitrary Rust `script_eval!` site. The reliable integration pattern
is: Rust updates deferred UI/application state, rendering exposes that state,
and Script reacts in its own render/event scope. A failure to call a
top-level Script helper directly is therefore an application integration
issue, not an accessibility-action failure.

## Verification commands

Run from the repository root. Use explicit manifests; the repository contains
nested workspaces and vendor sources are intentionally not formatted or
patched as part of this prototype.

```powershell
cargo fmt --manifest-path crates/toolkit-windows-automation/Cargo.toml
cargo fmt --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml
cargo check --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --all-targets
cargo test --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --lib
cargo build --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bins
& .\prototypes\ai-ui-test-tool-auto-probe\target\debug\ai-ui-reliability-runner.exe --output-dir reports\evidence\ai-ui-test-tool-reliability
```

The reliability run passes only when `summary.json` contains all markers:

- `AI_UI_RELIABILITY_PASSED`;
- `AI_UI_MULTI_WINDOW_PASSED`;
- `AI_UI_RUNTIME_FAILURE_DETECTED`;
- `AI_UI_EVIDENCE_EXPORT_PASSED`.

The previous runners remain regression gates and retain their markers
`AI_UI_AUTO_PROBE_PASSED` and `AI_UI_ADVANCED_INTERACTION_PASSED`.

## Known limits

- Physical input remains a Windows desktop-global facility; the lock and
  foreground verification serialize it but cannot make it semantic.
- `PrintWindow` behavior is application-defined. The BitBlt fallback can
  require a visible, unobscured window and is reported as not
  occlusion-independent.
- Draw-call, clip-generation, and authoritative stale-layout diagnostics need
  telemetry from the Makepad application to distinguish every CanvasHealth
  state conclusively.
- Reliability sessions discover all titled/visible top-level windows for the
  launched PID. Application-specific ownership rules for unusual hidden
  utility windows should be added by the application adapter, not to the
  generic bridge.
- The prototype attaches the accessibility provider in test/debug code. A
  production app should gate attachment with an explicit Windows/debug
  feature.




## Trusted observation and async-cleanup contract

The public observation adapter reads only a telemetry value published by a
resolved Makepad accessibility node, fixed external-observer aliases, named
trusted roots, and session-owned screenshots/tree evidence. It does not execute
arbitrary code, make arbitrary network calls, or traverse an untrusted path.
The short telemetry selector is resolved through the semantic tree before the
host receives the canonical AutomationId.

wait_for_api_response proves a response was observed; assert_api_success
requires completed transport, a non-error status, and
application_accepted_response: true. The error taxonomy preserves
EXTERNAL_REQUEST_TIMEOUT, EXTERNAL_REQUEST_FAILED,
EXTERNAL_RESPONSE_INVALID, and EXTERNAL_RESPONSE_NOT_ACCEPTED rather than
collapsing application rejection into transport failure.

assert_no_stale_window requires at least one visible primary application
window. A primary window has a non-empty root AutomationId or Window control
type. Hidden auxiliary panes such as Default IME and MSCTFIME UI therefore do
not create a false stale-window failure by themselves; a missing or hidden
primary app window still does.

extract produces a declared later-step variable. when and repeat_until remain
bounded, declarative script controls; repeat_until may poll only allowlisted
read-only observation operations. The deterministic async fixture, fixed
failure modes, and runnable scripts are documented in
[OBSERVATION_WORKFLOWS.md](OBSERVATION_WORKFLOWS.md).
