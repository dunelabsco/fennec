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

        /// Open the terminal UI (sessions / chat / channels panels)
        /// instead of the line-by-line CLI. Existing CLI mode is
        /// the default.
        #[arg(long)]
        tui: bool,
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
async fn build_agent(
    config: &FennecConfig,
    home_dir: &std::path::Path,
    model_override: Option<String>,
    channel_map: Option<ChannelMapHandle>,
) -> Result<(
    fennec::agent::Agent,
    Arc<dyn Memory>,
    Arc<Mutex<Option<CronOrigin>>>,
    fennec::bus::PendingReplies,
    fennec::bus::ChatDirectory,
)> {
    build_agent_with_callbacks(config, home_dir, model_override, channel_map, None).await
}

async fn build_agent_with_callbacks(
    config: &FennecConfig,
    home_dir: &std::path::Path,
    model_override: Option<String>,
    channel_map: Option<ChannelMapHandle>,
    callbacks: Option<fennec::agent::callbacks::CallbacksHandle>,
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

    let mut configured_builder = builder
        .identity_name(&config.identity.name)
        .identity_persona(&config.identity.persona)
        .max_tool_iterations(config.agent.max_tool_iterations as usize)
        .max_tokens(config.provider.max_tokens as usize)
        .temperature(config.provider.temperature)
        .memory_context_limit(config.memory.context_limit)
        .half_life_days(config.memory.half_life_days)
        .prompt_guard(prompt_guard);
    if let Some(handle) = callbacks {
        configured_builder = configured_builder.callbacks(handle);
    }
    let agent = configured_builder.build().context("building agent")?;

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
        build_agent(&config, &home_dir, model, None).await?;

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

/// Launch the terminal UI (`fennec agent --tui`).
///
/// Builds a real agent with `TuiBridge` callbacks so streaming
/// text deltas, tool starts/completes, status updates, and turn
/// boundaries fan out into the TUI's app state. The renderer
/// thread reads `App` for each frame; the agent loop runs on a
/// tokio task and produces events into an mpsc channel; a drain
/// task on the same runtime applies events to `App`.
async fn run_tui(
    config: FennecConfig,
    home_dir: std::path::PathBuf,
    model_override: Option<String>,
) -> Result<()> {
    use fennec::sessions::store::SessionStore;
    use fennec::tui::app::{App, ChatLine, SessionRow};
    use fennec::tui::callbacks::TuiBridge;

    // Real sessions list: query the SessionStore for prior
    // sessions. The current TUI session itself isn't recorded
    // yet (the agent doesn't auto-record turns into the store —
    // that's a separate cross-cutting wiring) but historical
    // sessions show up so /resume picks have context.
    let session_db = home_dir.join("sessions.db");
    let session_store_handle =
        match tokio::task::spawn_blocking(move || SessionStore::new(&session_db)).await {
            Ok(Ok(s)) => Some(std::sync::Arc::new(s)),
            Ok(Err(e)) => {
                tracing::warn!("session store init failed: {e}; sessions list will be empty");
                None
            }
            Err(_) => None,
        };
    let prior_sessions = if let Some(ref store) = session_store_handle {
        store.list_sessions(20).await.unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut app = App::new();
    // Current TUI session pinned to the top.
    app.sessions.push(SessionRow {
        code: "$ ".into(),
        who: "local".into(),
        subject: "current session".into(),
        count: "0".into(),
        has_unread: false,
    });
    // Historical sessions from the store, transformed into the
    // sessions-panel row shape.
    for rec in prior_sessions.iter().take(20) {
        let (code, who) = channel_label(&rec.channel);
        let subject = rec
            .summary
            .clone()
            .unwrap_or_else(|| short_id(&rec.id));
        app.sessions.push(SessionRow {
            code,
            who,
            subject,
            count: rec
                .ended_at
                .as_deref()
                .map(|_| "—".to_string())
                .unwrap_or_else(|| "·".to_string()),
            has_unread: false,
        });
    }
    app.selected_session = 0;

    // Real channels list from config — every enabled channel is
    // surfaced with a status reflecting whether the gateway is
    // running it (in TUI mode the gateway isn't started, so all
    // channels except cli show as 'not running').
    app.channels = build_channels_panel(&config);

    app.chat = vec![ChatLine::System {
        time: chrono::Local::now().format("%H:%M:%S").to_string(),
        body: format!(
            "session resumed · model {} · {} prior sessions · ready",
            if config.provider.model.is_empty() {
                "default"
            } else {
                &config.provider.model
            },
            prior_sessions.len()
        ),
    }];
    let app = std::sync::Arc::new(parking_lot::Mutex::new(app));

    // Build the agent with the TUI bridge wired in as callbacks.
    let (bridge, mut event_rx) = TuiBridge::new(app.clone());
    let bridge_handle: fennec::agent::callbacks::CallbacksHandle =
        std::sync::Arc::new(bridge);
    let (agent, _memory, _cron_origin, _pending_replies, _chat_directory) =
        build_agent_with_callbacks(
            &config,
            &home_dir,
            model_override,
            None,
            Some(bridge_handle),
        )
        .await?;
    let agent = std::sync::Arc::new(tokio::sync::Mutex::new(agent));

    // Drain task: consume agent events, mutate app state. Runs on
    // the same runtime as the agent itself; the renderer reads
    // app state under its own parking_lot mutex on the main
    // thread.
    let drain_app = app.clone();
    tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            apply_tui_event(&drain_app, ev);
        }
    });

    // Submit-handler task: watches the input editor for new
    // history entries (each Enter records one) and routes them
    // either to the slash-command registry or to a fresh agent
    // turn. Polling is intentional — the TUI's render loop runs
    // synchronously on the main thread so we can't share a
    // tokio mpsc with it without extra plumbing.
    let registry = std::sync::Arc::new(fennec::tui::commands::CommandRegistry::with_builtins());
    let submit_app = app.clone();
    let submit_agent = agent.clone();
    let submit_handle = tokio::spawn(async move {
        let mut last_submitted: Option<String> = None;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let pending = {
                let guard = submit_app.lock();
                if guard.should_quit {
                    return;
                }
                guard.input.history.front().cloned()
            };
            let prompt = match pending {
                Some(p) if last_submitted.as_ref() != Some(&p) => p,
                _ => continue,
            };
            last_submitted = Some(prompt.clone());

            // Slash command? Run it through the registry under
            // the app lock briefly, then handle any AgentAction
            // outcome with the agent lock released.
            if let Some((name, args)) = fennec::tui::commands::parse(&prompt) {
                let outcome = {
                    let mut guard = submit_app.lock();
                    registry.dispatch(name, args, &mut guard).unwrap_or_else(|e| {
                        fennec::tui::commands::CommandOutcome::Status(format!("error: {e}"))
                    })
                };
                handle_command_outcome(outcome, &submit_app, &submit_agent).await;
                continue;
            }

            // Plain text — run as an agent turn (streaming, so
            // text deltas + tool events surface live in the TUI).
            let mut agent_guard = submit_agent.lock().await;
            let _ = agent_guard.turn_streaming(&prompt).await;
        }
    });

    // Voice transcription polling task: when /voice off has
    // queued a WAV file, transcribe it via the user-configured
    // OpenAI Whisper key (same key the agent's
    // TranscribeAudioTool uses) and deliver the text back into
    // the TUI's input box on the next tick.
    let voice_app = app.clone();
    let voice_config = config.clone();
    let voice_home = home_dir.clone();
    let voice_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            let pending = {
                let guard = voice_app.lock();
                if guard.should_quit {
                    return;
                }
                guard.voice.take_pending_wav()
            };
            let Some(path) = pending else { continue };
            // Build a transcription tool on demand. The OpenAI
            // key is resolved the same way the agent's tool does
            // (config.api_key first, then OPENAI_API_KEY env).
            let key = fennec::tools::voice_tool::resolve_openai_key(
                &voice_config.provider.name,
                &voice_config.provider.api_key,
            );
            if key.is_empty() {
                voice_app.lock().voice.deliver_error(
                    "OPENAI_API_KEY not set; cannot transcribe".into(),
                );
                continue;
            }
            let _ = voice_home.clone(); // reserved for future cache-dir use
            let tool =
                match fennec::tools::voice_tool::TranscribeAudioTool::new_with_key(
                    key,
                    None,
                ) {
                    Some(t) => t,
                    None => {
                        voice_app.lock().voice.deliver_error(
                            "transcription tool init failed".into(),
                        );
                        continue;
                    }
                };
            // The tool expects a JSON arg payload with `path`.
            let args = serde_json::json!({ "path": path.to_string_lossy() });
            use fennec::tools::traits::Tool;
            match tool.execute(args).await {
                Ok(result) => {
                    if result.success {
                        voice_app.lock().voice.deliver_transcription(result.output);
                    } else {
                        voice_app.lock().voice.deliver_error(format!(
                            "transcribe: {}",
                            result.error.unwrap_or(result.output)
                        ));
                    }
                }
                Err(e) => voice_app
                    .lock()
                    .voice
                    .deliver_error(format!("transcribe: {e}")),
            }
        }
    });

    // Run the renderer (blocks until the user quits).
    let result = fennec::tui::run(app.clone());

    // Cleanly stop background tasks.
    {
        let mut guard = app.lock();
        guard.should_quit = true;
    }
    submit_handle.abort();
    voice_handle.abort();

    result
}

