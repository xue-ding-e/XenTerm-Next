use std::io::{BufRead, Read, Write};

use anyhow::{Context, Result};
use serde_json::{json, Value};

const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    LATEST_PROTOCOL_VERSION,
];

/// A single MCP request line may not exceed this; larger input is rejected
/// without allocating it whole, so a runaway client cannot grow this process
/// without bound (audit L-07).
const MAX_REQUEST_LINE: usize = 4 * 1024 * 1024;

pub(crate) fn run_stdio(allow_config_import: bool) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("create MCP runtime")?;
    let mut stdout = std::io::stdout().lock();
    loop {
        // `take` bounds the allocation `read_line` makes, so an oversized line
        // is cut at the cap instead of buffered whole.
        let mut line = String::new();
        let read = std::io::stdin()
            .lock()
            .take((MAX_REQUEST_LINE + 1) as u64)
            .read_line(&mut line)
            .context("read MCP request")?;
        if read == 0 {
            break;
        }
        if line.len() > MAX_REQUEST_LINE {
            discard_rest_of_line()?;
            let response = error_response(Value::Null, -32600, "request line too large");
            write_response(&mut stdout, &response)?;
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => runtime.block_on(handle(request, allow_config_import)),
            Err(error) => Some(error_response(Value::Null, -32700, &error.to_string())),
        };
        if let Some(response) = response {
            write_response(&mut stdout, &response)?;
        }
    }
    Ok(())
}

fn write_response(stdout: &mut impl Write, response: &Value) -> Result<()> {
    serde_json::to_writer(&mut *stdout, response).context("write MCP response")?;
    stdout.write_all(b"\n").context("finish MCP response")?;
    stdout.flush().context("flush MCP response")
}

/// Consume the remainder of an oversized line through the shared stdin buffer
/// without buffering it (bounded memory regardless of input length).
fn discard_rest_of_line() -> Result<()> {
    let mut stdin = std::io::stdin().lock();
    loop {
        let available = stdin.fill_buf().context("read MCP request")?;
        if available.is_empty() {
            return Ok(());
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(i) => {
                stdin.consume(i + 1);
                return Ok(());
            }
            None => {
                let len = available.len();
                stdin.consume(len);
            }
        }
    }
}

/// The MCP server's own on-switch, re-read per request so flipping the setting
/// takes effect on the next request without a restart. Fail closed: an
/// unreadable config means the server answers nothing (audit I-04).
fn mcp_enabled() -> bool {
    crate::config::ConfigStore::load()
        .map(|store| store.mcp_enabled())
        .unwrap_or(false)
}

async fn handle(request: Value, allow_config_import: bool) -> Option<Value> {
    handle_with_import(request, mcp_enabled(), allow_config_import).await
}

#[cfg(test)]
async fn handle_with(request: Value, enabled: bool) -> Option<Value> {
    handle_with_import(request, enabled, false).await
}

async fn handle_with_import(
    request: Value,
    enabled: bool,
    allow_config_import: bool,
) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str);
    if id.is_none() {
        return None;
    }
    let id = id.unwrap_or(Value::Null);
    // Every method — not only tool calls — requires the server to be enabled;
    // a disabled server must not even enumerate its capabilities.
    if !enabled {
        return Some(error_response(
            id,
            -32001,
            "MCP is disabled in Settings > Interface > MCP",
        ));
    }
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    match method {
        Some("initialize") => Some(success_response(id, initialize(&params))),
        Some("ping") => Some(success_response(id, json!({}))),
        Some("tools/list") => Some(success_response(
            id,
            json!({ "tools": super::tools::definitions() }),
        )),
        Some("tools/call") => Some(call_tool(id, &params, allow_config_import).await),
        Some(_) => Some(error_response(id, -32601, "method not found")),
        None => Some(error_response(id, -32600, "invalid request")),
    }
}

fn initialize(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(LATEST_PROTOCOL_VERSION);
    let protocol_version = SUPPORTED_PROTOCOL_VERSIONS
        .contains(&requested)
        .then_some(requested)
        .unwrap_or(LATEST_PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol_version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "xenterm",
            "title": "XenTerm MCP",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "Manage saved XenTerm sessions and run permitted SSH automation. Session metadata omits credential values; enabled command and file tools have the documented OS-account access."
    })
}

async fn call_tool(id: Value, params: &Value, allow_config_import: bool) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error_response(id, -32602, "missing tool name");
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match super::tools::call_mcp(name, &arguments, allow_config_import).await {
        Ok(value) => success_response(
            id,
            json!({
                "content": [{ "type": "text", "text": pretty_json(&value) }],
                "structuredContent": value,
                "isError": false
            }),
        ),
        Err(error) => success_response(
            id,
            json!({
                "content": [{ "type": "text", "text": error.to_string() }],
                "isError": true
            }),
        ),
    }
}

fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn success_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initialize_negotiates_a_supported_version() {
        let response = handle_with(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-06-18" }
            }),
            true,
        )
        .await
        .unwrap();
        assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(response["result"]["serverInfo"]["name"], "xenterm");
    }

    #[tokio::test]
    async fn disabled_server_refuses_every_method() {
        for method in ["initialize", "ping", "tools/list"] {
            let response = handle_with(
                json!({ "jsonrpc": "2.0", "id": 1, "method": method }),
                false,
            )
            .await
            .unwrap();
            assert_eq!(response["error"]["code"], -32001, "{method} must be gated");
        }
    }

    #[tokio::test]
    async fn notifications_do_not_receive_responses() {
        assert!(handle_with(
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
            true,
        )
        .await
        .is_none());
    }

    #[tokio::test]
    async fn lists_tools() {
        let response = handle_with(
            json!({
                "jsonrpc": "2.0",
                "id": "tools",
                "method": "tools/list"
            }),
            true,
        )
        .await
        .unwrap();
        assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 8);
    }
}
