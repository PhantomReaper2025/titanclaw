//! Autonomy plan CLI commands.

use std::sync::Arc;

use anyhow::Context;
use chrono::Utc;
use clap::Subcommand;
use uuid::Uuid;

use crate::agent::{Plan, PlanStatus, PlannerKind};
const DEFAULT_USER_ID: &str = "default";

#[derive(Subcommand, Debug, Clone)]
pub enum PlanCommand {
    /// Create a new plan for an existing goal
    Create {
        /// Goal ID to attach the plan to
        #[arg(long)]
        goal_id: Uuid,

        /// Owner user ID (used to validate goal ownership)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,

        /// Optional plan summary
        #[arg(long)]
        summary: Option<String>,

        /// Confidence score [0.0, 1.0]
        #[arg(long, default_value_t = 0.5)]
        confidence: f64,

        /// Explicit plan revision (default: next revision for the goal)
        #[arg(long)]
        revision: Option<i32>,

        /// Optional estimated execution time
        #[arg(long)]
        estimated_time_secs: Option<u64>,

        /// Optional estimated cost
        #[arg(long)]
        estimated_cost: Option<f64>,
    },

    /// List plans for a goal
    List {
        /// Goal ID
        #[arg(long)]
        goal_id: Uuid,

        /// Owner user ID (used to validate goal ownership)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Show one plan as JSON
    Show {
        /// Plan ID
        id: Uuid,

        /// Owner user ID (used to validate goal ownership)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Update plan status
    SetStatus {
        /// Plan ID
        id: Uuid,

        /// New status (draft|ready|running|paused|failed|completed|superseded)
        status: String,

        /// Owner user ID (used to validate goal ownership)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },
}

pub async fn run_plan_command(cmd: PlanCommand) -> anyhow::Result<()> {
    let db = connect_db().await?;

    match cmd {
        PlanCommand::Create {
            goal_id,
            user_id,
            summary,
            confidence,
            revision,
            estimated_time_secs,
            estimated_cost,
        } => {
            create_plan(
                db.as_ref(),
                goal_id,
                &user_id,
                summary,
                confidence,
                revision,
                estimated_time_secs,
                estimated_cost,
            )
            .await
        }
        PlanCommand::List { goal_id, user_id } => list_plans(db.as_ref(), goal_id, &user_id).await,
        PlanCommand::Show { id, user_id } => show_plan(db.as_ref(), id, &user_id).await,
        PlanCommand::SetStatus {
            id,
            status,
            user_id,
        } => set_plan_status(db.as_ref(), id, &status, &user_id).await,
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

async fn create_plan(
    db: &dyn crate::db::Database,
    goal_id: Uuid,
    user_id: &str,
    summary: Option<String>,
    confidence: f64,
    revision: Option<i32>,
    estimated_time_secs: Option<u64>,
    estimated_cost: Option<f64>,
) -> anyhow::Result<()> {
    if !(0.0..=1.0).contains(&confidence) {
        anyhow::bail!("confidence must be between 0.0 and 1.0");
    }

    let goal = db
        .get_goal(goal_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load goal {}", goal_id))?
        .ok_or_else(|| anyhow::anyhow!("goal not found: {}", goal_id))?;
    if goal.owner_user_id != user_id {
        anyhow::bail!("goal {} not found for user {}", goal_id, user_id);
    }

    let existing = db
        .list_plans_for_goal(goal_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to list plans for goal {}", goal_id))?;

    let revision =
        revision.unwrap_or_else(|| existing.iter().map(|p| p.revision).max().unwrap_or(0) + 1);
    if revision <= 0 {
        anyhow::bail!("revision must be a positive integer");
    }
    if existing.iter().any(|p| p.revision == revision) {
        anyhow::bail!(
            "plan revision {} already exists for goal {}",
            revision,
            goal_id
        );
    }

    let now = Utc::now();
    let plan = Plan {
        id: Uuid::new_v4(),
        goal_id,
        revision,
        status: PlanStatus::Draft,
        planner_kind: PlannerKind::ReasoningV1,
        source_action_plan: None,
        assumptions: serde_json::json!({}),
        confidence,
        estimated_cost,
        estimated_time_secs,
        summary: summary
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        created_at: now,
        updated_at: now,
    };

    db.create_plan(&plan)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to create plan for goal {}", goal_id))?;

    println!("Created plan {} for goal {}", plan.id, plan.goal_id);
    println!("{}", serde_json::to_string_pretty(&plan)?);
    Ok(())
}

async fn list_plans(
    db: &dyn crate::db::Database,
    goal_id: Uuid,
    user_id: &str,
) -> anyhow::Result<()> {
    let goal = db
        .get_goal(goal_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load goal {}", goal_id))?
        .ok_or_else(|| anyhow::anyhow!("goal not found: {}", goal_id))?;
    if goal.owner_user_id != user_id {
        anyhow::bail!("goal {} not found for user {}", goal_id, user_id);
    }

    let mut plans = db
        .list_plans_for_goal(goal_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to list plans for goal {}", goal_id))?;
    plans.sort_by_key(|p| (p.revision, p.updated_at));
    plans.reverse();

    println!("Plans (goal: {}, user: {})", goal_id, user_id);
    if plans.is_empty() {
        println!();
        println!("  (none)");
        return Ok(());
    }

    println!();
    for p in plans {
        let status = serde_json::to_string(&p.status)?;
        println!(
            "  rev={:<3}  {:<10}  conf={:.2}  {}  {}  {}",
            p.revision,
            status.trim_matches('"'),
            p.confidence,
            p.updated_at.to_rfc3339(),
            p.id,
            truncate(p.summary.as_deref().unwrap_or("(no summary)"), 48),
        );
    }

    Ok(())
}

async fn show_plan(
    db: &dyn crate::db::Database,
    plan_id: Uuid,
    user_id: &str,
) -> anyhow::Result<()> {
    let plan = db
        .get_plan(plan_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load plan {}", plan_id))?
        .ok_or_else(|| anyhow::anyhow!("plan not found: {}", plan_id))?;

    let goal = db
        .get_goal(plan.goal_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load goal {}", plan.goal_id))?
        .ok_or_else(|| anyhow::anyhow!("goal {} for plan {} not found", plan.goal_id, plan.id))?;
    if goal.owner_user_id != user_id {
        anyhow::bail!("plan {} not found for user {}", plan_id, user_id);
    }

    println!("{}", serde_json::to_string_pretty(&plan)?);
    Ok(())
}

async fn set_plan_status(
    db: &dyn crate::db::Database,
    plan_id: Uuid,
    status_raw: &str,
    user_id: &str,
) -> anyhow::Result<()> {
    let plan = db
        .get_plan(plan_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load plan {}", plan_id))?
        .ok_or_else(|| anyhow::anyhow!("plan not found: {}", plan_id))?;

    let goal = db
        .get_goal(plan.goal_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load goal {}", plan.goal_id))?
        .ok_or_else(|| anyhow::anyhow!("goal {} for plan {} not found", plan.goal_id, plan.id))?;
    if goal.owner_user_id != user_id {
        anyhow::bail!("plan {} not found for user {}", plan_id, user_id);
    }

    let status = parse_plan_status(status_raw)?;
    db.update_plan_status(plan_id, status)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to update plan {}", plan_id))?;

    let updated = db
        .get_plan(plan_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to reload plan {}", plan_id))?
        .ok_or_else(|| anyhow::anyhow!("plan disappeared after update: {}", plan_id))?;

    println!("Updated plan {}", plan_id);
    println!("{}", serde_json::to_string_pretty(&updated)?);
    Ok(())
}

fn parse_plan_status(s: &str) -> anyhow::Result<PlanStatus> {
    match s.trim().to_ascii_lowercase().as_str() {
        "draft" => Ok(PlanStatus::Draft),
        "ready" => Ok(PlanStatus::Ready),
        "running" => Ok(PlanStatus::Running),
        "paused" => Ok(PlanStatus::Paused),
        "failed" => Ok(PlanStatus::Failed),
        "completed" => Ok(PlanStatus::Completed),
        "superseded" => Ok(PlanStatus::Superseded),
        _ => anyhow::bail!(
            "invalid plan status '{}'; expected draft|ready|running|paused|failed|completed|superseded",
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
