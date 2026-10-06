# AI UI Test Tool — Operation Reference

Status: CURRENT  
Updated: 2026-08-23  
Public operations: 112
Source of truth: `prototypes/ai-ui-test-tool-auto-probe/operations.json`

This reference is generated from the public catalog. Each entry is a public Script API contract; direct-host aliases are intentionally excluded. The runner injects the current session ID after `session.open`, so examples omit it unless it is catalog-required.

## Observation and async boundary

Observation operations read only trusted Makepad telemetry published through the resolved accessibility node, session-owned captures, and named trusted file roots (`workspace`, `output`, `fixture`, `temp`). Transport completion is not application acceptance: `assert_api_success` requires both. Visual diagnostics return unavailable runtime facts as `Unknown` rather than fabricated values. Capture results include backend, HWND/PID owner, dimensions, non-blank validation, fallback usage and bounded diagnostics. The v1 public catalog has no media observation operations; `Playing` UI state alone is never audio/video proof.

## Generator and scheduler labels

`GENERATOR_REQUIRED` means a versioned generic template uses the operation. `GENERATOR_OPTIONAL` needs app-specific fixture/state. `INFRASTRUCTURE_ONLY` is runner/scheduler scaffolding. `NEGATIVE_ONLY` proves a rejection path. A physical operation requires the serialized Windows input resource unless its backend has a documented semantic path.

## `session.open`

- Purpose: Manage the public test-session lifecycle and its owned Windows process.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `unknown`
- Generator classification: `INFRASTRUCTURE_ONLY`
- Example:

```json
{"op":"session.open"}
```

## `session.list`

- Purpose: Manage the public test-session lifecycle and its owned Windows process.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `INFRASTRUCTURE_ONLY`
- Example:

```json
{"op":"session.list"}
```

## `session.close`

- Purpose: Manage the public test-session lifecycle and its owned Windows process.
- Required args: `session_id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `INFRASTRUCTURE_ONLY`
- Example:

```json
{"op":"session.close","session_id":"<session_id>"}
```

## `acquire_input_lock`

- Purpose: Manage the exclusive desktop-input lease used by physical operations.
- Required args: `session_id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `unknown`
- Generator classification: `INFRASTRUCTURE_ONLY`
- Example:

```json
{"op":"acquire_input_lock","session_id":"<session_id>"}
```

## `release_input_lock`

- Purpose: Manage the exclusive desktop-input lease used by physical operations.
- Required args: `session_id`, `token`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `INFRASTRUCTURE_ONLY`
- Example:

```json
{"op":"release_input_lock","session_id":"<session_id>","token":"<token>"}
```

## `inspect_ui`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `context_menu.lifecycle`, `window.resize`
- Example:

```json
{"op":"inspect_ui"}
```

## `inspect_semantic`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"inspect_semantic"}
```

## `inspect_spatial_target`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `spatial.health`
- Example:

```json
{"op":"inspect_spatial_target","id":"window.main/target"}
```

## `read`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `edit.value`
- Example:

```json
{"op":"read","id":"window.main/target"}
```

## `set_value`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `id`, `value`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `edit.value`
- Example:

```json
{"op":"set_value","id":"window.main/target","value":"example value"}
```

## `input`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `id`, `text`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"input","id":"window.main/target","text":"example value"}
```

## `click`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `button.invoke`, `modal.lifecycle`, `modal.lifecycle`, `runtime.task`, `external.request`, `file.output`, `trace.milestone`, `visual.region`
- Example:

```json
{"op":"click","id":"window.main/target"}
```

## `focus`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"focus","id":"window.main/target"}
```

## `double_click`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"double_click","id":"window.main/target"}
```

## `right_click`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `context_menu.lifecycle`
- Example:

```json
{"op":"right_click","id":"window.main/target"}
```

## `toggle`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `checkbox.toggle`
- Example:

```json
{"op":"toggle","id":"window.main/target"}
```

## `choose`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `id`, `value`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `dropdown.choose`
- Example:

```json
{"op":"choose","id":"window.main/target","value":"example value"}
```

## `select`

