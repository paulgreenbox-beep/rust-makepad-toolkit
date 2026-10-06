---
name: makepad-visual-debug
description: Diagnose Makepad UI that exists in state or tests but is invisible, clipped, misplaced, or drawn under another layer. Use for zero-size views, missing canvases, wrong coordinates, invalid clips, draw lifecycle failures, or z-order regressions.
---

# Name

Makepad Visual Debug

## Purpose

Find the first invalid runtime stage before changing geometry or rendering logic.

## Use when

Use when model state exists but the expected pixels do not appear correctly.

## Do not use when

Do not use for document parsing or fidelity problems already visible inside a correct surface.

## Architecture invariants

Trace in this order:

```text
parent layout
-> host rect
-> widget Walk
-> viewport rect
-> clip rect
-> draw_called
-> z-order
-> content rect
```

- Do not change page geometry until the first invalid runtime stage is identified.
- Use one coordinate space per value and label it.
- Do not add offsets twice or subtract scroll twice.
- Parent clip ownership wins over child overflow.
- Remove temporary debug rendering after the root cause is verified.

## Workflow

1. Reproduce with one static rectangle before domain content.
2. Add a temporary overlay/status panel showing host, viewport, clip, walk, zoom, scroll, draw count, z-order assumptions, and content rect.
3. Draw a distinct frame and label for every region.
4. Compare actual screen coordinates with expected coordinates.
5. Mark each stage valid/invalid and stop at the first invalid stage.
6. Fix only that owner or transform.
7. Verify resize and representative zoom levels.
8. Remove overlay and rerun the original reproduction.

## Cheapest verification

`CHEAPEST_VERIFICATION_PATH`: L0 geometry/transform test, then L2 isolated visual demo. Build a consumer only when the defect is consumer-specific.

## Tools

Use debug outlines, `Area::clipped_rect`, resolved `Walk`, clip/scissor inspection, draw counters, screenshots, and focused pure tests.

## Common failure modes

- Editing page dimensions to compensate for a zero-height host.
- Assuming state selection means the widget is visible.
- Ignoring an overlay or previous preview above the canvas.
- Using global coordinates as page-local coordinates.
- Leaving diagnostics in production output.

## Examples

If page placement is valid but `viewport.w == 0`, fix the host layout. Do not rescale A4.

## Stop conditions

Stop and report insufficient evidence if the first invalid stage cannot be observed. Report failure if the final physical surface is still not visibly correct.
