---
name: rust-makepad-toolkit
description: "Work in Rust_Makepad_Toolkit: find the owning public component or AI UI Test Tool contract, reuse it, choose the cheapest sufficient verification, and preserve evidence. Do not load for unrelated Makepad styling."
---

# Rust Makepad Toolkit

Use this skill for cross-cutting work in this repository. Follow `AGENTS.md` and
read `skills/INDEX.md` first; load a narrower project skill only when its topic
matches the task.

## Find the owner

- Start with `docs/INDEX.md`, `docs/COMPONENTS.md`, and the actual Cargo
  manifests/exports. Distinguish pure `makepad-toolkit-core`, Makepad widget
  adapters, Windows automation, and the separate current AI UI Test Tool under
  `prototypes/ai-ui-test-tool-auto-probe`.
- Reuse an existing Toolkit primitive when its public contract fits. A consumer
  adapter may translate domain data; it must not create a second production
  implementation of the same generic behavior.
- For `.aiui.json` tests, use the public Script API and
  `prototypes/ai-ui-test-tool-auto-probe/operations.json`. The backend's
  private/direct-host aliases are not the public contract. Scheduler jobs and
  generated scripts must go through the public runner.
- Do not edit `.deps/makepad` to work around a Toolkit or app symptom without
  a minimal isolated reproducer that proves an upstream defect.

## Verify proportionally

1. State the changed boundary and `CHEAPEST_VERIFICATION_PATH` before
   implementation. Use `rust-cheap-compile` for L0–L4 selection when choosing
   Cargo commands.
2. Prefer a focused unit test in the owner crate. Escalate to a widget demo,
   consumer integration, or real Windows E2E only when the changed contract or
   acceptance criterion requires it.
3. Do not equate compiling code with a working UI. For visual, accessibility,
   pointer, or scheduler claims, retain the relevant screenshot, semantic state,
   postcondition, cleanup, or report evidence.
4. Respect current capability and limitation docs. Mark an unverified or
   environment-blocked claim honestly; do not issue a PASS marker without its
   required real proof.

## Route to details

Use `reusable-component-first` before adding a common primitive,
`rust-makepad-architecture` for ownership or dependency changes,
`toolkit-component-authoring` for extracting a reusable component, and the
visual/canvas/identity/resource skills only for their matching problem.
The catalog and routing descriptions are in `skills/INDEX.md`.
