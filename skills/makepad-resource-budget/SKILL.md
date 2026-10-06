---
name: makepad-resource-budget
description: Audit and bound Makepad rendering work, caches, textures, images, inactive state, and memory ownership. Use for large documents, grids, canvases, scroll performance, cache policies, memory growth, or distinguishing process RSS from resources actually owned by an application.
---

# Name

Makepad Resource Budget

## Purpose

Keep data-heavy Makepad applications responsive with explicit resource ownership and bounds.

## Use when

Use when a component can scale with pages, cells, images, glyphs, or open sessions.

## Do not use when

Do not optimize from process RSS alone without measuring owned resources and workload.

## Architecture invariants

- Render visible content plus a small bounded overscan.
- Avoid widget-per-cell, widget-per-glyph, or widget-per-large-item designs.
- Every cache has an owner, key, maximum size, and eviction policy.
- Texture/image caches release entries when documents or sessions close.
- Heavy inactive state is unloaded or bounded explicitly.
- Distinguish allocator/runtime/GPU RSS from application-owned heap and caches.
- Measure before optimizing; keep correctness tests for every bound.

## Workflow

1. Identify resources whose count grows with input.
2. Record owner, lifetime, upper bound, and release event.
3. Instrument visible range, cache count, texture bytes where available, and close behavior.
4. Reproduce representative scroll/zoom/open/close workloads.
5. Fix the first unbounded owner.
6. Verify hit behavior and eviction do not corrupt visible output.

## Cheapest verification

`CHEAPEST_VERIFICATION_PATH`: pure visible-range/cache-policy tests, then one isolated rendering workload; profile a full consumer only when ownership crosses the adapter boundary.

## Tools

Use bounded collections, draw counters, cache inspection, process and heap profiling, and repeated open/close smoke tests.

## Common failure modes

- Calling all process RSS an application leak.
- Keeping decoded images for every page.
- Rendering all rows because scrolling is clipped visually.
- Evicting active resources without redraw/reload handling.
- Optimizing Makepad vendor before proving the application owner.

## Examples

An image cache capped at 12 textures with deterministic oldest-entry eviction is bounded; an unbounded map keyed by page is not.

## Stop conditions

Stop optimization when measurements no longer identify an owned bottleneck. Escalate vendor investigation only with an isolated reproducer.
