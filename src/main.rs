use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use fennec::agent::AgentBuilder;
use fennec::auth;
use fennec::bus::MessageBus;
use fennec::channels::cli::CliChannel;
use fennec::channels::traits::{Channel, SendMessage};
use fennec::channels::ChannelManager;
use fennec::collective::cache::CollectiveCache;
use fennec::collective::mock::MockCollective;
use fennec::collective::plurum::PlurumlClient;
use fennec::collective::search::CollectiveSearch;
use fennec::collective::traits::CollectiveLayer;
use fennec::config::FennecConfig;
use fennec::cron::{CronScheduler, JobStore};
use fennec::gateway::GatewayServer;
use fennec::memory::embedding::{NoopEmbedding, OpenAIEmbedding};
use fennec::memory::snapshot;
use fennec::memory::sqlite::SqliteMemory;
use fennec::memory::Memory;
use fennec::providers::anthropic::AnthropicProvider;
use fennec::providers::openai::OpenAIProvider;
use fennec::providers::ollama::OllamaProvider;
use fennec::providers::traits::Provider;
use fennec::security::prompt_guard::{GuardAction, PromptGuard};
use fennec::security::{PathSandbox, SecretStore};
use fennec::tools::collective_tools::{CollectiveGetExperienceTool, CollectivePublishTool, CollectiveReportTool, CollectiveSearchTool};
use fennec::tools::cron_tool::{CronOrigin, CronTool};
use fennec::tools::files::{ListDirTool, ReadFileTool, WriteFileTool};
use fennec::tools::memory_tools::{MemoryForgetTool, MemoryRecallTool, MemoryStoreTool};
use fennec::tools::send_message_tool::SendMessageTool;
use fennec::tools::shell::ShellTool;
use fennec::tools::todo_tool::TodoTool;
use fennec::tools::ask_user_tool::AskUserTool;
use fennec::channels::{ChannelMapHandle, new_channel_map};
use fennec::tools::web::{WebFetchTool, WebSearchTool};
use fennec::tools::browser_tool::BrowserTool;
use fennec::tools::vision_tool::VisionTool;
use fennec::tools::image_gen_tool::{default_output_dir as image_output_dir, ImageGenTool};
use fennec::tools::code_exec_tool::CodeExecTool;
use fennec::tools::voice_tool::{
    default_tts_output_dir, resolve_openai_key as voice_resolve_openai_key,
    TextToSpeechTool, TranscribeAudioTool,
};
use fennec::tools::pdf_read_tool::PdfReadTool;
use fennec::tools::screenshot_tool::{default_screenshot_dir, ScreenshotTool};
use fennec::tools::http_request_tool::HttpRequestTool;
use fennec::tools::weather_tool::WeatherTool;
use fennec::tools::image_info_tool::ImageInfoTool;
use fennec::tools::claude_code_cli_tool::ClaudeCodeCliTool;
use fennec::skills::{Skill, SkillsLoader};
use fennec::tools::skills_tool::SkillsTool;
use fennec::tools::delegate_tool::DelegateTool;
use fennec::tools::traits::Tool;

#[derive(Parser, Debug)]
#[command(name = "fennec", version, about = "The fastest personal AI agent with collective intelligence")]
struct Cli {
    #[arg(long, global = true)]
    config_dir: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start interactive agent session
    Agent {
        /// Single-shot message (non-interactive)
        #[arg(short, long)]
        message: Option<String>,

        /// Override the LLM model
        #[arg(long)]
        model: Option<String>,
    },
    /// Show agent status
    Status,
    /// Start gateway serving all configured channels
    Gateway {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
    },
    /// Run interactive setup wizard
    Onboard {
        /// Overwrite existing config
        #[arg(long)]
        force: bool,
    },
    /// Authenticate with Anthropic via OAuth
    Login,
    /// Run diagnostic checks — provider reachability, API key, memory DB, Plurum, config.
    Doctor,
}

/// Resolve the API key from config or provider-specific environment variable.
fn resolve_api_key(config: &FennecConfig, secret_store: &SecretStore) -> Result<String> {
    // Try config value first.
    if !config.provider.api_key.is_empty() {
        let decrypted = secret_store
            .decrypt(&config.provider.api_key)
            .context("decrypting API key from config")?;
        return Ok(decrypted);
    }

    // Fall back to provider-specific environment variable.
    let env_var = match config.provider.name.as_str() {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "kimi" | "moonshot" => "KIMI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "ollama" => return Ok(String::new()), // Ollama needs no key
        _ => "ANTHROPIC_API_KEY",
    };

    std::env::var(env_var)
        .with_context(|| format!("API key not found: set provider.api_key in config or {} env var", env_var))
}

/// Build the LLM provider based on config.
fn build_provider(
    config: &FennecConfig,
    api_key: String,
    model_override: Option<String>,
) -> Box<dyn Provider> {
    let model = model_override.unwrap_or_else(|| config.provider.model.clone());
    let base_url = if config.provider.base_url.is_empty() {
        None
    } else {
        Some(config.provider.base_url.clone())
    };

    match config.provider.name.as_str() {
        "anthropic" => {
            Box::new(AnthropicProvider::new(api_key, Some(model)))
        }
        "openai" => {
            Box::new(OpenAIProvider::new(api_key, Some(model), base_url, None))
        }
        "kimi" | "moonshot" => {
            let kimi_url = base_url.unwrap_or_else(|| {
                // Route by key prefix: sk-kimi-* → api.kimi.com, otherwise → api.moonshot.ai
                if api_key.starts_with("sk-kimi-") {
                    "https://api.kimi.com/coding/v1".to_string()
                } else {
                    "https://api.moonshot.ai/v1".to_string()
                }
            });
            // Detect "user kept the Anthropic-flavored default and switched
            // provider to Kimi" — fall back to a Kimi-shaped default. We
            // accept BOTH the new default (claude-sonnet-4-6, current) and
            // the old default (claude-sonnet-4-20250514, deprecated June
            // 2026) so users whose configs still hold the legacy string
            // continue to get the right behavior.
            let kimi_model = if model.is_empty()
                || model == "claude-sonnet-4-6"
                || model == "claude-sonnet-4-20250514"
                || model == "moonshot-v1-128k"
            {
                "kimi-k2.5".to_string()
            } else {
                model
            };
            Box::new(OpenAIProvider::new(api_key, Some(kimi_model), Some(kimi_url), Some(262_144)))
        }
        "openrouter" => {
            let or_url = base_url.unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
            Box::new(OpenAIProvider::new(api_key, Some(model), Some(or_url), None))
        }
        "ollama" => {
            // Same back-compat as kimi: accept both new and old Anthropic
            // defaults when falling back to Ollama's local default.
            let ollama_model = if model.is_empty()
                || model == "claude-sonnet-4-6"
                || model == "claude-sonnet-4-20250514"
            {
                "llama3.1".to_string()
            } else {
                model
            };
            Box::new(OllamaProvider::new(
                Some(ollama_model),
                base_url,
                None,
            ))
        }
        other => {
            // Unknown provider — treat as OpenAI-compatible with custom base_url
            tracing::warn!("Unknown provider '{other}', treating as OpenAI-compatible");
            Box::new(OpenAIProvider::new(api_key, Some(model), base_url, None))
        }
    }
}

