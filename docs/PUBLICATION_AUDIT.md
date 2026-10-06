# Source publication audit — 2026-10-06

Scope: Rust Makepad Toolkit source repository, MIT, version 0.1.0. No binary,
container-image, third-party asset or private consumer distribution is included.

## Public composition

The public tree contains the four Toolkit crates, generic examples/tests,
both required prototype source trees, the Scheduler, public schemas/catalogs,
reviewed documentation, skills and MCP plugin source. Generated targets,
runtime state, reports, captures, local archives and provider research are
excluded. No neighboring project source is included.

Private consumer paths/names were generalized in public documentation. The
Makepad bootstrap obtains a pinned public dependency instead of relying on
a private sibling checkout. MCP configuration has no committed machine path.

## Git and security

At audit start, main was unborn, with no tracked files, commits or remote.
Pre-existing unreferenced dependency Git objects are retained locally and
will not be ancestors of the public main branch. No history rewrite is needed.

Gitleaks 8.30.1 was downloaded from its official release and its published
SHA-256 checksum verified. The owned working tree (including local reports,
excluding builds/dependency source and the scanner's own files) and all
available existing Git objects reported zero credential findings. Pattern
checks also review private paths, internal addresses and excluded formats.
The exact committed public tree and its reachable history are scanned again
before push. No credential rotation was indicated by these results.

Copyright holder is the authenticated GitHub owner, paulgreenbox-beep.
Own Cargo manifests use MIT. [Third-party notices](../THIRD_PARTY_NOTICES.md)
describe the separate dependency/font redistribution boundary.

## Verification

Release gates are being completed before public push. The final source commit
will record the exact passed, failed and skipped checks below.

| Gate | Result |
| --- | --- |
| Root workspace cargo check/test --workspace --locked --offline | PASS; 56 tests passed, 0 failed, 2 interactive integration tests ignored |
| Auto Probe + Scheduler cargo check/test --workspace --locked --offline | PASS; 154 tests passed, 0 failed |
| Owned-package cargo fmt --check | PASS after mechanical formatting of two owned source files |
| Boundary / documentation / plugin static gates | PASS |
| Public Markdown links | In progress |
| Public .aiui example validation | PASS; schema + semantics, 12 steps |
| Fresh source clone + public dependency bootstrap | In progress |
| Fresh clone build/tests | In progress |
| Final public tree / Git history security scan | In progress |
| Interactive Windows physical / GUI / container soak | SKIPPED in this publication task |

Interactive UI and container runs are not inferred from compilation or unit
tests. Historical capability summaries remain dated local-audit results;
this publication does not issue new physical, Linux, WSLg, wslc or media PASS
markers. No Makepad vendor source was patched.

The default public Makepad revision is
105158e08c89d47f57af4621420ed68eb3a147ce. Stable Rust is selected by the
toolchain file. The tested toolchain is rustc 1.97.1 (8bab26f4f) and
Cargo 1.97.1 (c980f4866), Windows x86_64 MSVC. Existing upstream and unused
helper compiler warnings are retained; they are not test failures.
