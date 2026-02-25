//! Autonomy goal CLI commands.

use std::sync::Arc;

use anyhow::Context;
use chrono::Utc;
use clap::Subcommand;
use uuid::Uuid;

use crate::agent::{Goal, GoalRiskClass, GoalSource, GoalStatus};
const DEFAULT_USER_ID: &str = "default";

#[derive(Subcommand, Debug, Clone)]
pub enum GoalCommand {
    /// Create a new autonomy goal
    Create {
        /// Goal title (short label)
        #[arg(long)]
        title: String,

        /// Goal intent / description
        #[arg(long)]
        intent: String,

        /// Owner user ID (defaults to local CLI user)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,

        /// Priority score (higher = more important)
        #[arg(long, default_value_t = 0)]
        priority: i32,

        /// Optional channel context (e.g. web, cli, telegram)
        #[arg(long)]
        channel: Option<String>,

        /// Optional external thread context UUID
        #[arg(long)]
        thread_id: Option<Uuid>,
    },

    /// List autonomy goals for a user
    List {
        /// Owner user ID to filter by
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,

        /// Optional status filter (proposed|active|blocked|waiting|completed|abandoned)
        #[arg(long)]
        status: Option<String>,

        /// Max number of goals to print (most recent first)
        #[arg(long)]
        limit: Option<usize>,
    },

