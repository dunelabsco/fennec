pub mod traits;
pub mod cli;
pub mod manager;
pub mod telegram;

pub use traits::{Channel, ChannelMessage, SendMessage};
pub use cli::CliChannel;
pub use manager::ChannelManager;
pub use telegram::TelegramChannel;
