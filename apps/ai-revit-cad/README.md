# AI Revit CAD — Experimental

A Rust + Makepad BIM/CAD application with a unified project Document and
experimental structural, geological and foundation modules. This application
is an independent nested workspace in Rust Makepad Toolkit. It does not replace
or change the Toolkit workspace, tools or history.

The application source is MIT. Third-party code retains its own licenses;
see [LICENSE](LICENSE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
This is a source distribution, not a certified engineering product.

## Modules and current status

| Module | Available source surface | Status |
|---|---|---|
| BIM / Document | Levels, grids, walls, openings, slabs, project history, typed IDs, versioned Save/Open | Prototype; tests cover supported contracts |
| 2D CAD / Drafting | Drawing entities, selection/editing, dimensions/annotations, supported ASCII DXF subset | Experimental; no native DWG backend claim |
| Viewer / Geometry | Scene/camera, picking, mesh quantities, supported collision and section geometry | Experimental; private project assets excluded |
| GEO | Boreholes, intervals, IGE/material provenance, bounded TIN, geological sections and spatial queries | Partial; synthetic fixtures and limited interpolation, not site approval |
| GEO-SLOPE | Existing planar/circular dry-ground methods, Fellenius/Bishop and bounded critical search | Experimental / Partial; method-specific physical limits remain |
| STRUCTURAL | Bounded linear-elastic truss/frame core, typed loads, reactions, parameterized multi-section steel geometry | Partial; dense limits remain 64 nodes / 128 members; no full material-specific code compliance |
| FOUNDATION | Existing rigid raft / compression-only contact, explicit assumed bedding stiffness, source-bound reactions and ground context | Partial; spring displacements are preliminary estimates |
| Native UI / automation | Makepad 2D/3D source; manual app input | Experimental; public clean-build is separate from native acceptance |

Normative ULS bearing capacity, code-verified layered settlement, reinforcement,
punching, groundwater/pore pressure, piles and a complete building safety check
are **not** declared implemented. A positive numerical result is not engineering
approval. Synthetic geology is explicitly marked and does not represent a real
site. Mesh colors and visual triangles are not the source of calculation data.

### Public Windows accessibility boundary

The local development bridge depends on checked action tickets/lifetime fences
not exported by the public Toolkit baseline. This distribution explicitly does
not register that UIA provider. It does not alias checked operations to the
legacy bridge or weaken stale-object checks. UIA fixture modes are rejected;
manual Makepad input remains independent. Native, Remote and Linux GUI acceptance
are not inherited from any previous development snapshot.

Raw D3D11 device/texture access, GPU selection policy, viewport GPU export and
native GPU profiling also depend on SDK hooks absent from the public pin.
They explicitly return UNSUPPORTED_PUBLIC_SDK; adapter identity and VRAM remain
unknown. Required-GPU requests are not satisfied by a CPU fallback. The normal
Makepad draw path is unchanged; GPU/native qualification remains NOT_TESTED.

The nine proposed GEO/Foundation desktop agent routes are not registered in this
source snapshot. The ordinary app API is loopback-only; publication does not
start an application, service or API listener.

## Build

Requirements: Git, a current stable Rust toolchain with edition 2024 support,
and Makepad platform build tools. Windows needs MSVC C++ Build Tools and the
Windows SDK. Python 3.11+ is optional for source-bound engineering examples.

From the repository root (PowerShell):

```powershell
./tools/bootstrap-makepad.ps1
cd apps/ai-revit-cad
cargo build --locked -p ai_revit_desktop --bin ai_revit_desktop -j1
cargo test --locked --workspace --all-targets -j1 -- --test-threads=1
# Additional independent viewer-core tests:
cargo test --locked --manifest-path deps/viewer_core/Cargo.toml -j1 -- --test-threads=1
```

The repository bootstrap fetches the public Makepad revision
`105158e08c89d47f57af4621420ed68eb3a147ce` into the ignored root `.deps/makepad`.
Use a clean checkout of that revision for reproducibility. No operator-specific
folder, private SDK, submodule credential or project dataset is required.

For a source-bound GEO/Foundation analysis build:

```powershell
$env:AI_REVIT_GF_COMPILED_SOURCE_MANIFEST_SHA256 = python tools/source_fingerprint.py
cargo build --locked -p ai_revit_core --example geo_foundation_cli -j1
cargo build --locked -p ai_revit_desktop --bin ai_revit_desktop -j1
$env:AI_REVIT_GF_SOURCE_MANIFEST_SHA256 = $env:AI_REVIT_GF_COMPILED_SOURCE_MANIFEST_SHA256
$env:AI_REVIT_GF_EXECUTABLE_SHA256 = (Get-FileHash target/debug/ai_revit_desktop.exe -Algorithm SHA256).Hash
# Run manually when ready; not a native acceptance certificate:
./target/debug/ai_revit_desktop.exe
```

The fingerprint binds application source, lock and the pinned clean SDK revision;
it is not a full compiler/OS input trace. Source changes require rebuilding and a
new fingerprint. The CLI reads one bounded JSON request from stdin or a request
file. `demo_state` only constructs synthetic source data:

```powershell
'{"operation":"demo_state"}' | ./target/debug/examples/geo_foundation_cli.exe
```

See [the typed Core contract](crates/core/src/geo_foundation.rs) and
[the CLI request interface](crates/core/examples/geo_foundation_cli.rs). Analyses are
explicit operations. Open, Undo and Redo do not run a solver.

## Tests and distribution boundary

Small first-party synthetic inputs and independent high-precision numerical
references are included as test **source**. Large generated report archives are
not committed. Archive/persistence tests explicitly initialize fresh synthetic
fixtures in a new process-owned temporary directory, using the current solver
source identity. Their generated numbers are setup data, not independent goldens.
The separate historical-identity control verifies Stale behavior; it is not
proof from a historical solver run. Numerical golden tests remain separate.

Hardware CUDA tests require an explicitly selected supported NVIDIA adapter and
remain ignored without that opt-in. A Rust test or successful compile does not
prove native GUI, GPU, engineering or release acceptance.

Excluded: credentials/tokens, private configurations, user-profile paths, customer
and site materials, SAD RASTSVET materials, FH-87 project meshes/BREP/identity
maps, captures, historical acceptance bundles, local persistence-driver profiles,
SDK/vendor assets/fonts, executables, `target/`, caches and generated reports.
The generic mesh decoder and procedural synthetic viewer scenes remain available.
There is no DWG/STEP import/export promise without a real licensed backend.

See [PUBLICATION.md](PUBLICATION.md) for the checked public-source boundary and
actual build/test results. The original working checkouts and Toolkit history
are preserved.
