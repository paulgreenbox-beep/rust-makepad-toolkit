---
name: makepad-virtual-canvas
description: Implement or review reusable Makepad VirtualCanvas behavior with explicit zoom, scroll, clip, overlay, coordinate transforms, visibility, and resize ownership. Use for large 2D surfaces, page canvases, maps, diagrams, and data-heavy visual workspaces.
---

# Name

Makepad Virtual Canvas

## Purpose

Keep viewport behavior deterministic and reusable across Makepad applications.

## Use when

Use for canvas state, transforms, scrolling, zooming, clipping, overlays, or viewport lifecycle.

## Do not use when

Do not use for a normal flow layout that needs no virtual coordinate system.

## Architecture invariants

- `VirtualCanvas` is the Makepad draw/event adapter.
- `ZoomViewport` owns exactly one zoom transform.
- `ScrollViewport` owns exactly one scroll offset and clamp.
- One host owns the effective clip/scissor.
- `OverlayLayer` uses the same logical-to-screen transform as content.
- Store geometry and visibility planning in pure Rust.
- Makepad owns drawing, events, focus, textures, and redraw requests.
- Transform order is `logical * zoom - scroll + viewport_origin`.
- Anchored zoom preserves the logical point under the anchor.
- Render only visible pages/items plus bounded overscan.

## Workflow

1. Define logical content bounds and viewport size.
2. Compute a pure layout/render plan.
3. Set scroll extents and clamp once.
4. Convert logical geometry to screen once at the renderer boundary.
5. Apply the host clip once.
6. Draw content, then overlays, then scroll indicators.
7. Emit viewport state changes without introducing a second owner.
8. Verify resize, 50/100/200% zoom, both-axis scroll, and visibility restore.

## Cheapest verification

`CHEAPEST_VERIFICATION_PATH`: L0 transform/clamp/range tests, L1 core tests, then L2 `virtual-canvas-demo`.

## Tools

Use `CanvasRuntime`, `ZoomViewport`, `ScrollViewport`, `RuntimeLayout`, debug snapshots, and the standalone canvas demo.

## Common failure modes

- Native scroll view and canvas both subtract scroll.
- Domain view state also applies zoom.
- Overlay uses global coordinates while content uses page-local coordinates.
- Clip is derived before the widget Walk resolves.
- Unbounded draw work despite bounded visible ranges.

## Examples

A diagram editor can use one large synthetic surface and the same zoom/scroll/overlay contract without importing document formats.

## Stop conditions

Stop if ownership cannot be reduced to one zoom, one scroll, and one clip owner, or if a new local canvas duplicates an existing Toolkit component without a documented incompatibility.
