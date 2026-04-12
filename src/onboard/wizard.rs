use dialoguer::{Confirm, Input, Select};
use console::style;

/// Run the interactive setup wizard that creates `~/.fennec/config.toml`.
pub fn run_wizard(fennec_home: &std::path::Path) -> anyhow::Result<()> {
    // Banner
    println!();
    println!("  {}", style("Welcome to Fennec").bold().cyan());
    println!(
        "  {}",
        style("The fastest AI agent with collective intelligence").dim()
    );
    println!();

    // Step 1: Provider selection
    let providers = vec![
        "Anthropic (Claude)",
        "OpenAI (GPT-4o)",
        "Kimi (Moonshot)",
        "OpenRouter (any model)",
        "Ollama (local)",
    ];
    let provider_idx = Select::new()
        .with_prompt("Choose your LLM provider")
        .items(&providers)
        .default(0)
        .interact()?;

    let (provider_name, default_model, env_var) = match provider_idx {
        0 => ("anthropic", "claude-sonnet-4-20250514", "ANTHROPIC_API_KEY"),
        1 => ("openai", "gpt-4o", "OPENAI_API_KEY"),
        2 => ("kimi", "kimi-k2.5", "KIMI_API_KEY"),
        3 => (
            "openrouter",
            "anthropic/claude-sonnet-4",
            "OPENROUTER_API_KEY",
        ),
        4 => ("ollama", "llama3.1", ""),
        _ => ("anthropic", "claude-sonnet-4-20250514", "ANTHROPIC_API_KEY"),
    };

    // Step 2: Authentication
    let api_key = if provider_name == "anthropic" {
        // Anthropic: offer OAuth or API key
        let auth_methods = vec![
            "OAuth (sign in with your Claude account — free with subscription)",
            "API key (from console.anthropic.com — pay per use)",
        ];
        let auth_idx = Select::new()
            .with_prompt("How do you want to authenticate")
            .items(&auth_methods)
            .default(0)
            .interact()?;

        if auth_idx == 0 {
            // OAuth flow
            println!();
            println!("  {}", style("Starting OAuth login...").dim());
            match crate::auth::run_oauth_login(fennec_home) {
                Ok(_creds) => {
                    println!("  {} Authenticated with Claude!", style("✓").green());
                    String::new() // No API key needed — OAuth token stored separately
                }
                Err(e) => {
                    println!("  {} OAuth failed: {}", style("✗").red(), e);
                    println!("  {}", style("Falling back to API key...").dim());
                    Input::<String>::new()
                        .with_prompt("Enter your Anthropic API key")
                        .allow_empty(true)
                        .interact_text()?
                }
            }
        } else {
            // API key
            if let Ok(key) = std::env::var(env_var) {
                println!("  {} Using {} from environment", style("✓").green(), env_var);
                key
            } else {
                Input::<String>::new()
                    .with_prompt("Enter your Anthropic API key")
                    .allow_empty(true)
                    .interact_text()?
            }
        }
    } else if !env_var.is_empty() {
        // Non-Anthropic providers: API key only
        if let Ok(key) = std::env::var(env_var) {
            println!("  {} Using {} from environment", style("✓").green(), env_var);
            key
        } else {
            Input::<String>::new()
                .with_prompt("Enter your API key")
                .allow_empty(true)
                .interact_text()?
        }
    } else {
        String::new() // Ollama — no key needed
    };

    // Step 3: Agent name
    let agent_name: String = Input::new()
        .with_prompt("Agent name")
        .default("Fennec".to_string())
        .interact_text()?;

    // Step 4: Telegram setup
    let setup_telegram = Confirm::new()
        .with_prompt("Set up Telegram?")
        .default(false)
        .interact()?;

    let (telegram_token, telegram_user_id) = if setup_telegram {
        println!(
            "  {}",
            style("Create a bot via @BotFather on Telegram").dim()
        );
        let token: String = Input::new()
            .with_prompt("Telegram bot token")
            .interact_text()?;
        let user_id: String = Input::new()
            .with_prompt("Your Telegram user ID (message @userinfobot on Telegram to find it)")
            .allow_empty(true)
            .interact_text()?;
        (token, user_id)
    } else {
        (String::new(), String::new())
    };

    // Step 5: Discord setup
    let setup_discord = Confirm::new()
        .with_prompt("Set up Discord?")
        .default(false)
        .interact()?;

    let discord_token = if setup_discord {
        Input::<String>::new()
            .with_prompt("Discord bot token")
            .interact_text()?
    } else {
        String::new()
    };

    // Step 6: Collective (Plurum)
    let enable_collective = Confirm::new()
        .with_prompt("Enable collective intelligence (Plurum)?")
        .default(true)
        .interact()?;

    let plurum_key = if enable_collective {
        // Try auto-register
        println!(
            "  {} Registering with Plurum...",
            style("\u{27F3}").yellow()
        );
        match auto_register_plurum(&agent_name) {
            Ok(key) => {
                println!(
                    "  {} Registered! Key: {}...{}",
                    style("✓").green(),
                    &key[..key.len().min(16)],
                    &key[key.len().saturating_sub(4)..]
                );
                key
            }
            Err(e) => {
                println!(
                    "  {} Auto-register failed: {}",
                    style("✗").red(),
                    e
                );
                Input::<String>::new()
                    .with_prompt("Plurum API key (or Enter to skip)")
                    .allow_empty(true)
                    .interact_text()?
            }
        }
    } else {
        String::new()
    };

    // Write config
    let config = build_config_toml(
        &agent_name,
        provider_name,
        default_model,
        &api_key,
        &telegram_token,
        &telegram_user_id,
        &discord_token,
        &plurum_key,
    );

    std::fs::create_dir_all(fennec_home)?;
    std::fs::create_dir_all(fennec_home.join("memory"))?;
    std::fs::create_dir_all(fennec_home.join("skills"))?;
    std::fs::create_dir_all(fennec_home.join("pairing"))?;

    let config_path = fennec_home.join("config.toml");
    std::fs::write(&config_path, &config)?;

    println!();
    println!(
        "  {} Config written to {}",
        style("✓").green(),
        config_path.display()
    );
    println!();
    println!("  {}", style("Quick start:").bold());
    println!("    fennec agent               # Interactive chat");
    println!("    fennec agent -m 'Hello'    # Single message");
    println!("    fennec gateway             # Start all channels");
    println!();

    Ok(())
}

