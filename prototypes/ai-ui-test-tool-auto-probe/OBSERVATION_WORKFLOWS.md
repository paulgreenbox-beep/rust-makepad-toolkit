# AI UI Test Tool — Trusted Observation & Async Workflows

Status: CURRENT  
Updated: 2026-08-23

## Trust boundary

Observation operations consume only:

- JSON telemetry published by a Makepad accessibility node resolved from a stable AutomationId;
- session-owned screenshots and compact semantic trees;
- explicitly named trusted file roots: `workspace`, `output`, `fixture`, and `temp`;
- fixed observer aliases from `external-observers.json`.

No operation evaluates arbitrary code, dispatches arbitrary HTTP, or traverses outside trusted roots. The adapter resolves a short telemetry selector through the semantic tree before reading it, so a host receives the canonical AutomationId.

## Async semantics

`wait_for_api_response` proves only that a response was observed. `assert_api_success` additionally requires completed transport, a non-error status, and `application_accepted_response: true`.

Typed distinctions are preserved:

- `EXTERNAL_REQUEST_TIMEOUT`
- `EXTERNAL_REQUEST_FAILED`
- `EXTERNAL_RESPONSE_INVALID`
- `EXTERNAL_RESPONSE_NOT_ACCEPTED`

Task, file, trace, visual, and cleanup operations have similarly typed failures in [OPERATION_REFERENCE.md](../../OPERATION_REFERENCE.md).

## Script controls

- `extract` reads a JSONPath from a successful result and makes its named value available to later steps.
- `when` is a declarative condition over variables; it has no executable code branch.
- `repeat_until` polls only allowlisted read-only observation operations with explicit time and iteration limits.

Use `screenshot_window`, not the direct-host-only `screenshot` alias, in public scripts.

## Deterministic Makepad fixture

The advanced demo provides an opt-in, local fixture with fixed IDs:

- task: `fixture.async.workflow`
- request: `fixture.request.workflow`
- artifact: `fixture/async-output.json`
- UI trigger/result: `/button.async` and `/async.result`

For a self-contained `.aiui.json` scenario, set
`application.fixture_async_failure_mode` to `success`, `task_fails`,
`api_timeout`, `api_http_failure`, `api_invalid_response`, `api_not_accepted`,
`file_never_appears`, `file_never_stable`, or `missing_milestone`. The runner
passes the exact allowlisted value only to its child fixture and first removes
an inherited `AI_UI_TEST_ASYNC_FAILURE_MODE`; one test cannot contaminate the
next one. It is not a generic fault-injection, process-environment, or
command-argument interface, and fixture mode is omitted from evidence.

Runnable scenarios are in `prototypes/ai-ui-test-tool-auto-probe/examples/async-observation-*.aiui.json`. Every scenario includes no-pending-task, stale-window, evidence, and session-close cleanup checks.


## Extraction result shape

`extract` evaluates the successful operation result, not the outer runner response.
For a task waiter, the state is therefore at
`$.last_observation.task.state`, not `$.task.state`.

```json
{"extract":{"waiting_task_state":"$.last_observation.task.state"}}
```

## Reports and artifact references

Every public script run writes three artifacts in its output directory:

- `report.json` is the complete machine report;
- `ai-report.json` is the compact AI summary;
- `report.md` is the compact secret-safe handoff summary. It deliberately
  omits step response bodies.

Observation results keep `evidence_ref`; visual results additionally publish
`screenshot_ref`, and the script report publishes `report_ref`. A reference
uses a portable `relative_path` plus the legacy `resolved_path`. Local workers
also provide `absolute_path`. Container adapters may instead populate
`environment_path` and `host_artifact_path`; no `file://` URLs are generated.

## Container Level 0 boundary

The current verified Level 0 runner may parse bounded `extract`, `when`, and
`repeat_until` controls and emit the three report artifacts. It has no Makepad
window or UI session, so it does not claim runtime telemetry, UIA, screenshots,
visual assertions, or a UI-session file observation proof. Its network policy
is `none`, therefore the current trusted container catalog does not expose an
HTTP/local-service observer. A future image can add one only through a fixed
trusted adapter and explicit environment evidence; scripts still never choose a
URL.

## Semantic versus visual assertions
+
Prefer a semantic assertion when the contract is about state, value, role,
selection or invoke outcome. Use a visual assertion when the rendered result
itself matters or semantic coverage is insufficient. OCR is not a default
replacement for the semantic layer, and a screenshot is not element identity.
+
## Logs and traces
+
get_application_logs, wait_for_log, assert_log_contains,
assert_log_not_contains and assert_trace_event return bounded structured
observations. Keep selectors and retained fields allowlisted; redact tokens,
credentials and other secrets before evidence export. report.md omits response
bodies, while screenshot bitmap redaction is not automatic.
+
## Safe variable handoff
+
A successful result can produce named variables with supported JSONPath, and a
later step can consume them as \${name}. This schematic fragment is valid only
when the producer result contains the selected fields:
+
~~~json
{
  "extract": {
    "job_id": "$.job.id",
    "request_id": "$.request.id"
  }
}
~~~
+
Later arguments can use \${job_id} and \${request_id}; missing paths fail with
VARIABLE_EXTRACTION_FAILED. The verified fixture uses stable
fixture.async.workflow and fixture.request.workflow IDs, so this documents the
safe handoff mechanism without inventing dynamic IDs.
