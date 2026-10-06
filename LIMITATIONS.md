# AI UI Test Tool — Current Limitations

Status: CURRENT  
Updated: 2026-08-22  
Contract/API version: AI UI Test Tool v1 / ContainerEnvironment v1

This is the single canonical limitations document. Other README files link
here instead of restating a competing capability claim.

## Windows interaction

- Semantic actions are preferred. Windows physical input is available, but it
  is desktop-global and must be serialized with the input lock and foreground
  verification.
- Physical capability is real: the Script catalog includes relative click,
  double click, right click, hover, wheel, drag, region selection, hotkey,
  key press and text input. It is not available in Container Level 0.
- Screenshot capture can fall back from `PrintWindow` to visible `BitBlt` and
  is therefore not always occlusion-independent. Capture evidence now includes
  backend, HWND/PID owner, dimensions, non-blank validation, fallback usage,
  GPU-path inference and bounded diagnostics; it is still not Chromium proof.
- The public catalog currently has no media observation operations. Audio/video
  playback and recording state are Unknown/NotTested unless a real client and
  OS-level/output evidence are present; UI text such as `Playing` is not proof.
- Canvas health can report `Unknown` when a Makepad application has not
  published optional draw/clip/layout telemetry. `Unknown` is never Healthy.

## Script API versus direct host

- The stable `.aiui` vocabulary is exactly
  `prototypes/ai-ui-test-tool-auto-probe/operations.json` (version 1).
- The direct Windows hosts retain extra aliases such as window resolution and
  legacy sequence helpers. They are compatibility/debug surfaces, not valid
  `.aiui` or Scheduler operations unless added to the catalog in a future
  version.
- A script is declarative JSON. It cannot launch a shell, PowerShell, eval, an
  arbitrary binary or arbitrary arguments.

## ContainerEnvironment Level 0

- The verified backend is a Docker Desktop Linux `linux/amd64` container with
  one disposable container per attempt, a fixed public runner, no network and
  filesystem/environment/process isolation.
- It does not launch a Makepad window or provide Windows UIA, semantic UI,
  pointer/keyboard input, GPU, independent Windows desktop isolation, Windows
  containers or a RemoteWorker.
- It does not execute the test generator; it only runs an already prepared
  `.aiui` scenario through the fixed public `run` entry point.
- Crash recovery does not reattach to a running container. The stale attempt
  becomes `WorkerLost`, the owner-scoped reaper removes the owned container,
  and a fresh attempt runs in a fresh container.

## Application integration

- The generic Windows accessibility bridge requires an application to publish
  stable semantic snapshots and route action callbacks to the Makepad UI
  thread. It does not inspect private vendor internals or patch Makepad.
- The Auto Probe workspace can derive a Makepad widget inventory where its
  current runtime path is available; an application-specific production bridge
  still needs explicit lifecycle integration and feature gating.

See [CAPABILITIES.md](CAPABILITIES.md) for the positive counterpart and
[ContainerWorker Level 0](prototypes/ai-ui-test-tool-auto-probe/scheduler/container/README.md)
for reproducible L0 commands.

## Observation and session boundaries
+
- session.attach and session.resume are not implemented.
- There is no arbitrary process attachment; recovery starts with a fresh managed
  session and a new input lease.
- External/API observation uses trusted telemetry or fixed observer aliases. The
  Script API is not an arbitrary HTTP client; HTTP 200 and response_received do
  not imply application_accepted_response.
- File existence is not a completion proof. Use wait_for_file_stable before
  treating a generated artifact as finished.
- Semantic assertions are preferred. Visual assertions are appropriate when the
  rendered result matters; OCR is not a default replacement for semantics.
- Container Level 0 has no Makepad GUI, Windows UIA, physical input, visual
  automation or independent desktop. It can process trusted files, reports and
  bounded non-GUI Script API workflows.
- An independent desktop remains unavailable; the Windows interactive desktop is
  a serialized shared resource.

## Window-frame audit boundary

- `windows_set_window_bounds` and `windows_set_window_state` use Win32
  programmatic APIs for setup/recovery. They are not a user drag, edge/corner
  resize, or title-double-click proof.
- Real frame gestures require an unoccupied interactive desktop because they
  acquire the shared desktop input lease and temporarily move the pointer. The
  current audit implementation is intentionally marked
  `IMPLEMENTED_NOT_E2E_VERIFIED` until smoke and full soak evidence exist.
- Cross-monitor migration is not simulated. A one-monitor run records
  `NOT_TESTED_NO_SECOND_MONITOR`; a multi-monitor run cannot emit the final
  PASS marker until physical migration with fresh DPI/bounds readback is added.
- Windows Snap Layout, taskbar controls, virtual desktops and arbitrary OS
  window-manager automation remain outside v1.
- A custom app-drawn resize handle is distinct from the Win32 non-client frame
  and needs a separate semantic/spatial contract. The generic frame API does
  not infer app-specific handles or IDs.