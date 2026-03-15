//! Routine (cron) management CLI commands.
//!
//! Provides subcommands for managing routines (scheduled tasks):
//! - `list` - List all routines with status
//! - `add` - Create a new routine
//! - `rm` - Delete a routine
//! - `run` - Manually trigger a routine
//! - `status` - Show routine summary

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use clap::Subcommand;
use uuid::Uuid;

use crate::agent::routine::{
    NotifyConfig, Routine, RoutineAction, RoutineGuardrails, Trigger, next_cron_fire,
};

const DEFAULT_USER_ID: &str = "default";

#[derive(Subcommand, Debug, Clone)]
pub enum CronCommand {
    /// List all routines with their status
    List {
        /// Owner user ID to filter by
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,

        /// Show only enabled routines
        #[arg(long)]
        enabled: bool,

        /// Show only disabled routines
        #[arg(long)]
        disabled: bool,
    },

    /// Create a new routine
    Add {
        /// Unique name for the routine
        #[arg(long)]
        name: String,

        /// Description of what the routine does
        #[arg(long)]
        description: Option<String>,

        /// Trigger type: cron, event, webhook, or manual
        #[arg(long, default_value = "cron")]
        trigger: String,

        /// Cron schedule (required for cron trigger, e.g., "0 9 * * MON-FRI")
        #[arg(long)]
        schedule: Option<String>,

        /// Event pattern regex (required for event trigger)
        #[arg(long)]
        pattern: Option<String>,

        /// Channel filter for event trigger
        #[arg(long)]
        channel: Option<String>,

        /// Prompt/instructions for the routine
        #[arg(long)]
        prompt: String,

        /// Action type: lightweight (default) or full_job
        #[arg(long, default_value = "lightweight")]
        action: String,

        /// Cooldown in seconds between fires (default: 300)
        #[arg(long, default_value = "300")]
        cooldown: u64,

        /// Owner user ID
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Delete a routine
    Rm {
        /// Routine ID or name
        name_or_id: String,

        /// Owner user ID to validate access
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Manually trigger a routine
    Run {
        /// Routine ID or name
        name_or_id: String,

        /// Owner user ID to validate access
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Show routine status summary
    Status {
        /// Owner user ID to filter by
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },
}

pub async fn run_cron_command(cmd: CronCommand) -> anyhow::Result<()> {
    let db = connect_db().await?;

    match cmd {
        CronCommand::List {
            user_id,
            enabled,
            disabled,
        } => list_routines(db.as_ref(), &user_id, enabled, disabled).await,
        CronCommand::Add {
            name,
            description,
            trigger,
            schedule,
            pattern,
            channel,
            prompt,
            action,
            cooldown,
            user_id,
        } => {
            add_routine(
                db.as_ref(),
                name,
                description,
                trigger,
                schedule,
                pattern,
                channel,
                prompt,
                action,
                cooldown,
                user_id,
            )
            .await
        }
        CronCommand::Rm { name_or_id, user_id } => remove_routine(db.as_ref(), &name_or_id, &user_id).await,
        CronCommand::Run { name_or_id, user_id } => run_routine(db.as_ref(), &name_or_id, &user_id).await,
        CronCommand::Status { user_id } => status_routines(db.as_ref(), &user_id).await,
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

async fn list_routines(
    db: &dyn crate::db::Database,
    user_id: &str,
    enabled_only: bool,
    disabled_only: bool,
) -> anyhow::Result<()> {
    let routines = db
        .list_routines(user_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| "failed to list routines")?;

    let filtered: Vec<_> = routines
        .into_iter()
        .filter(|r| {
            if enabled_only && !r.enabled {
                return false;
            }
            if disabled_only && r.enabled {
                return false;
            }
            true
        })
        .collect();

    println!("Routines (user: {})", user_id);
    if filtered.is_empty() {
        println!();
        println!("  (none)");
        return Ok(());
    }

    println!();
    for r in filtered {
        let status = if !r.enabled {
            "disabled"
        } else if r.consecutive_failures > 0 {
            "failing"
        } else {
            "active"
        };

        let trigger_info = match &r.trigger {
            Trigger::Cron { schedule } => format!("cron: {}", schedule),
            Trigger::Event { pattern, channel, .. } => {
                let ch = channel.as_deref().unwrap_or("any");
                format!("event({}): /{}/", ch, pattern)
            }
            Trigger::Webhook { path, .. } => {
                let p = path.as_deref().unwrap_or("/");
                format!("webhook: {}", p)
            }
            Trigger::Manual => "manual".to_string(),
        };

        let next = r
            .next_fire_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "-".to_string());

        println!(
            "  {}  [{:<8}] {:<10}  {}  runs:{}  next:{}",
            r.id,
            status,
            r.name,
            trigger_info,
            r.run_count,
            next
        );

        if !r.description.is_empty() {
            println!("    {}", truncate(&r.description, 72));
        }
    }

    Ok(())
}

async fn add_routine(
    db: &dyn crate::db::Database,
    name: String,
    description: Option<String>,
    trigger_type: String,
    schedule: Option<String>,
    pattern: Option<String>,
    channel: Option<String>,
    prompt: String,
    action_type: String,
    cooldown_secs: u64,
    user_id: String,
) -> anyhow::Result<()> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("name is required");
    }
    if prompt.trim().is_empty() {
        anyhow::bail!("prompt is required");
    }

    // Build trigger
    let trigger = match trigger_type.as_str() {
        "cron" => {
            let schedule_str = schedule
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("cron trigger requires --schedule"))?;
            // Validate cron expression
            next_cron_fire(schedule_str)
                .map_err(|e| anyhow::anyhow!("invalid cron schedule: {}", e))?;
            Trigger::Cron {
                schedule: schedule_str.to_string(),
            }
        }
        "event" => {
            let pattern_str = pattern
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("event trigger requires --pattern"))?;
            // Validate regex
            regex::Regex::new(pattern_str)
                .map_err(|e| anyhow::anyhow!("invalid regex pattern: {}", e))?;
            Trigger::Event {
                channel: channel.filter(|s| !s.is_empty()),
                pattern: pattern_str.to_string(),
            }
        }
        "webhook" => Trigger::Webhook {
            path: None,
            secret: None,
        },
        "manual" => Trigger::Manual,
        other => anyhow::bail!(
            "unknown trigger type '{}'; expected cron|event|webhook|manual",
            other
        ),
    };

