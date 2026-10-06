---
name: reusable-component-first
description: Require agents to inspect the Rust Makepad Toolkit before adding common runtime or UI primitives. Use before creating scroll viewports, canvases, page stacks, typed registries, grids, text renderers, overlays, or other components likely to be reusable.
---

# Name

Reusable Component First

## Purpose

Prevent parallel local implementations and preserve one source of truth.

## Use when

Use before writing a component that resembles an existing Toolkit responsibility.

## Do not use when

Do not force a domain-only rule into the Toolkit merely because it shares a noun with a generic component.

## Architecture invariants

- Search Toolkit manifests, `docs/COMPONENTS.md`, public exports, and `skills/INDEX.md` first.
- Reuse or extend the Toolkit when the contract is generic.
- A local duplicate requires a written incompatibility explanation and owner.
- Compatibility facades re-export; they do not copy implementation.
- New reusable behavior gets pure tests and an isolated demo before consumer coupling.

## Workflow

1. Name the capability, owner, and required invariants.
2. Search Toolkit symbols and component catalog.
3. Compare the existing contract with the required behavior.
4. Reuse directly when compatible.
5. Extend generically when the gap is reusable.
6. Keep local only when the gap is domain-specific; document why.
7. Verify no second production implementation was introduced.

## Cheapest verification

`CHEAPEST_VERIFICATION_PATH`: inspect/search first, then test the smallest reused or extended Toolkit crate; build the consumer only for integration.

## Tools

Use `rg`, Cargo metadata, `docs/COMPONENTS.md`, source provenance, focused tests, and demos.

## Common failure modes

- Searching only the consumer repository.
- Copy-pasting a component to avoid a path dependency.
- Generalizing domain state prematurely.
- Leaving an old implementation active after migration.

## Examples

Before adding a new scroll model, inspect `ScrollViewport`; before adding document IDs, inspect `EntityId<T>` and `Registry<T>`.

## Stop conditions

Stop local implementation when a compatible Toolkit primitive exists. Stop extraction when genericity cannot be stated without domain vocabulary.
