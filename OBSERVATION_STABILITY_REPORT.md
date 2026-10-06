# AI UI Test Tool — Observation Stability Report v1

Status: **PASSED** — `AI_UI_OBSERVATION_LAYER_PASSED`

Accepted final evidence contains 247 passed script reports: 240 soak runs plus the final one-off/dedicated cases. Eleven failed reports from earlier, pre-fix diagnostics are retained separately as defect evidence and are not included in the accepted set.

## Soak results

| Set | Passed | Min / Mean / P95 / Max |
|---|---:|---|
| Async success | 100/100 | 7.99 / 9.01 / 9.41 / 9.57 s |
| Task fails | 20/20 | 3.91 / 4.15 / 4.31 / 4.45 s |
| API timeout | 20/20 | 6.08 / 6.46 / 6.63 / 6.72 s |
| File never stable | 20/20 | 9.36 / 9.66 / 9.84 / 10.14 s |
| Missing milestone | 20/20 | 7.81 / 7.98 / 8.12 / 8.20 s |
| Edge observation | 20/20 | 8.23 / 8.65 / 8.84 / 8.85 s |
| assert_api_failure | 20/20 | 5.81 / 5.99 / 6.13 / 6.25 s |
| Edge + direct get_task_state | 20/20 | 7.72 / 8.11 / 8.44 / 8.60 s |

Durations are derived from evidence-directory creation through `report.json` final write.

## Failure injection

All mandatory 20-repeat injections passed with consistent typed errors:

- `TASK_FAILED`
- `EXTERNAL_REQUEST_TIMEOUT` + `TASK_FAILED`
- `FILE_NOT_STABLE` + `TASK_FAILED`
- `RUNTIME_MILESTONE_TIMEOUT` + `RUNTIME_MILESTONE_NOT_FOUND`

The final one-off matrix also passed for `EXTERNAL_REQUEST_FAILED`, `EXTERNAL_RESPONSE_INVALID`, `EXTERNAL_RESPONSE_NOT_ACCEPTED`, `FILE_NOT_FOUND`, and `VISUAL_REGION_UNCHANGED`.

## Cleanup and regression gates

- Orphan test processes: 0
- Leaked input locks: 0
- Leaked leases/resources: 0
- Stale windows: 0
- Screenshot failures: 0
- Evidence consistency failures: 0

Fresh regression gates: auto-probe Rust 87+3 tests, scheduler 48 tests, Script API 19/19, generator 24/24, advanced runner 14/14, reliability runner 16/16, spatial required markers, and retained real container lifecycle E2E with zero owned containers left.

The historic 68-operation baseline remains preserved in `stability-report.json`; this versioned addendum records the 104-operation observation extension.
