# AI UI Local Scheduler + Worker Pool v1

Status: CURRENT
Updated: 2026-08-22
Contract/API version: Scheduler / ExecutionEnvironment v1

Current architecture, capabilities and limits are [PROJECT_MAP.md](../../../PROJECT_MAP.md), [CAPABILITIES.md](../../../CAPABILITIES.md) and [LIMITATIONS.md](../../../LIMITATIONS.md). Reproducible container instructions are in [container/README.md](container/README.md).

Historical milestone audit paths are redirects into archive; they are evidence, not operating instructions.

This crate orchestrates existing `.aiui.json` tests on local Windows workers and trusted disposable Container Level 0 workers. It is an orchestration layer, not another UI automation implementation.

Completed local-worker and ContainerWorker milestones are historical evidence. Current commands and limits are this README, [container/README.md](container/README.md), [CAPABILITIES.md](../../../CAPABILITIES.md) and [LIMITATIONS.md](../../../LIMITATIONS.md).

The invariant is that both backends cross the same public API boundary:

```text
script / suite / generated scripts
        -> Scheduler + Job Queue
        -> environment-matched worker
        -> LocalProcessEnvironment | ContainerEnvironment
        -> public `ai-ui-test run`
        -> existing Script API / Reliability / semantic / spatial layers
        -> existing reports and evidence
```

Worker code must not call `ReliabilityManager`, `WindowsAutomation`, low-level UIA, Makepad internals, or generator internals. `execution::verify_public_api_boundary`, `container::verify_container_public_api_boundary`, and `execution::verify_scheduler_environment_boundary` enforce the source boundaries. Makepad vendor sources are not changed.

## Layout

- `src/model.rs`: versioned `TestJob`, Run/Job/Attempt records, failure and worker contracts.
- `src/state.rs`: legal job-state transitions and retry decisions.
- `src/queue.rs`: deterministic priority + FIFO queue.
- `src/worker.rs`: worker registration, heartbeat, capacity, drain, and capability matching.
- `src/lease.rs`: renewable attempt leases.
- `src/resources.rs`: desktop-global and per-application resource accounting.
- `src/scheduler.rs`: orchestration state machine and event production.
- `src/execution.rs`: `ExecutionEnvironment` and the v1 `LocalProcessEnvironment`.
- `src/pool.rs`: local worker threads, heartbeats, cancellation, and backpressure.
- `src/submission.rs`: script, suite, and generated-script expansion through existing formats.
- `src/report.rs` and `src/persistence.rs`: aggregation and JSON/JSONL history.
- `src/e2e.rs`: real Windows proof harness.
- `src/environment.rs`: versioned environment requirements, capabilities, lifecycle, metadata and reports.
- `src/container_runtime.rs`: declarative Docker-compatible runtime contract and mock runtime.
- `src/container.rs`: disposable `ContainerEnvironment`, staging, artifact collection and owner-scoped cleanup.
- `src/mixed_e2e.rs` and `src/container_worker_loss_e2e.rs`: real mixed and crash/recovery proofs.
- `src/container_verification.rs`: strict final evidence, digest, regression and leak verifier.
- `examples/windows-e2e`: trusted app catalog and seven public Script API scenarios.

## Build

From `prototypes/ai-ui-test-tool-auto-probe`:

```powershell
cargo build -p ai-ui-test-tool-auto-probe --bin ai-ui-test --bin spatial-demo --bin ai-ui-advanced-demo
cargo build -p ai-ui-scheduler --bin ai-ui-scheduler
```

The scheduler binary and public runner are emitted beside each other under `target/debug`.

The Level 0 image is built from `container/Dockerfile` with an explicit named Makepad source context and pinned digest. See [container/README.md](container/README.md); no Makepad vendor patch is required.

## Trusted application boundary

A scenario stays declarative. Its `application.command` is an alias matched against a separate trusted scheduler catalog:

```json
{
  "version": 1,
  "applications": [
    {
      "key": "spatial-demo",
      "executable": "target/debug/spatial-demo.exe",
      "max_parallel": 1,
      "script_command_aliases": ["scheduler-fixture:spatial"]
    }
  ]
}
```

