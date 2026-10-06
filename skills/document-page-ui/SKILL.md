---
name: document-page-ui
description: Build reusable paged UI with PageSurface, PageStack, rulers, mixed page sizes, visible ranges, overscan, page-local content, and pure render planning. Use for document-like viewers without coupling the UI layer to DOCX, PDF, slides, or spreadsheet semantics.
---

# Name

Document Page UI

## Purpose

Represent and render physical page surfaces independently from file formats.

## Use when

Use for page placement, margins, rulers, mixed sizes, visible ranges, overscan, and page-local primitives.

## Do not use when

Do not use it to parse formats, interpret Office semantics, or implement editing commands.

## Architecture invariants

- `PageSurface` contains generic ID, label, size, and margins.
- `PageStack` computes placement, content bounds, gaps, and natural size in pure Rust.
- Mixed page sizes center independently.
- Content coordinates are page-local; page placement is added once.
- Horizontal and vertical rulers derive from the active visible page geometry.
- Visible range is exact; overscan is bounded.
- Domain adapters create page/content primitives outside the Toolkit.
- Rendering must not instantiate one widget per page cell or glyph.

## Workflow

1. Convert physical units to one logical unit system.
2. Build page surfaces and margins without domain types.
3. Compute a `PageStackLayout` for the logical viewport.
4. Determine visible and overscan ranges.
5. Build generic text/table/image/rect items in page-local coordinates.
6. Render page background before content and overlay/rulers after content.
7. Verify A4, Letter, landscape, mixed sizes, gaps, resize, and zoom.

## Cheapest verification

`CHEAPEST_VERIFICATION_PATH`: L0 placement/range tests, L1 core, then L2 `document-ui-demo` for visual evidence.

## Tools

Use `PageSurface`, `PageStack`, `PageStackLayout`, rulers, `DocumentFrame`, and the synthetic document demo.

## Common failure modes

- Storing DOCX nodes or PDF page handles in generic pages.
- Applying margins in both adapter and renderer.
- Centering all pages using the first page width.
- Rendering all pages for a large document.
- Treating rulers as independent coordinate owners.

## Examples

A synthetic A4/A3/Letter stack proves page UI behavior without loading any document file.

## Stop conditions

Stop if a proposed primitive requires format-specific state; keep it in the domain adapter until a format-neutral contract is proven.