- Purpose: Change or verify a semantic selection through the public accessibility contract.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `selection.single`
- Example:

```json
{"op":"select","id":"window.main/target"}
```

## `add_to_selection`

- Purpose: Change or verify a semantic selection through the public accessibility contract.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"add_to_selection","id":"window.main/target"}
```

## `remove_from_selection`

- Purpose: Change or verify a semantic selection through the public accessibility contract.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"remove_from_selection","id":"window.main/target"}
```

## `clear_selection`

- Purpose: Change or verify a semantic selection through the public accessibility contract.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"clear_selection"}
```

## `select_many`

- Purpose: Change or verify a semantic selection through the public accessibility contract.
- Required args: `ids`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `selection.multiple`
- Example:

```json
{"op":"select_many","ids":["window.main/item.1","window.main/item.2"]}
```

## `select_range`

- Purpose: Change or verify a semantic selection through the public accessibility contract.
- Required args: `start_id`, `end_id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `selection.range`
- Example:

```json
{"op":"select_range","start_id":"window.main/item.1","end_id":"window.main/item.2"}
```

## `ensure_visible`

- Purpose: Move or verify a target within a semantic scrolling surface.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `scroll.ensure_visible`
- Example:

```json
{"op":"ensure_visible","id":"window.main/target"}
```

## `scroll`

- Purpose: Move or verify a target within a semantic scrolling surface.
- Required args: `id`, `delta`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"scroll","id":"window.main/target","delta":1}
```

## `scroll_to_start`

- Purpose: Move or verify a target within a semantic scrolling surface.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"scroll_to_start","id":"window.main/target"}
```

## `scroll_to_end`

- Purpose: Move or verify a target within a semantic scrolling surface.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"scroll_to_end","id":"window.main/target"}
```

## `wait_for_automation_id`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `discovery.semantic`, `button.invoke`
- Example:

```json
{"op":"wait_for_automation_id","id":"window.main/target"}
```

## `wait_for_value`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `id`, `value`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `edit.value`, `dropdown.choose`
- Example:

```json
{"op":"wait_for_value","id":"window.main/target","value":"example value"}
```

## `wait_for_state`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `id`, `state`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `checkbox.toggle`, `selection.single`, `selection.multiple`, `selection.range`
- Example:

```json
{"op":"wait_for_state","id":"window.main/target","state":"Completed"}
```

## `wait_until`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `condition`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"wait_until","condition":"idle"}
```

## `wait_visible`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `modal.lifecycle`, `context_menu.lifecycle`
- Example:

```json
{"op":"wait_visible","id":"window.main/target"}
```

## `wait_hidden`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `modal.lifecycle`, `context_menu.lifecycle`
- Example:

```json
{"op":"wait_hidden","id":"window.main/target"}
```

## `wait_enabled`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"wait_enabled","id":"window.main/target"}
```

## `wait_disabled`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"wait_disabled","id":"window.main/target"}
```

## `wait_selected`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"wait_selected","id":"window.main/target"}
```

## `wait_count`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `id`, `count`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"wait_count","id":"window.main/target","count":1}
```

## `wait_idle`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"wait_idle"}
```

## `assert_visible`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `discovery.semantic`, `modal.lifecycle`
- Example:

```json
{"op":"assert_visible","id":"window.main/target"}
```

## `assert_interactable`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `button.invoke`, `scroll.ensure_visible`
- Example:

```json
{"op":"assert_interactable","id":"window.main/target"}
```

## `assert_valid_bounds`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `discovery.semantic`, `scroll.ensure_visible`
- Example:

```json
{"op":"assert_valid_bounds","id":"window.main/target"}
```

## `assert_canvas_healthy`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `spatial.health`, `spatial.click`, `spatial.drag`, `spatial.select_region`
- Example:

```json
{"op":"assert_canvas_healthy","id":"window.main/target"}
```

## `get_canvas_health`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"get_canvas_health","id":"window.main/target"}
```

## `move_pointer`

