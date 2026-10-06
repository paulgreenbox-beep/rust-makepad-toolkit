---
name: rust-makepad-architecture
description: Classify and structure reusable Rust and Makepad code with one-way dependencies, pure-core boundaries, domain adapters, and one source of truth. Use for architecture reviews, extractions, new crates, dependency changes, or deciding whether code belongs in an application or shared Toolkit.
---

# Name

Rust Makepad Architecture

## Purpose

Keep reusable contracts independent from application domains and Makepad lifecycle details.

## Use when

Use this skill after it is selected from `skills/INDEX.md` for architecture or dependency work.

## Do not use when

Do not load it for a local implementation change whose boundary and owner are already proven.

## Architecture invariants

- Dependency direction is `generic Rust core -> Makepad adapter -> domain adapter -> application` from provider to consumer.
- Generic crates never import application, document-format, database, ERP, or product modules.
- Keep geometry, identity, layout planning, visible ranges, and render plans in pure Rust.
- Keep drawing, events, GPU resources, and widget lifecycle in the Makepad adapter.
- Keep format/domain projection in the consumer adapter.
- Maintain one production source of truth. A compatibility crate may only re-export it and must be marked transitional.
- Check the Toolkit before creating viewport, canvas, page, identity, overlay, or text primitives.
- Do not patch Makepad vendor without an isolated reproducer proving an upstream blocker.
- Prefer a small cohesive crate or module; do not create crates for naming alone.

## Workflow

1. Inspect imports, manifests, public types, and actual consumers.
2. Classify code as generic Rust, generic Makepad, generic document UI, or domain-specific.
3. Draw the proposed dependency graph and reject reverse edges.
4. Define the generic contract before moving implementation.
5. Build and test the provider independently.
6. Migrate one real consumer through an adapter or temporary re-export.
7. Remove duplicate production implementation only after regressions pass.
8. Record origin, new path, maturity, and compatibility status.

## Cheapest verification

`CHEAPEST_VERIFICATION_PATH`: run pure crate tests first; build the Makepad adapter only if its public boundary changed; build the consumer only if integration changed.

## Tools

Use `rg`, Cargo metadata/tree, focused Cargo tests, and dependency-boundary scripts.

## Common failure modes

- Copying implementation without changing source-of-truth ownership.
- Putting domain types into a generic frame.
- Making pure geometry depend on Makepad widgets.
- Treating a path re-export as a permanent second implementation.
- Running a full application build before a core unit test.

## Examples

Place `PageStack::visible_range` in pure core; let a Makepad widget consume its render plan; let a DOCX adapter create page data outside the Toolkit.

## Stop conditions

Stop when dependency direction is ambiguous, a vendor patch appears necessary without an isolated proof, or migration would delete the only working implementation before the consumer is green.
