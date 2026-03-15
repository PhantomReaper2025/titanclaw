//! CLI subcommand definitions for `ironclaw approvals`.
//!
//! Manage pending approvals for tool executions that require user consent.

use clap::Subcommand;

/// Approvals subcommands for managing pending tool approvals.
#[derive(Subcommand, Debug, Clone)]
pub enum ApprovalsCommand {
    /// List all pending approvals awaiting user action.
    List {
        /// Filter by approval status (pending, approved, denied, expired).
        #[arg(short, long, default_value = "pending")]
        status: String,

        /// Maximum number of approvals to show.
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Show details of a specific approval request.
    Show {
        /// Approval ID to show details for.
        id: String,
    },

    /// Approve a pending tool execution.
    Approve {
        /// Approval ID to approve.
        id: String,

        /// Optional note explaining approval.
        #[arg(short, long)]
        note: Option<String>,
    },

    /// Deny a pending tool execution.
    Deny {
        /// Approval ID to deny.
        id: String,

        /// Reason for denial.
        #[arg(short, long)]
        reason: Option<String>,
    },

    /// Approve all pending approvals.
    ApproveAll {
        /// Optional filter by tool name.
        #[arg(short, long)]
        tool: Option<String>,

        /// Optional note for all approvals.
        #[arg(short, long)]
        note: Option<String>,
    },

    /// Deny all pending approvals.
    DenyAll {
        /// Optional filter by tool name.
        #[arg(short, long)]
        tool: Option<String>,

        /// Reason for all denials.
        #[arg(short, long)]
        reason: Option<String>,
    },

    /// Clear expired or completed approvals from history.
    Clear {
        /// Clear only expired approvals.
        #[arg(long)]
        expired: bool,

        /// Clear only denied approvals.
        #[arg(long)]
        denied: bool,

        /// Clear all non-pending approvals.
        #[arg(long)]
        all: bool,
    },

    /// Set auto-approval rules for specific tools.
    AutoApprove {
        /// Tool name to configure auto-approval for.
        #[arg(short, long)]
        tool: String,

        /// Enable or disable auto-approval (true/false).
        #[arg(short, long)]
        enabled: bool,

        /// Maximum risk level to auto-approve (low, medium, high).
        #[arg(short = 'L', long, default_value = "low")]
        max_risk_level: String,
    },

    /// Show current auto-approval rules.
    Rules,
}

/// Run the approvals command.
pub async fn run_approvals_command(cmd: ApprovalsCommand) -> anyhow::Result<()> {
    match cmd {
        ApprovalsCommand::List { status, limit } => {
            println!("Pending Approvals (status: {}, limit: {})", status, limit);
            println!();
            println!("No pending approvals found.");
            // TODO: Query approval store from database
            // SELECT * FROM approvals WHERE status = ? ORDER BY created_at DESC LIMIT ?
        }
        ApprovalsCommand::Show { id } => {
            println!("Approval Details: {}", id);
            println!();
            println!("Status: Not found");
            // TODO: Query approval details from database
        }
        ApprovalsCommand::Approve { id, note } => {
            println!("Approved: {}", id);
            if let Some(n) = note {
                println!("Note: {}", n);
            }
            // TODO: Update approval status in database
            // UPDATE approvals SET status = 'approved', note = ? WHERE id = ?
        }
        ApprovalsCommand::Deny { id, reason } => {
            println!("Denied: {}", id);
            if let Some(r) = reason {
                println!("Reason: {}", r);
            }
            // TODO: Update approval status in database
            // UPDATE approvals SET status = 'denied', reason = ? WHERE id = ?
        }
        ApprovalsCommand::ApproveAll { tool, note } => {
            let filter = tool.as_deref().unwrap_or("all tools");
            println!("Approving all pending approvals for: {}", filter);
            if let Some(n) = note {
                println!("Note: {}", n);
            }
            // TODO: Batch update approvals
        }
        ApprovalsCommand::DenyAll { tool, reason } => {
            let filter = tool.as_deref().unwrap_or("all tools");
            println!("Denying all pending approvals for: {}", filter);
            if let Some(r) = reason {
                println!("Reason: {}", r);
            }
            // TODO: Batch update approvals
        }
        ApprovalsCommand::Clear { expired, denied, all } => {
            let mode = if all {
                "all non-pending"
            } else if expired {
                "expired"
            } else if denied {
                "denied"
            } else {
                "expired and denied"
            };
            println!("Clearing {} approvals...", mode);
            // TODO: Delete cleared approvals from database
        }
        ApprovalsCommand::AutoApprove {
            tool,
            enabled,
            max_risk_level,
        } => {
            println!(
                "Auto-approval for '{}': {} (max risk: {})",
                tool,
                if enabled { "enabled" } else { "disabled" },
                max_risk_level
            );
            // TODO: Store auto-approval rule in database
        }
        ApprovalsCommand::Rules => {
            println!("Current Auto-Approval Rules:");
            println!();
            println!("No rules configured.");
            // TODO: Query and display auto-approval rules
        }
    }
    Ok(())
}