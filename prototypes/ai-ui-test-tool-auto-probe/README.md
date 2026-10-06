# Automatic Makepad UI Probe (Windows)

Status: CURRENT  
Updated: 2026-08-22  
Contract/API version: AI UI Test Tool v1

This is the current AI UI Test Tool workspace. It provides a public,
declarative Script API for Makepad UI testing on Windows, plus a Scheduler
that can run prepared scripts on LocalWorker or ContainerWorker Level 0.

The source map is [PROJECT_MAP.md](../../PROJECT_MAP.md). New agents should
start at [docs/INDEX.md](../../docs/INDEX.md), then use the root
[Quickstart](../../AI_UI_TEST_TOOL_QUICKSTART.md).

## Current layers

~~~text
operations.json + script schema
  -> .aiui runner and evidence contract
  -> semantic probe, reliability and spatial interaction on Windows
  -> inventory, generator and coverage
  -> Scheduler + LocalProcessEnvironment | ContainerEnvironment Level 0
~~~

- src/catalog.rs and operations.json define the Script API. The runner rejects
  unknown operation names.
- src/probe.rs, src/advanced.rs and src/reliability.rs provide semantic UI,
  verified interaction, Windows physical fallback and evidence.
- src/spatial.rs provides relative canvas coordinates, pointer, wheel, drag,
  region selection and capture.
- src/inventory.rs, generator.rs, templates.rs, coverage.rs and workflow.rs
  produce public-API inventory, test plans, .aiui scripts and coverage.
- scheduler/ owns job orchestration, worker pools and environments. Workers
  cross the public ai-ui-test run boundary only.

The neighboring prototypes/ai-ui-test-tool source remains a required
compatibility TestHost fixture; it is not the current implementation guide.

## Build and run

From the repository root:

~~~powershell
cargo build --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-test --bin spatial-demo
cargo run --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-test -- validate prototypes/ai-ui-test-tool-auto-probe/examples/spatial-smoke.aiui.json
cargo run --locked --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml --bin ai-ui-test -- run prototypes/ai-ui-test-tool-auto-probe/examples/spatial-smoke.aiui.json
~~~

The runnable examples are in examples/. Read [SCRIPT_QUICKSTART.md](SCRIPT_QUICKSTART.md)
before authoring a new script and [GENERATOR_QUICKSTART.md](GENERATOR_QUICKSTART.md)
before generating tests.

## Scheduler and ContainerWorker

See [scheduler/README.md](scheduler/README.md) for LocalWorker and scheduling
semantics. [scheduler/container/README.md](scheduler/container/README.md)
documents the verified Docker/Linux Container Level 0 backend.

Container Level 0 runs an already prepared .aiui file through the fixed public
runner. It does not provide Makepad GUI, UIA, semantic UI, pointer/keyboard
input, GPU or independent desktop isolation. The canonical capability and
limitation documents are [CAPABILITIES.md](../../CAPABILITIES.md) and
[LIMITATIONS.md](../../LIMITATIONS.md).

## Evidence

Generated reports stay local and are excluded from source publication.
Release checks are in [the publication audit](../../docs/PUBLICATION_AUDIT.md).
Reliability behavior and the evidence bundle contract are in
[RELIABILITY.md](RELIABILITY.md). No Makepad vendor source is patched by this workspace.