/// Parse the guard action string from config into a GuardAction enum.
fn parse_guard_action(action: &str) -> GuardAction {
    match action.to_lowercase().as_str() {
        "block" => GuardAction::Block,
        "sanitize" => GuardAction::Sanitize,
        _ => GuardAction::Warn,
    }
}

/// Build an Agent from configuration. Shared by both `run_agent` and `run_gateway`.
///
/// Returns `(agent, memory, cron_origin_handle, pending_replies, chat_directory)`.
/// The latter two are used by the gateway's inbound dispatch:
///
///   - `pending_replies` lets `ask_user`-style tools register a wait
///     for the next inbound from a specific chat. The gateway's main
///     loop must call `take_and_deliver` *before* forwarding to the
///     agent; if a waiter is registered, the message is consumed by
///     the tool and not turned into a fresh agent turn.
///   - `chat_directory` is the source of truth for the `send_message`
///     tool's `list` action and for resolving "send to telegram" without
///     an explicit chat_id. The gateway's main loop must call
///     `record(channel, chat_id)` on every inbound (including those
///     consumed by pending replies, so the directory stays up to date).
// `plugin_bus` is the bus handle plumbed into WASM plugin host
// imports. `None` in CLI mode (no channels); `Some(bus.clone())` in
// gateway mode so plugins can use `channel-send`.
async fn build_agent(
    config: &FennecConfig,
    home_dir: &std::path::Path,
    model_override: Option<String>,
    channel_map: Option<ChannelMapHandle>,
    plugin_bus: Option<MessageBus>,
) -> Result<(
    fennec::agent::Agent,
    Arc<dyn Memory>,
    Arc<Mutex<Option<CronOrigin>>>,
    fennec::bus::PendingReplies,
    fennec::bus::ChatDirectory,
)> {
    // Create SecretStore.
    let secret_store =
        SecretStore::new(home_dir.to_path_buf()).context("creating secret store")?;

    // Resolve provider: for Anthropic, try OAuth token first, then fall back to API key.
    let provider: Box<dyn Provider> = if config.provider.name == "anthropic" {
        if let Ok(Some(oauth_token)) = auth::load_oauth_token(home_dir) {
            tracing::info!("Using Anthropic OAuth token");
            let model = model_override.unwrap_or_else(|| config.provider.model.clone());
            Box::new(AnthropicProvider::new_with_oauth(oauth_token, Some(model)))
        } else {
            let api_key = resolve_api_key(config, &secret_store)?;
            build_provider(config, api_key, model_override)
        }
    } else {
        let api_key = resolve_api_key(config, &secret_store)?;
        build_provider(config, api_key, model_override)
    };

    // Promote the provider to `Arc` so it can be shared with DelegateTool
    // (which needs its own handle for sub-agent runs) while still being
    // passed into the AgentBuilder.
    let provider: Arc<dyn Provider> = Arc::from(provider);

    // Create embedding provider based on config.
    let embedder: Arc<dyn fennec::memory::embedding::EmbeddingProvider> =
        match config.memory.embedding_provider.as_str() {
            "openai" => {
                let embedding_key = if !config.memory.embedding_api_key.is_empty() {
                    config.memory.embedding_api_key.clone()
                } else {
                    std::env::var("OPENAI_API_KEY").unwrap_or_default()
                };
                if embedding_key.is_empty() {
                    tracing::warn!("embedding_provider is 'openai' but no API key found; falling back to noop");
                    Arc::new(NoopEmbedding::new(1536))
                } else {
                    Arc::new(OpenAIEmbedding::new(embedding_key, None, None, None))
                }
            }
            _ => Arc::new(NoopEmbedding::new(1536)),
        };

    // Create SqliteMemory.
    let db_path = config
        .memory
        .db_path
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home_dir.join("memory").join("brain.db"));
    let memory = SqliteMemory::new(
        db_path.clone(),
        config.memory.vector_weight as f32,
        config.memory.keyword_weight as f32,
        config.memory.cache_max,
        embedder,
    )
    .context("creating sqlite memory")?;
    let memory = Arc::new(memory);

    // Soul snapshot: hydrate from snapshot if DB is fresh and snapshot exists.
    let snapshot_path = home_dir.join("MEMORY_SNAPSHOT.md");
    let db_is_new = !db_path.exists() || {
        let entries = memory.list(None, 1).await.unwrap_or_default();
        entries.is_empty()
    };
    if db_is_new && snapshot_path.exists() {
        match snapshot::hydrate_from_snapshot(memory.as_ref(), &snapshot_path).await {
            Ok(count) => {
                tracing::info!(count, "hydrated memories from snapshot");
            }
            Err(e) => {
                tracing::warn!("failed to hydrate from snapshot: {e}");
            }
        }
    }

    // Create tools.
    let shell_tool = ShellTool::new(
        config.security.command_allowlist.clone(),
        config.security.forbidden_paths.clone(),
        config.security.command_timeout_secs,
    );
    // Shared filesystem sandbox driven by `security.forbidden_paths` from
    // config. Every tool that takes a file path from the LLM clones this
    // Arc so the denylist is enforced in one place.
    let path_sandbox = Arc::new(PathSandbox::new(config.security.forbidden_paths.clone()));
    let read_file_tool = ReadFileTool::new().with_sandbox(Arc::clone(&path_sandbox));
    let write_file_tool = WriteFileTool::new().with_sandbox(Arc::clone(&path_sandbox));
    let list_dir_tool = ListDirTool::new().with_sandbox(Arc::clone(&path_sandbox));
    let web_fetch_tool = WebFetchTool::new();
    let web_search_tool = WebSearchTool::new();
    let browser_tool = BrowserTool::new();

    // Vision tool: only wired when the configured provider supports vision
    // (anthropic, openai) AND we can resolve an API key. OAuth-only users and
    // non-vision providers silently skip it.
    let vision_api_key = resolve_api_key(config, &secret_store)
        .ok()
        .unwrap_or_default();
    let vision_tool = VisionTool::from_provider(
        &config.provider.name,
        vision_api_key,
        Some(config.provider.model.clone()),
    )
    .map(|t| t.with_sandbox(Arc::clone(&path_sandbox)));
    match &vision_tool {
        Some(_) => tracing::info!("Vision tool enabled ({})", config.provider.name),
        None => tracing::debug!(
            "Vision tool disabled: provider '{}' unsupported or no API key",
            config.provider.name
        ),
    }

    // Image generation tool: independent of the primary provider. Pulls an
    // OpenAI key from config (when provider is openai) or OPENAI_API_KEY env.
    // Users with Anthropic as primary can still generate images via their
    // OpenAI key.
    let img_config_key = resolve_api_key(config, &secret_store)
        .ok()
        .unwrap_or_default();
    let openai_key = ImageGenTool::resolve_openai_key(&config.provider.name, &img_config_key);
    let image_gen_tool = ImageGenTool::new_with_key(
        openai_key,
        image_output_dir(home_dir),
        None,
    );
    match &image_gen_tool {
        Some(_) => tracing::info!("Image generation tool enabled (OpenAI DALL-E 3)"),
        None => tracing::debug!("Image generation tool disabled: no OpenAI API key"),
    }

    // Code execution tool (python/node/bash subprocess, no sandbox). Always
    // wired — runners are checked at call time, not startup.
    let code_exec_tool = CodeExecTool::new(
        config.security.command_timeout_secs,
        home_dir.join("codeexec"),
    );
    tracing::info!("Code execution tool enabled (python/node/bash)");

    // Voice tools (transcription + TTS). Both use OpenAI; shared key
    // resolution with the image gen tool.
    let voice_key = voice_resolve_openai_key(&config.provider.name, &img_config_key);
    let transcribe_tool = TranscribeAudioTool::new_with_key(voice_key.clone(), None)
        .map(|t| t.with_sandbox(Arc::clone(&path_sandbox)));
    let tts_tool = TextToSpeechTool::new_with_key(
        voice_key,
        default_tts_output_dir(home_dir),
        None,
        None,
    );
    match (&transcribe_tool, &tts_tool) {
        (Some(_), Some(_)) => tracing::info!("Voice tools enabled (Whisper + OpenAI TTS)"),
        _ => tracing::debug!("Voice tools disabled: no OpenAI API key"),
    }

    // Create CronTool with shared origin handle. The same handle is
    // reused by AskUserTool so it knows the (channel, chat_id) the
    // current turn came from.
    let cron_origin: Arc<Mutex<Option<CronOrigin>>> = Arc::new(Mutex::new(None));
    let cron_tool = CronTool::new(
        home_dir.join("cron_jobs.json"),
        Arc::clone(&cron_origin),
    );

    // Create prompt guard from config security settings.
    let guard_action = parse_guard_action(&config.security.prompt_guard_action);
    let prompt_guard = PromptGuard::new(guard_action, config.security.prompt_guard_sensitivity);

    // Set up collective intelligence layer.
    let collective_search: Option<Arc<CollectiveSearch>> = if config.collective.enabled {
        // Resolve collective API key. Fail-closed: if the configured value is
        // present but cannot be decrypted, treat the integration as
        // unavailable rather than leaking the encrypted blob to the
        // collective endpoint as if it were the plaintext key.
        let collective_api_key = if !config.collective.api_key.is_empty() {
            match secret_store.decrypt(&config.collective.api_key) {
                Ok(key) => key,
                Err(e) => {
                    tracing::error!(
                        "Failed to decrypt collective API key: {e}; disabling collective \
                         (set a fresh encrypted key or unset to use PLURUM_API_KEY env var)"
                    );
                    String::new()
                }
            }
        } else {
            std::env::var("PLURUM_API_KEY").unwrap_or_default()
        };

        let remote: Option<Arc<dyn CollectiveLayer>> = if !collective_api_key.is_empty() {
            let base_url = if config.collective.base_url.is_empty() {
                None
            } else {
                Some(config.collective.base_url.clone())
            };
            let client = PlurumlClient::new(collective_api_key, base_url);
            tracing::info!("Collective intelligence enabled (Plurum remote)");
            Some(Arc::new(client))
        } else {
            tracing::info!("Collective intelligence enabled (local only, no API key)");
            None
        };

        let cache = CollectiveCache::new(memory.clone());
        let search_guard = PromptGuard::new(
            parse_guard_action(&config.security.prompt_guard_action),
            config.security.prompt_guard_sensitivity,
        );
        let search = CollectiveSearch::new(
            memory.clone(),
            cache,
            remote,
            Some(search_guard),
        );
        Some(Arc::new(search))
    } else {
        tracing::debug!("Collective intelligence disabled");
        None
    };

    // Build Agent.
    let mut builder = AgentBuilder::new()
        .provider(Arc::clone(&provider))
        .memory(memory.clone())
        .tool(Box::new(shell_tool))
        .tool(Box::new(read_file_tool))
        .tool(Box::new(write_file_tool))
        .tool(Box::new(list_dir_tool))
        .tool(Box::new(web_fetch_tool))
        .tool(Box::new(web_search_tool))
        .tool(Box::new(browser_tool))
        .tool(Box::new(cron_tool))
        .tool(Box::new(MemoryStoreTool::new(memory.clone())))
        .tool(Box::new(MemoryRecallTool::new(memory.clone())))
        .tool(Box::new(MemoryForgetTool::new(memory.clone())))
        .tool(Box::new(TodoTool::new()));

    if let Some(vt) = vision_tool {
        builder = builder.tool(Box::new(vt));
    }
    if let Some(igt) = image_gen_tool {
        builder = builder.tool(Box::new(igt));
    }
    builder = builder.tool(Box::new(code_exec_tool));
    builder = builder.tool(Box::new(
        PdfReadTool::new(home_dir.join("pdf_cache")).with_sandbox(Arc::clone(&path_sandbox)),
    ));
    builder = builder.tool(Box::new(ScreenshotTool::new(default_screenshot_dir(home_dir))));
    builder = builder.tool(Box::new(HttpRequestTool::new()));
    builder = builder.tool(Box::new(WeatherTool::new()));
    builder = builder.tool(Box::new(
        ImageInfoTool::new(home_dir.join("image_cache")).with_sandbox(Arc::clone(&path_sandbox)),
    ));
    if let Some(claude_tool) = ClaudeCodeCliTool::detect() {
        tracing::info!("Claude Code CLI tool enabled");
        builder = builder.tool(Box::new(claude_tool));
    } else {
        tracing::debug!("Claude Code CLI tool disabled: `claude` binary not on PATH");
    }
    if let Some(t) = transcribe_tool {
        builder = builder.tool(Box::new(t));
    }
    if let Some(t) = tts_tool {
        builder = builder.tool(Box::new(t));
    }

    // Shared turn-context handles. These are returned from build_agent so
    // the gateway's inbound dispatch can keep them populated.
    let pending_replies = fennec::bus::PendingReplies::new();
    let chat_directory = fennec::bus::ChatDirectory::new();

    // Add send_message + ask_user tools when running in gateway mode
    // (channel map and turn-origin available).
    if let Some(ref ch_map) = channel_map {
        // Pull each enabled channel's home_chat_id from config so the
        // send_message tool can route default-target sends ("send to
        // telegram") without the LLM having to know the chat_id.
        let mut home_chats: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if config.channels.telegram.enabled
            && !config.channels.telegram.home_chat_id.is_empty()
        {
            home_chats.insert(
                "telegram".to_string(),
                config.channels.telegram.home_chat_id.clone(),
            );
        }
        if config.channels.discord.enabled
            && !config.channels.discord.home_chat_id.is_empty()
        {
            home_chats.insert(
                "discord".to_string(),
                config.channels.discord.home_chat_id.clone(),
            );
        }
        if config.channels.slack.enabled
            && !config.channels.slack.home_chat_id.is_empty()
        {
            home_chats.insert(
                "slack".to_string(),
                config.channels.slack.home_chat_id.clone(),
            );
        }
        if config.channels.whatsapp.enabled
            && !config.channels.whatsapp.home_chat_id.is_empty()
        {
            home_chats.insert(
                "whatsapp".to_string(),
                config.channels.whatsapp.home_chat_id.clone(),
            );
        }
        if config.channels.email.enabled
            && !config.channels.email.home_chat_id.is_empty()
        {
            home_chats.insert(
                "email".to_string(),
                config.channels.email.home_chat_id.clone(),
            );
        }

        builder = builder.tool(Box::new(SendMessageTool::new(
            Arc::clone(ch_map),
            chat_directory.clone(),
            home_chats,
        )));

        builder = builder.tool(Box::new(AskUserTool::new(
            Arc::clone(ch_map),
            Arc::clone(&cron_origin),
            pending_replies.clone(),
        )));
    }

    // Add collective tools if enabled.
    if let Some(ref search) = collective_search {
        builder = builder.tool(Box::new(CollectiveSearchTool::new(Arc::clone(search))));

        // CollectiveReportTool needs a CollectiveLayer; create one based on config.
        if config.collective.publish_enabled {
            // Fail-closed on decrypt error: empty key → MockCollective fallback
            // below, so we never publish using the encrypted blob as plaintext.
            let collective_api_key = if !config.collective.api_key.is_empty() {
                match secret_store.decrypt(&config.collective.api_key) {
                    Ok(key) => key,
                    Err(e) => {
                        tracing::error!(
                            "Failed to decrypt collective API key for publish layer: {e}; \
                             falling back to MockCollective (publish disabled)"
                        );
                        String::new()
                    }
                }
            } else {
                std::env::var("PLURUM_API_KEY").unwrap_or_default()
            };

            let report_layer: Arc<dyn CollectiveLayer> = if !collective_api_key.is_empty() {
                let base_url = if config.collective.base_url.is_empty() {
                    None
                } else {
                    Some(config.collective.base_url.clone())
                };
                Arc::new(PlurumlClient::new(collective_api_key, base_url))
            } else {
                Arc::new(MockCollective::new())
            };
            builder = builder.tool(Box::new(CollectiveGetExperienceTool::new(Arc::clone(&report_layer))));
            builder = builder.tool(Box::new(CollectiveReportTool::new(Arc::clone(&report_layer))));
            builder = builder.tool(Box::new(CollectivePublishTool::new(report_layer, Arc::clone(search))));
        }
    }

    // Wire collective search into agent.
    if let Some(search) = collective_search {
        builder = builder.collective(search);
    }

    // Load skills from ~/.fennec/skills/, filter by PATH requirements, and
    // inject. Returns empty Vec if the directory doesn't exist yet.
    let skills_dir = home_dir.join("skills");
    let loaded_skills = SkillsLoader::load_from_directory(&skills_dir)
        .context("loading skills directory")?;
    let available_skills: Vec<Skill> = SkillsLoader::filter_available(&loaded_skills)
        .into_iter()
        .cloned()
        .collect();
    tracing::info!(
        total = loaded_skills.len(),
        available = available_skills.len(),
        dir = %skills_dir.display(),
        "skills loaded",
    );
    let skills_prompt = SkillsLoader::build_skills_prompt(&available_skills);
    builder = builder
        .skills_prompt(skills_prompt)
        .tool(Box::new(SkillsTool::new(available_skills)));

    // Wire DelegateTool so the agent can spawn read-only sub-agents for
    // bounded research / investigation tasks. Toolkit is intentionally
    // read-only: anything that writes files, spends money, or touches
    // live systems stays with the main agent.
    let delegate_subagent_tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ReadFileTool::new()),
        Arc::new(ListDirTool::new()),
        Arc::new(WebFetchTool::new()),
        Arc::new(WebSearchTool::new()),
    ];
    builder = builder.tool(Box::new(DelegateTool::new(
        Arc::clone(&provider),
        memory.clone(),
        delegate_subagent_tools,
    )));

    // Plugin system. Bundled plugins (compiled into the binary via
    // the `inventory` crate) are scanned here and any whose name
    // appears in `[plugins].enabled` is activated. The default
    // `enabled = []` skips the activation path entirely so installs
    // that never opt in have zero behavioural change.
    let mut plugin_registry = fennec::plugins::PluginRegistry::new();
    if let Err(e) = plugin_registry.load_bundled(&config.plugins.enabled) {
        // load_bundled() is "ok-even-if-some-fail" by design — this
        // arm only fires on a structural failure. Don't abort agent
        // startup; log and continue with no plugins.
        tracing::error!(
            "Plugin registry failed to load bundled: {e}; continuing without bundled plugins"
        );
    }

    // WASM-loaded user plugins discovered under `<home>/plugins/`.
    // Each plugin gets a clone of the host capability handles
    // (path sandbox, memory, http client, runtime handle). Discovery
    // and instantiation failures are non-fatal — one bad plugin
    // never blocks startup.
    let wasm_resources = fennec::plugins::WasmHostResources {
        path_sandbox: Arc::clone(&path_sandbox),
        memory: memory.clone(),
        http_client: fennec::tools::http::shared_client(),
        rt_handle: tokio::runtime::Handle::current(),
        settings: config.plugins.settings.clone(),
        bus: plugin_bus,
    };
    let plugins_root = home_dir.join("plugins");
    if let Err(e) = plugin_registry.load_wasm(
        &plugins_root,
        &config.plugins.enabled,
        wasm_resources,
    ) {
        tracing::error!(
            "Plugin registry failed to load WASM plugins: {e}; continuing without WASM plugins"
        );
    }

    // Resolve plugin runtime: tools, hooks, and the memory manager
    // (built from `[memory] provider = "<name>"`). Default
    // `provider = "builtin"` yields an empty manager — built-in
    // SQLite memory remains the only memory layer, behavior
    // unchanged.
    let runtime = plugin_registry.into_runtime(&config.memory.provider);
    for plugin_tool in runtime.tools {
        builder = builder.tool(plugin_tool);
    }
    builder = builder.hooks(Arc::new(runtime.hooks));
    builder = builder.memory_manager(Arc::new(runtime.memory_manager));

    let agent = builder
        .identity_name(&config.identity.name)
        .identity_persona(&config.identity.persona)
        .max_tool_iterations(config.agent.max_tool_iterations as usize)
        .max_tokens(config.provider.max_tokens as usize)
        .temperature(config.provider.temperature)
        .memory_context_limit(config.memory.context_limit)
        .half_life_days(config.memory.half_life_days)
        .prompt_guard(prompt_guard)
        .build()
        .context("building agent")?;

    Ok((agent, memory, cron_origin, pending_replies, chat_directory))
}

