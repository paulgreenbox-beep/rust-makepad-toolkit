# AI UI Test Tool — Observation Functional Audit v1

Status: **PASSED** — `AI_UI_OBSERVATION_LAYER_PASSED`

This is the versioned addendum to the historical 68-operation stability baseline. The public catalog now contains 104 operations: the historical 68 plus 36 observation operations. Every public operation has implementation, public exposure, documentation, scheduler classification, evidence, and repeated real Windows E2E coverage.

## Delivered domains

- Runtime tasks and milestones: typed task states, timeline and bounded waits/assertions.
- External/API requests: trusted telemetry state, typed failure taxonomy, and a clear distinction between response receipt and application acceptance.
- Filesystem postconditions: trusted roots, existence/absence, size/count/hash, and stability waits.
- Visual assertions: non-blank, changed/unchanged region, similarity and screenshot comparison.
- Trace/log assertions, safe extraction, `when`, bounded `repeat_until`, and no-pending/no-stale/no-phantom assertions.

The deterministic fixture publishes task state, milestones, API state, structured logs, UI result, and a fixture-owned output file. Scripts do not make arbitrary HTTP requests or read arbitrary filesystem paths.

## Coverage and consistency

| Item | Result |
|---|---:|
| Public operations | 104 |
| VERIFIED_STABLE | 104 |
| Flaky | 0 |
| Without real Windows E2E | 0 |
| New observation operations | 36 |
| Minimum direct E2E repeats for new operations | 20 |
| Script schema examples validated | 19/19 |
| Generated scripts validated | 24/24 |
| Generator classifications | 104/104 |
| Scheduler classifications | 104/104 |
| Required generator templates missing | 0 |
| Backend aliases classified | 41 |

The JSON Schema intentionally provides a generic `op: string` envelope; semantic validation is performed by the public Script API against the canonical `operations.json` catalog. `OPERATION_REFERENCE.md` contains 104 operation sections.

## Real Windows evidence

- The 100-run deterministic success flow includes task/API/milestone observation, safe field extraction, file appearance and stability, visual change/non-blank checks, trace assertion, health and cleanup.
- The same 100 runs satisfy the required 100 task, file-observation and variable-extraction iterations, and exceed the required 50 visual assertion iterations.
- Four mandatory 20-run failure soaks passed: task failure, API timeout, non-stable file and missing milestone.
- Edge operation coverage passed in 20 direct repeats: `get_task_state`, file absence/size/count/hash, log assertions, visual unchanged/similarity/compare, and phantom-node assertion.
- `assert_api_failure` passed in 20 direct repeats.
- The one-off failure matrix passed for HTTP failure, invalid response, application rejection, missing file and visual failure.

## P1 remediation proven

1. Fixture failure mode is scoped to the child process.
2. File failures wait for a terminal accepted API state before asserting success.
3. The unstable-file fixture transitions the API to accepted completion before it reaches its bounded failure.
4. `expect_error` is strict; an unexpected success no longer satisfies an expected error.
5. ScrollYView `ensure_visible` applies the exact needed wheel delta and passes the real Group-role regression.

No P0, P1, or P2 defects remain in the accepted final set. The only P3 item is intentional: session attach/resume remains feasibility-only.

## Boundaries

- `session.attach` / `session.resume` are not implemented; arbitrary user-process attachment is not allowed.
- External observation uses trusted aliases/config; scripts do not become an arbitrary HTTP client.
- Container L0 is suitable for non-GUI file/report/local-observer work. Makepad GUI visual and UIA telemetry stay Windows-only.

Machine-readable details: `observation-capability-audit.json` and `observation-stability-report.json`.
