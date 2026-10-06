# ContainerEnvironment v1

Status: CURRENT
Updated: 2026-08-22
Contract/API version: ContainerEnvironment v1 / Level 0

[CAPABILITIES.md](../../../../CAPABILITIES.md) and [LIMITATIONS.md](../../../../LIMITATIONS.md) are the canonical human summaries; this document contains reproducible runtime commands.

`ContainerEnvironment` is the disposable Level 0 backend for the AI UI Scheduler. It executes the same fixed public entrypoint as a local worker:

```text
/opt/ai-ui/ai-ui-test run /work/tests/resolved-script.aiui.json --output /output
```

It does not expose a shell, an arbitrary executable, an image, or host mount selection to `.aiui` files. The Scheduler sees only `ExecutionRequest`, `ExecutionOutcome`, capabilities and lifecycle metadata; Docker details remain in the backend.

## Proven scope

- Docker Desktop Linux engine, `linux/amd64`.
- Process, filesystem and environment isolation.
- One disposable container per attempt.
- Read-only root filesystem, isolated tmpfs profile roots and `--network none`.
- Memory, CPU and PID limits.
- Public Script API outcomes: PASS, TEST_FAILED, INFRASTRUCTURE_FAILED, TIMEOUT and CANCELLED.
- Owner-scoped cleanup, startup reaper and WorkerLost recovery.
- Mixed `LocalProcessEnvironment` + `ContainerEnvironment` scheduling.

The current backend is `ContainerLevel0`. It does not prove or advertise a Makepad window, Windows UIA, physical input, GPU, an independent Windows desktop, or Windows-container isolation.

## Reproducible image build

The Dockerfile uses a named `makepad-src` build context so the Makepad dependency is copied from an explicit trusted checkout instead of patching vendor sources. From the Toolkit workspace root:

```powershell
$MakepadSource = 'C:\path\to\trusted\makepad'
$MakepadRevision = git -C $MakepadSource rev-parse HEAD

docker build `
  --file prototypes/ai-ui-test-tool-auto-probe/scheduler/container/Dockerfile `
  --build-context makepad-src=$MakepadSource `
  --build-arg MAKEPAD_REVISION=$MakepadRevision `
  --tag ai-ui/makepad-runner:container-v1 `
  .

docker image inspect ai-ui/makepad-runner:container-v1 `
  --format '{{ index .Config.Labels "org.opencontainers.image.makepad-revision" }}|{{.Id}}|{{.Os}}|{{.Architecture}}'
```

The verified image was built from Makepad revision `c1e3eae162efa314ae2eb42993a6a37fc4788ada` and has local content digest:

```text
sha256:7c178bb370b30fc6c1532146aada8117ffed526477181b1d1b0504d6e45ce098
```

After rebuilding, update `image_digest` in both trusted catalogs before running jobs. A digest mismatch fails environment health/preparation; it is never silently accepted.

The runtime stage runs as unprivileged UID/GID `10001`, fixes the entrypoint to `/opt/ai-ui/ai-ui-test`, and carries the Makepad revision as an OCI label.

## Trusted catalogs

- `environment-catalog.json` is the normal Level 0 catalog.
- `environment-e2e-catalog.json` adds trusted probe modes used only by verification.
- `trusted-apps.json` and `verification-trusted-apps.json` provide application aliases; scenarios cannot inject executable paths.

Static mounts are empty in the verified catalog. Per-attempt mounts are created internally:

```text
/work/tests       read-only staged script
/work/fixtures    read-only staged fixtures, when present
/output           per-attempt artifact output
/tmp and /run     per-container tmpfs
```

The source repository is not mounted into the runtime container.

## Commands

Build the public runner and scheduler from `prototypes/ai-ui-test-tool-auto-probe`:

```powershell
cargo build --locked --bin ai-ui-test
cargo build --locked --manifest-path scheduler/Cargo.toml --bin ai-ui-scheduler
```

Inspect the trusted environment and live runtime health:

```powershell
target/debug/ai-ui-scheduler.exe environments `
  --environments scheduler/container/environment-catalog.json `
  --check-runtime `
  --container-runtime docker
```

Run a trusted Level 0 probe:

```powershell
target/debug/ai-ui-scheduler.exe run scheduler/container/environment-probe.aiui.json `
  --applications scheduler/container/verification-trusted-apps.json `
  --environment docker-linux-level0 `
  --environments scheduler/container/environment-catalog.json `
  --container-runtime docker `
  --output reports/scheduler/my-container-run
```

Run the focused real proofs:

```powershell
target/debug/ai-ui-scheduler.exe verify-mixed-environments `
  --output reports/scheduler/my-mixed-proof

target/debug/ai-ui-scheduler.exe verify-container-worker-loss `
  --output reports/scheduler/my-worker-loss-proof
```

Validate the complete milestone evidence, current runtime and pinned image:

```powershell
target/debug/ai-ui-scheduler.exe verify-container `
  --output reports/scheduler/container-e2e-verification-v1
```

`verify-container` emits final container-worker markers only after all real runs, isolation evidence, WorkerLost/reaper evidence, mixed stress, critical regressions, source boundaries, current image digest and the final global owned-container leak audit pass. GUI-related markers are listed as withheld and are never printed.

## Cleanup model

Every runtime resource carries `ai-ui.tool=true` plus run/job/attempt/worker and owner-scope labels. Normal cleanup removes the exact attempt container. Preflight and reconciliation reapers list and remove only the configured owner scope. The final verifier performs a global read-only `ai-ui.tool=true` leak check; it does not remove foreign or unrelated containers.

If a scheduler process dies while its attempt container is running, the resumed pool first expires the stale worker instance as `WorkerLost`; the startup reaper removes the stale owner-scoped container, and a fresh worker instance executes a new attempt. Attempt, worker-instance and container identities remain distinct in history.
## Useful Level 0 scope
+
Container Level 0 is useful for file observation over staged trusted roots and
per-attempt output, report processing, and bounded non-GUI Script API workflows
such as validation, extraction, when, repeat_until and file assertions.
A fixed trusted observer can be used only when the image and environment catalog
explicitly provide it; the verified catalog currently has network: none and
exposes no such observer.
+
It must never be described as Makepad GUI, UIA, physical input,
screenshot/visual automation, GPU or independent-desktop support.
