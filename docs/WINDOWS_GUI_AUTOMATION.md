# Windows GUI automation

Status: `EXPERIMENTAL` on Windows. Non-Windows builds expose the same Rust surface and return `ACCESSIBILITY_UNAVAILABLE`.

## Purpose

The component gives agents a deterministic path for native GUI acceptance:

```text
discover window -> window_id -> enumerate/resolve control -> element_id
-> action by ID -> state query -> target-window screenshot -> evidence
```

Screen coordinates are never public identity. Computer Use is fallback-only after the native backend returns a documented limitation such as `ACCESSIBILITY_UNAVAILABLE`, `ELEMENT_NOT_FOUND`, or `ACTION_NOT_SUPPORTED`.

## Architecture

`toolkit-windows-automation` is a generic Rust/Windows crate and has no Makepad or Office dependency.

```text
MCP stdio server
      |
GuiToolRegistry (single tool dispatch and schemas)
      +-> WindowsAutomation
      |     +-> Microsoft UI Automation
      |     +-> Win32 window/input APIs
      |     +-> target-window PNG capture
      +-> bounded EvidenceLog
```

The Toolkit had no pre-existing tool registry, RPC/MCP layer, GUI helper, screenshot helper, input injector, or evidence log. The milestone therefore introduces one cohesive registry and one backend instead of parallel per-feature frameworks.

The stdio server uses newline-delimited JSON-RPC and supports `initialize`, `ping`, `tools/list`, and `tools/call`.

## Result envelope

Every `tools/call` returns `result.structuredContent` with this shape:

```json
{
  "ok": true,
  "result": {},
  "evidence_sequence": 1
}
```

On failure, `ok` is false and `error` contains `code`, `message`, and optional structured `details`.

## Tool schemas

All schemas set `additionalProperties: false`.

| Tool | Required input | Optional input |
|---|---|---|
| `windows_find_window` | at least one selector is required semantically | `process_name: string`, `process_id: integer >= 1`, `title: string`, `title_contains: string` |
| `windows_get_control_tree` | `window_id: string` | `name`, `name_contains`, `control_type`, `automation_id`, `class_name`, `parent_element_id`, `visible_only`; `max_depth: 0..256`; `max_elements: 1..20000`; `view: control|raw` |
| `windows_find_control` | `window_id: string` and at least one semantic selector | `name`, `name_contains`, `control_type`, `automation_id`, `class_name`, `parent_element_id`, `visible_only`, `index >= 0` |
| `windows_click_automation_id` | `window_id: string`, `automation_id: string` | none |
| `windows_focus_automation_id` | `window_id: string`, `automation_id: string` | none |
| `windows_send_keys_to_automation_id` | `window_id: string`, `automation_id: string`, `keys: string` | none |
| `windows_get_automation_id_state` | `window_id: string`, `automation_id: string` | none |
| `windows_set_value_to_automation_id` | `window_id: string`, `automation_id: string`, `value: string` | none |
| `windows_click_element` | `window_id: string`, `element_id: string` | none |
| `windows_focus_element` | `window_id: string`, `element_id: string` | none |
| `windows_send_keys` | `window_id: string`, `keys: string` | none |
| `windows_get_element_state` | `window_id: string`, `element_id: string` | none |
| `windows_set_window_bounds` | `window_id: string`, `width: integer >= 1`, `height: integer >= 1` | `x: integer`, `y: integer` |
| `windows_capture_window` | `window_id: string` | `output_path: string` |
| `windows_focus_window` | `window_id: string` | none |
| `windows_get_evidence` | none | `since_sequence: integer >= 0`, `limit: 1..4096` |

`index` is accepted only by `windows_find_control` and only as a secondary discriminator after every semantic filter. An ambiguous match is never resolved silently.

## Identity and lifetime

A backend instance is one automation session. It creates a nonce from process identity and start time, then uses the accepted `rudra-runtime::IdAllocator<T>` independently for windows and elements.

