use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessage {
    pub id: String,
    pub sender: String,
    pub content: String,
    pub channel: String,
    pub chat_id: String,
    pub timestamp: u64,
    pub reply_to: Option<String>,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundMessage {
    pub content: String,
    pub channel: String,
    pub chat_id: String,
    pub reply_to: Option<String>,
    pub metadata: HashMap<String, String>,
}
