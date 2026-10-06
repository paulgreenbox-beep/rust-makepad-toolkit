# AI UI Test Tool — Backend API Classification

Status: CURRENT  
Updated: 2026-08-23  
Public Script API source: `prototypes/ai-ui-test-tool-auto-probe/operations.json` (104 operations)

Only names in `operations.json` are valid in `.aiui.json` files. The aliases below may exist in direct hosts for compatibility or diagnostics, but they are not promoted by this document. `PUBLIC_CANDIDATE` means intentionally withheld pending a stable public contract and real E2E evidence.

## Classification legend

- `PRIVATE_INTERNAL`: internal transport, host, or diagnostic helper; never emit in a Script API file.
- `LEGACY_COMPAT`: accepted compatibility spelling; use the listed public replacement.
- `DEPRECATED`: lifecycle compatibility only; use the public replacement.
- `PUBLIC_CANDIDATE`: deliberately not public; no Script API promotion is implied.

## Alias inventory

| Alias | Classification | Public replacement | Rationale |
| --- | --- | --- | --- |
| `session.resolve_window` | `PRIVATE_INTERNAL` | — | Internal session/window resolution supports public lifecycle calls and diagnostics. |
| `windows_find_window` | `PRIVATE_INTERNAL` | — | Internal Windows lookup is intentionally hidden behind public session targeting. |
| `wait_for_window` | `PRIVATE_INTERNAL` | — | Internal discovery helper; public session.open and wait_for_automation_id provide supported flows. |
| `list_test_windows` | `PRIVATE_INTERNAL` | — | Diagnostic enumeration is not a stable public application-testing contract. |
| `get_active_window` | `PRIVATE_INTERNAL` | — | Foreground diagnostics remain internal to avoid exposing ambient desktop state. |
| `ensure_active_window` | `LEGACY_COMPAT` | `activate_window` | Legacy spelling maps to the public activation operation. |
| `get_control_tree` | `LEGACY_COMPAT` | `inspect_ui` | Legacy tree name is covered by the public inspect_ui contract. |
| `wait_for_element` | `LEGACY_COMPAT` | `wait_for_automation_id` | Legacy element wait is covered by the public stable-automation-id operation. |
| `screenshot` | `LEGACY_COMPAT` | `screenshot_window` | Legacy screenshot alias maps to the public window capture operation. |
| `windows_list_automation_ids` | `PRIVATE_INTERNAL` | — | Internal probe diagnostic; inspect_ui exposes the stable semantic tree. |
| `windows_assert_automation_ids` | `PRIVATE_INTERNAL` | — | Internal probe assertion; public assertions operate on declared automation ids. |
| `windows_run_sequence` | `PRIVATE_INTERNAL` | — | Internal composition helper; public scripts provide the supported sequence format. |
| `launch` | `DEPRECATED` | `session.open` | Superseded by the managed public session lifecycle. |
| `restart_app` | `DEPRECATED` | `session.close` | No standalone restart API is promised; use a fresh managed session. |
| `shutdown` | `DEPRECATED` | `session.close` | Superseded by managed session teardown. |
| `get_logs` | `PRIVATE_INTERNAL` | — | Internal diagnostics are exported through evidence rather than a public log API. |
| `get_ui_changes` | `PRIVATE_INTERNAL` | — | Internal incremental observation helper; inspect_ui is the stable public read. |
| `windows_move_pointer` | `LEGACY_COMPAT` | `move_pointer` | Windows-prefixed legacy spelling maps to the public spatial operation. |
| `windows_pointer_down` | `LEGACY_COMPAT` | `pointer_down` | Windows-prefixed legacy spelling maps to the public spatial operation. |
| `windows_pointer_up` | `LEGACY_COMPAT` | `pointer_up` | Windows-prefixed legacy spelling maps to the public spatial operation. |
| `windows_click_relative` | `LEGACY_COMPAT` | `click_relative` | Windows-prefixed legacy spelling maps to the public spatial operation. |
| `windows_double_click_relative` | `LEGACY_COMPAT` | `double_click_relative` | Windows-prefixed legacy spelling maps to the public spatial operation. |
| `windows_right_click_relative` | `LEGACY_COMPAT` | `right_click_relative` | Windows-prefixed legacy spelling maps to the public spatial operation. |
| `windows_drag_relative` | `LEGACY_COMPAT` | `drag_relative` | Windows-prefixed legacy spelling maps to the public spatial operation. |
| `scroll_to` | `LEGACY_COMPAT` | `ensure_visible` | Legacy target-scroll behavior is represented by the public visibility operation. |
| `scroll_page` | `PRIVATE_INTERNAL` | — | Low-level paging helper lacks a portable postcondition contract. |
| `select_row` | `LEGACY_COMPAT` | `select` | Table row selection is represented by the generic public select contract. |
| `select_cell` | `LEGACY_COMPAT` | `select` | Table cell selection is represented by the generic public select contract. |
| `select_rows` | `LEGACY_COMPAT` | `select_many` | Multi-row selection is represented by the generic public select_many contract. |
| `select_cells` | `LEGACY_COMPAT` | `select_many` | Multi-cell selection is represented by the generic public select_many contract. |
| `read_cell` | `LEGACY_COMPAT` | `read` | Cell read is represented by the generic public read contract. |
| `activate_row` | `LEGACY_COMPAT` | `click` | Row activation is represented by the generic public click contract. |
| `activate_cell` | `LEGACY_COMPAT` | `click` | Cell activation is represented by the generic public click contract. |
| `expand` | `PUBLIC_CANDIDATE` | — | Potential future semantic-tree action, retained private until a stable public contract and E2E are defined. |
| `open` | `PUBLIC_CANDIDATE` | — | Potential future semantic-open action, not promoted by this remediation milestone. |
| `collapse` | `PUBLIC_CANDIDATE` | — | Potential future semantic-tree action, not promoted by this remediation milestone. |
| `close` | `PUBLIC_CANDIDATE` | — | Potential semantic close action; not equivalent to session.close and requires product definition. |
| `focus` | `LEGACY_COMPAT` | `focus` | Backend compatibility spelling has an exact public equivalent. |
| `close_window` | `PUBLIC_CANDIDATE` | — | Per-window close semantics differ from session.close and require a dedicated contract before promotion. |
| `hover` | `LEGACY_COMPAT` | `hover_relative` | Legacy hover is covered by the public relative-coordinate hover operation. |
| `wait_value` | `LEGACY_COMPAT` | `wait_for_value` | Legacy wait spelling maps to the public value wait contract. |

## Rule for scripts and agents

Use the public operation listed in the replacement column when one exists. Otherwise keep the need within the direct-host/debug boundary or propose a future public contract with schema, scheduler classification, docs, unit tests, and real Windows E2E evidence.

## Four-way reconciliation view
+
All 41 backend aliases have an explicit classification. The machine report
keeps precise PRIVATE_INTERNAL and PUBLIC_CANDIDATE source labels; this
normalized view maps both explicit non-public states to PRIVATE.
+
| Required class | Count | Source labels | Meaning |
| --- | ---: | --- | --- |
| PUBLIC | 0 | No backend alias | Public names come from operations.json; aliases are not promoted silently. |
| PRIVATE | 16 | PRIVATE_INTERNAL 11 + PUBLIC_CANDIDATE 5 | Direct-host/internal or deliberately withheld names. |
| LEGACY_COMPAT | 22 | LEGACY_COMPAT 22 | Compatibility spelling with a documented public replacement. |
| DEPRECATED | 3 | DEPRECATED 3 | Lifecycle compatibility only; use the replacement. |
+
The normalized total is 41, with zero unclassified aliases.