- Purpose: Perform a Windows physical/spatial gesture against a resolved semantic target.
- Required args: `id`, `position`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"move_pointer","id":"window.main/target","position":{"x":0.5,"y":0.5}}
```

## `pointer_down`

- Purpose: Perform a Windows physical/spatial gesture against a resolved semantic target.
- Required args: `id`, `position`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"pointer_down","id":"window.main/target","position":{"x":0.5,"y":0.5}}
```

## `pointer_up`

- Purpose: Perform a Windows physical/spatial gesture against a resolved semantic target.
- Required args: `button`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"pointer_up","button":"left"}
```

## `click_relative`

- Purpose: Perform a Windows physical/spatial gesture against a resolved semantic target.
- Required args: `id`, `position`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `spatial.click`, `negative.relative.invalid`
- Example:

```json
{"op":"click_relative","id":"window.main/target","position":{"x":0.5,"y":0.5}}
```

## `double_click_relative`

- Purpose: Perform a Windows physical/spatial gesture against a resolved semantic target.
- Required args: `id`, `position`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"double_click_relative","id":"window.main/target","position":{"x":0.5,"y":0.5}}
```

## `right_click_relative`

- Purpose: Perform a Windows physical/spatial gesture against a resolved semantic target.
- Required args: `id`, `position`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"right_click_relative","id":"window.main/target","position":{"x":0.5,"y":0.5}}
```

## `hover_relative`

- Purpose: Perform a Windows physical/spatial gesture against a resolved semantic target.
- Required args: `id`, `position`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `spatial.health`
- Example:

```json
{"op":"hover_relative","id":"window.main/target","position":{"x":0.5,"y":0.5}}
```

## `wheel_relative`

- Purpose: Perform a Windows physical/spatial gesture against a resolved semantic target.
- Required args: `id`, `position`, `delta_y`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"wheel_relative","id":"window.main/target","position":{"x":0.5,"y":0.5},"delta_y":1}
```

## `drag`

- Purpose: Perform a Windows physical/spatial gesture against a resolved semantic target.
- Required args: `id`, `to`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"drag","id":"window.main/target","to":{"x":0.8,"y":0.8}}
```

## `drag_to`

- Purpose: Perform a Windows physical/spatial gesture against a resolved semantic target.
- Required args: `id`, `target_id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"drag_to","id":"window.main/target","target_id":"window.main/target"}
```

## `drag_relative`

- Purpose: Perform a Windows physical/spatial gesture against a resolved semantic target.
- Required args: `id`, `from`, `to`
- Optional public args: `button`
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `spatial.drag`
- Example:

```json
{"op":"drag_relative","id":"window.main/target","from":{"x":0.2,"y":0.2},"to":{"x":0.8,"y":0.8}}
```

## `select_region`

- Purpose: Change or verify a semantic selection through the public accessibility contract.
- Required args: `id`, `from`, `to`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with image/region metrics or capture metadata and a portable evidence reference.
- Typed failure family: `CAPTURE_FAILED`, `VISUAL_BLANK`, `VISUAL_REGION_UNCHANGED`, `VISUAL_REGION_CHANGED`, `VISUAL_SIMILARITY_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `spatial.select_region`
- Example:

```json
{"op":"select_region","id":"window.main/target","from":{"x":0.2,"y":0.2},"to":{"x":0.8,"y":0.8}}
```

## `hotkey`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `keys`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"hotkey","keys":["Ctrl","S"]}
```

## `press`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `key`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `context_menu.lifecycle`
- Example:

```json
{"op":"press","key":"Escape"}
```

## `type_text`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `text`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"type_text","text":"example value"}
```

