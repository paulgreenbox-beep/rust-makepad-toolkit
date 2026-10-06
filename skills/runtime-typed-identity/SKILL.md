---
name: runtime-typed-identity
description: Design runtime-local strongly typed identities with typed EntityId, IdAllocator, and Registry contracts. Use for entity registries, multi-document sessions, stale-handle safety, lifecycle commands, or separating public identity from paths, indexes, widgets, and backend handles.
---

# Name

Runtime Typed Identity

## Purpose

Give runtime entities stable, type-safe, owner-scoped identities without leaking storage or UI details.

## Use when

Use when entities outlive one operation or commands must target one of several live entities.

## Do not use when

Do not use runtime IDs as persistent database identifiers or cross-process protocol IDs.

## Architecture invariants

- Identity is not a path.
- Identity is not an array or tab index.
- Identity is not `WidgetUid`.
- Identity is not a backend engine handle.
- Active entity is state that references an identity; active is not identity.
- `EntityId<T>` prevents mixing unrelated entity types.
- An allocator is monotonic, non-zero, non-wrapping, and owner-scoped.
- Removing an entry permanently stales that ID within the owner lifetime.
- Lookup never silently falls back to the active entity.

## Workflow

1. Define the entity type and registry owner.
2. Allocate only after construction succeeds.
3. Store backend handles inside the entity, not in the public ID.
4. Address commands with `EntityId<T>` explicitly.
5. Release external resources before registry removal.
6. Clear active state only when the closed ID was active.
7. Return typed stale/unsupported errors.

## Cheapest verification

`CHEAPEST_VERIFICATION_PATH`: L0 allocator/registry tests; add one consumer smoke test only when lifecycle integration changes.

## Tools

Use `rudra-runtime`, compile-fail tests, duplicate-value tests, stale-ID tests, and targeted lifecycle smoke tests.

## Common failure modes

- Deriving identity from a filename.
- Reusing removed numeric IDs.
- Exposing engine handles through inspection APIs.
- Routing target commands through global active state.
- Removing registry state before releasing backend resources.

## Examples

Use separate `Registry<Project>`, `Registry<Scene>`, and `Registry<Asset>` values even when their numeric IDs overlap.

## Stop conditions

Stop if the intended ID must be persistent or serialized; define a separate persistent-ID contract instead of extending runtime IDs implicitly.
