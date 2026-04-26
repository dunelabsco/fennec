use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Describes a tool exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolSpec {
    /// The tool name.
    pub name: String,
    /// Human-readable description of the tool.
    pub description: String,
    /// JSON Schema for the tool's input.
    pub input_schema: Value,
}

/// A JSON-RPC 2.0 request — always carries an `id` so a response can be
/// correlated back. See [`JsonRpcNotification`] for the no-id variant.
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC 2.0 notification — no `id` field per the spec. Compliant
/// servers will neither send a response nor expect one; non-compliant
/// servers that treat a notification as a request and try to reply would
/// previously desync the stdio stream, because the old
/// `send_notification` bumped `next_id` and emitted a `JsonRpcRequest`
/// with an id — then the server's reply would arrive later and be
/// consumed as if it were the response to a different request.
#[derive(Debug, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    #[allow(dead_code)]
    pub jsonrpc: String,
    #[allow(dead_code)]
    pub id: Option<u64>,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error.
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[allow(dead_code)]
    pub data: Option<Value>,
}

/// The result of calling an MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    /// Content blocks returned by the tool.
    pub content: Vec<McpContent>,
    /// Whether the tool reported an error.
    pub is_error: bool,
}

/// A single content block in an MCP tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpContent {
    /// Content type (e.g. `"text"`).
    #[serde(rename = "type")]
    pub type_: String,
    /// Text content (present when `type_ == "text"`).
    pub text: Option<String>,
}
