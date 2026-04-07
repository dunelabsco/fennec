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
            model: "claude-sonnet-4-20250514".to_string(),
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
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            forbidden_paths: vec!["/etc", "/root", "/boot", "/dev", "/proc", "/sys"]
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
}

impl Default for ChannelsConfig {
    fn default() -> Self {
        Self {
            telegram: ChannelEntry::default(),
            discord: ChannelEntry::default(),
            slack: SlackChannelEntry::default(),
            whatsapp: WhatsAppChannelEntry::default(),
            email: EmailChannelEntry::default(),
        }
    }
}

/// Generic channel entry (Telegram, Discord).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ChannelEntry {
    pub enabled: bool,
    pub token: String,
    pub allowed_users: Vec<String>,
}

/// Slack-specific channel entry (requires bot_token + app_token).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SlackChannelEntry {
    pub enabled: bool,
    pub bot_token: String,
    pub app_token: String,
    pub allowed_users: Vec<String>,
}

/// WhatsApp Cloud API channel entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WhatsAppChannelEntry {
    pub enabled: bool,
    pub phone_number_id: String,
    pub access_token: String,
    pub verify_token: String,
    pub webhook_port: u16,
    pub allowed_users: Vec<String>,
}

impl Default for WhatsAppChannelEntry {
    fn default() -> Self {
        Self {
            enabled: false,
            phone_number_id: String::new(),
            access_token: String::new(),
            verify_token: String::new(),
            webhook_port: 9443,
            allowed_users: Vec::new(),
        }
    }
}

/// Email channel entry (IMAP polling + SMTP sending).
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
