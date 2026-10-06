---
name: toolkit-component-authoring
description: Add a reusable component to Rust Makepad Toolkit through a generic contract, pure Rust tests, an isolated Makepad adapter or demo, provenance, maturity, and consumer integration. Use when creating or extracting a new shared primitive rather than implementing application-only behavior.
---

# Name

Toolkit Component Authoring

## Purpose

Move a reusable idea from contract to proven consumer without introducing reverse dependencies or duplicate implementations.

## Use when

Use for new Toolkit primitives and extraction of reusable consumer code.

## Do not use when

Do not use for a feature whose contract is inherently product- or format-specific.

## Architecture invariants

- Define the generic contract before implementation.
- Prefer pure Rust state, geometry, and render plans.
- Add a Makepad adapter only for draw/event/GPU lifecycle.
- Prove behavior in an isolated demo without consumer imports.
- Integrate one real consumer after provider gates pass.
- Keep one source of truth; transitional facades only re-export.
- Record origin, old path, new path, milestone, and maturity.
- Do not patch Makepad vendor without isolated proof.

## Workflow

1. Classify the candidate and inspect existing Toolkit components.
2. Write contract, invariants, non-goals, and `CHEAPEST_VERIFICATION_PATH`.
3. Implement pure types and focused tests.
4. Implement the smallest Makepad adapter if needed.
5. Add or extend a standalone demo.
6. Build/test Toolkit independently.
7. Migrate a consumer through an adapter or re-export.
8. Run consumer regressions.
9. Remove duplicate production implementation.
10. Update component maturity, provenance, and limitations.

## Cheapest verification

Start at L0 for pure logic, L1 for the provider crate, L2 for the demo, and L3 only after the consumer dependency changes. Reserve L4 for the extraction milestone.

## Tools

Use Cargo package gates, dependency searches, source hashes, demos, component docs, and boundary validators.

## Common failure modes

- Designing the public API from one consumer's private model.
- Copying code before checking dependencies.
- Calling compile-only evidence a working visual demo.
- Leaving ownership split between Toolkit and consumer.
- Marking a new visual component stable without runtime evidence.

## Examples

Extract page placement into pure core, keep `VirtualCanvas` as the Makepad adapter, and let a document application provide a format-specific projection.

## Stop conditions

Stop if independent Toolkit tests fail, the demo imports a consumer, duplicate production code remains, or the dependency graph points from Toolkit to the application.
