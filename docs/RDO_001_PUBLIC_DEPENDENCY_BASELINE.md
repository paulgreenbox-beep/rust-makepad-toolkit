# RDO-001: proposed public dependency baseline

This proposal starts from Toolkit public commit
`5ae780a75060f00e583500490782a6ebec35b595`. The reviewed Office source baseline
is `3cd18c4645e7bddd729644cb01f573d0dc597289`.

## Windows API

The Office app imports `AccessibilityAction`, `AccessibilityActionRequest`,
`AccessibilityActions`, `AccessibilityNode`, `AccessibilityRole`,
`AccessibilitySelection`, and `AccessibilityTree`. The launcher imports
`AccessibilityRect`, `AccessibilityActionHandler`, and
`WindowsAccessibilityBridge`, using `attach`, `update`, and `with_generation`.

Source review by the Office backend owner found these contracts already
present in the public Toolkit. There is no required Windows API delta in this
PR. This conclusion is source-level compatibility, not a fresh Windows build
or UI Automation runtime certificate.

The development-only experimental Makepad Remote implementation is a different
dependency surface. Office does not import that Rust feature directly. Its
router has linked process, Arbiter, WSLC and request-control ownership; copying
one router file would not provide a complete independent baseline. No part of
that implementation is included here, and this PR does not enable mutable MCP.

## Verification Core

The seven modules in `verification-economy/core-manifest.json` are the exact
existing Core 1.0.0 bytes required by Office. Their SHA-256 pins remain valid.
No validator, threshold, resource policy, acceptance history, or consumer
manifest is weakened. Existing pure tests and their five sample profile
dependencies are included, along with the unchanged full provenance test
source. No historical evidence or executable is published.

Local check of the proposed files: Node 24.22.1, 88/88 existing Core and
identity-capture tests, no skipped/cancelled tests, actual exit0 and empty
stderr. These source checks are not GUI or clean Office build acceptance.
The full synthetic WSLC provenance suite has no new PASS; its earlier combined
local run did not complete. Any consumer requiring it must run its own exact
gate and cannot substitute these 88 tests.

## Makepad revision boundary

The public Toolkit bootstrap currently proposes upstream Makepad
`105158e08c89d47f57af4621420ed68eb3a147ce`. It is a real upstream revision,
but it has not been qualified as the native Office SDK baseline by this PR.

The operator development dependency is a linked vendor directory with no
independent Git root. Asking Git for HEAD there traverses to another enclosing
repository. That enclosing commit is **not a Makepad revision**. No exact
upstream identity for those vendor bytes is asserted here. The bootstrap
default is not substituted for an observed vendor identity either.

Before an Office clean-build acceptance, the consumer owner must select a
real independent public Makepad checkout, freeze its commit and relevant byte
identity, review API differences, and validate the exact Cargo graph/lockfile
and Windows toolchain/SDK. No vendor edit or global SDK switch occurs in this
proposal. Do not re-label the separate pinned experimental Makepad71 runtime
as the Office production dependency.

## Review and limitations

This is a dependency-source PR for owner review, not an auto-merge, release,
production MCP promotion, or claim that RDO-001 is fully closed. No Cargo or
Office build was run here; the Office coordinator owns the independent clean
checkout and build checks. No source from Office or other products, user
documents, credentials, local settings, receipts or build artifacts is included.

The original dirty Toolkit checkout is preserved. The proposal publishes an
allowlist of generic Core sources and tests rather than a snapshot of that
checkout. Repository MIT licensing applies to these Toolkit files; Makepad is
not redistributed in this PR and retains its upstream licensing boundary.
