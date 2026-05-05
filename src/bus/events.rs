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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OutboundMessage {
    pub content: String,
    pub channel: String,
    pub chat_id: String,
    pub reply_to: Option<String>,
    pub metadata: HashMap<String, String>,
    /// Optional binary attachments. Channels that don't support
    /// media (CLI, email-text, etc.) ignore this field; rich
    /// channels (Matrix, Telegram, Discord, Slack) upload each
    /// attachment as a separate platform-native message.
    #[serde(default)]
    pub attachments: Vec<MediaAttachment>,
}

/// Kind of a media attachment. Maps to channel-specific message
/// types — Matrix `m.image`/`m.file`/`m.audio`/`m.video`,
/// Telegram `sendPhoto`/`sendDocument`/`sendAudio`/`sendVideo`,
/// etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    File,
    Audio,
    Video,
}

/// A binary attachment to an outbound message. The bytes travel
/// in-process — channels upload them to whatever blob endpoint
/// they speak. Keeping the payload in memory (rather than on disk
/// behind a path) avoids race conditions where the file is removed
/// between agent emit and channel send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAttachment {
    pub kind: MediaKind,
    pub bytes: Vec<u8>,
    pub mime: String,
    /// Display filename. Optional — channels generate one when absent.
    #[serde(default)]
    pub filename: Option<String>,
    /// Image / video dimensions, if known. None means the channel
    /// either doesn't need them or will probe them itself.
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Audio / video duration in milliseconds, if known.
    #[serde(default)]
    pub duration_ms: Option<u32>,
}

impl MediaAttachment {
    pub fn image(bytes: Vec<u8>, mime: impl Into<String>) -> Self {
        Self {
            kind: MediaKind::Image,
            bytes,
            mime: mime.into(),
            filename: None,
            width: None,
            height: None,
            duration_ms: None,
        }
    }

    pub fn file(bytes: Vec<u8>, mime: impl Into<String>, filename: impl Into<String>) -> Self {
        Self {
            kind: MediaKind::File,
            bytes,
            mime: mime.into(),
            filename: Some(filename.into()),
            width: None,
            height: None,
            duration_ms: None,
        }
    }
}
