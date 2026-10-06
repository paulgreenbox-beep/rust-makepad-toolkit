---
name: rust-cheap-compile
description: Select and execute the cheapest sufficient Cargo verification level for Rust and Makepad changes. Use when planning tests, iterating on a focused fix, avoiding unnecessary workspace builds, or defining a task's CHEAPEST_VERIFICATION_PATH.
---

# Name

Rust Cheap Compile

## Purpose

Shorten the development loop while preserving evidence proportional to the changed boundary.

## Use when

Use before choosing Cargo commands for any Rust or Makepad change.

## Do not use when

Do not use it to skip a required release gate or consumer regression after a public integration change.

## Architecture invariants

Never run the most expensive build when a cheaper verification level proves the change.

- L0 pure unit: one named test in the smallest pure crate.
- L1 crate: all tests for one small crate.
- L2 adapter/demo: check the Makepad component and run its isolated demo.
- L3 consumer integration: test or build the consuming application.
- L4 release/workspace gate: full workspace or release verification, only for milestones and releases.

Every task must state `CHEAPEST_VERIFICATION_PATH` before implementation.

## Workflow

1. Identify the smallest changed public boundary.
2. Choose the lowest level that can falsify the change.
3. Run that level after each iteration.
4. Escalate one level only when the lower level is green or insufficient.
5. Run all explicitly required higher gates once at the milestone boundary.

## Cheapest verification

Examples:

```text
L0: cargo test -p makepad-toolkit-core visible_and_overscan_ranges_are_bounded
L1: cargo test -p makepad-toolkit-core
L2: cargo check -p makepad-toolkit-widgets
    cargo run -p toolkit-demos --bin virtual-canvas-demo
L3: cargo test -p your-consumer-app
L4: cargo test --workspace
```

## Tools

Use Cargo package/test filters, `cargo check`, and isolated demos. Avoid `cargo fmt --all` when it would traverse upstream path dependencies; name owned packages explicitly.

## Common failure modes

- Building the Office consumer for a pure page-range test.
- Calling a release build during every edit.
- Claiming integration from a unit test alone.
- Skipping L3 after changing a public adapter contract.

## Examples

Task: fix `PageStack` visible range. Verify: L0 named test, then L1 core. Do not build the application unless the public integration changed.

## Stop conditions

Stop escalating when the selected level proves the task. Escalate to L4 only for an agreed milestone/release gate.
