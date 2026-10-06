# Current Project Map

Status: CURRENT  
Updated: 2026-08-22  
Contract/API version: Toolkit workspace + AI UI Test Tool v1

The repository contains a reusable Makepad Toolkit and an isolated current AI
UI Test Tool workspace. They deliberately have different Cargo-workspace
boundaries.

```text
Rust_Makepad_Toolkit/                         CURRENT repository root
├── crates/                                   CURRENT reusable Toolkit crates
│   ├── rudra-runtime/                        typed runtime identity
│   ├── makepad-toolkit-core/                 pure Rust geometry/layout
│   ├── makepad-toolkit-widgets/              Makepad adapters
│   └── toolkit-windows-automation/           Windows UIA/Win32 bridge + evidence
├── prototypes/ai-ui-test-tool-auto-probe/    CURRENT AI UI Test Tool workspace
│   ├── src/                                  public Script API and Windows layers
│   ├── scheduler/                            scheduler, workers, environments
│   ├── examples/                             runnable `.aiui.json` fixtures
│   └── reports/                              generated AI UI evidence
├── prototypes/ai-ui-test-tool/               SUPPORT compatibility TestHost fixture
├── reports/                                  generated Toolkit evidence
├── docs/                                     CURRENT documentation entry point
├── archive/                                  HISTORICAL documents only
└── target/                                   GENERATED, rebuildable Cargo artifacts
```

## AI UI Test Tool layers

| Layer | Source | Public/current role |
| --- | --- | --- |
| Core contracts | `src/catalog.rs`, `contract.rs`, `script.rs` | `operations.json` is the versioned Script API truth; schema plus semantic validation define `.aiui.json`. |
| Semantic Probe | `src/probe.rs`, `src/advanced.rs`, `src/reliability.rs` | Windows semantic tree, roles, names, bounds, state and action routing. |
| Interaction | `src/reliability.rs`, `src/advanced.rs` | Verified semantic actions; Windows-only physical fallback with foreground/input controls. |
| Spatial | `src/spatial.rs`, `src/advanced.rs` | Relative canvas points/rectangles, pointer, drag, wheel, region selection and capture. |
| Reliability | `src/reliability.rs` | Session/window identity, health checks, postconditions, waits, evidence and input locking. |
| Evidence / contract integrity | `src/contract.rs`, `src/runner.rs` | Canonical response/evidence validation and script reports. |
| Public Script API | `src/cli.rs`, `src/api.rs`, `src/sdk.rs`, `src/runner.rs` | `ai-ui-test validate|normalize|dry-run|list|run|inventory|coverage|generate|audit`. |
| `.aiui` DSL | `src/script.rs`, `ai-ui-test-script.schema.json` | Declarative JSON only; no shell, PowerShell, eval or arbitrary code. |
| Test Generator / Coverage | `src/inventory.rs`, `generator.rs`, `templates.rs`, `coverage.rs`, `workflow.rs` | Public-API inventory, deterministic planning, generated scripts and coverage reports. |
| Scheduler | `scheduler/src/scheduler.rs`, `queue.rs`, `state.rs`, `report.rs` | Persistent jobs, leases, resources, retry policy and JSON/JSONL reports. |
| Worker Pool | `scheduler/src/pool.rs`, `worker.rs`, `submission.rs` | `LocalWorker` pool, capability matching, cancellation and backpressure. |
| Local execution | `scheduler/src/execution.rs` | `LocalProcessEnvironment` invokes only public `ai-ui-test run`. |
| Container execution | `scheduler/src/container.rs`, `container_runtime.rs`, `environment.rs` | Disposable `ContainerEnvironment` Level 0; only public `ai-ui-test run`. |
| Future environments | `scheduler/src/environment.rs` | `Remote` is a contract enum only; no RemoteWorker/backend is implemented. |

## Boundary rules

1. `.aiui` scripts, generated tests and the Scheduler use only names in
   `operations.json` through the public runner.
2. `ReliabilityManager` and `AdvancedHost` expose extra direct-host aliases for
   compatibility/debugging. They are not the `.aiui` contract and must not be
   used to bypass the runner or Scheduler.
3. `prototypes/ai-ui-test-tool/src/host.rs` is a compatibility dependency of
   the current workspace and a root Cargo member. It is SUPPORT, not dead code;
   moving it would break the current source path.
4. Makepad vendor remains an ignored upstream dependency under `.deps/makepad`.
   No current AI UI code patches it.
