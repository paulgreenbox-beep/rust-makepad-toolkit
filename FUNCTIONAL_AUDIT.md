# AI UI Test Tool — Functional Audit v2

Status: HISTORICAL / SUPERSEDED
Superseded by: OBSERVATION_FUNCTIONAL_AUDIT.md

Evidence-first verdict: PASSED. Rust implementation presence was not accepted as proof; every public operation below has cited real Windows E2E evidence.

## Outcome

| Metric | Value |
| --- | ---: |
| Public operations | 68 |
| Physical-input operations | 31 |
| VERIFIED_STABLE | 68 |
| Verified but flaky | 0 |
| Without real Windows E2E | 0 |
| Generator-supported operations | 29 |
| Generator gaps | 39 |

## Complete capability matrix

| Operation | Impl. | Public API | Docs | Unit | Integration | Windows E2E | Generator | Scheduler | Evidence | Stability status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| session.open | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| session.list | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| session.close | yes | yes | yes | yes | yes | yes | no | yes | yes | VERIFIED_STABLE |
| acquire_input_lock | yes | yes | yes | yes | yes | yes | no | yes | yes | VERIFIED_STABLE |
| release_input_lock | yes | yes | yes | yes | yes | yes | no | yes | yes | VERIFIED_STABLE |
| inspect_ui | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| inspect_semantic | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| inspect_spatial_target | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| read | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| set_value | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| input | yes | yes | yes | yes | yes | yes | no | yes | yes | VERIFIED_STABLE |
| click | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| focus | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| double_click | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| right_click | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| toggle | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| choose | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| select | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| add_to_selection | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| remove_from_selection | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| clear_selection | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| select_many | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| select_range | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| ensure_visible | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| scroll | yes | yes | yes | yes | yes | yes | no | yes | yes | VERIFIED_STABLE |
| scroll_to_start | yes | yes | yes | yes | yes | yes | no | yes | yes | VERIFIED_STABLE |
| scroll_to_end | yes | yes | yes | yes | yes | yes | no | yes | yes | VERIFIED_STABLE |
| wait_for_automation_id | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| wait_for_value | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| wait_for_state | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| wait_until | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| wait_visible | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| wait_hidden | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| wait_enabled | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| wait_disabled | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| wait_selected | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| wait_count | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| wait_idle | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| assert_visible | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| assert_interactable | yes | yes | yes | yes | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| assert_valid_bounds | yes | yes | yes | yes | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| assert_canvas_healthy | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| get_canvas_health | yes | yes | yes | yes | yes | yes | no | yes | yes | VERIFIED_STABLE |
| move_pointer | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| pointer_down | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| pointer_up | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| click_relative | yes | yes | yes | yes | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| double_click_relative | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| right_click_relative | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| hover_relative | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| wheel_relative | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| drag | yes | yes | yes | yes | yes | yes | no | yes | yes | VERIFIED_STABLE |
| drag_to | yes | yes | yes | yes | yes | yes | no | yes | yes | VERIFIED_STABLE |
| drag_relative | yes | yes | yes | yes | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| select_region | yes | yes | yes | yes | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| hotkey | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| press | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| type_text | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| activate_window | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| windows_set_window_bounds | yes | yes | yes | no | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| screenshot_window | yes | yes | yes | yes | yes | yes | no | yes | yes | VERIFIED_STABLE |
| screenshot_element | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| capture_region | yes | yes | yes | yes | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| windows_capture_region | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| get_runtime_health | yes | yes | yes | yes | yes | yes | yes | yes | yes | VERIFIED_STABLE |
| get_runtime_stderr | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |
| windows_export_evidence | yes | yes | yes | yes | yes | yes | no | yes | yes | VERIFIED_STABLE |
| windows_validate_evidence | yes | yes | yes | no | yes | yes | no | yes | yes | VERIFIED_STABLE |

Generator gap is non-blocking for runtime stability but applies to 39 operations: session.open, session.list, session.close, acquire_input_lock, release_input_lock, inspect_semantic, input, focus, double_click, add_to_selection, remove_from_selection, clear_selection, scroll, scroll_to_start, scroll_to_end, wait_until, wait_enabled, wait_disabled, wait_selected, wait_count, wait_idle, get_canvas_health, move_pointer, pointer_down, pointer_up, double_click_relative, right_click_relative, wheel_relative, drag, drag_to, hotkey, type_text, activate_window, screenshot_window, screenshot_element, windows_capture_region, get_runtime_stderr, windows_export_evidence, windows_validate_evidence.

## P0/P1 fixed and retested

- P0-001 [FIXED_AND_RETESTED]: Pointer/gesture cleanup could leave a physical input state after interrupted work. Proof: Final cleanup regression and all current soak/injection artifacts report zero leaked input locks and zero orphans.
- P1-001 [FIXED_AND_RETESTED]: Keyboard focus race could yield WINDOW_NOT_FOCUSED or unverified text input. Proof: Post-fix keyboard soak: 100/100, zero failures or cleanup leaks.
- P1-002 [FIXED_AND_RETESTED]: Saved operation-result fields were not usable as Script API variables. Proof: The runner now resolves save_as field paths; 61 unit tests and public API gap soak 100/100 pass.
- P1-003 [FIXED_AND_RETESTED]: The gap soak manifest used a report name different from the runner script name. Proof: Manifest corrected; 100/100 evidence now links final report.json correctly.
- P1-004 [FIXED_AND_RETESTED]: Runtime/visibility/screenshot failure classification and scroll/drag focus convergence had historical reliability defects. Proof: Fresh remediation 8/8 and failure injection 5/5 passed with zero cleanup failures.

## Markers

- AI_UI_FUNCTIONAL_AUDIT_PASSED: PASSED — All 68 public operations have real Windows E2E evidence and an audit status.
- AI_UI_OPERATION_COVERAGE_PASSED: PASSED — 68 of 68 catalog entries are VERIFIED_STABLE.
- AI_UI_STABILITY_SOAK_PASSED: PASSED — The post-fix evidence portfolio is clean: 380 unaffected full-soak runs, 100 keyboard runs, 100 gap-coverage runs and 120 core repeats.
- AI_UI_FAILURE_INJECTION_PASSED: PASSED — Five injected Windows failure scenarios pass classification, evidence and cleanup checks.
- AI_UI_CLEANUP_STABILITY_PASSED: PASSED — All final remediation, injection and soak artifacts report zero locks, leases, orphans and stale windows.
- AI_UI_CAPABILITY_CONSISTENCY_PASSED: PASSED — Every public operation is mapped across catalog, Script API, scheduler class, evidence and generator support; generator gaps are explicitly classified.
- AI_UI_STABILITY_AUDIT_PASSED: PASSED — Critical public operations meet implementation, public exposure, real E2E, repeat stability, cleanup and evidence criteria.