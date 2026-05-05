use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level Fennec configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FennecConfig {
    pub identity: IdentityConfig,
    pub provider: ProviderConfig,
    pub memory: MemoryConfig,
    pub security: SecurityConfig,
    pub agent: AgentConfig,
    pub channels: ChannelsConfig,
    pub gateway: GatewayConfig,
    pub cron: CronConfig,
    pub collective: CollectiveConfig,
}

impl Default for FennecConfig {
    fn default() -> Self {
        Self {
            identity: IdentityConfig::default(),
            provider: ProviderConfig::default(),
            memory: MemoryConfig::default(),
            security: SecurityConfig::default(),
            agent: AgentConfig::default(),
            channels: ChannelsConfig::default(),
            gateway: GatewayConfig::default(),
            cron: CronConfig::default(),
            collective: CollectiveConfig::default(),
        }
    }
}

impl FennecConfig {
    /// Resolve the Fennec home directory.
    /// Priority: override arg > $FENNEC_HOME > ~/.fennec
    pub fn resolve_home(override_path: Option<&str>) -> PathBuf {
        if let Some(p) = override_path {
            return PathBuf::from(p);
        }
        if let Ok(env_home) = std::env::var("FENNEC_HOME") {
            return PathBuf::from(env_home);
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".fennec")
    }

    /// Load configuration from a TOML file at `path`.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: FennecConfig = toml::from_str(&contents)?;
        Ok(config)
    }
}

/// Identity configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IdentityConfig {
    pub name: String,
    pub persona: String,
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            name: "Fennec".to_string(),
            persona: "Your personal AI agent — sharp, resourceful, and always on.".to_string(),
        }
    }
}

/// LLM provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub name: String,
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub temperature: f64,
    pub max_tokens: u32,
    pub fallback_models: Vec<String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            api_key: String::new(),
            base_url: String::new(),
            temperature: 0.7,
            max_tokens: 8192,
            fallback_models: Vec::new(),
        }
    }
}

/// Memory subsystem configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    pub db_path: Option<String>,
    pub vector_weight: f64,
    pub keyword_weight: f64,
    pub half_life_days: f64,
    pub cache_max: usize,
    pub context_limit: usize,
    pub embedding_provider: String,
    pub embedding_api_key: String,
    pub consolidation_enabled: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            db_path: None,
            vector_weight: 0.7,
            keyword_weight: 0.3,
            half_life_days: 7.0,
            cache_max: 10000,
            context_limit: 5,
            embedding_provider: "noop".to_string(),
            embedding_api_key: String::new(),
            consolidation_enabled: true,
        }
    }
}

/// Security configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub prompt_guard_action: String,
    pub prompt_guard_sensitivity: f64,
    pub encrypt_secrets: bool,
    pub command_allowlist: Vec<String>,
    pub forbidden_paths: Vec<String>,
    pub command_timeout_secs: u64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            prompt_guard_action: "warn".to_string(),
            prompt_guard_sensitivity: 0.7,
            encrypt_secrets: true,
            command_allowlist: vec![
                "git", "ls", "cat", "grep", "find", "echo", "pwd", "wc", "head", "tail", "date",
                "df", "du", "uname", "cargo", "npm", "node", "python", "python3", "pip",
                "curl", "wget", "which", "env", "sort", "uniq", "tr", "cut", "sed", "awk",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            forbidden_paths: vec![
                "/etc", "/root", "/boot", "/dev", "/proc", "/sys",
                ".fennec/config.toml", ".fennec/.secret_key", ".fennec/.anthropic_oauth.json",
                ".ssh", ".gnupg", ".aws", ".config/gcloud",
            ]
                .into_iter()
                .map(String::from)
                .collect(),
            command_timeout_secs: 60,
        }
    }
}

/// Agent behaviour configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub max_tool_iterations: u32,
    pub context_window: u64,
    pub compression_threshold: f64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_tool_iterations: 15,
            context_window: 200_000,
            compression_threshold: 0.50,
        }
    }
}