fn auto_register_plurum(agent_name: &str) -> anyhow::Result<String> {
    let username = format!(
        "{}-{}",
        agent_name.to_lowercase().replace(' ', "-"),
        &uuid::Uuid::new_v4().to_string()[..8]
    );
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post("https://api.plurum.ai/api/v1/agents/register")
        .json(&serde_json::json!({"name": agent_name, "username": username}))
        .timeout(std::time::Duration::from_secs(10))
        .send()?;
    let json: serde_json::Value = resp.json()?;
    json["api_key"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("No api_key in response"))
}

fn build_config_toml(
    name: &str,
    provider: &str,
    model: &str,
    api_key: &str,
    telegram_token: &str,
    telegram_user_id: &str,
    discord_token: &str,
    plurum_key: &str,
) -> String {
    let allowed_users_line = if telegram_user_id.is_empty() {
        "allowed_users = []".to_string()
    } else {
        format!("allowed_users = [\"{}\"]", telegram_user_id)
    };

    format!(
        r#"[identity]
name = "{name}"
persona = "A fast, helpful AI assistant with collective intelligence."

[provider]
name = "{provider}"
model = "{model}"
api_key = "{api_key}"
base_url = ""
temperature = 0.7
max_tokens = 8192

[memory]
vector_weight = 0.7
keyword_weight = 0.3
half_life_days = 7.0
consolidation_enabled = true

[security]
prompt_guard_action = "warn"
prompt_guard_sensitivity = 0.7
encrypt_secrets = true
command_timeout_secs = 60

[agent]
max_tool_iterations = 15
context_window = 200000

[channels.telegram]
enabled = {telegram_enabled}
token = "{telegram_token}"
{allowed_users_line}

[channels.discord]
enabled = {discord_enabled}
token = "{discord_token}"

[channels.slack]
enabled = false
bot_token = ""
app_token = ""

[gateway]
host = "0.0.0.0"
port = 8990

[cron]
enabled = false

[collective]
enabled = {collective_enabled}
api_key = "{plurum_key}"
base_url = "https://api.plurum.ai"
publish_enabled = true
search_enabled = true
"#,
        name = name,
        provider = provider,
        model = model,
        api_key = api_key,
        telegram_token = telegram_token,
        allowed_users_line = allowed_users_line,
        telegram_enabled = if telegram_token.is_empty() {
            "false"
        } else {
            "true"
        },
        discord_token = discord_token,
        discord_enabled = if discord_token.is_empty() {
            "false"
        } else {
            "true"
        },
        plurum_key = plurum_key,
        collective_enabled = if plurum_key.is_empty() {
            "false"
        } else {
            "true"
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_config_toml_defaults() {
        let config = build_config_toml(
            "TestBot",
            "anthropic",
            "claude-sonnet-4-20250514",
            "sk-test-key",
            "",
            "",
            "",
            "",
        );
        assert!(config.contains("name = \"TestBot\""));
        assert!(config.contains("name = \"anthropic\""));
        assert!(config.contains("api_key = \"sk-test-key\""));
        assert!(config.contains("[channels.telegram]\nenabled = false"));
        assert!(config.contains("[channels.discord]\nenabled = false"));
        assert!(config.contains("[collective]\nenabled = false"));
    }

    #[test]
    fn test_build_config_toml_with_channels() {
        let config = build_config_toml(
            "Fennec",
            "openai",
            "gpt-4o",
            "sk-openai",
            "123:ABC",
            "987654321",
            "discord-token",
            "plurum-key",
        );
        assert!(config.contains("[channels.telegram]\nenabled = true"));
        assert!(config.contains("token = \"123:ABC\""));
        assert!(config.contains("allowed_users = [\"987654321\"]"));
        assert!(config.contains("[channels.discord]\nenabled = true"));
        assert!(config.contains("token = \"discord-token\""));
        assert!(config.contains("[collective]\nenabled = true"));
        assert!(config.contains("api_key = \"plurum-key\""));
    }
}
