# AI UI Test Tool Script Quickstart

Status: CURRENT  
Updated: 2026-08-22  
Contract/API version: AI UI Test Tool Script API v1

A .aiui.json file is an ordinary declarative client of the public Tool API.
The machine-readable truths are operations.json and
ai-ui-test-script.schema.json. The runner does not accept shell, PowerShell,
eval or arbitrary code.

## Validate, normalize and run

From the repository root:

~~~powershell
cargo run --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-test -- validate prototypes/ai-ui-test-tool-auto-probe/examples/basic-auto-probe.aiui.json
cargo run --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-test -- normalize prototypes/ai-ui-test-tool-auto-probe/examples/basic-auto-probe.aiui.json
cargo run --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-test -- run prototypes/ai-ui-test-tool-auto-probe/examples/basic-auto-probe.aiui.json
~~~

The runner adds the current session ID where appropriate, substitutes
variables, stores save_as output, performs teardown and writes ai-report.json
and report.json.

A `${name}` reference resolves a declared variable or fixture. A property
reference such as `${lock.token}` resolves a field from an earlier `save_as`
result (for example, an input-lock token). expect_error turns an expected
canonical error into PASS; depends_on turns a step with a failed dependency into
SKIPPED_DEPENDENCY_FAILED.

## Minimal shape

~~~json
{
  "version": 1,
  "name": "semantic-smoke",
  "application": {"command": "target/debug/my-app.exe"},
  "options": {"stop_on_error": true, "evidence_on_failure": true},
  "steps": [
    {"op": "session.open", "save_session_as": "app"},
    {"op": "inspect_ui", "session": "app", "save_as": "tree"},
    {"op": "wait_for_automation_id", "session": "app", "id": "login.submit"},
    {"op": "click", "session": "app", "id": "login.submit"},
    {"op": "windows_export_evidence", "session": "app"}
  ],
  "teardown": [{"op": "session.close", "session": "app"}]
}
~~~

## Operations and physical work

Use operations.json for exact required fields. In particular, spatial/physical
operations such as click_relative, wheel_relative and drag_relative require a
known semantic target and run on Local Windows only. They must not be assumed
available in Container Level 0. See the [Agent Guide](../../AI_UI_TEST_TOOL_AGENT_GUIDE.md)
and [Limitations](../../LIMITATIONS.md).

Included examples cover basic inspection, contract-negative validation, error
collection, full Script API flow, spatial smoke and a spatial suite.

## Deterministic async-fixture mode

The optional `application.fixture_async_failure_mode` exists only for the
trusted local async fixture. It accepts `success`, `task_fails`,
`api_timeout`, `api_http_failure`, `api_invalid_response`, `api_not_accepted`,
`file_never_appears`, `file_never_stable`, or `missing_milestone`.

The runner passes that value only to the spawned fixture process and removes
an inherited `AI_UI_TEST_ASYNC_FAILURE_MODE` first. It is not a generic
process-environment or command-argument mechanism, and the value is omitted
from evidence and reports.

## Next steps

- Use [GENERATOR_QUICKSTART.md](GENERATOR_QUICKSTART.md) for inventory,
  deterministic generated tests and coverage.
- Use [scheduler/README.md](scheduler/README.md) to schedule scripts/suites.
- Use [RELIABILITY.md](RELIABILITY.md) for verified actions, runtime health and
  evidence semantics.


## Trusted observation workflows

Use public observation operations when a workflow needs a bounded proof of
runtime task state, milestones/timeline, an external-request result, a trusted
file artifact, a visual assertion, logs, or trace events. Their inputs are
limited to the session telemetry node, named trusted roots, fixed observer
aliases, and session-owned evidence; they do not evaluate code or dispatch
arbitrary HTTP.

A successful extract object is also a declared producer: its named JSONPath
value may be referenced by later steps. when evaluates a declarative condition
over those variables. repeat_until polls only allowlisted read-only observation
operations and requires finite time/iteration bounds. Use public
screenshot_window, never the direct-host screenshot compatibility alias.

See [Trusted Observation & Async Workflows](OBSERVATION_WORKFLOWS.md) for the
telemetry contract, typed failures, fixture IDs, and runnable async examples.
The canonical verified workflow is
[async-observation-success.aiui.json](examples/async-observation-success.aiui.json).
wait_for_file proves existence only; use wait_for_file_stable for a quiet write
window. Prefer semantic assertions before visual assertions, and do not treat
OCR as the default semantic layer.
+
See [Trusted Observation & Async Workflows](OBSERVATION_WORKFLOWS.md) for the
telemetry contract, typed failures, fixture IDs, extraction handoff and runnable
async examples.
