# Generic Makepad Windows accessibility bridge

Status: CURRENT
Updated: 2026-08-22
Contract/API version: WindowsAccessibilityBridge v1

`toolkit-windows-automation` now includes a Windows UI Automation provider that
can be attached to an existing Makepad `HWND`. The provider is snapshot-driven:
the application owns widget semantics and publishes a fresh
`AccessibilityTree` after layout, visibility, focus, or selection changes.

The bridge exposes:

- the complete semantic widget tree through UIA fragment navigation;
- stable node IDs, roles, names, screen-space bounds, visibility, enabled and focus state;
- `Selection` and `SelectionItem` patterns, including selected children and selection-container flags;
- `Invoke`, `Select`, `AddToSelection`, `RemoveFromSelection`, and `SetFocus` actions.
- `ValuePattern` for read-only values and semantic `SetValue` input actions.

## Attach to a Makepad window

```rust
use std::sync::Arc;
use toolkit_windows_automation::{
    AccessibilityActionHandler, AccessibilityNode, AccessibilityRect, AccessibilityRole,
    AccessibilitySelection, AccessibilityTree, WindowsAccessibilityBridge,
};

let root = AccessibilityNode::new(
    "app",
    AccessibilityRole::Window,
    "My Makepad app",
    AccessibilityRect { x: 0.0, y: 0.0, width: 800.0, height: 600.0 },
);
let tree = AccessibilityTree::new(root)?.with_generation(1);

let action_handler: AccessibilityActionHandler = Arc::new(move |request| {
    // Marshal this request to the Makepad UI thread and dispatch it by stable node ID.
    dispatch_makepad_accessibility_action(request)?;
    Ok(())
});

let bridge = WindowsAccessibilityBridge::attach(hwnd, tree, action_handler)?;
```

Keep `bridge` alive for the lifetime of the window. Publish updates with
`bridge.update(next_tree)`. IDs must remain stable for the logical lifetime of
the widget; do not use child indexes or transient draw IDs.

`AccessibilityRect` uses screen-space physical pixels, matching UI Automation's
`BoundingRectangle`. A tree with duplicate IDs, empty IDs, or invalid bounds is
rejected before it reaches Windows.

## Action and selection contract

Set the corresponding flags on each node before publishing it:

```rust
button.actions.invoke = true;
button.actions.focus = true;
let mut edit = AccessibilityNode::new(
    "input.name",
    AccessibilityRole::Edit,
    "Name",
    bounds,
);
edit.value = Some(String::new());
edit.actions.set_value = true;
tab_item.selected = Some(true);
tab_item.actions.select = true;
tab_list.selection = Some(AccessibilitySelection {
    can_select_multiple: false,
    is_selection_required: true,
});
```

A node with `value: Some(...)` exposes its current value through UIA `ValuePattern`. Set `actions.set_value = true` to allow `ValuePattern::SetValue` to reach the callback as `AccessibilityAction::SetValue(String)`. 
The action callback is intentionally generic. In Makepad, use Cx::post_action to enqueue
the request onto the UI/event thread, update application state in handle_actions, and
publish a new snapshot after state or layout changes.
The bridge does not inspect private Makepad widget internals and does not mutate
the vendor Makepad source.

## Relation to the existing automation backend

The existing UIA client backend remains useful for native controls and window
discovery. The new provider is the server-side bridge for custom Makepad-drawn
widgets. It answers `WM_GETOBJECT` on the Makepad window and supplies the
semantic fragment tree to any UIA client, including the Toolkit's automation
client.

The current implementation subclasses the target `HWND`, so no Makepad vendor
patch is required. Integrating it into an application still requires the app
to build and publish its semantic snapshot and route action callbacks to the
Makepad event loop.

## Verification

```powershell
cargo test --offline -p toolkit-windows-automation --lib
cargo check --offline -p toolkit-windows-automation --all-targets
```

Interactive UIA inspection additionally requires a Windows desktop and a
Makepad application that attaches the bridge and publishes a non-empty tree.
## End-to-end Makepad smoke

    cargo build --offline -p toolkit-demos --bin accessibility-bridge-demo
    cargo run --offline -p toolkit-windows-automation --bin windows-accessibility-bridge-smoke

The smoke test launches a real custom-drawn Makepad window, verifies the UIA
fragment tree, roles, names, DPI-scaled bounds, stable element identity,
InvokePattern, SelectionItem.Select, and the updated selected state. It writes
the control tree, summary, and screenshot under
reports/evidence/makepad-accessibility-bridge-smoke.

The current AI UI Test Tool is [the Auto Probe workspace](../prototypes/ai-ui-test-tool-auto-probe/README.md). The older TestHost source remains a compatibility fixture; its historical description is archived rather than used as the current guide.
