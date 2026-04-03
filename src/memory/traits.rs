use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Categorizes memory entries for different retention and decay policies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryCategory {
    Core,
    Daily,
    Conversation,
    Custom(String),
}

impl Serialize for MemoryCategory {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            MemoryCategory::Core => serializer.serialize_str("core"),
            MemoryCategory::Daily => serializer.serialize_str("daily"),
            MemoryCategory::Conversation => serializer.serialize_str("conversation"),
            MemoryCategory::Custom(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for MemoryCategory {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "core" => Ok(MemoryCategory::Core),
            "daily" => Ok(MemoryCategory::Daily),
            "conversation" => Ok(MemoryCategory::Conversation),
            other => Ok(MemoryCategory::Custom(other.to_string())),
        }
    }
}

/// A single memory record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub key: String,
    pub content: String,
    pub category: MemoryCategory,
    pub created_at: String,
    pub updated_at: String,
    pub session_id: Option<String>,
    pub namespace: String,
    pub importance: Option<f64>,
    pub score: Option<f64>,
    pub superseded_by: Option<String>,
}

impl Default for MemoryEntry {
    fn default() -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            key: String::new(),
            content: String::new(),
            category: MemoryCategory::Conversation,
            created_at: now.clone(),
            updated_at: now,
            session_id: None,
            namespace: "default".to_string(),
            importance: None,
            score: None,
            superseded_by: None,
        }
    }
}

/// Async trait for memory backends.
#[async_trait]
pub trait Memory: Send + Sync {
    /// Human-readable name of this backend.
    fn name(&self) -> &str;

    /// Store or update a memory entry.
    async fn store(&self, entry: MemoryEntry) -> Result<()>;

    /// Search for memories matching a query string.
    async fn recall(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Retrieve a single entry by key.
    async fn get(&self, key: &str) -> Result<Option<MemoryEntry>>;

    /// List entries, optionally filtered by category.
    async fn list(&self, category: Option<&MemoryCategory>, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Delete a memory entry by key.
    async fn forget(&self, key: &str) -> Result<bool>;

    /// Count stored entries, optionally filtered by category.
    async fn count(&self, category: Option<&MemoryCategory>) -> Result<usize>;

    /// Check if the backend is healthy and reachable.
    async fn health_check(&self) -> Result<()>;
}
