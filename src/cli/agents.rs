//! CLI subcommand definitions for `ironclaw agents`.
//!
//! Manage subagents and isolated execution sessions.

use clap::Subcommand;

/// Agents subcommands for spawning and managing subagents.
#[derive(Subcommand, Debug, Clone)]
pub enum AgentsCommand {
    /// List available agent types that can be spawned.
    List {
        /// Show only installed/configured agents.
        #[arg(short, long)]
        installed: bool,
    },

    /// Spawn a new subagent session.
    Spawn {
        /// Agent type to spawn (e.g., "codex", "claude-code", "opencode").
        agent_type: String,

        /// Task description for the subagent.
        #[arg(short, long)]
        task: String,

        /// Working directory for the subagent.
        #[arg(short, long)]
        cwd: Option<String>,

        /// Run in background (don't wait for completion).
        #[arg(short, long)]
        background: bool,

        /// Timeout in seconds.
        #[arg(short, long, default_value = "600")]
        timeout: u32,

        /// Model to use for this subagent.
        #[arg(short, long)]
        model: Option<String>,
    },

    /// Show status of active subagents.
    Status {
        /// Session ID to show status for (omit for all).
        session_id: Option<String>,
    },

    /// Send a message to a running subagent.
    Send {
        /// Session ID to send message to.
        session_id: String,

        /// Message to send.
        message: String,
    },

    /// Kill a running subagent.
    Kill {
        /// Session ID to kill.
        session_id: String,

        /// Force kill (no graceful shutdown).
        #[arg(short, long)]
        force: bool,
    },

    /// Wait for a subagent to complete and show results.
    Wait {
        /// Session ID to wait for.
        session_id: String,

        /// Maximum time to wait in seconds.
        #[arg(short, long, default_value = "300")]
        timeout: u32,
    },

    /// View output/logs from a subagent.
    Logs {
        /// Session ID to view logs for.
        session_id: String,

        /// Number of lines to show.
        #[arg(short, long, default_value = "50")]
        lines: usize,

        /// Follow logs in real-time.
        #[arg(short, long)]
        follow: bool,
    },
}

/// Run the agents command.
pub async fn run_agents_command(cmd: AgentsCommand) -> anyhow::Result<()> {
    match cmd {
        AgentsCommand::List { installed } => {
            println!("Available Agents:");
            println!();
            if installed {
                println!("  (no agents installed)");
            } else {
                println!("Built-in:");
                println!("  codex        OpenAI Codex agent for coding tasks");
                println!("  claude-code  Anthropic Claude Code agent");
                println!("  opencode     OpenCode agent for general tasks");
                println!();
                println!("Install with: titanclaw agents spawn <type> --task '...'");
            }
        }
        AgentsCommand::Spawn {
            agent_type,
            task,
            cwd,
            background,
            timeout,
            model,
        } => {
            println!("Spawning {} agent...", agent_type);
            println!("Task: {}", task);
            if let Some(c) = &cwd {
                println!("Working directory: {}", c);
            }
            println!("Timeout: {}s", timeout);
            if let Some(m) = &model {
                println!("Model: {}", m);
            }
            if background {
                println!("Mode: background");
                println!();
                println!("Session ID: (not implemented - requires runtime)");
            } else {
                println!("Mode: blocking (wait for completion)");
                println!();
                println!("Subagent spawning requires runtime connection.");
                println!("Run 'titanclaw run' first to enable agent orchestration.");
            }
            // TODO: Integrate with agent runtime/orchestrator
        }
        AgentsCommand::Status { session_id } => {
            if let Some(id) = session_id {
                println!("Session Status: {}", id);
                println!("Status: Not found");
            } else {
                println!("Active Subagents:");
                println!();
                println!("No active subagents.");
            }
            // TODO: Query session manager from runtime
        }
        AgentsCommand::Send {
            session_id,
            message,
        } => {
            println!("Sending message to session {}", session_id);
            println!("Message: {}", message);
            println!();
            println!("Message delivery requires runtime connection.");
            // TODO: Send message via session manager
        }
        AgentsCommand::Kill {
            session_id,
            force,
        } => {
            println!("{} killing session: {}", if force { "Force" } else { "Gracefully" }, session_id);
            // TODO: Kill session via session manager
        }
        AgentsCommand::Wait {
            session_id,
            timeout,
        } => {
            println!("Waiting for session {} (timeout: {}s)...", session_id, timeout);
            println!();
            println!("Waiting requires runtime connection.");
            // TODO: Wait for session completion
        }
        AgentsCommand::Logs {
            session_id,
            lines,
            follow,
        } => {
            println!("Logs for session {} (last {} lines)", session_id, lines);
            if follow {
                println!("Following... (Ctrl+C to stop)");
            }
            println!();
            println!("Log retrieval requires runtime connection.");
            // TODO: Retrieve logs from session
        }
    }
    Ok(())
}