- Window IDs have the opaque form `win:<session>:<monotonic>`.
- Element IDs have the opaque form `el:<session>:<monotonic>`.
- The backend retains the associated HWND or UIA interface.
- A display name, index, path, automation ID, native handle, and coordinate are metadata—not canonical Toolkit identity.
- Re-enumerating the same live UIA element reuses its existing element ID through `IUIAutomation::CompareElements`.
- A removed/replaced UIA element produces `STALE_ELEMENT`; the action layer resolves the semantic AutomationId again inside the current top-level window before retrying an action.
- A closed or recycled top-level HWND produces `WINDOW_NOT_FOUND` with stale-window detail.
- IDs are intentionally not valid in a new process/session.

## Native execution paths

Window discovery uses `EnumWindows` and process metadata. Multiple matches return `AMBIGUOUS_WINDOW` with deterministic candidate information.

Control discovery uses `CUIAutomation` with `ControlViewWalker` or `RawViewWalker`. Filtering is applied server-side and results include role/type, accessible name, automation ID, class/framework, bounds, visibility/offscreen, enabled/focus/selection/expand state, supported capabilities, and native HWND when available.

Every action is window-scoped: the backend revalidates the live top-level HWND, obtains a fresh UIA root, and re-resolves the requested semantic element before acting. A Makepad fragment may have no `NativeWindowHandle`; it inherits the validated owner HWND from the top-level window.

Activation priority is:

1. `IUIAutomationInvokePattern::Invoke`;
2. `IUIAutomationSelectionItemPattern::Select`;
3. `BM_CLICK` only for a native Button owned by the same top-level HWND;
4. keyboard activation after UIA focus for a semantic Button.

Coordinate clicks and mouse injection are not used by the ID-addressed action path. A stale element, missing owner HWND, unavailable pattern, and UIA invocation failure are reported as distinct machine-readable errors.

Keyboard input first resolves and activates the requested window, verifies the foreground HWND, then uses `SendInput`. Required chords include `CTRL+TAB`, `CTRL+SHIFT+TAB`, and `CTRL+W`; named keys, modifier chords, and Unicode text are also supported.

Window resize/move uses `SetWindowPos` and returns both requested and actual bounds. Focus uses `ShowWindow`, `SetForegroundWindow`, `BringWindowToTop`, and temporary `AttachThreadInput` when Windows foreground rules require it.

## Window-frame interaction boundary

`windows_set_window_bounds` and `windows_set_window_state` are programmatic setup/recovery operations; they are not evidence of a user drag or resize. The generic inspector exposes fresh `WM_NCHITTEST` title/edge/corner targets, DPI, monitor and state metadata for an opaque `window_id`.

Physical `windows_drag_title`, `windows_resize_frame`, and `windows_double_click_title` are exposed through the AI UI Test Tool Script API. They require its existing single global physical-input lock and record fresh post-gesture bounds, semantic layout, runtime health and pointer cleanup. Passing a session-owned opaque `window_id` binds that owned HWND before the operation; it does not merely validate the ID and cannot select an arbitrary external process window. The direct generic MCP registry is intentionally limited to inspection, target resolution and programmatic state management so it cannot bypass that lock. See [Window frame and desktop interaction audit](WINDOW_INTERACTION_AUDIT.md).

Target capture first attempts `PrintWindow(PW_RENDERFULLCONTENT)` and falls back to `BitBlt(WindowDC)`. The returned capture metadata records the backend, owning HWND/PID, bounds, dimensions, timestamp, method, non-blank validation, fallback usage, GPU compatibility inferred from the selected path, and bounded diagnostics. `PrintWindow` support is application-defined; some GPU surfaces can require the visible BitBlt fallback. The GPU flag describes the observed capture path and is not renderer introspection.

## Structured errors

The stable machine-readable codes are:

```text
WINDOW_NOT_FOUND
AMBIGUOUS_WINDOW
WINDOW_NOT_MOVABLE
WINDOW_NOT_RESIZABLE
WINDOW_FRAME_TARGET_UNAVAILABLE
WINDOW_TITLE_REGION_UNAVAILABLE
WINDOW_RESIZE_CONSTRAINT
WINDOW_BOUNDS_NOT_CHANGED
WINDOW_MOVE_VERIFICATION_FAILED
WINDOW_RESIZE_VERIFICATION_FAILED
WINDOW_DPI_CONVERSION_FAILED
WINDOW_STATE_TRANSITION_FAILED
ELEMENT_NOT_FOUND
AMBIGUOUS_ELEMENT
STALE_ELEMENT
WINDOW_NOT_FOCUSED
ACTION_NOT_SUPPORTED
INPUT_INJECTION_FAILED
SCREENSHOT_FAILED
ACCESSIBILITY_UNAVAILABLE
OWNER_HWND_UNAVAILABLE
INVOCATION_FAILED
PERMISSION_DENIED
INVALID_ARGUMENT
TOOL_NOT_FOUND
BACKEND_FAILURE
```

Human-readable messages are diagnostic only; automation must branch on `code`.

## Evidence

The registry owns a bounded 4096-entry log. Each entry records sequence, Unix timestamp, tool, window/element IDs, original request, execution method, structured result, resulting state when available, and screenshot path. `windows_get_evidence` supports incremental reads by sequence. Rust callers can also export the complete bounded log as JSON.

The intended screenshot-after-action workflow remains composable:

```text
windows_find_control
windows_focus_element
windows_click_element
windows_get_element_state
windows_capture_window
windows_get_evidence
```

## Computer Use fallback policy

Computer Use is not a convenience path. Before using it, the ID-addressed backend must have returned and logged a native limitation. The fallback reason and a post-action screenshot are mandatory; native state queries should still be used when available. The backend never converts `ELEMENT_NOT_FOUND` into a visual coordinate guess.

## MCP startup

```powershell
cargo run --offline -p toolkit-windows-automation --bin toolkit-windows-gui-mcp
```

Example request (one JSON object per line):

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"windows_find_window","arguments":{"process_name":"example.exe"}}}
```

Use the returned `structuredContent.result.window_id` in subsequent calls from the same server process.

## Verification

```powershell
cargo test --offline -p toolkit-windows-automation --lib
cargo check --offline -p toolkit-windows-automation --all-targets
cargo test --offline -p toolkit-windows-automation
cargo test --offline -p toolkit-windows-automation --test windows_gui_integration -- --ignored --nocapture
# Optional external-application HWND probe (interactive desktop):
$env:FORGE_UIA_TITLE_CONTAINS="..."
cargo test --offline -p toolkit-windows-automation --test windows_hwnd_scoped_integration -- --ignored --nocapture
cargo run --offline -p toolkit-windows-automation --bin windows-gui-self-test -- --fixture target/debug/windows-gui-self-test-fixture.exe --output-dir reports/evidence/windows-gui-self-test
```

The ignored integration test requires an interactive Windows desktop. Pure/unit tests and non-Windows compilation do not.

## Current Makepad Windows limitation

The backend correctly resolves the reference consumer application top-level MakepadWindow, focuses/resizes/captures it, and binds keyboard injection to its window_id. The existing consumer application still exposes only the top-level window through UIA because it does not attach a semantic provider.

The Toolkit now includes a generic Windows UI Automation bridge in MAKEPAD_WINDOWS_ACCESSIBILITY.md. A Makepad application must build an AccessibilityTree, attach WindowsAccessibilityBridge to its HWND, publish updates after semantic/layout/selection changes, and route the action callback to the Makepad UI thread. The bridge does not inspect private widget internals or patch the Makepad vendor.

Until the application attaches that bridge, native queries for its custom TabItem and button drawing remain ELEMENT_NOT_FOUND. That is an application integration gap, not a reason to guess coordinates.