## `activate_window`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"activate_window"}
```

## `windows_set_window_bounds`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `width`, `height`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with dispatch/verification fields and target state when applicable.
- Typed failure family: `INPUT_LOCK_REQUIRED`, `WINDOW_FOCUS_CONFLICT`, `ELEMENT_NOT_FOUND`, `ACTION_POSTCONDITION_FAILED`
- Physical input: yes — scheduler serializes desktop input
- Scheduler classification: `physical`
- Idempotency: `not declared`
- Generator classification: `GENERATOR_REQUIRED` via `window.resize`
- Example:

```json
{"op":"windows_set_window_bounds","width":640,"height":640}
```

## `screenshot_window`

- Purpose: Capture bounded evidence from the session-owned window or target.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with image/region metrics or capture metadata and a portable evidence reference.
- Typed failure family: `CAPTURE_FAILED`, `VISUAL_BLANK`, `VISUAL_REGION_UNCHANGED`, `VISUAL_REGION_CHANGED`, `VISUAL_SIMILARITY_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"screenshot_window"}
```

## `screenshot_element`

- Purpose: Capture bounded evidence from the session-owned window or target.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with image/region metrics or capture metadata and a portable evidence reference.
- Typed failure family: `CAPTURE_FAILED`, `VISUAL_BLANK`, `VISUAL_REGION_UNCHANGED`, `VISUAL_REGION_CHANGED`, `VISUAL_SIMILARITY_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"screenshot_element","id":"window.main/target"}
```

## `capture_region`

- Purpose: Capture bounded evidence from the session-owned window or target.
- Required args: `id`, `region`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with image/region metrics or capture metadata and a portable evidence reference.
- Typed failure family: `CAPTURE_FAILED`, `VISUAL_BLANK`, `VISUAL_REGION_UNCHANGED`, `VISUAL_REGION_CHANGED`, `VISUAL_SIMILARITY_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `spatial.health`
- Example:

```json
{"op":"capture_region","id":"window.main/target","region":{"x":0.1,"y":0.1,"width":0.8,"height":0.8}}
```

## `windows_capture_region`

- Purpose: Capture bounded evidence from the session-owned window or target.
- Required args: `id`, `region`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with image/region metrics or capture metadata and a portable evidence reference.
- Typed failure family: `CAPTURE_FAILED`, `VISUAL_BLANK`, `VISUAL_REGION_UNCHANGED`, `VISUAL_REGION_CHANGED`, `VISUAL_SIMILARITY_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"windows_capture_region","id":"window.main/target","region":{"x":0.1,"y":0.1,"width":0.8,"height":0.8}}
```

## `get_runtime_health`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `INFRASTRUCTURE_ONLY` via `button.invoke`, `window.resize`, `runtime.health`
- Example:

```json
{"op":"get_runtime_health"}
```

## `get_runtime_stderr`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `INFRASTRUCTURE_ONLY`
- Example:

```json
{"op":"get_runtime_stderr"}
```

## `windows_export_evidence`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `INFRASTRUCTURE_ONLY`
- Example:

```json
{"op":"windows_export_evidence"}
```

## `windows_validate_evidence`

- Purpose: Perform the documented public AI UI Test Tool operation.
- Required args: `bundle`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `not declared`
- Generator classification: `NEGATIVE_ONLY`
- Example:

```json
{"op":"windows_validate_evidence","bundle":{}}
```

## `get_task_state`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: `operation_id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TASK_NOT_FOUND`, `TASK_STATE_TIMEOUT`, `TASK_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"get_task_state","operation_id":"fixture.async.workflow"}
```

## `wait_for_task`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `operation_id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TASK_NOT_FOUND`, `TASK_STATE_TIMEOUT`, `TASK_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"wait_for_task","operation_id":"fixture.async.workflow"}
```

## `wait_for_task_state`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `operation_id`, `state`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TASK_NOT_FOUND`, `TASK_STATE_TIMEOUT`, `TASK_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"wait_for_task_state","operation_id":"fixture.async.workflow","state":"Completed"}
```

## `wait_for_task_complete`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `operation_id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TASK_NOT_FOUND`, `TASK_STATE_TIMEOUT`, `TASK_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `runtime.task`, `visual.region`
- Example:

```json
{"op":"wait_for_task_complete","operation_id":"fixture.async.workflow"}
```

## `assert_task_success`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `operation_id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TASK_NOT_FOUND`, `TASK_STATE_TIMEOUT`, `TASK_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `runtime.task`
- Example:

```json
{"op":"assert_task_success","operation_id":"fixture.async.workflow"}
```

