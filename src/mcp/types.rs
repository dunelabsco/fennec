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
///
/// Bidirectional: the MCP client serializes outgoing requests; the
/// MCP server (when Fennec is acting as one) deserializes incoming
/// requests on stdin. The id is kept as `serde_json::Value` because
/// the spec accepts numbers, strings, and null — sticking to a
/// stricter Rust type would silently disconnect any client that
/// uses string ids.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Construct a request with a numeric id (the most common case;
    /// the existing `McpClient` uses sequential `u64` ids).
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Value::from(id),
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
#[derive(Debug, Serialize, Deserialize)]
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
///
/// Bidirectional: the MCP client deserializes responses from servers
/// it called; Fennec's server side serializes responses going out on
/// stdout. The `id` mirrors the original request's id verbatim
/// (number, string, or null) to satisfy clients that use non-numeric
/// ids.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Construct a successful response.
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Construct an error response.
    pub fn error_response(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// A JSON-RPC 2.0 error.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
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
