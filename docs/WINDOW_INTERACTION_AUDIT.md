# Window frame and desktop interaction audit v1

Status: VERIFIED_STABLE

This is the completed real-Windows evidence audit for the generic Makepad
window-frame contract. It does not infer success from Rust implementation or
programmatic SetWindowPos alone: every marker below comes from saved physical
desktop evidence.

## Final outcome

The final smoke and full runner both passed on an unoccupied interactive
Windows desktop. The full run emitted `AI_UI_WINDOW_INTERACTION_V1_PASSED` with
1,629 assertions and zero failures.

| Evidence run | Result | Evidence |
|---|---:|---|
| Final smoke | `AI_UI_WINDOW_INTERACTION_SMOKE_COMPLETE`; 128 assertions, 0 failures | `prototypes/ai-ui-test-tool-auto-probe/reports/evidence/ai-ui-window-frame-smoke/summary.json` |
| Full physical run | `AI_UI_WINDOW_INTERACTION_V1_PASSED`; 1,629 assertions, 0 failures | `prototypes/ai-ui-test-tool-auto-probe/reports/evidence/ai-ui-window-frame/summary.json` |

The first diagnostic smoke exposed two generic implementation defects and one
strict multi-monitor gate. It was not accepted as a passing smoke. After the
specific generic fixes below, the final smoke passed and only then was the full
runner started.

## Full-run evidence

- 30/30 physical title moves.
- 80/80 edge-resize gestures: 20 cycles for each edge.
- 80/80 corner-resize gestures: 20 cycles for each corner.
- 60/60 programmatic maximize/minimize/normal state transitions and 40/40
  physical title-double-click maximize/restore transitions.
- 60/60 fresh semantic clicks after a move or resize. Every associated
  relative-click result had valid visible layout bounds.
- 301/301 title/frame targets were obtained from fresh `WM_NCHITTEST` evidence.
- 363/363 physical operations released buttons and modifiers and left the host
  input guard disarmed.
- 8/8 screenshots were non-blank and matched the active target window.
- 66/66 runtime-health reads reported a healthy Makepad process.
- The 40/65/85 percent size matrix, grow/shrink, attempted below-minimum
  resize, large resize, four-direction movement and near-work-area moves all
  completed in the full runner.

The primary full-run target was a session-owned `MakepadWindow` identity, not a
title lookup. Its HWND/PID/window-id and every pre/post bounds record are in the
full `summary.json`; each physical gesture captured fresh bounds and a fresh
semantic layout.

## Desktop and cross-monitor proof

Two monitors were present. The runner physically moved the primary Makepad
window from `HMONITOR(0x1007d)` to `HMONITOR(0x20001)`, refreshed the target
identity, DPI (96) and scale factor (1.0), completed a semantic click and
captured a non-blank screenshot at bounds `x=-1761, y=108, width=960,
height=700`. It then physically returned the window to
`HMONITOR(0x1007d)` at `x=481, y=226, width=960, height=700`.

The migration screenshot is:
`prototypes/ai-ui-test-tool-auto-probe/reports/evidence/ai-ui-window-frame/session_makepad_1/screenshot_0583.png`.

## Capability audit matrix

| Area / public operation | Final status | Real Windows proof |
|---|---|---|
| `windows_set_window_bounds` | VERIFIED_STABLE | used only for deterministic setup/recovery; actual bounds read back |
| `inspect_window_interaction` | VERIFIED_STABLE | fresh HWND/PID, monitor, DPI, state and frame data throughout the full run |
| `windows_drag_title` | VERIFIED_STABLE | 30 requested moves plus boundary, isolation and cross-monitor drags |
| `windows_resize_frame` | VERIFIED_STABLE | 20 cycles for every edge and corner with fresh target evidence |
| `windows_double_click_title` | VERIFIED_STABLE | 40/40 physical maximize/restore transitions |
| `windows_set_window_state` | VERIFIED_STABLE | 60/60 maximize/minimize/normal readbacks |
| post-move / post-resize semantic actions | VERIFIED_STABLE | 60/60 click plus semantic/runtime readback |
| screenshot refresh | VERIFIED_STABLE | 8/8 non-blank target-matched captures |
| popup after move / canvas after resize | VERIFIED_STABLE | lifecycle and spatial assertions passed |
| same-PID A/B isolation | VERIFIED_STABLE | each primary/secondary drag moved only its selected window |
| cross-process isolation | VERIFIED_STABLE | primary drag did not move the independent process target |
| fixed-size typed refusal | VERIFIED_STABLE | native fixed-size fixture returned `WINDOW_NOT_RESIZABLE` |
| physical cross-monitor migration | VERIFIED_STABLE | target monitor/DPI/semantic click/screenshot/return all passed |
| Windows Snap Layout | OUT_OF_SCOPE_V1 | intentionally not automated |

## Generic fixes required by the first smoke

1. `ProbeConfig::target_window_id` now binds a semantic snapshot to the
   intended top-level Makepad window. With multiple top-level windows the probe
   refuses ambiguous traversal order instead of silently exporting an auxiliary
   window's tree. Invoke targets are retained only for the selected subtree.
2. `windows_set_window_state(normal)` now performs a bounded, readback-driven
   second restore when a minimized maximized window first restores to its prior
   maximized state. This makes `normal` a literal verified result.
3. The runner now enumerates desktop monitors and, when more than one exists,
   performs a real title-drag migration, target DPI/scale refresh, semantic
   click, screenshot verification and safe return. It uses monitor work areas
   rather than fixed coordinates.

These fixes changed Toolkit/Auto-Probe code after the first smoke. They are
not Playground-specific and no Makepad vendor source was changed.

## Isolation, cleanup and reliability

- Same-PID primary and auxiliary windows passed bidirectional isolation.
- Cross-process isolation passed.
- Popup lifecycle after moving and canvas drag after resize passed.
- Fixed-size refusal passed without a coordinate workaround.
- Final cleanup closed the session, released the global input lease and left
  0 fixture processes, 0 stuck pointer buttons/modifiers and no target windows
  from the run. No screenshot or evidence failures were recorded.
- The successful full run had zero failed assertions; no scenario rerun was
  required after the passing smoke.

## Verification completed

| Check | Result |
|---|---|
| `cargo test -p toolkit-windows-automation` | PASS: 14 tests; 2 interactive tests intentionally ignored |
| `cargo test --lib` in Auto Probe | PASS: 102 tests |
| build of spatial demo and window-frame runner | PASS |
| final real Windows smoke | PASS: 128 assertions, 0 failures |
| final real Windows full runner | PASS: 1,629 assertions, 0 failures |

## Boundaries retained intentionally

- A custom application resize handle is not treated as a Windows non-client
  resize target; it needs its own semantic/spatial contract.
- The Makepad public configuration still has no portable Windows fixed-size
  flag. The generic native fixture is the proof for typed refusal.
- Windows Snap Layout, taskbar, virtual desktop and arbitrary window-manager
  automation remain outside v1 scope.