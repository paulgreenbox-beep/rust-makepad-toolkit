# AI UI Test Tool — Stability Report v2

Status: HISTORICAL / SUPERSEDED
Superseded by: OBSERVATION_STABILITY_REPORT.md

## Verdict

PASSED. All critical public operations have implementation, public exposure, real Windows E2E, repeat execution, cleanup proof and evidence proof.

## Repeat / soak evidence

| Evidence set | Pass / fail / flaky | Duration ms (min / mean / p95 / max) |
| --- | --- | --- |
| Historical full soak | 429 / 1 / 1 | 1076 / 5822.46 / 22306 / 22842 |
| Post-fix keyboard soak | 100 / 0 / 0 | 1941 / 2088.08 / 2768 / 2832 |
| Post-fix public API gap soak | 100 / 0 / 0 | 2865 / 3011.84 / 3130 / 3461 |
| Core action soak | 120 / 0 / 0 | 40448 / 46896.8 / 55203 / 55203 |

Accepted post-fix repeat portfolio: 700 clean runs (380 unaffected full-soak runs + 100 keyboard + 100 public API gap + 120 core actions).

## Cleanup and injected failure evidence

| Gate | Result | Evidence |
| --- | --- | --- |
| Remediation E2E | 8/8 suites passed; locks/leases/orphans/stale = 0/0/0/0 | reports/stability-audit-v1/remediation-e2e/20260822-230323/e2e-run.json |
| Failure injection | 5/5 passed; observed: APPLICATION_RUNTIME_FAILURE, EVIDENCE_CAPTURE_INJECTED_FAILURE, WAIT_TIMEOUT, WINDOW_NOT_FOUND | reports/stability-audit-v1/failure-injection-e2e/20260822-230414/failure-injection-run.json |
| Scheduler Windows E2E | jobs=7, passed=5, WorkerLost retries=1, max concurrent physical=1, orphans=0 | reports/stability-audit-v1/scheduler-windows-e2e/20260822-230506/windows-e2e-report.json |
| Container lifecycle | PASSED; owned containers remaining=0 | reports/stability-audit-v1/container-lifecycle-current/container-e2e-verification-report.json |

## Race conditions

| Scenario | Result |
| --- | --- |
| launch -> immediate inspect | PASSED — reports/stability-audit-v1/remediation-e2e/20260822-230323/e2e-run.json |
| click -> immediate readback | PASSED — reports/stability-audit-v1/soak/core-actions-soak-120.json |
| selection -> immediate readback | PASSED — reports/stability-audit-v1/physical-soak/20260822-214056/physical-soak-run.json |
| resize -> immediate spatial action | PASSED — reports/stability-audit-v1/physical-soak/20260822-214056/physical-soak-run.json |
| modal open/close | PASSED — reports/stability-audit-v1/physical-soak/20260822-214056/physical-soak-run.json |
| context-menu lifecycle | PASSED — reports/stability-audit-v1/physical-soak/20260822-214056/physical-soak-run.json |
| parallel semantic jobs | PASSED — reports/stability-audit-v1/scheduler-windows-e2e/20260822-230506/windows-e2e-report.json |
| serialized physical jobs | PASSED — reports/stability-audit-v1/scheduler-windows-e2e/20260822-230506/windows-e2e-report.json |

## Open non-blocking items

- P2-001 [OPEN_NONBLOCKING]: Generator templates explicitly cover 29 of 68 public operations; 39 are classified as GENERATOR_GAP.
- P3-001 [OPEN_NONBLOCKING]: Detailed prose examples are not yet available for every operation/argument/result combination.
- P3-002 [OPEN_NONBLOCKING]: Backend/private aliases need an explicit public/private reference table.

## Marker status

- AI_UI_FUNCTIONAL_AUDIT_PASSED: PASSED — All 68 public operations have real Windows E2E evidence and an audit status.
- AI_UI_OPERATION_COVERAGE_PASSED: PASSED — 68 of 68 catalog entries are VERIFIED_STABLE.
- AI_UI_STABILITY_SOAK_PASSED: PASSED — The post-fix evidence portfolio is clean: 380 unaffected full-soak runs, 100 keyboard runs, 100 gap-coverage runs and 120 core repeats.
- AI_UI_FAILURE_INJECTION_PASSED: PASSED — Five injected Windows failure scenarios pass classification, evidence and cleanup checks.
- AI_UI_CLEANUP_STABILITY_PASSED: PASSED — All final remediation, injection and soak artifacts report zero locks, leases, orphans and stale windows.
- AI_UI_CAPABILITY_CONSISTENCY_PASSED: PASSED — Every public operation is mapped across catalog, Script API, scheduler class, evidence and generator support; generator gaps are explicitly classified.
- AI_UI_STABILITY_AUDIT_PASSED: PASSED — Critical public operations meet implementation, public exposure, real E2E, repeat stability, cleanup and evidence criteria.