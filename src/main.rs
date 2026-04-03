use clap::{Parser, Subcommand};

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
        #[arg(short, long)]
        message: Option<String>,
        #[arg(short, long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
    },
    /// Show agent status
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Agent { message, .. } => {
            if let Some(msg) = message {
                println!("Single-shot mode: {msg}");
            } else {
                println!("Interactive mode (not yet implemented)");
            }
        }
        Commands::Status => {
            println!("Fennec v{}", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}
