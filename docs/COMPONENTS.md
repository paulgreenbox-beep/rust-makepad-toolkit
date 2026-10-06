# Components and maturity

Maturity values:

- `STABLE` — accepted contract with type/lifecycle tests and a real consumer.
- `REFERENCE` — proven implementation used by a real consumer; API may still be refined compatibly.
- `EXPERIMENTAL` — isolated and tested, but contract or fidelity is not mature.
- `DEPRECATED` — transitional compatibility surface; no implementation ownership.

| Component | Maturity | Origin | Toolkit source | Milestone / notes |
|---|---|---|---|---|
| `EntityId<T>` / `IdAllocator<T>` / `Registry<T>` | STABLE | `consumer/workspace/crates/rudra-runtime` | `crates/rudra-runtime` | `EPIC-RUNTIME-ID-001`; API preserved |
| Logical geometry | REFERENCE | `office-ui-runtime/src/geometry.rs` | `crates/makepad-toolkit-core/src/geometry.rs` | `EPIC-UI-RUNTIME-001`; pure Rust |
| `ZoomViewport` / `ScrollViewport` | REFERENCE | `office-ui-runtime/src/viewport.rs` | `crates/makepad-toolkit-core/src/viewport.rs` | One zoom/scroll owner; pure Rust |
| Visibility/frame state and `RuntimeLayout` | REFERENCE | `office-ui-runtime/src/viewport.rs` | `crates/makepad-toolkit-core/src/viewport.rs` | Pure render/layout plan |
| `PageSurface` / `PageStack` | REFERENCE | `office-ui-runtime/src/pages.rs` | `crates/makepad-toolkit-core/src/pages.rs` | Mixed sizes, gap, visible range, overscan tests |
| Horizontal/vertical rulers and editable markers | REFERENCE | `office-ui-runtime/src/rulers.rs`; editable markers added for document-editor consumer | `crates/makepad-toolkit-core/src/rulers.rs`, `crates/makepad-toolkit-widgets/src/virtual_canvas.rs` | Generic page-local markers, hit testing, drag actions, and accessibility snapshots; real reference consumer application consumer |
| `DocumentFrame` / `OverlayLayer` | REFERENCE | `office-ui-runtime/src/model.rs` (`OfficeFrame`) | `crates/makepad-toolkit-core/src/model.rs` | Office-neutral name in provider; alias only in consumer facade |
| Page text/paragraph primitives | EXPERIMENTAL | `office-ui-runtime/src/content.rs`, `text_layout.rs` | `crates/makepad-toolkit-core/src` | Unicode/mixed-run tests; renderer contract not yet standalone |
| Page table/image primitives | EXPERIMENTAL | `office-ui-runtime/src/content.rs` | `crates/makepad-toolkit-core/src/content.rs` | Demonstrated synthetically; fidelity is not stable |
| `VirtualCanvas` | REFERENCE | `office-ui-runtime/src/virtual_canvas.rs` | `crates/makepad-toolkit-widgets/src/virtual_canvas.rs` | Real reference consumer application consumer plus standalone demos |
| `toolkit-windows-automation` / `GuiToolRegistry` / `WindowsAutomation` | EXPERIMENTAL | New generic Toolkit component | `crates/toolkit-windows-automation` | ID-addressed UIA/Win32 automation, MCP, bounded evidence and generic fixtures |
| `office-ui-runtime` compatibility facade | DEPRECATED | reference consumer application local crate | reference consumer application facade re-exporting Toolkit | Transitional path compatibility only |

## Rich text status

No `rich-text-rendering` skill is published in this milestone. The current Makepad renderer uses real glyph preparation when available, but still contains a production character-count fallback and is embedded in `VirtualCanvas`. It is therefore `EXPERIMENTAL`, not a standalone stable renderer contract.
