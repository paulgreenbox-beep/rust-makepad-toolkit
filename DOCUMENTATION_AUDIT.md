# AI UI Test Tool Documentation Audit

Status: HISTORICAL / SUPERSEDED
Superseded by: DOCUMENTATION_RECONCILIATION.md

Updated: 2026-08-22  
Contract/API version: AI UI Test Tool v1

## Source-of-truth order

| Subject | Canonical source | Checked implementation |
| --- | --- | --- |
| Script operation names and metadata | `prototypes/ai-ui-test-tool-auto-probe/operations.json` | `src/catalog.rs` embeds it; `src/script.rs` rejects unknown script operations. |
| Script structure | `ai-ui-test-script.schema.json` | `src/script.rs` schema and semantic validation. |
| Public runner commands | `src/cli.rs` | `ai-ui-test validate|normalize|dry-run|list|run|inventory|coverage|generate|audit`. |
| Scheduler commands/environment boundary | `scheduler/src/cli.rs`, `scheduler/src/execution.rs` | Scheduler invokes fixed public `ai-ui-test run` only. |
| Container capabilities | `scheduler/container/environment-catalog.json` | `scheduler/src/environment.rs`, `scheduler/src/container.rs`. |
| Human capability summary | `CAPABILITIES.md` | Current local and L0 evidence. |
| Human operating limits | `LIMITATIONS.md` | Current container catalog and scheduler source. |

## Findings and disposition

| Finding | Classification | Disposition |
| --- | --- | --- |
| `GENERATOR_QUICKSTART.md` was mojibake and could not serve an agent. | DOCUMENTATION_GAP | Rewritten as a current, UTF-8 generator guide. |
| Root README did not route to the isolated current Tool workspace, Script API, generator, Scheduler or L0 limitations. | DOCUMENTATION_GAP | Root README and `docs/INDEX.md` now provide one route. |
| Existing agent text mixed direct-host aliases (`wait_for_window`, `windows_find_window`, `windows_run_sequence`, etc.) with the `.aiui` catalog. | STALE_OPERATION / API_EXPOSURE_GAP | Canonical guide distinguishes the direct compatibility host from the stable Script API. |
| Physical relative pointer, wheel, drag and hotkey operations existed in `operations.json` but were not summarized as a documented Script API group. | API_EXPOSURE_GAP | Guide and quickstart now name the physical/spatial group and its Windows-only constraints. |
| Legacy prototype README said automatic widget-tree export was unavailable. The current Auto Probe now has a different runtime path. | STALE_EXAMPLE | Original description is archived; the old path becomes a compatibility redirect. |
| Completed local/container milestone audits looked current beside operational docs. | HISTORICAL_DOCUMENTATION | Archived with redirects; Project Audit, map, matrix and limitations are current. |

No API was added for documentation. The direct-host superset remains intact;
only the public-contract boundary is described accurately.

## Documentation checks

- `tools/check-markdown-links.ps1` validates local Markdown links outside build
  and dependency trees.
- `tools/verify-ai-ui-documentation.ps1` reads only the root README,
  `docs/INDEX.md` and the canonical quickstart, then checks that the eight
  required blind-read topics are explicitly present.
- An independent agent was not available in this workspace; the blind-agent
  requirement is therefore recorded as a deterministic documentation-only
  smoke test rather than a claim of independent human/agent review.

## Result

- Markdown link audit: 65 files, zero broken local links.
- Blind-read packet: root README, docs index and Quickstart explicitly cover
  Toolkit identity, a UI test, physical drag, .aiui, generation, Scheduler,
  Container Level 0 and evidence.
- Regression: current Tool check plus 48 library tests, Scheduler 47 tests,
  Script API validate/dry-run, and root workspace check/test passed.

Markers: AI_UI_DOCUMENTATION_AUDIT_PASSED,
AI_UI_DOCUMENTATION_CURRENT_PASSED, AI_UI_LINK_AUDIT_PASSED and