/// Build the TUI's CHANNELS panel from the running config.
/// The gateway isn't started in `--tui` mode so non-CLI channels
/// show as "not running" — when fennec gateway is active in a
/// separate process the user would still see them as "available"
/// rather than "connected" (live channel-manager wiring is out
/// of scope for F1-1; that's a richer integration that needs
/// a shared bus across the gateway and TUI processes, which
/// belongs to F2's dashboard work).
fn build_channels_panel(config: &FennecConfig) -> Vec<fennec::tui::app::ChannelState> {
    use fennec::tui::app::{ChannelConnState, ChannelState};
    let mut out = vec![ChannelState {
        code: "$ ".into(),
        name: "cli".into(),
        state: ChannelConnState::Attached,
        detail: "this session".into(),
    }];
    let cs = &config.channels;
    if cs.telegram.enabled {
        out.push(ChannelState {
            code: "TG".into(),
            name: "telegram".into(),
            state: ChannelConnState::Idle,
            detail: "configured".into(),
        });
    }
    if cs.discord.enabled {
        out.push(ChannelState {
            code: "DC".into(),
            name: "discord".into(),
            state: ChannelConnState::Idle,
            detail: "configured".into(),
        });
    }
    if cs.slack.enabled {
        out.push(ChannelState {
            code: "SL".into(),
            name: "slack".into(),
            state: ChannelConnState::Idle,
            detail: "configured".into(),
        });
    }
    if cs.whatsapp.enabled {
        out.push(ChannelState {
            code: "WA".into(),
            name: "whatsapp".into(),
            state: ChannelConnState::Idle,
            detail: "configured".into(),
        });
    }
    if cs.email.enabled {
        out.push(ChannelState {
            code: "@ ".into(),
            name: "email".into(),
            state: ChannelConnState::Idle,
            detail: format!("imap.{}", first_dot_segment(&cs.email.imap_host)),
        });
    }
    out
}

