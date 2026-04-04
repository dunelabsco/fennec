use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use fennec::agent::AgentBuilder;
use fennec::bus::MessageBus;
use fennec::channels::cli::CliChannel;
use fennec::channels::traits::{Channel, SendMessage};
use fennec::channels::ChannelManager;
use fennec::config::FennecConfig;
use fennec::cron::{CronScheduler, JobStore};
use fennec::gateway::GatewayServer;
use fennec::memory::embedding::{NoopEmbedding, OpenAIEmbedding};
use fennec::memory::snapshot;
use fennec::memory::sqlite::SqliteMemory;
use fennec::memory::Memory;
use fennec::providers::anthropic::AnthropicProvider;
use fennec::security::prompt_guard::{GuardAction, PromptGuard};
use fennec::security::SecretStore;
use fennec::tools::files::{ListDirTool, ReadFileTool, WriteFileTool};
use fennec::tools::shell::ShellTool;
use fennec::tools::web::{WebFetchTool, WebSearchTool};

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
}

/// Resolve the Anthropic API key from config or environment variable.
fn resolve_api_key(config: &FennecConfig, secret_store: &SecretStore) -> Result<String> {
    // Try config value first.
    if !config.provider.api_key.is_empty() {
        let decrypted = secret_store
            .decrypt(&config.provider.api_key)
            .context("decrypting API key from config")?;
        return Ok(decrypted);
    }

    // Fall back to environment variable.
    std::env::var("ANTHROPIC_API_KEY")
        .context("API key not found: set provider.api_key in config or ANTHROPIC_API_KEY env var")
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
async fn build_agent(
    config: &FennecConfig,
    home_dir: &std::path::Path,
    model_override: Option<String>,
) -> Result<(fennec::agent::Agent, Arc<dyn Memory>)> {
    // Create SecretStore.
    let secret_store =
        SecretStore::new(home_dir.to_path_buf()).context("creating secret store")?;

    // Resolve API key.
    let api_key = resolve_api_key(config, &secret_store)?;

    // Create AnthropicProvider with optional model override.
    let provider =
        AnthropicProvider::new(api_key, model_override.or(Some(config.provider.model.clone())));

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
    let read_file_tool = ReadFileTool::new();
    let write_file_tool = WriteFileTool::new();
    let list_dir_tool = ListDirTool::new();
    let web_fetch_tool = WebFetchTool::new();
    let web_search_tool = WebSearchTool::new();

    // Create prompt guard from config security settings.
    let guard_action = parse_guard_action(&config.security.prompt_guard_action);
    let prompt_guard = PromptGuard::new(guard_action, config.security.prompt_guard_sensitivity);

    // Build Agent.
    let agent = AgentBuilder::new()
        .provider(Box::new(provider))
        .memory(memory.clone())
        .tool(Box::new(shell_tool))
        .tool(Box::new(read_file_tool))
        .tool(Box::new(write_file_tool))
        .tool(Box::new(list_dir_tool))
        .tool(Box::new(web_fetch_tool))
        .tool(Box::new(web_search_tool))
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

    Ok((agent, memory))
}

async fn run_agent(
    config: FennecConfig,
    home_dir: std::path::PathBuf,
    message: Option<String>,
    model: Option<String>,
) -> Result<()> {
    let (mut agent, memory) = build_agent(&config, &home_dir, model).await?;

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
    // 1. Build Agent.
    let (agent, _memory) = build_agent(&config, &home_dir, None).await?;
    let agent = Arc::new(tokio::sync::Mutex::new(agent));

    // 2. Create MessageBus.
    let (bus, mut receiver) = MessageBus::new(256);

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

    // 4. Create ChannelManager, start all channels.
    let manager = ChannelManager::new(channels, bus.clone());
    let _listener_handles = manager.start_all();
    let _dispatch_handle = manager.spawn_outbound_dispatch(receiver.outbound_rx);

    // 5. Start CronScheduler if enabled.
    let _cron_handle = if config.cron.enabled {
        let cron_path = home_dir.join("cron_jobs.json");
        let mut store = JobStore::new(cron_path);
        if let Err(e) = store.load() {
            tracing::warn!("Failed to load cron jobs: {e}");
        }
        let mut scheduler = CronScheduler::new(store, bus.clone(), None);
        tracing::info!("Cron scheduler enabled");
        Some(tokio::spawn(async move {
            scheduler.run().await;
        }))
    } else {
        None
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
        tokio::spawn(async move {
            while let Some(msg) = receiver.inbound_rx.recv().await {
                let mut agent_lock = agent.lock().await;
                match agent_lock.turn(&msg.content).await {
                    Ok(response) => {
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
                        tracing::error!(
                            "Agent turn failed for message from {}: {e}",
                            msg.channel
                        );
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
    }

    Ok(())
}