## `get_runtime_milestones`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TRACE_TIMEOUT`, `TRACE_NOT_FOUND`, `RUNTIME_MILESTONE_TIMEOUT`, `RUNTIME_MILESTONE_NOT_FOUND`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"get_runtime_milestones"}
```

## `wait_for_runtime_milestone`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `name`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TRACE_TIMEOUT`, `TRACE_NOT_FOUND`, `RUNTIME_MILESTONE_TIMEOUT`, `RUNTIME_MILESTONE_NOT_FOUND`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `trace.milestone`
- Example:

```json
{"op":"wait_for_runtime_milestone","name":"task.completed"}
```

## `assert_runtime_milestone`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `name`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TRACE_TIMEOUT`, `TRACE_NOT_FOUND`, `RUNTIME_MILESTONE_TIMEOUT`, `RUNTIME_MILESTONE_NOT_FOUND`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `trace.milestone`
- Example:

```json
{"op":"assert_runtime_milestone","name":"task.completed"}
```

## `get_runtime_timeline`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"get_runtime_timeline"}
```

## `get_api_request_state`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: `request_id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `EXTERNAL_REQUEST_NOT_FOUND`, `EXTERNAL_REQUEST_TIMEOUT`, `EXTERNAL_REQUEST_FAILED`, `EXTERNAL_RESPONSE_INVALID`, `EXTERNAL_RESPONSE_NOT_ACCEPTED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"get_api_request_state","request_id":"fixture.request.workflow"}
```

## `wait_for_api_state`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `request_id`, `state`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `EXTERNAL_REQUEST_NOT_FOUND`, `EXTERNAL_REQUEST_TIMEOUT`, `EXTERNAL_REQUEST_FAILED`, `EXTERNAL_RESPONSE_INVALID`, `EXTERNAL_RESPONSE_NOT_ACCEPTED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"wait_for_api_state","request_id":"fixture.request.workflow","state":"Completed"}
```

## `wait_for_api_response`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `request_id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `EXTERNAL_REQUEST_NOT_FOUND`, `EXTERNAL_REQUEST_TIMEOUT`, `EXTERNAL_REQUEST_FAILED`, `EXTERNAL_RESPONSE_INVALID`, `EXTERNAL_RESPONSE_NOT_ACCEPTED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `external.request`
- Example:

```json
{"op":"wait_for_api_response","request_id":"fixture.request.workflow"}
```

## `assert_api_success`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `request_id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `EXTERNAL_REQUEST_NOT_FOUND`, `EXTERNAL_REQUEST_TIMEOUT`, `EXTERNAL_REQUEST_FAILED`, `EXTERNAL_RESPONSE_INVALID`, `EXTERNAL_RESPONSE_NOT_ACCEPTED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `external.request`
- Example:

```json
{"op":"assert_api_success","request_id":"fixture.request.workflow"}
```

## `assert_api_failure`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `request_id`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `EXTERNAL_REQUEST_NOT_FOUND`, `EXTERNAL_REQUEST_TIMEOUT`, `EXTERNAL_REQUEST_FAILED`, `EXTERNAL_RESPONSE_INVALID`, `EXTERNAL_RESPONSE_NOT_ACCEPTED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"assert_api_failure","request_id":"fixture.request.workflow"}
```

## `wait_for_file`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `root`, `path`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with trusted-root file metadata and, where applicable, a portable evidence reference.
- Typed failure family: `FILE_ROOT_NOT_ALLOWED`, `FILE_NOT_FOUND`, `FILE_NOT_STABLE`, `FILE_HASH_MISMATCH`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `file.output`
- Example:

```json
{"op":"wait_for_file","root":"fixture","path":"async-output.json"}
```

## `assert_file_exists`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `root`, `path`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with trusted-root file metadata and, where applicable, a portable evidence reference.
- Typed failure family: `FILE_ROOT_NOT_ALLOWED`, `FILE_NOT_FOUND`, `FILE_NOT_STABLE`, `FILE_HASH_MISMATCH`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `file.output`
- Example:

```json
{"op":"assert_file_exists","root":"fixture","path":"async-output.json"}
```

## `assert_file_not_exists`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `root`, `path`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with trusted-root file metadata and, where applicable, a portable evidence reference.
- Typed failure family: `FILE_ROOT_NOT_ALLOWED`, `FILE_NOT_FOUND`, `FILE_NOT_STABLE`, `FILE_HASH_MISMATCH`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"assert_file_not_exists","root":"fixture","path":"async-output.json"}
```

