# Verification Economy Core 1.0.0

This is the existing Toolkit planner and evidence verifier, published for
consumers that bind the exact Core 1.0.0 module bytes. It does not launch
applications, Cargo, desktop input or containers. It is not another runner or
validator implementation.

`core-manifest.json` records the seven dependency module hashes. A consumer
keeps its own `verification/toolkit-core.json`, policy, adapter and acceptance
history; publishing these files does not authorize rewriting those pins or
claiming a historical certificate for new sources.

The existing RudraOffice Core binding matches these seven files exactly.
Import `core/index.mjs` through the consumer's existing hash-checking adapter.
The package remains private to npm; this source publication is covered by the
repository's MIT license.

## Cheap source checks

Use Node.js 24:

```sh
node --test verification-economy/tests/core.test.mjs verification-economy/tests/identity-capture.test.mjs
```

These are synthetic planning/evidence tests. They do not establish GUI, GPU,
UIA, application lifecycle, WSLC, or production MCP acceptance. Profiles are
examples with synthetic scenarios and unknown consumer history, not approved
production launch profiles or accepted execution evidence.

The existing `tests/wslc-executable-provenance.test.mjs` is retained in full.
It is outside this narrow smoke command: a separate local combined provenance
run failed to complete and was interrupted, so this proposal claims no passing
result for that suite. Do not replace a required consumer provenance gate with
the 88-test source smoke.

## RDO-001 scope

This baseline makes the previously absent Core dependency publicly available.
It does not modify the already published Windows UIA bridge, add the
experimental Makepad Remote feature, change an MCP configuration, or qualify
an Office/Makepad build. See
[`docs/RDO_001_PUBLIC_DEPENDENCY_BASELINE.md`](../docs/RDO_001_PUBLIC_DEPENDENCY_BASELINE.md)
for the unresolved SDK identity and clean-build boundaries.
