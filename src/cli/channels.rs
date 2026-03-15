//! Channel management CLI commands.
//!
//! Commands for viewing and managing chat channels (Telegram, Discord, etc).

use std::path::PathBuf;

use clap::Subcommand;

use crate::channels::wasm::{discover_channels, bundled_channel_names, available_channel_names};
use crate::settings::Settings;

/// Default channels directory.
fn default_channels_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".ironclaw").join("channels"))
        .unwrap_or_else(|| PathBuf::from(".ironclaw/channels"))
}

#[derive(Subcommand, Debug, Clone)]
pub enum ChannelsCommand {
    /// List all available channels and their status
    List {
        /// Show detailed information
        #[arg(short, long)]
        verbose: bool,
    },

    /// Show status of a specific channel
    Status {
        /// Channel name to check
        name: String,
    },

    /// Show bundled channels available for installation
    Available,

    /// Install a bundled channel
    Install {
        /// Channel name to install
        name: String,
    },
}

/// Run a channels command.
pub async fn run_channels_command(cmd: ChannelsCommand) -> anyhow::Result<()> {
    match cmd {
        ChannelsCommand::List { verbose } => list_channels(verbose).await,
        ChannelsCommand::Status { name } => channel_status(&name).await,
        ChannelsCommand::Available => list_available_channels().await,
        ChannelsCommand::Install { name } => install_channel(&name).await,
    }
}