## `assert_file_size`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `root`, `path`, `expected_bytes`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with trusted-root file metadata and, where applicable, a portable evidence reference.
- Typed failure family: `FILE_ROOT_NOT_ALLOWED`, `FILE_NOT_FOUND`, `FILE_NOT_STABLE`, `FILE_HASH_MISMATCH`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"assert_file_size","root":"fixture","path":"async-output.json","expected_bytes":1}
```

## `assert_file_count`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `root`, `path`, `count`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with trusted-root file metadata and, where applicable, a portable evidence reference.
- Typed failure family: `FILE_ROOT_NOT_ALLOWED`, `FILE_NOT_FOUND`, `FILE_NOT_STABLE`, `FILE_HASH_MISMATCH`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"assert_file_count","root":"fixture","path":"async-output.json","count":1}
```

## `assert_file_hash`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `root`, `path`, `sha256`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with trusted-root file metadata and, where applicable, a portable evidence reference.
- Typed failure family: `FILE_ROOT_NOT_ALLOWED`, `FILE_NOT_FOUND`, `FILE_NOT_STABLE`, `FILE_HASH_MISMATCH`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"assert_file_hash","root":"fixture","path":"async-output.json","sha256":"<64-char-sha256>"}
```

## `wait_for_file_stable`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `root`, `path`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with trusted-root file metadata and, where applicable, a portable evidence reference.
- Typed failure family: `FILE_ROOT_NOT_ALLOWED`, `FILE_NOT_FOUND`, `FILE_NOT_STABLE`, `FILE_HASH_MISMATCH`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `file.output`
- Example:

```json
{"op":"wait_for_file_stable","root":"fixture","path":"async-output.json"}
```

## `assert_not_blank`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `visual.region`
- Example:

```json
{"op":"assert_not_blank"}
```

## `assert_region_changed`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `baseline_path`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with image/region metrics or capture metadata and a portable evidence reference.
- Typed failure family: `CAPTURE_FAILED`, `VISUAL_BLANK`, `VISUAL_REGION_UNCHANGED`, `VISUAL_REGION_CHANGED`, `VISUAL_SIMILARITY_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"assert_region_changed","baseline_path":"reports/baseline.png"}
```

## `assert_region_unchanged`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `baseline_path`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with image/region metrics or capture metadata and a portable evidence reference.
- Typed failure family: `CAPTURE_FAILED`, `VISUAL_BLANK`, `VISUAL_REGION_UNCHANGED`, `VISUAL_REGION_CHANGED`, `VISUAL_SIMILARITY_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"assert_region_unchanged","baseline_path":"reports/baseline.png"}
```

## `assert_similarity`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `baseline_path`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"assert_similarity","baseline_path":"reports/baseline.png"}
```

## `compare_screenshot`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `baseline_path`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with image/region metrics or capture metadata and a portable evidence reference.
- Typed failure family: `CAPTURE_FAILED`, `VISUAL_BLANK`, `VISUAL_REGION_UNCHANGED`, `VISUAL_REGION_CHANGED`, `VISUAL_SIMILARITY_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"compare_screenshot","baseline_path":"reports/baseline.png"}
```

