# Rust Makepad Toolkit MCP plugin

This is the Codex plugin source. A local installation is managed by the
operator's Codex marketplace.
It bundles a focused Windows GUI automation skill and starts the existing
`toolkit-windows-gui-mcp` binary from the Toolkit workspace. It does not
copy the backend or expose the AI UI Test Tool Script API/Scheduler.

## Requirements and scope

- Windows and Rust/Cargo on `PATH`. From this repository the launcher discovers
  the checkout relative to its source directory. When installed into a plugin
  cache, set `RUST_MAKEPAD_TOOLKIT_ROOT` in the local MCP server environment
  to your checkout. Keep that machine-specific path out of committed config.
  `cwd` remains relative to the installed plugin copy.
- The launcher builds/runs the existing Cargo target offline. The first start
  may take longer while cached Rust dependencies compile; if they are not
  cached, build them explicitly before using the plugin.
- The server exposes UI Automation window/control inspection and actions,
  native window bounds/state/capture, and bounded evidence. Its 20 tools are
  defined in `crates/toolkit-windows-automation/src/tool_registry.rs`.
- GUI mutations should be performed only for a user-requested action and
  with the intended window/element identity verified. Installing this plugin
  does not grant authority to control arbitrary desktop windows.
- The MCP config requests prompt approval, but approval behavior has not been
  verified in an installed Codex session. Confirm the effective plugin-scoped
  tool policy before allowing physical or state-changing GUI calls.

## Local checks

From this plugin directory:

```powershell
./tests/validate_plugin.ps1
```

After installation or updates, use a new chat for Codex to load its MCP tools.