/// Map a channel name (as stored in `SessionRecord.channel`) to
/// the (code, who) pair the sessions panel renders.
fn channel_label(channel: &str) -> (String, String) {
    match channel {
        "cli" | "" => ("$ ".into(), "local".into()),
        "telegram" => ("TG".into(), "telegram".into()),
        "discord" => ("DC".into(), "discord".into()),
        "slack" => ("SL".into(), "slack".into()),
        "signal" => ("SG".into(), "signal".into()),
        "matrix" => ("MX".into(), "matrix".into()),
        "whatsapp" => ("WA".into(), "whatsapp".into()),
        "email" => ("@ ".into(), "email".into()),
        other => ("? ".into(), other.to_string()),
    }
}

fn short_id(s: &str) -> String {
    s.chars().take(8).collect()
}

fn first_dot_segment(s: &str) -> String {
    s.split('.').next().unwrap_or(s).to_string()
}

/// Apply a `CommandOutcome` to the app state and (if needed)
/// dispatch the queued `AgentAction` against the agent. Lives
/// here rather than inside the registry because executing
/// agent operations needs both the parking_lot app mutex and
/// the tokio agent mutex; the registry is sync.
async fn handle_command_outcome(
    outcome: fennec::tui::commands::CommandOutcome,
    app: &std::sync::Arc<parking_lot::Mutex<fennec::tui::App>>,
    agent: &std::sync::Arc<tokio::sync::Mutex<fennec::agent::Agent>>,
) {
    use fennec::tui::commands::{AgentAction, CommandOutcome};
    match outcome {
        CommandOutcome::Quit => {
            app.lock().should_quit = true;
        }
        CommandOutcome::Text(body) => {
            let mut guard = app.lock();
            guard.chat.push(fennec::tui::app::ChatLine::System {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                body,
            });
        }
        CommandOutcome::Status(msg) => {
            app.lock().set_status(msg);
        }
        CommandOutcome::Unknown(name) => {
            app.lock().set_status(format!("unknown command: /{name}"));
        }
        CommandOutcome::NotImplemented(name) => {
            app.lock()
                .set_status(format!("/{name}: not yet implemented"));
        }
        CommandOutcome::Agent(action) => match action {
            AgentAction::Clear => {
                let mut guard = agent.lock().await;
                guard.clear_history();
            }
            AgentAction::Retry | AgentAction::Undo | AgentAction::Steer(_) => {
                // The agent doesn't yet expose retry/undo/steer
                // primitives. The chat-side state change has
                // already been applied by the command handler so
                // the user sees an immediate visual effect; the
                // agent-side replay lands in F1-2.
                let mut guard = app.lock();
                guard.set_status("queued — agent-side replay arrives in F1-2");
            }
            AgentAction::Run(prompt) => {
                let mut guard = agent.lock().await;
                let _ = guard.turn(&prompt).await;
            }
        },
    }
}

