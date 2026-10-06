---
name: rust-makepad-windows-gui
description: Safely use Rust Makepad Toolkit's Windows GUI MCP to inspect accessible app windows, interact with UI Automation controls, and collect bounded evidence. Use only when direct Windows GUI interaction or evidence is requested.
---

# Toolkit Windows GUI MCP

Use the bundled `rust-makepad-toolkit-windows-gui` MCP server only for its implemented Windows UI Automation and native-window operations.

- Resolve the intended top-level window with `windows_find_window`; continue only when the selector identifies exactly one target, and retain its returned `window_id`.
- Inspect the current window/control tree before acting. Resolve controls to returned `element_id` or app-owned `AutomationId`; do not invent IDs or infer that a widget is absent from the application just because it is absent from the UIA tree.
- Invoke, focus, set values, or send keys only when the user's request calls for changing that app. Re-read the relevant state after the action.
- For evidence, use `windows_capture_window` for a single target-window image and `windows_get_evidence` for the server's bounded structured event trail. Report exactly what these artifacts establish; do not treat a screenshot or API response alone as proof of a full end-to-end flow.
- `windows_set_window_state` and `windows_set_window_bounds` are programmatic operations. `windows_resolve_window_frame_target` only resolves a hit-test target; it does not perform physical title-bar movement or resizing.

This server does not provide arbitrary mouse gestures, the Makepad semantic accessibility bridge, the Toolkit Script API, or the AI UI Test Tool Scheduler. Semantic coverage depends on what the target application publishes through Windows UI Automation.

For Toolkit code, public-contract, component, or verification work, use the project-level `rust-makepad-toolkit` skill.
