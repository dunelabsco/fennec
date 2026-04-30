//! Host functions exposed to WASM plugins.
//!
//! Each function is gated by the same security primitives the agent
//! itself uses:
//!
//! - HTTP goes through [`url_guard::validate_url_str`].
//! - File I/O goes through [`PathSandbox`].
//! - Memory writes are scoped to a per-plugin namespace.
//!
//! The plugin can never do anything the agent itself couldn't —
//! plugins are a *capability extension* mechanism, not a privilege
//! escalation.

use std::sync::Arc;

use tokio::runtime::Handle;

use crate::memory::traits::{Memory, MemoryCategory, MemoryEntry};
use crate::security::path_sandbox::PathSandbox;
use crate::security::url_guard;

/// Per-plugin host state. Held inside each plugin's `Store`.
///
/// Constructed by the loader once per plugin. Fields are the
/// **capability surface** — anything a plugin can do is brokered
/// through one of these handles.
pub struct PluginHostState {
    pub plugin_name: String,
    pub path_sandbox: Arc<PathSandbox>,
    pub memory: Arc<dyn Memory>,
    pub http_client: reqwest::Client,
    pub rt_handle: Handle,
}

impl PluginHostState {
    fn memory_category(&self) -> MemoryCategory {
        MemoryCategory::Custom(format!("plugin:{}", self.plugin_name))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

pub fn host_log(state: &PluginHostState, level: LogLevel, message: &str) {
    // tracing macros require a `'static` target literal, so we put
    // the plugin name in a structured field instead. Filter via
    // `RUST_LOG=fennec::plugins::wasm::host=info` to see only plugin
    // logs, then narrow to a specific plugin by grepping the
    // `plugin=...` field.
    let plugin = state.plugin_name.as_str();
    match level {
        LogLevel::Trace => tracing::trace!(plugin = %plugin, "{}", message),
        LogLevel::Debug => tracing::debug!(plugin = %plugin, "{}", message),
        LogLevel::Info => tracing::info!(plugin = %plugin, "{}", message),
        LogLevel::Warn => tracing::warn!(plugin = %plugin, "{}", message),
        LogLevel::Error => tracing::error!(plugin = %plugin, "{}", message),
    }
}

pub fn host_now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct WasmHttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

pub struct WasmHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

const MAX_HTTP_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

pub fn host_http_request(
    state: &PluginHostState,
    req: WasmHttpRequest,
) -> Result<WasmHttpResponse, String> {
    url_guard::validate_url_str(&req.url)
        .map_err(|e| format!("url rejected by sandbox: {e}"))?;

    let method = match req.method.to_ascii_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "PATCH" => reqwest::Method::PATCH,
        "DELETE" => reqwest::Method::DELETE,
        "HEAD" => reqwest::Method::HEAD,
        other => return Err(format!("unsupported method: {other}")),
    };

    let mut builder = state.http_client.request(method, &req.url);
    for (name, value) in &req.headers {
        builder = builder.header(name, value);
    }
    if let Some(body) = req.body {
        builder = builder.body(body);
    }

    state.rt_handle.block_on(async move {
        let resp = builder
            .send()
            .await
            .map_err(|e| format!("http request failed: {e}"))?;
        let status = resp.status().as_u16();
        let headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (k.as_str().to_string(), s.to_string()))
            })
            .collect();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("http body read failed: {e}"))?;
        if bytes.len() > MAX_HTTP_RESPONSE_BYTES {
            return Err(format!(
                "http response too large: {} bytes (limit {})",
                bytes.len(),
                MAX_HTTP_RESPONSE_BYTES
            ));
        }
        Ok(WasmHttpResponse {
            status,
            headers,
            body: bytes.to_vec(),
        })
    })
}

const MAX_READ_FILE_BYTES: u64 = 50 * 1024 * 1024;

pub fn host_read_file(state: &PluginHostState, path: &str) -> Result<Vec<u8>, String> {
    let resolved = state
        .path_sandbox
        .check(std::path::Path::new(path))
        .map_err(|e| format!("path rejected by sandbox: {e}"))?;

    let meta = std::fs::metadata(&resolved).map_err(|e| format!("stat failed: {e}"))?;
    if meta.len() > MAX_READ_FILE_BYTES {
        return Err(format!(
            "file too large: {} bytes (limit {})",
            meta.len(),
            MAX_READ_FILE_BYTES
        ));
    }
    std::fs::read(&resolved).map_err(|e| format!("read failed: {e}"))
}

pub fn host_write_file(
    state: &PluginHostState,
    path: &str,
    contents: &[u8],
) -> Result<(), String> {
    let resolved = state
        .path_sandbox
        .check(std::path::Path::new(path))
        .map_err(|e| format!("path rejected by sandbox: {e}"))?;
    if let Some(parent) = resolved.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir parent failed: {e}"))?;
    }
    std::fs::write(&resolved, contents).map_err(|e| format!("write failed: {e}"))
}

pub struct WasmMemoryEntry {
    pub key: String,
    pub content: String,
    pub category: String,
    pub created_at: String,
}

pub fn host_memory_recall(
    state: &PluginHostState,
    query: &str,
    limit: u32,
) -> Result<Vec<WasmMemoryEntry>, String> {
    let limit = limit.clamp(1, 50) as usize;
    let memory = Arc::clone(&state.memory);
    let q = query.to_string();
    state.rt_handle.block_on(async move {
        memory
            .recall(&q, limit)
            .await
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|e| WasmMemoryEntry {
                        key: e.key,
                        content: e.content,
                        category: format_category(&e.category),
                        created_at: e.created_at,
                    })
                    .collect()
            })
            .map_err(|e| format!("memory recall failed: {e}"))
    })
}

pub fn host_memory_store(
    state: &PluginHostState,
    key: &str,
    content: &str,
) -> Result<(), String> {
    let category = state.memory_category();
    let now = chrono::Utc::now().to_rfc3339();
    let entry = MemoryEntry {
        key: key.to_string(),
        content: content.to_string(),
        category,
        created_at: now.clone(),
        updated_at: now,
        ..MemoryEntry::default()
    };
    let memory = Arc::clone(&state.memory);
    state.rt_handle.block_on(async move {
        memory
            .store(entry)
            .await
            .map_err(|e| format!("memory store failed: {e}"))
    })
}

fn format_category(c: &MemoryCategory) -> String {
    match c {
        MemoryCategory::Core => "core".into(),
        MemoryCategory::Daily => "daily".into(),
        MemoryCategory::Conversation => "conversation".into(),
        MemoryCategory::Custom(s) => s.clone(),
    }
}