/// Apply a single `TuiEvent` to the app state. Called from the
/// drain task; runs under the parking_lot mutex briefly.
fn apply_tui_event(
    app: &std::sync::Arc<parking_lot::Mutex<fennec::tui::App>>,
    ev: fennec::tui::callbacks::TuiEvent,
) {
    use fennec::tui::app::{ChatLine, LiveTool, ToolHistoryEntry};
    use fennec::tui::callbacks::TuiEvent;
    let now = chrono::Local::now().format("%H:%M:%S").to_string();
    let mut guard = app.lock();
    match ev {
        TuiEvent::TurnStart(prompt) => {
            guard.chat.push(ChatLine::User {
                time: now,
                body: prompt,
            });
            guard.finalize_bot_message();
        }
        TuiEvent::TurnComplete(summary) => {
            // If streaming was active, the in-flight bot message
            // already contains the full text — just close it. If
            // streaming wasn't used (e.g. the non-streaming
            // `turn()` path), the bot reply hasn't been pushed
            // yet, so push it now.
            match guard.in_flight_bot_idx {
                Some(_) => {
                    guard.finalize_bot_message();
                }
                None if !summary.is_empty() => {
                    guard.chat.push(ChatLine::Bot {
                        time: now,
                        body: summary,
                    });
                }
                _ => {}
            }
            guard.live_tool = None;
        }
        TuiEvent::TextDelta(delta) => {
            guard.append_bot_delta(&delta);
        }
        TuiEvent::ReasoningDelta(_delta) => {
            // Reasoning rendering — F1-1 ships text streaming,
            // reasoning-block rendering (the dim "thinking…" panel
            // upstream shows above the reply) lands in F1-2.
        }
        TuiEvent::ToolStart(s) => {
            // A tool call breaks the streaming continuity — close
            // the in-flight bot message so the next text delta
            // (after the tool result) starts a fresh message.
            guard.finalize_bot_message();
            let started = std::time::Instant::now();
            guard.chat.push(ChatLine::ToolCall {
                call: format!("{}({})", s.name, s.preview),
            });
            // Inline running indicator under the tool-call line.
            // Replaced with ToolResult on completion so the
            // scrollback ends up as call → result, with the spinner
            // visible only while the tool is in flight.
            guard.chat.push(fennec::tui::app::ChatLine::ToolRunning {
                label: "running…".into(),
                started_at: started,
            });
            guard.live_tool = Some(LiveTool {
                name: s.name,
                args_preview: s.preview,
                started_at: started,
                progress: None,
            });
        }
        TuiEvent::ToolProgress(p) => {
            if let Some(ref mut lt) = guard.live_tool {
                lt.args_preview = p.preview;
            }
        }
        TuiEvent::ToolComplete(c) => {
            let summary = c
                .summary
                .clone()
                .unwrap_or_else(|| "(done)".to_string());
            // Drop the most recent inline running indicator before
            // pushing the result so the chat reads call → result.
            if let Some(idx) = guard
                .chat
                .iter()
                .rposition(|l| matches!(l, fennec::tui::app::ChatLine::ToolRunning { .. }))
            {
                guard.chat.remove(idx);
            }
            guard.chat.push(ChatLine::ToolResult { summary });
            guard.recent_tools.insert(
                0,
                ToolHistoryEntry {
                    ok: c.error.is_none(),
                    name: c.name,
                    note: format!("{}ms", c.duration_ms),
                },
            );
            guard.recent_tools.truncate(8);
            guard.live_tool = None;
        }
        TuiEvent::Status(msg) => {
            guard.set_status(msg);
        }
    }
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
        build_agent(&config, &home_dir, None, Some(channel_map.clone())).await?;
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

    let cli = Cli::parse();

    // Load config: try from config dir, fall back to defaults.
    let home_dir = FennecConfig::resolve_home(cli.config_dir.as_deref());
    let config_path = home_dir.join("config.toml");
    let config = if config_path.exists() {
        FennecConfig::load(&config_path)
            .with_context(|| format!("loading config from {}", config_path.display()))?
    } else {
        FennecConfig::default()
    };

    match cli.command {
        Commands::Agent { message, model, tui } => {
            if tui {
                run_tui(config, home_dir, model).await?;
            } else {
                run_agent(config, home_dir, message, model).await?;
            }
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