/// Channel configuration for all supported messaging channels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelsConfig {
    pub telegram: ChannelEntry,
    pub discord: ChannelEntry,
    pub slack: SlackChannelEntry,
    pub whatsapp: WhatsAppChannelEntry,
    pub email: EmailChannelEntry,
    pub matrix: MatrixChannelEntry,
}

impl Default for ChannelsConfig {
    fn default() -> Self {
        Self {
            telegram: ChannelEntry::default(),
            discord: ChannelEntry::default(),
            slack: SlackChannelEntry::default(),
            whatsapp: WhatsAppChannelEntry::default(),
            email: EmailChannelEntry::default(),
            matrix: MatrixChannelEntry::default(),
        }
    }
}

/// Matrix channel — speaks the Matrix Client-Server API directly
/// (no SDK dependency). Auth is either a static `access_token` or
/// `user_id`+`password` (which the channel exchanges for a token at
/// startup). Provide `device_id` for stable session identity.
///
/// Room visibility:
///   - `allowed_users`: Matrix user-id allowlist for DM senders. Empty
///     means everyone is allowed. Format: `@user:server`.
///   - `allowed_rooms`: room-id allowlist. Empty disables group rooms;
///     `*` allows all; otherwise an explicit list of `!roomId:server`.
///   - `free_response_rooms`: rooms in which the bot replies even
///     without an explicit `@mention` (subset of `allowed_rooms`).
///
/// Threading:
///   - `auto_thread` (default true): in non-free rooms, replies are
///     wrapped in a thread anchored on the inbound mention.
///   - `dm_auto_thread` (default false): same behavior in DMs.
///   - `dm_mention_threads` (default false): in DMs, only thread when
///     the inbound message contains a mention.
///
/// `markdown_to_html` (default true) controls whether the agent's
/// markdown output is rendered to `formatted_body` HTML alongside the
/// plain `body`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MatrixChannelEntry {
    pub enabled: bool,
    /// Homeserver base URL, e.g. `https://matrix.org`.
    pub homeserver: String,
    /// Long-lived access token. Preferred over password.
    pub access_token: String,
    /// Bot user-id, e.g. `@fennec:matrix.org`. Required.
    pub user_id: String,
    /// Password (only used when `access_token` is empty).
    pub password: String,
    /// Stable device id; recommended even without E2EE so the
    /// homeserver doesn't churn devices on each restart.
    pub device_id: String,
    /// DM allowlist (matrix user-ids). Empty means everyone.
    pub allowed_users: Vec<String>,
    /// Room allowlist (`!roomId:server` or `*` or empty).
    pub allowed_rooms: Vec<String>,
    /// Rooms where the bot answers without requiring an `@mention`.
    pub free_response_rooms: Vec<String>,
    /// Whether to require an `@mention` in non-free rooms. Default true.
    pub require_mention: bool,
    /// Auto-thread replies in non-free rooms. Default true.
    pub auto_thread: bool,
    /// Auto-thread replies in DMs. Default false.
    pub dm_auto_thread: bool,
    /// In DMs, only thread when the inbound has a mention. Default false.
    pub dm_mention_threads: bool,
    /// Render markdown to HTML `formatted_body`. Default true.
    pub markdown_to_html: bool,
    /// Optional directory for caching inbound media files
    /// (`m.image` / `m.file` / `m.audio` / `m.video`). When set,
    /// the channel downloads the binary on receipt and surfaces the
    /// local path in `metadata.matrix_media_path` so vision /
    /// transcription tools can consume it directly. Empty disables
    /// auto-download (only the `mxc://` URL is surfaced).
    pub media_cache_dir: String,
    /// Default destination for `send_message` calls without a chat
    /// id. Empty falls back to most-recent inbound.
    pub home_chat_id: String,
}

