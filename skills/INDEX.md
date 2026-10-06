# Skill index

Load only the skill needed for the current task.

- `rust-makepad-toolkit` — use for cross-cutting Toolkit work: choose the public owner, reuse components, and retain proportional evidence.
- `rust-makepad-architecture` — use for dependency direction, extraction, crate boundaries, and source-of-truth decisions.
- `rust-cheap-compile` — use before choosing Cargo verification; defines L0–L4 and `CHEAPEST_VERIFICATION_PATH`.
- `makepad-visual-debug` — use when state exists but UI is invisible, clipped, misplaced, or behind another layer.
- `makepad-virtual-canvas` — use for zoom, scroll, clip, overlay, transforms, resize, and canvas ownership.
- `runtime-typed-identity` — use for `EntityId<T>`, `Registry<T>`, stale IDs, and targeted lifecycle commands.
- `document-page-ui` — use for pages, rulers, mixed sizes, visible ranges, overscan, and page-local primitives.
- `reusable-component-first` — use before adding a viewport, canvas, page stack, registry, grid, text renderer, or overlay.
- `makepad-resource-budget` — use for bounded rendering, caches, textures, inactive state, and memory investigations.
- `toolkit-component-authoring` — use when adding or extracting a reusable Toolkit component end to end.

`rich-text-rendering` is intentionally absent until the renderer has a standalone public contract and no production character-count measurement fallback.