Only the trusted catalog supplies the executable. Scenarios cannot provide a runner, shell, PowerShell, `eval`, or arbitrary command line. `LocalProcessEnvironment` invokes only the fixed public runner as:

```text
ai-ui-test run <resolved-script.aiui.json> --output <attempt-dir>
```

Application arguments are rejected in v1 because the Public Script API does not yet carry them safely.

## CLI quickstart

Run one script, a suite, or a directory of generated scripts and wait for completion:

```powershell
target/debug/ai-ui-scheduler.exe run scheduler/examples/windows-e2e/pass-spatial.aiui.json `
  --applications scheduler/examples/windows-e2e/trusted-apps.json `
  --workers 2 `
  --working-dir . `
  --output reports/scheduler/my-run
```

Submit without starting workers:

```powershell
target/debug/ai-ui-scheduler.exe submit scheduler/examples/windows-e2e/suite.json `
  --applications scheduler/examples/windows-e2e/trusted-apps.json `
  --run-id run:local:001 `
  --output reports/scheduler/local-001
```

Resume a persisted submission:

```powershell
target/debug/ai-ui-scheduler.exe run reports/scheduler/local-001 --resume --workers 4 --working-dir .
```

Inspect persisted state:

```powershell
target/debug/ai-ui-scheduler.exe status reports/scheduler/local-001
target/debug/ai-ui-scheduler.exe workers reports/scheduler/local-001
```

Request cancellation from another terminal. Omit `--job` to cancel the whole run:

```powershell
target/debug/ai-ui-scheduler.exe cancel reports/scheduler/local-001 --job job:...
```

A cancelled terminal run intentionally returns a non-zero process status even though its summary is persisted correctly.


List trusted environments and check the live runtime:

```powershell
target/debug/ai-ui-scheduler.exe environments --environments scheduler/container/environment-catalog.json --check-runtime
```

Run a trusted Container Level 0 job:

```powershell
target/debug/ai-ui-scheduler.exe run scheduler/container/environment-probe.aiui.json `
  --applications scheduler/container/verification-trusted-apps.json `
  --environment docker-linux-level0 `
  --environments scheduler/container/environment-catalog.json `
  --output reports/scheduler/my-container-run
```

## Scheduling contract

- Larger numeric `priority` runs first; equal priorities are FIFO by enqueue sequence.
- Worker and resource capacity provide backpressure. Submitting 1000 jobs does not create 1000 processes.
- `windows.interactive_desktop` has capacity 1. Any physical-input or conservatively classified scenario holds it for the whole attempt.
- Per-application concurrency defaults to the trusted catalog's `max_parallel` (normally 1).
- Semantic-only jobs may run concurrently only when catalog analysis proves that every operation is safe. Unknown or fallback-capable operations are classified conservatively.
- Capability matching includes platform, interactive desktop, GPU, semantic UI, physical input, and Makepad support.
- Every lease identifies one job, worker instance, attempt, expiry, resources, and application slot. Heartbeats renew it.
- Worker loss preserves the old attempt as `WorkerLost`; an allowed retry creates a new attempt instead of overwriting history.
- `TestFailure` is not retried by default. `InfrastructureFailure` and `WorkerLost` may retry. Timeout retry is opt-in. Cancellation never retries.
- Non-idempotent work is not retried unless explicitly enabled.
- Capability matching also includes exact environment key/kind and minimum isolation. A GUI/physical job cannot be leased to the Level 0 container worker.
- Startup recovery expires a stale worker instance before a stable logical worker ID is registered again; an allowed retry always receives a new attempt and worker-instance identity.
- Job timeout terminates only the PID-owned public runner tree. No process-name-wide kill is used.
- Queued job, running job, and whole-run cancellation are supported. A running cancellation signals `ExecutionControl`, terminates the owned runner tree, and releases lease/resource/application allocations.

## Persistence and reports

A run directory contains:

```text
run-summary.json
run.json
jobs.json
attempts.json
workers.json
scheduler-state.json
scheduler-events.json
scheduler-events.jsonl
jobs/<safe-job-id>/attempt-001/
    resolved-script.aiui.json
    runner-process.json
    runner-stdout.log
    runner-stderr.log
    <script-name>/report.json
    <script-name>/ai-report.json
    <script-name>/final-report.json
    <existing evidence bundle files>
    environment-report.json
