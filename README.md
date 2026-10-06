# Rust Makepad Toolkit

Rust Makepad Toolkit is an AI-native testability, automation and verification layer for Makepad applications.

~~~text
PASS =
  action dispatched
  + state/postcondition verified
  + application healthy
  + required evidence
~~~

**A successful dispatch is not a PASS.** The application may report completion;
Toolkit independently checks the outcome, runtime health and required evidence.

This source release is version 0.1.0 under the [MIT License](LICENSE).
Windows is the current UI automation target. Individual capabilities and their
verification limits are described in [CAPABILITIES.md](CAPABILITIES.md) and
[LIMITATIONS.md](LIMITATIONS.md); publication is not a production-stability claim.

## Semantic UI for AI

~~~text
AutomationId = semantic UI API for AI

login.username
login.password
login.submit

find login.submit
-> verify visible/enabled
-> invoke
-> wait
-> verify state
-> runtime health
-> evidence
~~~

An application publishes stable IDs, roles, names, bounds and state, and routes
accessibility actions to its UI thread. The
[Windows accessibility bridge](docs/MAKEPAD_WINDOWS_ACCESSIBILITY.md)
provides that integration without inspecting private Makepad widget internals.

## What is included

| Area | Current surface |
| --- | --- |
| Semantic UI / UIA | Widget probe, generic Windows accessibility bridge, ID-addressed inspection, invoke, value and selection actions. |
| Verified actions | Bounded waits, explicit postconditions, runtime health and structured failures. |
| Mouse / keyboard / spatial | Shared-desktop input locking; relative click, drag_relative, wheel, region selection, hotkeys and text input. |
| Window interaction | HWND identity, bounds, DPI, monitor and frame targets; programmatic setup and separate physical gesture operations. |
| Screenshots / visual assertions | Target-window and region evidence, capture diagnostics and optional application visual telemetry. |
| Runtime / async / API observation | stderr, milestones, task state, logs/traces and fixed trusted observer aliases. No arbitrary HTTP execution in scripts. |
| File assertions | Named trusted roots; existence, size, hash, count and write-stability checks. |
| Evidence bundles | report.json, ai-report.json, report.md and session-owned artifacts. |
| .aiui.json Script API | Declarative public catalog and schema, validation, normalization and runner. |
| Generation / coverage | Deterministic inventory, templates, test plans and coverage reports. |
| Scheduler / Worker Pool | Queue, leases, resource classification, cancellation and retries; public runner boundary. |
| Execution | Local Windows workers and Docker/Linux Container Level 0 for supported non-GUI jobs. |
| MCP / Codex | Windows GUI stdio MCP server, local plugin source and focused reusable skills. |
| Reusable Rust / widgets | Typed runtime identities, pure geometry/layout/page primitives and Makepad widget adapters. |

Container Level 0 has no Makepad GUI, UIA, physical input, GPU, screenshots or
independent desktop. Local physical jobs share the interactive Windows desktop
and must be serialized. The Windows GUI MCP exposes its own 20 tools; the
.aiui Script API and Scheduler are separate public interfaces.

## Install and build

Requirements: Git, a current stable Rust toolchain and the platform build tools
required by [Makepad](https://github.com/makepad/makepad). Windows builds need
the MSVC C++ toolchain and Windows SDK. The Makepad dependency is fetched
separately; its source and assets are not redistributed in this repository.

~~~powershell
git clone https://github.com/paulgreenbox-beep/rust-makepad-toolkit.git
cd rust-makepad-toolkit
./tools/bootstrap-makepad.ps1
cargo check --workspace --locked
cargo test --workspace --locked
~~~

The bootstrap uses a pinned public upstream revision. An operator may instead
supply a compatible existing checkout with -MakepadPath. See
[third-party notices](THIRD_PARTY_NOTICES.md) before distributing a compiled
application or container with dependency code or bundled fonts.

## Run one verified workflow (Windows)

The included form demo uses only synthetic data and publishes stable IDs such
as window.main/body/form.main/input.name and
window.main/body/form.main/button.create.

From the repository root:

~~~powershell
Push-Location prototypes/ai-ui-test-tool-auto-probe
cargo build --locked --bin ai-ui-test --bin ai-ui-auto-probe-demo
cargo run --locked --bin ai-ui-test -- validate examples/public-form.aiui.json
cargo run --locked --bin ai-ui-test -- run examples/public-form.aiui.json
Pop-Location
~~~

The [public form script](prototypes/ai-ui-test-tool-auto-probe/examples/public-form.aiui.json)
opens the demo, inspects the tree, sets a name and reads it back, invokes the
button with a result postcondition, checks runtime health, captures evidence
and closes the session in teardown. Run UI jobs on an available interactive
desktop: the runner may activate its own test window and acquire physical input.

Generated results are local under reports/. Captures may contain application
data; review them before sharing. This release's rerun and skipped gates are
recorded in [the publication audit](docs/PUBLICATION_AUDIT.md).

## Other entry points

- [Quickstart](AI_UI_TEST_TOOL_QUICKSTART.md) and [agent guide](AI_UI_TEST_TOOL_AGENT_GUIDE.md).
- [Operation reference](OPERATION_REFERENCE.md), [operation catalog](prototypes/ai-ui-test-tool-auto-probe/operations.json) and [script schema](prototypes/ai-ui-test-tool-auto-probe/ai-ui-test-script.schema.json).
- [Generator / coverage](prototypes/ai-ui-test-tool-auto-probe/GENERATOR_QUICKSTART.md).
- [Scheduler and ai-ui-scheduler](prototypes/ai-ui-test-tool-auto-probe/scheduler/README.md), [Container Level 0](prototypes/ai-ui-test-tool-auto-probe/scheduler/container/README.md).
- [MCP plugin setup](plugins/rust-makepad-toolkit/README.md) and [Toolkit skill](skills/rust-makepad-toolkit/SKILL.md).

For the MCP server directly, build the dependencies once, then start it from
the checkout:

~~~powershell
cargo build --locked -p toolkit-windows-automation --bin toolkit-windows-gui-mcp
cargo run --offline --locked -p toolkit-windows-automation --bin toolkit-windows-gui-mcp
~~~

## Source layout and verification

The root workspace contains crates/, generic examples/ and the compatibility
TestHost fixture under prototypes/ai-ui-test-tool/. The current runner and
scheduler have their own workspace under prototypes/ai-ui-test-tool-auto-probe/.
That compatibility fixture remains required source.

See [architecture and trust model](docs/ARCHITECTURE.md),
[components](docs/COMPONENTS.md), [source map](PROJECT_MAP.md) and
[development gates](docs/DEV_LOOP.md). Release checks cover both Cargo
workspaces plus the documented boundary and contract gates. Historical local
audit summaries are dated evidence, not a guarantee for another machine.

## Roadmap / experimental

Linux / WSLg GUI execution, wslc, VM workers and independent desktops are
experimental or separate integration work. They are not advertised as fully
supported Toolkit backends. Media playback/recording observation, arbitrary
session attachment and a RemoteWorker are also outside the current verified
public surface.

The source package excludes local build caches, runtime state, provider
research, private consumer integrations and generated evidence. Optional
consumer adapters can use the public contracts without adding their private
implementations here.
