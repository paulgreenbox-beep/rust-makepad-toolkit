# Development loop

Every task starts with:

```text
CHEAPEST_VERIFICATION_PATH: <lowest sufficient level and command>
```

Never run the most expensive build when a cheaper verification level proves the change.

## L0 — pure unit

Use one named test in the smallest pure crate.

```powershell
cargo test --offline -p makepad-toolkit-core visible_and_overscan_ranges_are_bounded
cargo test --offline -p rudra-runtime removed_ids_are_not_reused
cargo test --offline -p toolkit-windows-automation required_keyboard_chords_expand_to_balanced_input_sequences
```

## L1 — crate

Run all tests for the changed small crate.

```powershell
cargo test --offline -p makepad-toolkit-core
cargo test --offline -p rudra-runtime
cargo test --offline -p toolkit-windows-automation
```

## L2 — adapter or demo

Compile the Makepad adapter and run only the isolated demonstration that exercises the visual contract.

```powershell
cargo check --offline -p makepad-toolkit-widgets
cargo run --offline -p toolkit-demos --bin virtual-canvas-demo
cargo run --offline -p toolkit-demos --bin document-ui-demo
cargo test --offline -p toolkit-windows-automation --test windows_gui_integration -- --ignored --nocapture
```

The Windows GUI integration test requires an interactive desktop and uses only the generic Toolkit fixture.

## L3 — consumer integration

Use only when a public Toolkit boundary, path dependency, compatibility facade, domain adapter, or real consumer acceptance boundary changed.

```powershell
cargo test --offline -p your-consumer-app
```

Run this command from the consumer workspace. A GUI trial against a final consumer EXE is evidence, not a substitute for its own tests and not automatic epic acceptance.

## L4 — release/workspace gate

Use for a milestone, extraction acceptance, or release—not for each edit.

```powershell
cargo check --offline --workspace
cargo test --offline --workspace
```

## Formatting

Format/check owned packages explicitly so the command does not traverse the entire upstream Makepad path workspace.

```powershell
cargo fmt -p rudra-runtime -p makepad-toolkit-core -p makepad-toolkit-widgets -p toolkit-non-office-identity-demo -p toolkit-demos -p toolkit-windows-automation -- --check
```

## Escalation rule

Escalate only after the cheaper level is green or cannot observe the changed boundary. A pure helper change normally stops at L1. A widget or native adapter change normally reaches L2. A dependency or public integration change reaches L3. L4 is the final milestone gate.