container-reaper-preflight.json
container-reaper-reconciliation.json
```

`run-summary.json` v2 stores counts and references, not copies of large reports. Container attempts additionally aggregate environment IDs, isolation modes, fingerprints, timings, failure codes and cleanup state. Attempts link to the existing Script API report and evidence paths. Scheduler events include enqueue, ready, lease, attempt start/finish, retry, worker registration/loss, resource acquire/release, and run completion.

## Verification

Pure and pool tests:

```powershell
cargo test -p ai-ui-scheduler
```

Scheduler-only scale proof:

```powershell
target/debug/ai-ui-scheduler.exe verify-stress --jobs 500 --workers 4 --output reports/scheduler/stress
```

Real Windows multi-application proof through the public runner:

```powershell
target/debug/ai-ui-scheduler.exe verify-windows --workers 2 --output reports/scheduler/windows-e2e
```

The Windows proof contains PASS, genuine `TEST_FAILED`, genuine `INFRASTRUCTURE_FAILED`, WorkerLost followed by requeue/new attempt, two physical jobs serialized by the desktop resource, and two proven semantic-only jobs on distinct workers. Markers are printed only after the harness validates persisted reports, events, released resources/leases, and absence of owned orphan processes.

Real mixed-environment and WorkerLost/reaper proofs:

```powershell
target/debug/ai-ui-scheduler.exe verify-mixed-environments --output reports/scheduler/my-mixed-proof
target/debug/ai-ui-scheduler.exe verify-container-worker-loss --output reports/scheduler/my-worker-loss-proof
```

Strict final verification of the current runtime/image and all saved evidence:

```powershell
target/debug/ai-ui-scheduler.exe verify-container --output reports/scheduler/container-e2e-verification-v1
```

The final command prints real-container markers only after validating PASS/TEST_FAILED/INFRASTRUCTURE_FAILED/TIMEOUT/CANCELLED, two-attempt isolation, mixed scheduling, WorkerLost startup recovery, mixed stress, critical regressions, source boundaries, pinned image digest and zero globally listed `ai-ui.tool=true` containers. Unsupported GUI markers are withheld.

## Known limits

- v1 has local-process and Docker-compatible disposable container backends. There is no network protocol, remote worker or VM backend.
- Container v1 proves Level 0 only on the active Docker Linux engine. It does not execute the Windows Makepad demos and does not provide GUI, UIA, physical input or independent Windows desktop isolation.
- Crash recovery deliberately does not reattach to an already-running container. The stale worker attempt becomes `WorkerLost`, the owner-scoped startup reaper removes its container, and a fresh attempt starts in a fresh container.
- One cancellation request file is supported at a time per run directory.

- Continuous high-priority arrivals can starve lower priorities; aging is intentionally deferred.
- Semantic analysis is intentionally conservative. A script that cannot be proven semantic-only consumes the interactive desktop resource.
- Desktop isolation models the current Windows interactive desktop as one resource; multi-session desktop partitioning is deferred.
- Each v1 `LocalWorker` executes one attempt at a time. More local workers provide parallel capacity.
- `ContainerEnvironment` and `LocalProcessEnvironment` share the same neutral contract. `RemoteEnvironment` and a separately proven independent-desktop backend remain future work.

These limits are deliberate v1 boundaries, not silently claimed capabilities.
## Observation jobs and resource classification
+
The catalog marks runtime task/milestone, trusted API, file postcondition,
visual assertion, trace and bounded script-control operations as non-physical.
A job containing only these operations does not reserve
windows.interactive_desktop solely because it observes state.
+
Live Makepad task/API telemetry, screenshots and visual diagnostics still
require a matching local Windows application environment. Container Level 0 can
run prepared non-GUI file/report and bounded Script API workflows. Physical or
unknown/fallback-capable operations remain conservatively serialized.
