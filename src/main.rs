use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use fennec::agent::AgentBuilder;
use fennec::channels::cli::CliChannel;
use fennec::channels::traits::{Channel, SendMessage};
use fennec::config::FennecConfig;
use fennec::memory::embedding::NoopEmbedding;
use fennec::memory::sqlite::SqliteMemory;
use fennec::providers::anthropic::AnthropicProvider;
use fennec::security::SecretStore;
use fennec::tools::files::{ListDirTool, ReadFileTool, WriteFileTool};
use fennec::tools::shell::ShellTool;

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

async fn run_agent(
    config: FennecConfig,
    home_dir: std::path::PathBuf,
    message: Option<String>,
    model: Option<String>,
) -> Result<()> {
    // Create SecretStore.
    let secret_store =
        SecretStore::new(home_dir.clone()).context("creating secret store")?;

    // Resolve API key.
    let api_key = resolve_api_key(&config, &secret_store)?;

    // Create AnthropicProvider with optional model override.
    let provider = AnthropicProvider::new(api_key, model.or(Some(config.provider.model.clone())));

    // Create SqliteMemory.
    let db_path = config
        .memory
        .db_path
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home_dir.join("memory").join("brain.db"));
    let embedder = Arc::new(NoopEmbedding::new(1536));
    let memory = SqliteMemory::new(
        db_path,
        config.memory.vector_weight as f32,
        config.memory.keyword_weight as f32,
        config.memory.cache_max,
        embedder,
    )
    .context("creating sqlite memory")?;
    let memory = Arc::new(memory);

    // Create tools.
    let shell_tool = ShellTool::new(
        config.security.command_allowlist.clone(),
        config.security.forbidden_paths.clone(),
        config.security.command_timeout_secs,
    );
    let read_file_tool = ReadFileTool::new();
    let write_file_tool = WriteFileTool::new();
    let list_dir_tool = ListDirTool::new();

    // Build Agent.
    let mut agent = AgentBuilder::new()
        .provider(Box::new(provider))
        .memory(memory)
        .tool(Box::new(shell_tool))
        .tool(Box::new(read_file_tool))
        .tool(Box::new(write_file_tool))
        .tool(Box::new(list_dir_tool))
        .identity_name(&config.identity.name)
        .identity_persona(&config.identity.persona)
        .max_tool_iterations(config.agent.max_tool_iterations as usize)
        .max_tokens(config.provider.max_tokens as usize)
        .temperature(config.provider.temperature)
        .memory_context_limit(config.memory.context_limit)
        .half_life_days(config.memory.half_life_days)
        .build()
        .context("building agent")?;

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
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                }
            }
        }

        listen_handle.abort();
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
    }

    Ok(())
}