    /// Show one goal as JSON
    Show {
        /// Goal ID
        id: Uuid,

        /// Owner user ID to validate access
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Update goal status
    SetStatus {
        /// Goal ID
        id: Uuid,

        /// New status (proposed|active|blocked|waiting|completed|abandoned)
        status: String,

        /// Owner user ID to validate access
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Update goal priority
    SetPriority {
        /// Goal ID
        id: Uuid,

        /// New priority score (higher = more important)
        priority: i32,

        /// Owner user ID to validate access
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Mark a goal as completed
    Complete {
        /// Goal ID
        id: Uuid,

        /// Owner user ID to validate access
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Cancel a goal (alias for abandoned)
    Cancel {
        /// Goal ID
        id: Uuid,

        /// Owner user ID to validate access
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Mark a goal as abandoned
    Abandon {
        /// Goal ID
        id: Uuid,

        /// Owner user ID to validate access
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },
}

pub async fn run_goal_command(cmd: GoalCommand) -> anyhow::Result<()> {
    let db = connect_db().await?;

    match cmd {
        GoalCommand::Create {
            title,
            intent,
            user_id,
            priority,
            channel,
            thread_id,
        } => {
            create_goal(
                db.as_ref(),
                title,
                intent,
                user_id,
                priority,
                channel,
                thread_id,
            )
            .await
        }
        GoalCommand::List {
            user_id,
            status,
            limit,
        } => list_goals(db.as_ref(), &user_id, status.as_deref(), limit).await,
        GoalCommand::Show { id, user_id } => show_goal(db.as_ref(), id, &user_id).await,
        GoalCommand::SetStatus {
            id,
            status,
            user_id,
        } => set_goal_status(db.as_ref(), id, &status, &user_id).await,
        GoalCommand::SetPriority {
            id,
            priority,
            user_id,
        } => set_goal_priority(db.as_ref(), id, priority, &user_id).await,
        GoalCommand::Complete { id, user_id } => {
            set_goal_status(db.as_ref(), id, "completed", &user_id).await
        }
        GoalCommand::Cancel { id, user_id } => {
            set_goal_status(db.as_ref(), id, "abandoned", &user_id).await
        }
        GoalCommand::Abandon { id, user_id } => {
            set_goal_status(db.as_ref(), id, "abandoned", &user_id).await
        }
    }
}

async fn connect_db() -> anyhow::Result<Arc<dyn crate::db::Database>> {
    let config = crate::config::Config::from_env()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let db = crate::db::connect_from_config(&config.database)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    db.run_migrations()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(db)
}

async fn create_goal(
    db: &dyn crate::db::Database,
    title: String,
    intent: String,
    user_id: String,
    priority: i32,
    channel: Option<String>,
    thread_id: Option<Uuid>,
) -> anyhow::Result<()> {
    let title = title.trim();
    let intent = intent.trim();
    if title.is_empty() {
        anyhow::bail!("title is required");
    }
    if intent.is_empty() {
        anyhow::bail!("intent is required");
    }

    let now = Utc::now();
    let goal = Goal {
        id: Uuid::new_v4(),
        owner_user_id: user_id,
        channel: channel
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        thread_id,
        title: title.to_string(),
        intent: intent.to_string(),
        priority,
        status: GoalStatus::Active,
        risk_class: GoalRiskClass::Medium,
        acceptance_criteria: serde_json::json!({}),
        constraints: serde_json::json!({}),
        source: GoalSource::UserRequest,
        created_at: now,
        updated_at: now,
        completed_at: None,
    };

    db.create_goal(&goal)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| "failed to create goal")?;

    println!("Created goal {}", goal.id);
    println!("{}", serde_json::to_string_pretty(&goal)?);
    Ok(())
}

async fn list_goals(
    db: &dyn crate::db::Database,
    user_id: &str,
    status_filter: Option<&str>,
    limit: Option<usize>,
) -> anyhow::Result<()> {
    let mut goals = db
        .list_goals()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| "failed to list goals")?;

    let status_filter = match status_filter {
        Some(raw) => Some(parse_goal_status(raw)?),
        None => None,
    };

    goals.retain(|g| g.owner_user_id == user_id);
    if let Some(status) = status_filter {
        goals.retain(|g| g.status == status);
    }
    goals.sort_by_key(|g| (g.updated_at, g.created_at));
    goals.reverse();
    if let Some(limit) = limit {
        if limit == 0 {
            anyhow::bail!("limit must be >= 1");
        }
        goals.truncate(limit);
    }

    println!("Goals (user: {})", user_id);
    if goals.is_empty() {
        println!();
        println!("  (none)");
        return Ok(());
    }

    println!();
    for g in goals {
        let status = serde_json::to_string(&g.status)?;
        println!(
            "  {}  {:<10}  p={:<3}  {}  {}",
            g.id,
            status.trim_matches('"'),
            g.priority,
            g.updated_at.to_rfc3339(),
            truncate(&g.title, 72)
        );
    }

    Ok(())
}

async fn show_goal(db: &dyn crate::db::Database, id: Uuid, user_id: &str) -> anyhow::Result<()> {
    let goal = db
        .get_goal(id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load goal {}", id))?
        .ok_or_else(|| anyhow::anyhow!("goal not found: {}", id))?;

    if goal.owner_user_id != user_id {
        anyhow::bail!("goal {} not found for user {}", id, user_id);
    }

    println!("{}", serde_json::to_string_pretty(&goal)?);
    Ok(())
}

async fn set_goal_status(
    db: &dyn crate::db::Database,
    id: Uuid,
    status_raw: &str,
    user_id: &str,
) -> anyhow::Result<()> {
    let goal = db
        .get_goal(id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load goal {}", id))?
        .ok_or_else(|| anyhow::anyhow!("goal not found: {}", id))?;

    if goal.owner_user_id != user_id {
        anyhow::bail!("goal {} not found for user {}", id, user_id);
    }

    let status = parse_goal_status(status_raw)?;
    db.update_goal_status(id, status)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to update goal {}", id))?;

    let updated = db
        .get_goal(id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to reload goal {}", id))?
        .ok_or_else(|| anyhow::anyhow!("goal disappeared after update: {}", id))?;

    println!("Updated goal {}", id);
    println!("{}", serde_json::to_string_pretty(&updated)?);
    Ok(())
}

async fn set_goal_priority(
    db: &dyn crate::db::Database,
    id: Uuid,
    priority: i32,
    user_id: &str,
) -> anyhow::Result<()> {
    let goal = db
        .get_goal(id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load goal {}", id))?
        .ok_or_else(|| anyhow::anyhow!("goal not found: {}", id))?;

    if goal.owner_user_id != user_id {
        anyhow::bail!("goal {} not found for user {}", id, user_id);
    }

    db.update_goal_priority(id, priority)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to update goal priority {}", id))?;

    let updated = db
        .get_goal(id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to reload goal {}", id))?
        .ok_or_else(|| anyhow::anyhow!("goal disappeared after update: {}", id))?;

    println!("Updated goal {}", id);
    println!("{}", serde_json::to_string_pretty(&updated)?);
    Ok(())
}

fn parse_goal_status(s: &str) -> anyhow::Result<GoalStatus> {
    match s.trim().to_ascii_lowercase().as_str() {
        "proposed" => Ok(GoalStatus::Proposed),
        "active" => Ok(GoalStatus::Active),
        "blocked" => Ok(GoalStatus::Blocked),
        "waiting" => Ok(GoalStatus::Waiting),
        "completed" => Ok(GoalStatus::Completed),
        "abandoned" => Ok(GoalStatus::Abandoned),
        _ => anyhow::bail!(
            "invalid goal status '{}'; expected proposed|active|blocked|waiting|completed|abandoned",
            s
        ),
    }
}

fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(3);
    let mut out = String::with_capacity(max);
    for ch in s.chars().take(keep) {
        out.push(ch);
    }
    out.push_str("...");
    out
}
