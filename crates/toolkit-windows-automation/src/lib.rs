mod accessibility;
mod contract;
mod evidence;
mod identity;
mod mcp;
mod platform;
mod tool_registry;

pub use accessibility::*;
pub use contract::*;
pub use evidence::{EvidenceEntry, EvidenceLog, now_unix_ms};
pub use mcp::run_mcp_stdio;
pub use platform::WindowsAutomation;
#[cfg(windows)]
pub use platform::{
    AccessibilityAction, AccessibilityActionHandler, AccessibilityActionRequest,
    WindowsAccessibilityBridge,
};
pub use tool_registry::{GuiToolRegistry, ToolCallEnvelope, ToolDefinition};
