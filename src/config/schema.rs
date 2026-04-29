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

    /// Resolve the home directory for a named profile.
    ///
    /// Profiles are a friendly convention layered on top of
    /// [`Self::resolve_home`]: instead of typing the full
    /// `--config-dir ~/.fennec/profiles/work` every time, the user
    /// types `--profile work` and we resolve the same path. The base
    /// directory follows the normal `$FENNEC_HOME` / `~/.fennec`
    /// resolution; the profile name is appended under a `profiles/`
    /// subdirectory.
    ///
    /// `name` is validated to keep it usable as a directory component
    /// — alphanumeric plus `-` and `_`, length 1-64. This rejects
    /// path-traversal attempts (`..`, `/foo`), surprising whitespace,
    /// and other shenanigans before we ever touch the filesystem.
    /// Returns `Err` with a clear message on rejection.
    pub fn resolve_profile_home(name: &str) -> Result<PathBuf> {
        validate_profile_name(name)?;
        let base = Self::resolve_home(None);
        Ok(base.join("profiles").join(name))
    }

    /// Load configuration from a TOML file at `path`.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: FennecConfig = toml::from_str(&contents)?;
        Ok(config)
    }
}

/// Profile name validation. Accepts ASCII alphanumeric + `-` + `_`,
/// 1-64 characters. Rejects everything else with a contextual error.
///
/// The constraints exist for two reasons:
///
/// 1. **Path safety.** A profile name is interpolated into a directory
///    path. Without validation, `--profile ../../../etc` would resolve
///    to a path outside `~/.fennec/profiles/` and we'd happily try to
///    write a `.key` file there. Restricting to `[A-Za-z0-9_-]` makes
///    every accepted name a valid single path component on every
///    supported platform, with no separator characters and no parent
///    references.
/// 2. **Predictability across shells.** Profile names show up in
///    shell history, scripts, and systemd unit names. Disallowing
///    spaces and quote characters means an operator can copy-paste
///    a profile name into any context without quoting hazards.
///
/// The 64-char cap is well above any realistic profile name and well
/// below any path-length limit. Empty names are rejected because they
/// would resolve to `~/.fennec/profiles/`, which is the parent of all
/// profile dirs and not a valid state directory itself.
fn validate_profile_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("profile name cannot be empty");
    }
    if name.len() > 64 {
        anyhow::bail!(
            "profile name '{}' is too long ({} chars; max 64)",
            name,
            name.len()
        );
    }
    for (i, ch) in name.chars().enumerate() {
        let ok = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_';
        if !ok {
            anyhow::bail!(
                "profile name '{}' contains invalid character '{}' at position {}; \
                 allowed: ASCII letters, digits, '-', '_'",
                name,
                ch,
                i
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn validates_simple_name() {
        assert!(validate_profile_name("work").is_ok());
        assert!(validate_profile_name("personal").is_ok());
        assert!(validate_profile_name("test-1").is_ok());
        assert!(validate_profile_name("user_2").is_ok());
        assert!(validate_profile_name("a").is_ok());
        assert!(validate_profile_name("ABC123").is_ok());
    }

    #[test]
    fn rejects_empty_name() {
        let err = validate_profile_name("").unwrap_err().to_string();
        assert!(err.contains("empty"), "expected 'empty' in: {}", err);
    }

    #[test]
    fn rejects_path_traversal() {
        // Both `..` and slash-bearing names must be rejected — these are
        // the realistic attack shapes against an unvalidated profile name.
        assert!(validate_profile_name("..").is_err());
        assert!(validate_profile_name("../../etc").is_err());
        assert!(validate_profile_name("foo/bar").is_err());
        assert!(validate_profile_name("/absolute").is_err());
    }

    #[test]
    fn rejects_whitespace_and_quotes() {
        assert!(validate_profile_name("with space").is_err());
        assert!(validate_profile_name("with\ttab").is_err());
        assert!(validate_profile_name("with'quote").is_err());
        assert!(validate_profile_name("with\"dquote").is_err());
    }

    #[test]
    fn rejects_overlong_name() {
        let long = "a".repeat(65);
        let err = validate_profile_name(&long).unwrap_err().to_string();
        assert!(err.contains("too long"), "expected 'too long' in: {}", err);
        // Boundary: exactly 64 chars is fine.
        let limit = "a".repeat(64);
        assert!(validate_profile_name(&limit).is_ok());
    }

    #[test]
    fn rejects_dotfiles() {
        // A leading dot would create a hidden profile dir which conflicts
        // with the convention that everything visible under
        // `profiles/` is a real profile. Rejected by the alphanumeric
        // rule (`.` isn't alphanumeric or `-`/`_`).
        assert!(validate_profile_name(".hidden").is_err());
    }

    #[test]
    fn resolve_profile_home_appends_under_profiles() {
        // We don't assert the absolute base here because it depends on
        // $HOME / $FENNEC_HOME at test time; we just check the relative
        // shape: <base>/profiles/<name>.
        let resolved = FennecConfig::resolve_profile_home("work").unwrap();
        let s = resolved.to_string_lossy();
        assert!(
            s.ends_with("/profiles/work"),
            "expected path to end with /profiles/work, got: {}",
            s
        );
    }

    #[test]
    fn resolve_profile_home_rejects_invalid_name() {
        assert!(FennecConfig::resolve_profile_home("..").is_err());
        assert!(FennecConfig::resolve_profile_home("with/slash").is_err());
        assert!(FennecConfig::resolve_profile_home("").is_err());
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
