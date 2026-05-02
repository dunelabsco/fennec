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
    pub signal: SignalChannelEntry,
}

impl Default for ChannelsConfig {
    fn default() -> Self {
        Self {
            telegram: ChannelEntry::default(),
            discord: ChannelEntry::default(),
            slack: SlackChannelEntry::default(),
            whatsapp: WhatsAppChannelEntry::default(),
            email: EmailChannelEntry::default(),
            signal: SignalChannelEntry::default(),
        }
    }
}

/// Signal channel — connects to a `signal-cli` daemon over HTTP.
///
/// The daemon must be running externally:
///
/// ```text
/// signal-cli daemon --http=127.0.0.1:8080
/// ```
///
/// Fennec speaks JSON-RPC 2.0 to `POST /api/v1/rpc` for outbound
/// sends and consumes the SSE stream at `GET /api/v1/events` for
/// inbound. There is no auth — the daemon's security model is
/// "bind to localhost"; do not expose `http_url` to the network.
///
/// Group messages are opt-in via `group_allowed_users`. Default
/// is empty (groups disabled). Set `*` to allow every group, or a
/// comma-separated list of group ids for explicit allowlisting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SignalChannelEntry {
    /// Master switch.
    pub enabled: bool,
    /// signal-cli HTTP daemon URL. Default `http://127.0.0.1:8080`.
    pub http_url: String,
    /// Sender's E.164 phone number, e.g. `+15551234567`.
    pub account: String,
    /// DM allowlist — Signal phone numbers / UUIDs that may
    /// interact with the agent. Empty list means everyone.
    pub allowed_users: Vec<String>,
    /// Group allowlist — group ids (base64) or `*` for all groups.
    /// Empty disables groups entirely (default).
    pub group_allowed_users: Vec<String>,
    /// Drop incoming Signal stories. Default true.
    pub ignore_stories: bool,
    /// Default destination for `send_message` calls without an
    /// explicit chat id. Empty → falls back to most-recent inbound.
    pub home_chat_id: String,
}

impl Default for SignalChannelEntry {
    fn default() -> Self {
        Self {
            enabled: false,
            http_url: "http://127.0.0.1:8080".to_string(),
            account: String::new(),
            allowed_users: Vec::new(),
            group_allowed_users: Vec::new(),
            ignore_stories: true,
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
