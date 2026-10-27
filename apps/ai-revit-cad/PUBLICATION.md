# Public source distribution

The CAD package is isolated under `apps/ai-revit-cad/`. The original Toolkit
tracked tree, root workspace and history are preserved. This is not deployment
of the local SDK, a GUI acceptance certificate or engineering certification.

## Components

The source contains the unified BIM Document/history/persistence, geometry,
2D drafting and DXF subset, the independent viewer core, GEO data/queries,
existing GEO-SLOPE mathematics/reporting/jobs/host, structural math and
multi-section steel configuration, and rigid foundation contact integration.
GEO, FOUNDATION and STRUCTURAL remain **Experimental / Partial**.

The minimal vendored CADCraft DXF crate keeps its original MIT/Apache notices.
No DWG backend, private site model, font bundle or SDK tree is redistributed.
See [third-party notices](THIRD_PARTY_NOTICES.md) and
[exclusion rules](excluded-files.json).

## Reproducibility and limitations

- Separate CAD workspace and lock; Viewer is included as a dependency and has its own
  standalone workspace/lock. Both use the same audited registry versions.
- Makepad is a clean public checkout at
  `105158e08c89d47f57af4621420ed68eb3a147ce`, fetched by the unchanged repository
  bootstrap into ignored `.deps/makepad`.
- Application source is normalized to LF as required by the repository
  attributes. Numerical goldens were not replaced with solver output.
- Large archive fixtures are generated only by explicit test setup. Production
  project Open and formatting do not run a solver. A synthetic old-identity
  fixture tests Stale behavior; it is not historical solver evidence.
- Private checked UIA and raw D3D11 observation/readback/profiling hooks are not
  part of the pinned public SDK. The public boundary is explicitly unsupported;
  it does not invent GPU identity/VRAM or weaken required-GPU admission.
- Native 2D/3D workflows, GPU hardware qualification and normative engineering
  methods are not accepted by a successful compile or unit test.

## Validation

Verified on 2026-10-27 with Rust/Cargo 1.97.1, Windows x86_64 MSVC and the clean
public Makepad pin above. Cargo used the committed locks, one build job and one
test thread. Compiler warnings remain; no build or test failure remains.

| Check | Observed result |
|---|---|
| Desktop `cargo check` | PASS |
| Workspace, all targets | 953 passed, 0 failed, 1 hardware CUDA test ignored |
| Standalone Viewer | 26 passed, 0 failed |
| Ordinary desktop EXE and GEO/Foundation CLI build | PASS |
| Synthetic CLI lifecycle | 20 fresh processes, 82 assertions, 0 failures |
| Native 2D/3D and hardware GPU qualification | NOT_TESTED |

The CLI lifecycle checks explicit synthetic creation, structural/foundation
calculations, Save/Open in another process, editing/stale state, Undo/Redo and
invalid/unsupported input refusal. It does not launch the Makepad GUI.

Application source fingerprint:
`03b2ca295f0a747f92164f5f88a5dcad9e6a7ed1d62a4ccc25ffe1804d1e8030`.
The helper's declared scope excludes JSON fixtures and documentation; Git commit
identity covers the complete published tree. See [validation.json](validation.json)
for commands, counts and hashes of the locally built executables. Those hashes
identify this local build and are not a reproducible-build guarantee.

Local logs, executable files, process receipts and generated calculation archives
stay outside Git. No old GEO-SLOPE, Foundation or native acceptance is promoted by
this source publication.