async fn run_agent(
    config: FennecConfig,
    home_dir: std::path::PathBuf,
    message: Option<String>,
    model: Option<String>,
) -> Result<()> {
    // CLI mode doesn't run the gateway, so the channel-aware handles are
    // unused here. We still need to bind them to suppress unused-let
    // warnings without losing the build_agent return-shape.
    let (mut agent, memory, _cron_origin, _pending_replies, _chat_directory) =
        build_agent(&config, &home_dir, model, None, None).await?;

    if let Some(msg) = message {
        // Single-shot mode.
        let response = agent.turn(&msg).await?;
        println!("{response}");
    } else {
        // Interactive mode.
        println!("Fennec v{} — interactive mode", env!("CARGO_PKG_VERSION"));
        println!("Type /quit or /exit to leave.\n");

        let channel = CliChannel::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);

        // Spawn the channel listener in the background.
        let listen_handle = tokio::spawn(async move {
            if let Err(e) = channel.listen(tx).await {
                eprintln!("Channel error: {e}");
            }
        });

        print!("You: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();

        let consolidation_enabled = config.memory.consolidation_enabled;

        // Process messages.
        while let Some(msg) = rx.recv().await {
            if msg.content.is_empty() {
                continue;
            }

            match agent.turn(&msg.content).await {
                Ok(response) => {
                    let send_msg = SendMessage::new(&response, "user");
                    // Print via channel send (writes to stdout).
                    println!("Fennec: {}", send_msg.content);

                    // Fire-and-forget consolidation hint.
                    if consolidation_enabled {
                        tracing::debug!(
                            "consolidation: would consolidate after turn (provider sharing needed for full implementation)"
                        );
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                }
            }
        }

        // Clean exit -- export soul snapshot.
        let snapshot_path = home_dir.join("MEMORY_SNAPSHOT.md");
        match snapshot::export_snapshot(memory.as_ref(), &snapshot_path).await {
            Ok(count) => {
                tracing::info!(count, path = %snapshot_path.display(), "exported soul snapshot on exit");
            }
            Err(e) => {
                tracing::warn!("failed to export soul snapshot on exit: {e}");
            }
        }

        listen_handle.abort();
    }

    Ok(())
}

async fn run_gateway(
    config: FennecConfig,
    home_dir: std::path::PathBuf,
    host_override: Option<String>,
    port_override: Option<u16>,
) -> Result<()> {
    // 1. Create MessageBus and channel map handle.
    let (bus, mut receiver) = MessageBus::new(256);
    let channel_map = new_channel_map();

    // 2. Build Agent (pass channel map so ask_user tool can reach channels).
    let (agent, _memory, cron_origin, pending_replies, chat_directory) =
        build_agent(
            &config,
            &home_dir,
            None,
            Some(channel_map.clone()),
            // Plugins running under the gateway get the bus so
            // their `channel-send` host import works. The CLI
            // path passes None above.
            Some(bus.clone()),
        )
        .await?;
    let agent = Arc::new(tokio::sync::Mutex::new(agent));

    // 3. Build channel list from config.
    let mut channels: Vec<Arc<dyn Channel>> = Vec::new();

    let ch_config = &config.channels;

    if ch_config.telegram.enabled && !ch_config.telegram.token.is_empty() {
        let ch = fennec::channels::TelegramChannel::new(
            ch_config.telegram.token.clone(),
            ch_config.telegram.allowed_users.clone(),
        );
        channels.push(Arc::new(ch));
        tracing::info!("Telegram channel enabled");
    }

    if ch_config.discord.enabled && !ch_config.discord.token.is_empty() {
        let ch = fennec::channels::DiscordChannel::new(
            ch_config.discord.token.clone(),
            ch_config.discord.allowed_users.clone(),
        );
        channels.push(Arc::new(ch));
        tracing::info!("Discord channel enabled");
    }

    if ch_config.slack.enabled
        && !ch_config.slack.bot_token.is_empty()
        && !ch_config.slack.app_token.is_empty()
    {
        let ch = fennec::channels::SlackChannel::new(
            ch_config.slack.bot_token.clone(),
            ch_config.slack.app_token.clone(),
            ch_config.slack.allowed_users.clone(),
        );
        channels.push(Arc::new(ch));
        tracing::info!("Slack channel enabled");
    }

    if ch_config.whatsapp.enabled && !ch_config.whatsapp.access_token.is_empty() {
        let ch = fennec::channels::WhatsAppChannel::new(
            ch_config.whatsapp.phone_number_id.clone(),
            ch_config.whatsapp.access_token.clone(),
            ch_config.whatsapp.verify_token.clone(),
            ch_config.whatsapp.webhook_port,
            ch_config.whatsapp.allowed_users.clone(),
            ch_config.whatsapp.app_secret.clone(),
        );
        channels.push(Arc::new(ch));
        tracing::info!("WhatsApp channel enabled");
    }

    if ch_config.email.enabled
        && !ch_config.email.smtp_host.is_empty()
        && !ch_config.email.imap_host.is_empty()
    {
        let ch = fennec::channels::EmailChannel::new(
            ch_config.email.imap_host.clone(),
            ch_config.email.imap_port,
            ch_config.email.imap_user.clone(),
            ch_config.email.imap_password.clone(),
            ch_config.email.smtp_host.clone(),
            ch_config.email.smtp_port,
            ch_config.email.smtp_user.clone(),
            ch_config.email.smtp_password.clone(),
            ch_config.email.from_address.clone(),
            ch_config.email.allowed_senders.clone(),
            ch_config.email.poll_interval_secs,
        );
        channels.push(Arc::new(ch));
        tracing::info!("Email channel enabled");
    }

    // 3a. Populate the channel map so tools (e.g. ask_user) can reach channels.
    {
        let mut map = channel_map.write();
        for ch in &channels {
            map.insert(ch.name().to_string(), Arc::clone(ch));
        }
    }

    // 4. Create ChannelManager, start all channels.
    let manager = ChannelManager::new(channels, bus.clone());
    let _listener_handles = manager.start_all();
    let _dispatch_handle = manager.spawn_outbound_dispatch(receiver.outbound_rx);

    // 5. Start CronScheduler — always runs in gateway mode so user-created
    //    reminders/jobs are delivered. The config.cron.enabled flag is reserved
    //    for future heartbeat/auto-task features.
    let _cron_handle = {
        let cron_path = home_dir.join("cron_jobs.json");
        let mut store = JobStore::new(cron_path);
        if let Err(e) = store.load() {
            tracing::warn!("Failed to load cron jobs: {e}");
        }
        let mut scheduler = CronScheduler::new(store, bus.clone(), None);
        tracing::info!("Cron scheduler started");
        Some(tokio::spawn(async move {
            scheduler.run().await;
        }))
    };

    // 6. Start GatewayServer in a background task.
    let host = host_override.unwrap_or_else(|| config.gateway.host.clone());
    let port = port_override.unwrap_or(config.gateway.port);
    let addr = format!("{host}:{port}");

    let auth_token = if config.gateway.auth_token.is_empty() {
        None
    } else {
        Some(config.gateway.auth_token.clone())
    };

    let gateway = GatewayServer::new(addr);
    let gateway_agent = Arc::clone(&agent);
    let gateway_handle = tokio::spawn(async move {
        if let Err(e) = gateway.run(gateway_agent, auth_token).await {
            tracing::error!("Gateway server error: {e}");
        }
    });

    // 7. Main loop: consume inbound messages from bus, run agent, publish outbound.
    let agent_loop = {
        let agent = Arc::clone(&agent);
        let bus = bus.clone();
        let manager_ref = Arc::new(manager);
        let cron_origin = Arc::clone(&cron_origin);
        let pending_replies = pending_replies.clone();
        let chat_directory = chat_directory.clone();
        tokio::spawn(async move {
            while let Some(msg) = receiver.inbound_rx.recv().await {
                // Always record the (channel, chat_id) pair in the
                // directory, even for messages we'll consume via a
                // pending reply or treat as a /reset command. This
                // keeps the directory's "recently seen" view honest.
                chat_directory.record(&msg.channel, &msg.chat_id);

                // Cron-fired messages carry `metadata["source"] = "cron"`
                // and represent the agent's own scheduled prompt, not a
                // user reply. They must not be redirected to a pending
                // ask_user wait (a cron output happening to land while
                // the agent is mid-question would otherwise be consumed
                // as the user's "answer"). Skip the pending check for
                // anything sourced internally.
                let is_user_initiated = msg
                    .metadata
                    .get("source")
                    .map(|s| s.as_str() != "cron")
                    .unwrap_or(true);

                if is_user_initiated {
                    let pending_origin = fennec::bus::TurnOrigin {
                        channel: msg.channel.clone(),
                        chat_id: msg.chat_id.clone(),
                    };
                    if pending_replies.take_and_deliver(&pending_origin, msg.clone()) {
                        tracing::debug!(
                            "Inbound from {}:{} consumed by pending tool reply",
                            msg.channel,
                            msg.chat_id,
                        );
                        continue;
                    }
                }

                // Handle /new and /reset commands: clear agent history and
                // send a confirmation instead of running a full agent turn.
                if msg.metadata.get("command").map(|s| s.as_str()) == Some("reset") {
                    let mut agent_lock = agent.lock().await;
                    agent_lock.clear_history();
                    let outbound = fennec::bus::OutboundMessage {
                        content: "Session reset! Starting fresh.".to_string(),
                        channel: msg.channel.clone(),
                        chat_id: msg.chat_id.clone(),
                        reply_to: Some(msg.id.clone()),
                        metadata: std::collections::HashMap::new(),
                    };
                    let _ = bus.publish_outbound(outbound).await;
                    continue;
                }

                // Spawn continuous typing indicator that fires every 4 seconds
                // until the agent finishes processing.
                let typing_channel: Option<Arc<dyn Channel>> = manager_ref.get_channel(&msg.channel);
                let typing_chat_id = msg.chat_id.clone();
                let typing_handle = tokio::spawn(async move {
                    if let Some(ch) = typing_channel {
                        loop {
                            let _ = ch.send_typing(&typing_chat_id).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;
                        }
                    }
                });

                // Set the CronTool's origin so any jobs created during this
                // turn know which channel/chat to deliver results to.
                {
                    let mut origin = cron_origin.lock().unwrap();
                    *origin = Some(CronOrigin {
                        channel: msg.channel.clone(),
                        chat_id: msg.chat_id.clone(),
                    });
                }

                let mut agent_lock = agent.lock().await;
                match agent_lock.turn(&msg.content).await {
                    Ok(response) => {
                        // Stop typing indicator.
                        typing_handle.abort();

                        // If the response starts with "[SILENT]", the agent
                        // decided nothing needs to be said — skip outbound.
                        if response.starts_with("[SILENT]") {
                            tracing::debug!(
                                "Agent response marked [SILENT], suppressing outbound"
                            );
                            continue;
                        }

                        // Check if the channel supports streaming and use
                        // streaming delivery for a progressive typing effect.
                        let streaming_channel = manager_ref.get_channel(&msg.channel)
                            .filter(|ch| ch.supports_streaming());

                        if let Some(ch) = streaming_channel {
                            // Drop agent lock before streaming delivery.
                            drop(agent_lock);

                            match ch.send_streaming_start(&msg.chat_id).await {
                                Ok(Some(mid)) => {
                                    // Deliver the response in chunks to simulate
                                    // streaming on the channel side.
                                    let mut accumulated = String::new();
                                    let chunk_size = 80;
                                    let mut chars = response.chars().peekable();

                                    while chars.peek().is_some() {
                                        let chunk: String = chars.by_ref().take(chunk_size).collect();
                                        accumulated.push_str(&chunk);
                                        let _ = ch
                                            .send_streaming_delta(
                                                &msg.chat_id,
                                                &mid,
                                                &accumulated,
                                            )
                                            .await;
                                        // Small delay between chunks for visual effect;
                                        // the channel's own rate limiter handles throttling.
                                        tokio::time::sleep(tokio::time::Duration::from_millis(50))
                                            .await;
                                    }

                                    let _ = ch
                                        .send_streaming_end(
                                            &msg.chat_id,
                                            &mid,
                                            &accumulated,
                                        )
                                        .await;
                                    // Streaming already delivered — skip outbound bus.
                                    continue;
                                }
                                Ok(None) | Err(_) => {
                                    // Streaming start failed; fall through to
                                    // normal outbound delivery below.
                                    tracing::warn!(
                                        "Streaming start failed for channel '{}', falling back",
                                        msg.channel
                                    );
                                }
                            }
                        }

                        let outbound = fennec::bus::OutboundMessage {
                            content: response,
                            channel: msg.channel.clone(),
                            chat_id: msg.chat_id.clone(),
                            reply_to: Some(msg.id.clone()),
                            metadata: std::collections::HashMap::new(),
                        };
                        if let Err(e) = bus.publish_outbound(outbound).await {
                            tracing::error!("Failed to publish outbound: {e}");
                        }
                    }
                    Err(e) => {
                        // Stop typing indicator.
                        typing_handle.abort();

                        tracing::error!(
                            "Agent turn failed for message from {}: {e}",
                            msg.channel
                        );

                        // Send error message back to the user.
                        let error_msg = format!(
                            "Something went wrong: {}\n\nTry /new to start a fresh session.",
                            e
                        );
                        let outbound = fennec::bus::OutboundMessage {
                            content: error_msg,
                            channel: msg.channel.clone(),
                            chat_id: msg.chat_id.clone(),
                            reply_to: Some(msg.id.clone()),
                            metadata: std::collections::HashMap::new(),
                        };
                        let _ = bus.publish_outbound(outbound).await;
                    }
                }
            }
        })
    };

    // 8. Handle SIGINT gracefully.
    tokio::signal::ctrl_c().await?;
    tracing::info!("Received SIGINT, shutting down...");

    // Abort tasks.
    gateway_handle.abort();
    agent_loop.abort();
    if let Some(h) = _cron_handle {
        h.abort();
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // -- Two-phase argv handling ----------------------------------------
    //
    // Plugin-contributed subcommands (`fennec spotify play ...`) are
    // discovered at runtime, so clap can't resolve them via derive
    // alone. We do a cheap first pass to learn the plugin command
    // names from manifests (no plugin instantiation), then build a
    // dynamic clap Command that includes both the derive-defined
    // built-ins AND the plugin subcommands.
    //
    // To learn `[plugins].enabled` and the home directory, we have
    // to look at `--config-dir` and load config BEFORE the full
    // parse. The first pass is allowed to be permissive — it just
    // peeks for those two values.

    let raw_args: Vec<String> = std::env::args().collect();
    let pre_config_dir = preparse_config_dir(&raw_args);
    let pre_home = FennecConfig::resolve_home(pre_config_dir.as_deref());
    let pre_config_path = pre_home.join("config.toml");
    let pre_config = if pre_config_path.exists() {
        FennecConfig::load(&pre_config_path).unwrap_or_default()
    } else {
        FennecConfig::default()
    };

    // Lightweight CLI command discovery — no plugin instantiation.
    // Returns the spec list the plugins want exposed as `fennec
    // <name>` subcommands, validated and deduplicated against
    // built-in command names and against each other.
    let plugin_cli_specs = discover_plugin_cli_specs(&pre_home, &pre_config.plugins.enabled);

    // Build the clap command tree: derive-defined built-ins plus
    // dynamic plugin subcommands. Each plugin subcommand accepts
    // `args = [...]` as positional, trailing-var-arg, so anything
    // after `fennec spotify` lands intact in the plugin handler.
    let mut clap_cmd = <Cli as clap::CommandFactory>::command();
    for spec in &plugin_cli_specs {
        // Leak the spec name into a 'static str so clap's
        // string-pool is happy. Plugin command names are
        // bounded (1-32 chars, validated above) and the leak is
        // bounded to startup-time discovery — not a real memory
        // concern.
        let static_name: &'static str = Box::leak(spec.name.clone().into_boxed_str());
        let static_about: &'static str = Box::leak(spec.description.clone().into_boxed_str());
        clap_cmd = clap_cmd.subcommand(
            clap::Command::new(static_name)
                .about(static_about)
                .arg(
                    clap::Arg::new("args")
                        .num_args(0..)
                        .trailing_var_arg(true)
                        .allow_hyphen_values(true)
                        .help("Arguments forwarded to the plugin"),
                ),
        );
    }
    let matches = clap_cmd.get_matches_from(&raw_args);

    // If a plugin subcommand matched, dispatch to the plugin
    // BEFORE doing any agent / gateway work. Plugin commands run
    // in the foreground and return an exit code.
    if let Some((sub_name, sub_matches)) = matches.subcommand() {
        if plugin_cli_specs.iter().any(|s| s.name == sub_name) {
            // Now do the full plugin load to get the handler.
            return dispatch_plugin_cli(
                &pre_config,
                &pre_home,
                sub_name,
                sub_matches,
            )
            .await;
        }
    }

    // No plugin command matched — re-parse via the derive-typed
    // path so we get the strongly-typed `Commands` enum for the
    // built-in subcommands. This second parse is essentially free
    // and gives us back the structured arguments without
    // hand-rolling translation from `ArgMatches`.
    let cli = Cli::parse_from(&raw_args);

    // Load config: try from config dir, fall back to defaults.
    // (Re-uses pre_home and pre_config so we don't re-read config.toml.)
    let home_dir = pre_home;
    let config = pre_config;

    match cli.command {
        Commands::Agent { message, model } => {
            run_agent(config, home_dir, message, model).await?;
        }
        Commands::Status => {
            println!("Fennec v{}", env!("CARGO_PKG_VERSION"));
            println!("Status: ready");
        }
        Commands::Gateway { host, port } => {
            run_gateway(config, home_dir, host, port).await?;
        }
        Commands::Onboard { force } => {
            let config_path = home_dir.join("config.toml");
            if config_path.exists() && !force {
                eprintln!(
                    "Config already exists at {}. Use --force to overwrite.",
                    config_path.display()
                );
                std::process::exit(1);
            }
            fennec::onboard::run_wizard(&home_dir)?;
        }
        Commands::Login => {
            auth::run_oauth_login(&home_dir)?;
        }
        Commands::Doctor => {
            run_doctor(&config, &home_dir).await?;
        }
    }

    Ok(())
}

/// Pre-parse the `--config-dir` flag from raw argv. Used at startup
/// before clap is fully configured (we need to know the home dir
/// to read config + discover plugin CLI commands BEFORE clap can
/// build a complete subcommand tree).
///
/// Permissive: ignores unknown flags, doesn't fail on malformed
/// argv. Worst case we miss the override and fall back to the
/// default home dir, which clap will catch on the real parse.
fn preparse_config_dir(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == "--config-dir" {
            return iter.next().cloned();
        }
        if let Some(v) = a.strip_prefix("--config-dir=") {
            return Some(v.to_string());
        }
    }
    None
}

/// Lightweight discovery of plugin-contributed CLI command specs.
/// Iterates bundled plugin static refs (cheap — just trait method
/// calls, no instantiation) and reads WASM `plugin.toml` manifests
/// from `<home>/plugins/`. Used by `main` to extend clap before
/// argv is parsed.
///
/// Validation, dedup, and reserved-name checks are applied here —
/// the returned specs are safe to feed into clap as subcommands.
fn discover_plugin_cli_specs(
    home_dir: &std::path::Path,
    enabled: &[String],
) -> Vec<fennec::plugins::CliCommandSpec> {
    use fennec::plugins::PluginEntry;
    use std::collections::HashSet;

    let want: HashSet<&str> = enabled.iter().map(String::as_str).collect();
    if want.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<fennec::plugins::CliCommandSpec> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();

    // Bundled plugins — call cli_commands() on each enabled static
    // reference. No `register()` invocation, no agent build.
    for entry in fennec::plugins::inventory::iter::<PluginEntry> {
        let manifest = entry.plugin.manifest();
        if !want.contains(manifest.name.as_str()) {
            continue;
        }
        for spec in entry.plugin.cli_commands() {
            if let Err(e) = fennec::plugins::validate_command_name(&spec.name) {
                tracing::warn!(
                    plugin = %manifest.name,
                    command = %spec.name,
                    "Bundled plugin CLI command rejected at discovery: {e}"
                );
                continue;
            }
            if !seen_names.insert(spec.name.clone()) {
                tracing::warn!(
                    plugin = %manifest.name,
                    command = %spec.name,
                    "Plugin CLI command name '{}' already taken; dropping",
                    spec.name
                );
                continue;
            }
            out.push(spec);
        }
    }

    // WASM plugins — read manifests from disk. The `cli_commands`
    // field of `plugin.toml` declares names + descriptions; we
    // bind handlers later when a plugin command actually matches.
    let plugins_root = home_dir.join("plugins");
    if let Ok(discovered) =
        fennec::plugins::wasm::discover_wasm_plugins(&plugins_root)
    {
        for d in discovered {
            if !want.contains(d.manifest.name.as_str()) {
                continue;
            }
            for spec in &d.manifest.cli_commands {
                if let Err(e) = fennec::plugins::validate_command_name(&spec.name) {
                    tracing::warn!(
                        plugin = %d.manifest.name,
                        command = %spec.name,
                        "WASM plugin CLI command rejected at discovery: {e}"
                    );
                    continue;
                }
                if !seen_names.insert(spec.name.clone()) {
                    tracing::warn!(
                        plugin = %d.manifest.name,
                        command = %spec.name,
                        "Plugin CLI command name '{}' already taken; dropping",
                        spec.name
                    );
                    continue;
                }
                out.push(spec.clone());
            }
        }
    }

    out
}

/// Dispatch a matched plugin CLI command. Does the full plugin
/// load (bundled + WASM) so the matched command's handler is
/// available, then runs it.
///
/// The agent is NOT built and NOT started — plugin CLI commands
/// run as standalone tools. Returns `Ok(())` regardless of the
/// plugin's exit code; the exit code is propagated via
/// `std::process::exit` so shell pipelines see the right thing.
async fn dispatch_plugin_cli(
    config: &FennecConfig,
    home_dir: &std::path::Path,
    sub_name: &str,
    sub_matches: &clap::ArgMatches,
) -> Result<()> {
    // Pull args following `fennec <plugin-cmd>`. clap stores them
    // under the "args" id we set on the dynamic subcommand.
    let args: Vec<String> = sub_matches
        .get_many::<String>("args")
        .map(|vs| vs.cloned().collect())
        .unwrap_or_default();

    // Full plugin load. Most fields aren't relevant for a CLI-only
    // dispatch (we don't need memory, channel bus, etc.), but the
    // load path requires them. Use stub-friendly choices for the
    // resources the plugins might touch.
    let mut registry = fennec::plugins::PluginRegistry::new();
    if let Err(e) = registry.load_bundled(&config.plugins.enabled) {
        tracing::error!(
            "Plugin registry failed to load bundled for CLI dispatch: {e}"
        );
    }

    // For WASM, we still need the full host resources because the
    // plugin's CLI handler may use `http_request` / `read_file` /
    // etc. as host imports. Construct an in-memory `SqliteMemory`
    // (under home/memory.db) for the plugin to use; channel bus
    // is None because we're not in gateway mode.
    let path_sandbox = Arc::new(PathSandbox::new(config.security.forbidden_paths.clone()));
    let memory_path = home_dir.join("memory.db");
    let memory: Arc<dyn Memory> = Arc::new(
        SqliteMemory::new(
            memory_path,
            config.memory.vector_weight as f32,
            config.memory.keyword_weight as f32,
            config.memory.cache_max,
            Arc::new(NoopEmbedding::new(1536)),
        )
        .context("opening memory store for plugin CLI dispatch")?,
    );
    let wasm_resources = fennec::plugins::WasmHostResources {
        path_sandbox,
        memory,
        http_client: fennec::tools::http::shared_client(),
        rt_handle: tokio::runtime::Handle::current(),
        settings: config.plugins.settings.clone(),
        bus: None,
    };
    let plugins_root = home_dir.join("plugins");
    if let Err(e) = registry.load_wasm(
        &plugins_root,
        &config.plugins.enabled,
        wasm_resources,
    ) {
        tracing::error!("Plugin registry failed to load WASM for CLI dispatch: {e}");
    }

    let runtime = registry.into_runtime(&config.memory.provider);

    let exit_code = match runtime.dispatch_cli(sub_name, args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("plugin command '{sub_name}' failed: {e}");
            1
        }
    };
    std::process::exit(exit_code);
}

async fn run_doctor(config: &FennecConfig, home_dir: &std::path::Path) -> Result<()> {
    use fennec::doctor;
    let secret_store = SecretStore::new(home_dir.to_path_buf()).context("creating secret store")?;
    let use_color = console::Term::stdout().is_term()
        && std::env::var("NO_COLOR").is_err();

    let heading = if use_color {
        console::style("Fennec Doctor").cyan().bold().to_string()
    } else {
        "Fennec Doctor".to_string()
    };
    println!();
    println!("  {}", heading);
    let rule = "─".repeat(40);
    let rule_styled = if use_color {
        console::style(rule).dim().to_string()
    } else {
        rule
    };
    println!("  {}", rule_styled);
    println!();

    let results = doctor::run_all(config, home_dir, &secret_store).await;
    for r in &results {
        println!("  {}", doctor::render_result(r, use_color));
    }
    println!();
    let (summary, any_failed) = doctor::render_summary(&results, use_color);
    println!("  {}", summary);
    println!();
    if any_failed {
        std::process::exit(1);
    }
    Ok(())
}
