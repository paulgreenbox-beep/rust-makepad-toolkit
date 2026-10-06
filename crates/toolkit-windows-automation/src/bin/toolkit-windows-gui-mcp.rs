fn main() {
    if let Err(error) = toolkit_windows_automation::run_mcp_stdio() {
        eprintln!("toolkit Windows GUI MCP server failed: {error}");
        std::process::exit(1);
    }
}