    // Build action
    let action = match action_type.as_str() {
        "lightweight" => RoutineAction::Lightweight {
            prompt: prompt.trim().to_string(),
            context_paths: vec![],
            max_tokens: 4096,
        },
        "full_job" => RoutineAction::FullJob {
            title: name.to_string(),
            description: prompt.trim().to_string(),
            max_iterations: 10,
        },
        other => anyhow::bail!(
            "unknown action type '{}'; expected lightweight|full_job",
            other
        ),
    };

    // Compute next fire time for cron
    let next_fire = if let Trigger::Cron { ref schedule } = trigger {
        next_cron_fire(schedule).unwrap_or(None)
    } else {
        None
    };

    let now = Utc::now();
    let routine = Routine {
        id: Uuid::new_v4(),
        name: name.to_string(),
        description: description.unwrap_or_default().trim().to_string(),
        user_id,
        enabled: true,
        trigger,
        action,
        guardrails: RoutineGuardrails {
            cooldown: Duration::from_secs(cooldown_secs),
            max_concurrent: 1,
            dedup_window: None,
        },
        notify: NotifyConfig::default(),
        last_run_at: None,
        next_fire_at: next_fire,
        run_count: 0,
        consecutive_failures: 0,
        state: serde_json::json!({}),
        created_at: now,
        updated_at: now,
    };

