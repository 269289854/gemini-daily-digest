//! JSON-RPC 2.0 MCP server protocol dispatcher (`initialize`, `ping`, `tools/list`, `tools/call`).

use std::str::FromStr;

use digest_core::constants::SERVICE_NAME;
use digest_core::domain::{McpAccessMode, McpToolName};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::redact::sanitize_error_text;
use crate::tools::all_tool_descriptors;
use crate::McpDispatcher;

const JSON_RPC_VERSION: &str = "2.0";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const DEFAULT_PROTOCOL_VERSION: &str = "2025-03-26";

pub(crate) const JSONRPC_PARSE_ERROR: i64 = -32700;
pub(crate) const JSONRPC_INVALID_REQUEST: i64 = -32600;
const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;
const JSONRPC_INVALID_PARAMS: i64 = -32602;

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    #[must_use]
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: JSON_RPC_VERSION,
            id,
            result: Some(result),
            error: None,
        }
    }

    #[must_use]
    pub fn err(id: Value, code: i64, message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            jsonrpc: JSON_RPC_VERSION,
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data,
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawJsonRpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

/// Processes one JSON-RPC 2.0 text line (used by `stdio` transport).
pub async fn handle_json_rpc_line(
    line: &str,
    dispatcher: &McpDispatcher,
    granted_mode: McpAccessMode,
) -> Option<JsonRpcResponse> {
    let raw_value: Value = match serde_json::from_str(line) {
        Ok(val) => val,
        Err(err) => {
            return Some(JsonRpcResponse::err(
                Value::Null,
                JSONRPC_PARSE_ERROR,
                format!("Parse error: {err}"),
                None,
            ));
        }
    };

    handle_json_rpc_value(raw_value, dispatcher, granted_mode).await
}

/// Processes a parsed JSON-RPC 2.0 request object (`None` for notifications).
pub async fn handle_json_rpc_value(
    raw_value: Value,
    dispatcher: &McpDispatcher,
    granted_mode: McpAccessMode,
) -> Option<JsonRpcResponse> {
    let req: RawJsonRpcRequest = match serde_json::from_value(raw_value) {
        Ok(parsed) => parsed,
        Err(err) => {
            return Some(JsonRpcResponse::err(
                Value::Null,
                JSONRPC_INVALID_REQUEST,
                format!("Invalid Request: {err}"),
                None,
            ));
        }
    };

    if req.jsonrpc != JSON_RPC_VERSION || req.method.trim().is_empty() {
        let id = req.id.unwrap_or(Value::Null);
        return Some(JsonRpcResponse::err(
            id,
            JSONRPC_INVALID_REQUEST,
            "Invalid Request: jsonrpc must be '2.0' and method must be non-empty",
            None,
        ));
    }

    // Notifications carry no `id` and MUST NOT emit a response.
    let id = req.id?;

    let response = match req.method.as_str() {
        "initialize" => {
            let requested_version = req
                .params
                .as_ref()
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Value::as_str);
            let negotiated = requested_version
                .and_then(|v| {
                    SUPPORTED_PROTOCOL_VERSIONS
                        .iter()
                        .find(|s| **s == v)
                        .copied()
                })
                .unwrap_or(DEFAULT_PROTOCOL_VERSION);
            JsonRpcResponse::ok(
                id,
                json!({
                    "protocolVersion": negotiated,
                    "capabilities": {
                        "tools": {
                            "listChanged": false
                        }
                    },
                    "serverInfo": {
                        "name": SERVICE_NAME,
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
        }
        "ping" => JsonRpcResponse::ok(id, json!({})),
        "tools/list" => {
            let tools = all_tool_descriptors();
            JsonRpcResponse::ok(id, json!({ "tools": tools }))
        }
        "tools/call" => handle_tools_call(id, req.params, dispatcher, granted_mode).await,
        other => JsonRpcResponse::err(
            id,
            JSONRPC_METHOD_NOT_FOUND,
            format!("Method not found: {other}"),
            None,
        ),
    };

    Some(response)
}

async fn handle_tools_call(
    id: Value,
    params: Option<Value>,
    dispatcher: &McpDispatcher,
    granted_mode: McpAccessMode,
) -> JsonRpcResponse {
    let Some(Value::Object(map)) = params else {
        return JsonRpcResponse::err(
            id,
            JSONRPC_INVALID_PARAMS,
            "Invalid params: tools/call requires an object with 'name'",
            None,
        );
    };

    let Some(name_raw) = map.get("name").and_then(Value::as_str) else {
        return JsonRpcResponse::err(
            id,
            JSONRPC_INVALID_PARAMS,
            "Invalid params: 'name' must be a string",
            None,
        );
    };

    let Ok(tool) = McpToolName::from_str(name_raw) else {
        return JsonRpcResponse::err(
            id,
            JSONRPC_INVALID_PARAMS,
            format!("Unknown MCP tool: {name_raw}"),
            None,
        );
    };

    let arguments = map
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

    match dispatcher.dispatch(granted_mode, tool, arguments).await {
        Ok(data) => {
            let pretty = serde_json::to_string_pretty(&data).unwrap_or_else(|_| data.to_string());
            JsonRpcResponse::ok(
                id,
                json!({
                    "content": [{ "type": "text", "text": pretty }],
                    "structuredContent": data,
                    "isError": false
                }),
            )
        }
        Err(err) => {
            let code = err.error_code();
            let message = sanitize_error_text(&err.to_string());
            let err_json = json!({
                "code": code,
                "message": message,
            });
            let text = serde_json::to_string(&err_json).unwrap_or_else(|_| message.clone());
            JsonRpcResponse::ok(
                id,
                json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": true
                }),
            )
        }
    }
}