/// List all channels and their status.
async fn list_channels(verbose: bool) -> anyhow::Result<()> {
    let settings = Settings::default();
    let channels_dir = settings
        .channels
        .wasm_channels_dir
        .clone()
        .unwrap_or_else(default_channels_dir);

    println!("TitanClaw Channels");
    println!("==================\n");

    // Built-in channels
    println!("Built-in Channels:");
    
    // CLI channel
    let cli_status = if settings.channels.http_enabled || std::env::var("CLI_ENABLED")
        .map(|v| v != "false" && v != "0")
        .unwrap_or(true)
    {
        "enabled"
    } else {
        "disabled"
    };
    println!("  {:15} [builtin]  {}", "cli", cli_status);

    // HTTP webhook channel
    let http_enabled = settings.channels.http_enabled
        || std::env::var("HTTP_PORT").is_ok()
        || std::env::var("HTTP_HOST").is_ok();
    let http_status = if http_enabled {
        let port = settings.channels.http_port
            .or_else(|| std::env::var("HTTP_PORT").ok().and_then(|p| p.parse().ok()))
            .unwrap_or(3000);
        format!("enabled (port {})", port)
    } else {
        "disabled".to_string()
    };
    println!("  {:15} [builtin]  {}", "http", http_status);

    // Gateway (web UI) channel
    let gateway_enabled = std::env::var("GATEWAY_ENABLED")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(true);
    let gateway_status = if gateway_enabled {
        let port = std::env::var("GATEWAY_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3000);
        format!("enabled (port {})", port)
    } else {
        "disabled".to_string()
    };
    println!("  {:15} [builtin]  {}", "gateway", gateway_status);

    // WASM channels
    println!("\nWASM Channels:");
    
    if !channels_dir.exists() {
        println!("  Directory not found: {}", channels_dir.display());
        println!("  Run `titanclaw channels available` to see installable channels.");
    } else {
        // Discover installed WASM channels
        match discover_channels(&channels_dir).await {
            Ok(discovered_map) => {
                if discovered_map.is_empty() {
                    println!("  No WASM channels installed.");
                } else {
                    for (name, channel) in discovered_map {
                        let has_capabilities = channel.capabilities_path.is_some();
                        let config_status = if has_capabilities {
                            "configured"
                        } else {
                            "not configured"
                        };
                        
                        if verbose {
                            println!(
                                "  {:15} [wasm]    {} ({})",
                                name,
                                config_status,
                                channel.wasm_path.display()
                            );
                        } else {
                            println!(
                                "  {:15} [wasm]    {}",
                                name,
                                config_status
                            );
                        }
                    }
                }
            }
            Err(e) => {
                println!("  Error discovering channels: {}", e);
            }
        }
    }

    // Configuration hints
    if verbose {
        println!("\nConfiguration:");
        println!("  Channels directory: {}", channels_dir.display());
        println!("  WASM channels enabled: {}", settings.channels.wasm_channels_enabled);
        if let Some(owner) = settings.channels.telegram_owner_id {
            println!("  Telegram owner ID: {}", owner);
        }
        println!("\nEnvironment Variables:");
        println!("  TELEGRAM_BOT_TOKEN    - Telegram bot token");
        println!("  TELEGRAM_OWNER_ID     - Restrict bot to this user");
        println!("  WHATSAPP_ACCESS_TOKEN - WhatsApp Business API token");
        println!("  SLACK_BOT_TOKEN       - Slack bot OAuth token");
    } else {
        println!("\nRun with --verbose for more details.");
    }

    Ok(())
}

/// Show status of a specific channel.
async fn channel_status(name: &str) -> anyhow::Result<()> {
    let settings = Settings::default();
    let channels_dir = settings
        .channels
        .wasm_channels_dir
        .clone()
        .unwrap_or_else(default_channels_dir);

    println!("Channel Status: {}", name);
    println!("{}\n", "=".repeat(17 + name.len()));

    // Check built-in channels
    match name {
        "cli" => {
            println!("Type: Built-in");
            println!("Status: {}", if std::env::var("CLI_ENABLED").map(|v| v != "false").unwrap_or(true) { "enabled" } else { "disabled" });
            println!("Description: Interactive terminal interface");
            return Ok(());
        }
        "http" => {
            println!("Type: Built-in");
            let enabled = settings.channels.http_enabled
                || std::env::var("HTTP_PORT").is_ok()
                || std::env::var("HTTP_HOST").is_ok();
            println!("Status: {}", if enabled { "enabled" } else { "disabled" });
            if enabled {
                println!("Host: {}", std::env::var("HTTP_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()));
                println!("Port: {}", std::env::var("HTTP_PORT").unwrap_or_else(|_| "3000".to_string()));
            }
            println!("Description: HTTP webhook receiver for external integrations");
            return Ok(());
        }
        "gateway" => {
            println!("Type: Built-in");
            let enabled = std::env::var("GATEWAY_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true);
            println!("Status: {}", if enabled { "enabled" } else { "disabled" });
            if enabled {
                println!("Host: {}", std::env::var("GATEWAY_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()));
                println!("Port: {}", std::env::var("GATEWAY_PORT").unwrap_or_else(|_| "3000".to_string()));
                if let Ok(token) = std::env::var("GATEWAY_AUTH_TOKEN") {
                    println!("Auth token: {}...", &token[..8.min(token.len())]);
                } else {
                    println!("Auth token: (auto-generated at startup)");
                }
            }
            println!("Description: Web UI for browser-based chat");
            return Ok(());
        }
        _ => {}
    }

    // Check WASM channels
    if !channels_dir.exists() {
        anyhow::bail!("Channels directory not found: {}", channels_dir.display());
    }

    match discover_channels(&channels_dir).await {
        Ok(discovered_map) => {
            let channel = discovered_map
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("Channel not found: {}", name))?;

            println!("Type: WASM");
            println!("Path: {}", channel.wasm_path.display());
            let has_capabilities = channel.capabilities_path.is_some();
            println!("Has capabilities: {}", has_capabilities);

            // Check for known channel types and show relevant env vars
            if name == "telegram" || name.contains("telegram") {
                println!("\nConfiguration:");
                let token_set = std::env::var("TELEGRAM_BOT_TOKEN").is_ok();
                let owner_set = std::env::var("TELEGRAM_OWNER_ID").is_ok()
                    || settings.channels.telegram_owner_id.is_some();
                println!("  TELEGRAM_BOT_TOKEN: {}", if token_set { "set" } else { "not set" });
                println!("  TELEGRAM_OWNER_ID: {}", if owner_set { "set" } else { "not set" });
            } else if name == "whatsapp" || name.contains("whatsapp") {
                println!("\nConfiguration:");
                let token_set = std::env::var("WHATSAPP_ACCESS_TOKEN").is_ok();
                println!("  WHATSAPP_ACCESS_TOKEN: {}", if token_set { "set" } else { "not set" });
            } else if name == "slack" || name.contains("slack") {
                println!("\nConfiguration:");
                let token_set = std::env::var("SLACK_BOT_TOKEN").is_ok();
                println!("  SLACK_BOT_TOKEN: {}", if token_set { "set" } else { "not set" });
            }

            // Show if it's in the enabled list
            let enabled_list = &settings.channels.wasm_channels;
            if enabled_list.contains(&name.to_string()) {
                println!("\nStatus: enabled (in wasm_channels list)");
            } else {
                println!("\nStatus: auto-enabled" );
            }
        }
        Err(e) => {
            anyhow::bail!("Error discovering channels: {}", e);
        }
    }

    Ok(())
}

/// List available bundled channels for installation.
async fn list_available_channels() -> anyhow::Result<()> {
    println!("Available Bundled Channels");
    println!("==========================\n");

    let bundled = bundled_channel_names();
    
    if bundled.is_empty() {
        println!("No bundled channels available.");
        return Ok(());
    }

    // Also show all available (including non-bundled)
    let all_available = available_channel_names();
    
    for name in &bundled {
        println!("  {} [bundled]", name);
    }
    
    for name in all_available.iter().filter(|n| !bundled.contains(n)) {
        println!("  {} [external]", name);
    }

    println!("\nInstall with: titanclaw channels install <name>");

    Ok(())
}

/// Install a bundled channel.
async fn install_channel(name: &str) -> anyhow::Result<()> {
    let settings = Settings::default();
    let channels_dir = settings
        .channels
        .wasm_channels_dir
        .clone()
        .unwrap_or_else(default_channels_dir);

    // Check if already installed
    let wasm_path = channels_dir.join(format!("{}.wasm", name));
    if wasm_path.exists() {
        println!("Channel '{}' already installed at {}", name, wasm_path.display());
        return Ok(());
    }

    // Try to install bundled channel
    println!("Installing channel '{}'...", name);
    
    // Create directory if needed
    if !channels_dir.exists() {
        std::fs::create_dir_all(&channels_dir)?;
        println!("Created directory: {}", channels_dir.display());
    }

    // Use the install_bundled_channel function
    // force=false because we already checked if it exists
    match crate::channels::wasm::install_bundled_channel(name, &channels_dir, false).await {
        Ok(()) => {
            println!("Channel '{}' installed successfully!", name);
            println!("Configure it by setting the required environment variables.");
        }
        Err(e) => {
            anyhow::bail!("Failed to install channel '{}': {}", name, e);
        }
    }

    Ok(())
}