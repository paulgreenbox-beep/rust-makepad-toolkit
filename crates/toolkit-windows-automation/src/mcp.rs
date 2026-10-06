use crate::contract::{AutomationError, ErrorCode};
use crate::tool_registry::GuiToolRegistry;
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

pub fn run_mcp_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut registry = GuiToolRegistry::new()?;

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                write_message(
                    &mut stdout,
                    &json_rpc_error(
                        Value::Null,
                        -32700,
                        "Parse error",
                        json!({"detail": error.to_string()}),
                    ),
                )?;
                continue;
            }
        };
        let id = request.get("id").cloned();
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if id.is_none() {
            continue;
        }
        let id = id.unwrap_or(Value::Null);
        let response = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {
                        "name": "rust-makepad-toolkit-windows-gui",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }),
            "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": registry.definitions()}
            }),
            "tools/call" => {
                let params = request.get("params").cloned().unwrap_or(Value::Null);
                let name = params.get("name").and_then(Value::as_str);
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match name {
                    Some(name) => {
                        let envelope = registry.invoke(name, arguments);
                        let text = serde_json::to_string(&envelope)?;
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{"type": "text", "text": text}],
                                "structuredContent": envelope,
                                "isError": !envelope.ok
                            }
                        })
                    }
                    None => json_rpc_error(
                        id,
                        -32602,
                        "Invalid params",
                        json!({"detail": "tools/call requires params.name"}),
                    ),
                }
            }
            _ => json_rpc_error(
                id,
                -32601,
                "Method not found",
                serde_json::to_value(AutomationError::new(
                    ErrorCode::ToolNotFound,
                    format!("unknown MCP method: {method}"),
                ))
                .unwrap_or(Value::Null),
            ),
        };
        write_message(&mut stdout, &response)?;
    }
    Ok(())
}

fn json_rpc_error(id: Value, code: i32, message: &str, data: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
            "data": data
        }
    })
}

fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, message)?;
    writer.write_all(b"\n")?;
    writer.flush()
}