    db.create_routine(&routine)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| "failed to create routine")?;

    println!("Created routine {}", routine.id);
    println!("  name:   {}", routine.name);
    println!("  trigger: {}", routine.trigger.type_tag());
    if let Some(next) = routine.next_fire_at {
        println!("  next:   {}", next.to_rfc3339());
    }

    Ok(())
}

async fn remove_routine(
    db: &dyn crate::db::Database,
    name_or_id: &str,
    user_id: &str,
) -> anyhow::Result<()> {
    let routine = find_routine(db, name_or_id, user_id).await?;

    let deleted = db
        .delete_routine(routine.id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| "failed to delete routine")?;

    if deleted {
        println!("Deleted routine '{}' ({})", routine.name, routine.id);
    } else {
        println!("Routine '{}' not found", routine.name);
    }

    Ok(())
}

async fn run_routine(
    db: &dyn crate::db::Database,
    name_or_id: &str,
    user_id: &str,
) -> anyhow::Result<()> {
    let routine = find_routine(db, name_or_id, user_id).await?;

    // Create a routine run record
    let run = crate::agent::routine::RoutineRun {
        id: Uuid::new_v4(),
        routine_id: routine.id,
        trigger_type: "manual".to_string(),
        trigger_detail: Some("cli trigger".to_string()),
        started_at: Utc::now(),
        completed_at: None,
        status: crate::agent::routine::RunStatus::Running,
        result_summary: None,
        tokens_used: None,
        job_id: None,
        created_at: Utc::now(),
    };

    db.create_routine_run(&run)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| "failed to create routine run")?;

    println!("Triggered routine '{}' ({})", routine.name, routine.id);
    println!("  run_id: {}", run.id);
    println!();
    println!("Note: This creates a run record but does not execute the routine.");
    println!("The routine engine will pick it up for execution.");

    Ok(())
}

async fn status_routines(
    db: &dyn crate::db::Database,
    user_id: &str,
) -> anyhow::Result<()> {
    let routines = db
        .list_routines(user_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| "failed to list routines")?;

    let total = routines.len() as u64;
    let enabled = routines.iter().filter(|r| r.enabled).count() as u64;
    let disabled = total - enabled;
    let failing = routines
        .iter()
        .filter(|r| r.consecutive_failures > 0)
        .count() as u64;

    let today_start = Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|dt| dt.and_utc());
    let runs_today = if let Some(start) = today_start {
        routines
            .iter()
            .filter(|r| r.last_run_at.is_some_and(|ts| ts >= start))
            .count() as u64
    } else {
        0
    };

    println!("Routine Status");
    println!("===============");
    println!();
    println!("  User:        {}", user_id);
    println!("  Total:       {}", total);
    println!("  Enabled:     {}", enabled);
    println!("  Disabled:    {}", disabled);
    println!("  Failing:     {}", failing);
    println!("  Runs today:  {}", runs_today);

    if !routines.is_empty() {
        println!();
        println!("Next scheduled:");
        for r in routines.iter().filter(|r| r.enabled && r.next_fire_at.is_some()) {
            let next = r.next_fire_at.unwrap();
            println!(
                "  {}  {}  {}",
                next.to_rfc3339(),
                r.name,
                r.trigger.type_tag()
            );
        }
    }

    Ok(())
}

async fn find_routine(
    db: &dyn crate::db::Database,
    name_or_id: &str,
    user_id: &str,
) -> anyhow::Result<Routine> {
    // Try parsing as UUID first
    if let Ok(id) = Uuid::parse_str(name_or_id) {
        let routine = db
            .get_routine(id)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?
            .ok_or_else(|| anyhow::anyhow!("routine not found: {}", id))?;

        if routine.user_id != user_id {
            anyhow::bail!("routine {} not found for user {}", id, user_id);
        }
        return Ok(routine);
    }

    // Otherwise look up by name
    let routine = db
        .get_routine_by_name(user_id, name_or_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?
        .ok_or_else(|| anyhow::anyhow!("routine not found: {}", name_or_id))?;

    Ok(routine)
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