impl Default for MatrixChannelEntry {
    fn default() -> Self {
        Self {
            enabled: false,
            homeserver: String::new(),
            access_token: String::new(),
            user_id: String::new(),
            password: String::new(),
            device_id: String::new(),
            allowed_users: Vec::new(),
            allowed_rooms: Vec::new(),
            free_response_rooms: Vec::new(),
            require_mention: true,
            auto_thread: true,
            dm_auto_thread: false,
            dm_mention_threads: false,
            markdown_to_html: true,
            media_cache_dir: String::new(),
            home_chat_id: String::new(),
        }
    }
}

/// Generic channel entry (Telegram, Discord).
///
/// `home_chat_id`, when set, is the default destination the agent uses
/// when it calls `send_message` without an explicit chat_id (e.g. the
/// LLM asks to "send a reminder to telegram"). Empty string means no
/// home chat is configured; the tool then falls back to the most
/// recently-seen chat on this channel, or refuses if nothing has been
/// seen yet.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ChannelEntry {
    pub enabled: bool,
    pub token: String,
    pub allowed_users: Vec<String>,
    pub home_chat_id: String,
}

/// Slack-specific channel entry (requires bot_token + app_token).
///
/// See [`ChannelEntry::home_chat_id`] for the role of `home_chat_id`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SlackChannelEntry {
    pub enabled: bool,
    pub bot_token: String,
    pub app_token: String,
    pub allowed_users: Vec<String>,
    pub home_chat_id: String,
}

/// WhatsApp Cloud API channel entry.
///
/// `app_secret` is the Meta App Secret used to verify the
/// `X-Hub-Signature-256` HMAC on incoming webhook POSTs. If empty, signature
/// verification is skipped (for dev / not yet configured) and a warning is
/// logged at startup — do not leave it empty for internet-reachable deploys.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WhatsAppChannelEntry {
    pub enabled: bool,
    pub phone_number_id: String,
    pub access_token: String,
    pub verify_token: String,
    pub app_secret: String,
    pub webhook_port: u16,
    pub allowed_users: Vec<String>,
    /// See [`ChannelEntry::home_chat_id`] for semantics.
    pub home_chat_id: String,
}

impl Default for WhatsAppChannelEntry {
    fn default() -> Self {
        Self {
            enabled: false,
            phone_number_id: String::new(),
            access_token: String::new(),
            verify_token: String::new(),
            app_secret: String::new(),
            webhook_port: 9443,
            allowed_users: Vec::new(),
            home_chat_id: String::new(),
        }
    }
}

/// Email channel entry (IMAP polling + SMTP sending).
///
/// See [`ChannelEntry::home_chat_id`] for the role of `home_chat_id` —
/// for email this is typically the user's own email address.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmailChannelEntry {
    pub enabled: bool,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_user: String,
    pub imap_password: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_password: String,
    pub from_address: String,
    pub allowed_senders: Vec<String>,
    pub poll_interval_secs: u64,
    pub home_chat_id: String,
}

impl Default for EmailChannelEntry {
    fn default() -> Self {
        Self {
            enabled: false,
            imap_host: String::new(),
            imap_port: 993,
            imap_user: String::new(),
            imap_password: String::new(),
            smtp_host: String::new(),
            smtp_port: 587,
            smtp_user: String::new(),
            smtp_password: String::new(),
            from_address: String::new(),
            allowed_senders: Vec::new(),
            poll_interval_secs: 30,
            home_chat_id: String::new(),
        }
    }
}

/// HTTP gateway configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayConfig {
    pub host: String,
    pub port: u16,
    pub auth_token: String,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8990,
            auth_token: String::new(),
        }
    }
}

/// Cron scheduler configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CronConfig {
    pub enabled: bool,
}

impl Default for CronConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

/// Collective intelligence configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CollectiveConfig {
    pub enabled: bool,
    pub api_key: String,
    pub base_url: String,
    pub publish_enabled: bool,
    pub search_enabled: bool,
    pub cache_ttl_days: u64,
}

impl Default for CollectiveConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: String::new(),
            base_url: "https://api.plurum.ai".to_string(),
            publish_enabled: true,
            search_enabled: true,
            cache_ttl_days: 30,
        }
    }
}