## `get_makepad_visual_diagnostics`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with image/region metrics or capture metadata and a portable evidence reference.
- Typed failure family: `CAPTURE_FAILED`, `VISUAL_BLANK`, `VISUAL_REGION_UNCHANGED`, `VISUAL_REGION_CHANGED`, `VISUAL_SIMILARITY_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `visual.region`
- Example:

```json
{"op":"get_makepad_visual_diagnostics"}
```

## `get_application_logs`

- Purpose: Read bounded semantic, runtime, file, visual, or evidence state without mutating the application.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TRACE_TIMEOUT`, `TRACE_NOT_FOUND`, `RUNTIME_MILESTONE_TIMEOUT`, `RUNTIME_MILESTONE_NOT_FOUND`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"get_application_logs"}
```

## `wait_for_log`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: `contains`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TRACE_TIMEOUT`, `TRACE_NOT_FOUND`, `RUNTIME_MILESTONE_TIMEOUT`, `RUNTIME_MILESTONE_NOT_FOUND`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"wait_for_log","contains":"<contains>"}
```

## `assert_log_contains`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `contains`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TRACE_TIMEOUT`, `TRACE_NOT_FOUND`, `RUNTIME_MILESTONE_TIMEOUT`, `RUNTIME_MILESTONE_NOT_FOUND`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"assert_log_contains","contains":"<contains>"}
```

## `assert_log_not_contains`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `contains`
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TRACE_TIMEOUT`, `TRACE_NOT_FOUND`, `RUNTIME_MILESTONE_TIMEOUT`, `RUNTIME_MILESTONE_NOT_FOUND`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"assert_log_not_contains","contains":"<contains>"}
```

## `assert_trace_event`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `event`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TRACE_TIMEOUT`, `TRACE_NOT_FOUND`, `RUNTIME_MILESTONE_TIMEOUT`, `RUNTIME_MILESTONE_NOT_FOUND`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `trace.milestone`
- Example:

```json
{"op":"assert_trace_event","event":"<event>"}
```

## `assert_no_stale_window`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `STALE_WINDOW`, `APPLICATION_RUNTIME_FAILURE`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"assert_no_stale_window"}
```

## `assert_no_pending_task`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical success envelope with observed telemetry state, bounded last observation, and elapsed time.
- Typed failure family: `TELEMETRY_NOT_PUBLISHED`, `TASK_NOT_FOUND`, `TASK_STATE_TIMEOUT`, `TASK_FAILED`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_REQUIRED` via `runtime.task`
- Example:

```json
{"op":"assert_no_pending_task"}
```

## `assert_no_phantom_semantic_node`

- Purpose: Assert an observable postcondition and return typed evidence on failure.
- Required args: `id`
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `semantic`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"assert_no_phantom_semantic_node","id":"window.main/target"}
```

## `repeat_until`

- Purpose: Wait with an explicit bound for an observable public condition; it never evaluates arbitrary code.
- Required args: none
- Optional public args: none declared by the catalog
- Result: A canonical { operation, success, result } response; failure uses { error: { code, message, details } }.
- Typed failure family: `INVALID_ARGUMENT`, `ELEMENT_NOT_FOUND`, `APPLICATION_RUNTIME_FAILURE when applicable`
- Physical input: no
- Scheduler classification: `local`
- Idempotency: `idempotent`
- Generator classification: `GENERATOR_OPTIONAL`
- Example:

```json
{"op":"repeat_until"}
```



## Window-frame operations (Windows)

These operations apply to a session-owned top-level Windows window. The
coordinate model is physical screen pixels. Inspect immediately before every
physical gesture; never reuse a stale title or resize point after a move, DPI
change, resize, state transition, or monitor change.

| Operation | Required public arguments | Input / scheduler | Generator |
| --- | --- | --- | --- |
| `inspect_window_interaction` | none | read-only / semantic | `window.interaction` |
| `windows_set_window_bounds` | `width`, `height` | programmatic setup or recovery / semantic | not a physical UX proof |
| `windows_set_window_state` | `state` | bounded programmatic state transition / semantic | setup or recovery |
| `windows_drag_title` | `delta_x`, `delta_y` | physical / `windows.interactive_desktop` | opt-in `window.move` |
| `windows_resize_frame` | `target` | physical / `windows.interactive_desktop` | opt-in `window.frame_resize` |
| `windows_double_click_title` | none | physical / `windows.interactive_desktop` | opt-in `window.state` |

Physical operations acquire the existing global input lease, foreground the
owned window, use a freshly resolved `WM_NCHITTEST` target, release buttons and
modifiers, then record fresh bounds, DPI, semantic layout and runtime-health
evidence. A non-resizable native window returns the typed
`WINDOW_NOT_RESIZABLE` refusal instead of guessing coordinates.
