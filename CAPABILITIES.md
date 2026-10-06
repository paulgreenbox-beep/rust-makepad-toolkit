# Current Capability Matrix

Status: CURRENT  
Updated: 2026-08-23  
Contract/API version: AI UI Test Tool v1 / ContainerEnvironment v1

The table reports only capabilities evidenced by source and saved current
reports. “Local Windows” requires an interactive Windows desktop; it is not a
claim about every platform.

| Capability | Local Windows | Container Level 0 | Evidence / boundary |
| --- | --- | --- | --- |
| Semantic UI tree (roles, names, bounds, state) | YES | NO | `probe.rs`, `reliability.rs`; container catalog sets `semantic_ui=false`. |
| Invoke, value, selection and verified waits | YES | NO | Script catalog + reliability evidence; no UIA in L0. |
| Physical pointer / drag / wheel / hotkeys | YES, serialized by desktop/input lock | NO | `operations.json` spatial/physical operations; L0 sets `physical_input=false`. |
| Spatial canvas / relative interaction | YES | NO | `spatial.rs`, spatial regression evidence; L0 has no Makepad/UI surface. |
| Windows UI Automation | YES | NO | `toolkit-windows-automation` and current Windows runners; L0 sets `uia=false`. |
| `.aiui` validation and public `run` | YES | YES, fixed runner only | `ai-ui-test run`; ContainerEnvironment invokes the same entry point. |
| Test generation / coverage | YES | NO | `generator.rs`, `workflow.rs`; L0 is a run-only worker, not a generator backend. |
| Scheduler / worker pooling | YES | YES | `LocalProcessEnvironment` and `ContainerEnvironment` share `ExecutionEnvironment`. |
| Process/filesystem/environment isolation | Process-bound local execution; no sandbox claim | YES | L0 evidence proves process, filesystem and environment isolation. |
| Network isolation | Not provided by the local worker contract | YES (`network: none`) | Trusted container catalog. |
| Independent desktop | NO | NO | Local physical input is desktop-global; L0 withheld desktop marker. |
| Makepad GUI / GPU | YES on a compatible Windows desktop | NO | L0 has `interactive_gui=false`, `gpu=false`, `makepad=false`. |
| RemoteWorker / network worker transport | NO | NO | Only a future enum/contract exists. |

Current Level 0 verification is recorded in
`prototypes/ai-ui-test-tool-auto-probe/reports/scheduler/container-e2e-verification-v1/container-e2e-verification-report.json`.
It explicitly withholds GUI, UIA, physical-input and independent-desktop
markers.


## Observation / async-workflow addendum (2026-08-23)

| Capability | Local Windows | Container Level 0 | Evidence / boundary |
| --- | --- | --- | --- |
| Trusted runtime/API/log/trace observations | YES | NO interactive-app proof | Resolved Makepad telemetry node and fixed external-observers.json; no arbitrary code or network dispatch. |
| Trusted file lifecycle, count, size, hash and stability assertions | YES | NO UI-session proof | Named workspace, output, fixture, temp roots only; paths outside them are rejected. |
| Screenshot visual assertions and Makepad visual diagnostics | YES | NO | Session-owned captures plus optional application telemetry; unavailable visual facts are reported as Unknown. |
| Generic Windows capture ownership/metadata | YES for native HWND targets | NO | `windows_capture_window` evidence carries backend, HWND/PID, dimensions, non-blank, fallback and diagnostics; Chromium/media proof requires a real owned client surface. |
| Audio/video/recording validation | NOT EXPOSED in v1 catalog | NO | No public media operations or Playground media client; no playback/recording marker is claimed. |
| Declarative extract, when, repeat_until controls | YES | YES, schema/runner only | No executable hook; repeat_until is bounded and read-only allowlisted. |
| Canonical `report.json` / `ai-report.json` / `report.md` output | YES | YES | Fixed public runner writes structured and compact reports; no step response bodies in `report.md`. |
| Canonical evidence, screenshot and report references | YES | Report ref only | Local results expose portable relative plus absolute metadata; L0 has no capture/session result. |
| Trusted HTTP or local-service observation | YES with a fixed trusted observer alias | NO | Current Level 0 profile is `network: none`; scripts cannot supply arbitrary URLs. |
| Attach / resume an arbitrary prior GUI session | NO | NO | Explicitly unsupported: [SESSION_ATTACH_RESUME_FEASIBILITY.md](SESSION_ATTACH_RESUME_FEASIBILITY.md). |

The local Windows rows require the same managed session and interactive
Makepad application as the core UI operations. A Level 0 container can validate
and run its supported non-GUI runner path, but it does not supply a Makepad
window, UIA tree, screenshots, or trusted application telemetry.
## Explicit observation matrix
+
| Capability | Local Windows | Container Level 0 | Boundary |
| --- | --- | --- | --- |
| Runtime task observation | YES, managed Makepad telemetry | NO live Makepad app/telemetry | Requires the resolved application telemetry node. |
| External/API observation | YES, fixed trusted observer alias/config | NO in the verified catalog | No arbitrary HTTP; the current L0 profile is network: none. |
| File assertions | YES, named trusted roots | YES, staged trusted roots and per-attempt output | Existence, not-exists, size, count, hash and stability are non-GUI contracts. |
| Visual assertions | YES, session-owned screenshots and diagnostics | NO Makepad GUI/screenshot proof | Unavailable visual facts are reported as Unknown. |
| Logs/traces | YES, bounded app telemetry | NO live Makepad telemetry; staged report/log processing only | Structured observations require secret redaction. |
| Limited workflow | YES, bounded declarative controls | YES, schema/runner controls only | extract, when and repeat_until have no eval or executable hooks. |
+
Container Level 0 is useful for file observation, report processing and
non-GUI Script API workflows. It does not supply a Makepad window, UIA tree,
physical input, GPU, screenshots or an independent desktop.

## Window-frame interaction addendum

| Capability | Local Windows | Container Level 0 | Evidence / boundary |
| --- | --- | --- | --- |
| Generic top-level frame inspection | IMPLEMENTED: live HWND/PID, physical pixels, DPI, monitor, state, native/custom frame and WM_NCHITTEST targets | NO | `inspect_window_interaction`; current physical E2E evidence is deferred until the desktop is unoccupied. |
| Physical title / edge / corner / title-double-click gestures | IMPLEMENTED, serialized by the shared `windows.interactive_desktop` lease | NO | `windows_drag_title`, `windows_resize_frame`, `windows_double_click_title`; bounds, layout, runtime and cleanup are verified after the gesture. |
| Owned same-PID HWND switching | IMPLEMENTED | NO | Passing `window_id` now binds the session's launched-process HWND before the operation; it cannot target an arbitrary external process. |
| Cross-monitor frame migration | PARTIAL | NO | Live monitor/DPI data exists. With one monitor the result is `NOT_TESTED_NO_SECOND_MONITOR`; with more, the audit runner refuses PASS until real migration coverage exists. |
| Windows Snap Layout | NOT IMPLEMENTED by v1 scope | NO | No taskbar, virtual-desktop or Snap flyout automation is claimed. |

Programmatic `windows_set_window_bounds` and `windows_set_window_state` are
setup/recovery only and never substitute for a physical desktop UX proof.