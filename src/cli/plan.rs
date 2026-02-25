//! Autonomy plan CLI commands.

use std::sync::Arc;

use anyhow::Context;
use chrono::Utc;
use clap::Subcommand;
use uuid::Uuid;

use crate::agent::{Plan, PlanStatus, PlanStep, PlanStepStatus, PlannerKind};
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

    /// Mark a plan as completed
    Complete {
        /// Plan ID
        id: Uuid,

        /// Owner user ID (used to validate goal ownership)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Mark a plan as superseded
    Supersede {
        /// Plan ID
        id: Uuid,

        /// Owner user ID (used to validate goal ownership)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Create a new revision from an existing plan (replan workflow)
    Replan {
        /// Existing plan ID to replan from
        id: Uuid,

        /// Owner user ID (used to validate goal ownership)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,

        /// Optional summary override for the new revision
        #[arg(long)]
        summary: Option<String>,

        /// Optional confidence override [0.0, 1.0]
        #[arg(long)]
        confidence: Option<f64>,

        /// Optional status override (default: draft)
        #[arg(long)]
        status: Option<String>,

        /// Optional planner kind override (reasoning_v1|rule_based|hybrid)
        #[arg(long)]
        planner_kind: Option<String>,

        /// Optional estimated execution time override
        #[arg(long)]
        estimated_time_secs: Option<u64>,

        /// Optional estimated cost override
        #[arg(long)]
        estimated_cost: Option<f64>,

        /// Keep the source plan status unchanged (do not mark superseded)
        #[arg(long)]
        no_supersede_current: bool,

        /// Copy source plan steps into the new revision (reset to pending)
        #[arg(long)]
        copy_steps: bool,
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
        PlanCommand::Complete { id, user_id } => {
            set_plan_status(db.as_ref(), id, "completed", &user_id).await
        }
        PlanCommand::Supersede { id, user_id } => {
            set_plan_status(db.as_ref(), id, "superseded", &user_id).await
        }
        PlanCommand::Replan {
            id,
            user_id,
            summary,
            confidence,
            status,
            planner_kind,
            estimated_time_secs,
            estimated_cost,
            no_supersede_current,
            copy_steps,
        } => {
            replan_plan(
                db.as_ref(),
                id,
                &user_id,
                summary,
                confidence,
                status,
                planner_kind,
                estimated_time_secs,
                estimated_cost,
                !no_supersede_current,
                copy_steps,
            )
            .await
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

#[allow(clippy::too_many_arguments)]
async fn replan_plan(
    db: &dyn crate::db::Database,
    source_plan_id: Uuid,
    user_id: &str,
    summary: Option<String>,
    confidence: Option<f64>,
    status_raw: Option<String>,
    planner_kind_raw: Option<String>,
    estimated_time_secs: Option<u64>,
    estimated_cost: Option<f64>,
    supersede_current: bool,
    copy_steps: bool,
) -> anyhow::Result<()> {
    if let Some(confidence) = confidence
        && !(0.0..=1.0).contains(&confidence)
    {
        anyhow::bail!("confidence must be between 0.0 and 1.0");
    }

    let source_plan = db
        .get_plan(source_plan_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load source plan {}", source_plan_id))?
        .ok_or_else(|| anyhow::anyhow!("plan not found: {}", source_plan_id))?;

    let goal = db
        .get_goal(source_plan.goal_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load goal {}", source_plan.goal_id))?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "goal {} for plan {} not found",
                source_plan.goal_id,
                source_plan.id
            )
        })?;
    if goal.owner_user_id != user_id {
        anyhow::bail!("plan {} not found for user {}", source_plan_id, user_id);
    }

    let existing = db
        .list_plans_for_goal(source_plan.goal_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to list plans for goal {}", source_plan.goal_id))?;
    let revision = existing.iter().map(|p| p.revision).max().unwrap_or(0) + 1;

    let now = Utc::now();
    let new_plan = Plan {
        id: Uuid::new_v4(),
        goal_id: source_plan.goal_id,
        revision,
        status: match status_raw {
            Some(s) => parse_plan_status(&s)?,
            None => PlanStatus::Draft,
        },
        planner_kind: match planner_kind_raw {
            Some(s) => parse_planner_kind(&s)?,
            None => source_plan.planner_kind,
        },
        source_action_plan: source_plan.source_action_plan.clone(),
        assumptions: source_plan.assumptions.clone(),
        confidence: confidence.unwrap_or(source_plan.confidence),
        estimated_cost: estimated_cost.or(source_plan.estimated_cost),
        estimated_time_secs: estimated_time_secs.or(source_plan.estimated_time_secs),
        summary: match summary {
            Some(s) => {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            None => source_plan.summary.clone(),
        },
        created_at: now,
        updated_at: now,
    };

    db.create_plan(&new_plan)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to create replanned revision for {}", source_plan_id))?;

    let copied_steps = if copy_steps {
        let source_steps = db
            .list_plan_steps_for_plan(source_plan.id)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
            .with_context(|| format!("failed to list source plan steps for {}", source_plan.id))?;

        if source_steps.is_empty() {
            0usize
        } else {
            let now = Utc::now();
            let copied: Vec<PlanStep> = source_steps
                .into_iter()
                .map(|step| PlanStep {
                    id: Uuid::new_v4(),
                    plan_id: new_plan.id,
                    sequence_num: step.sequence_num,
                    kind: step.kind,
                    status: PlanStepStatus::Pending,
                    title: step.title,
                    description: step.description,
                    tool_candidates: step.tool_candidates,
                    inputs: step.inputs,
                    preconditions: step.preconditions,
                    postconditions: step.postconditions,
                    rollback: step.rollback,
                    policy_requirements: step.policy_requirements,
                    started_at: None,
                    completed_at: None,
                    created_at: now,
                    updated_at: now,
                })
                .collect();

            db.create_plan_steps(&copied)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .with_context(|| {
                    format!(
                        "failed to copy source plan steps into new plan {}",
                        new_plan.id
                    )
                })?;
            copied.len()
        }
    } else {
        0usize
    };

    if supersede_current {
        db.update_plan_status(source_plan.id, PlanStatus::Superseded)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
            .with_context(|| format!("failed to supersede source plan {}", source_plan.id))?;
    }

    println!(
        "Created replanned plan {} (rev {}) from {}{}",
        new_plan.id,
        new_plan.revision,
        source_plan.id,
        if copy_steps {
            format!(" with {} copied step(s)", copied_steps)
        } else {
            String::new()
        }
    );
    println!("{}", serde_json::to_string_pretty(&new_plan)?);
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

fn parse_planner_kind(s: &str) -> anyhow::Result<PlannerKind> {
    match s.trim().to_ascii_lowercase().as_str() {
        "reasoning_v1" => Ok(PlannerKind::ReasoningV1),
        "rule_based" => Ok(PlannerKind::RuleBased),
        "hybrid" => Ok(PlannerKind::Hybrid),
        _ => anyhow::bail!(
            "invalid planner kind '{}'; expected reasoning_v1|rule_based|hybrid",
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
