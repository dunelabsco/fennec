pub mod traits;
pub mod cli;
pub mod manager;
pub mod telegram;
pub mod discord;
pub mod slack;
pub mod whatsapp;
pub mod email;
pub mod webhook;

pub use traits::{Channel, ChannelMessage, SendMessage};
pub use cli::CliChannel;
pub use manager::ChannelManager;
pub use telegram::TelegramChannel;
pub use discord::DiscordChannel;
pub use slack::SlackChannel;
pub use whatsapp::WhatsAppChannel;
pub use email::EmailChannel;
pub use webhook::WebhookChannel;

use std::collections::HashMap;
use std::sync::Arc;

/// Shared handle to a map of channels by name, used by tools that need to
/// interact with channels (e.g. `AskUserTool`).
pub type ChannelMapHandle = Arc<parking_lot::RwLock<HashMap<String, Arc<dyn Channel>>>>;

/// Create a new empty [`ChannelMapHandle`].
pub fn new_channel_map() -> ChannelMapHandle {
    Arc::new(parking_lot::RwLock::new(HashMap::new()))
}
