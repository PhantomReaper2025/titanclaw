//! Autonomy plan-step CLI commands.

use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use chrono::Utc;
use clap::Subcommand;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::agent::{Plan, PlanStep, PlanStepKind, PlanStepStatus};

const DEFAULT_USER_ID: &str = "default";

#[derive(Subcommand, Debug, Clone)]
pub enum PlanStepCommand {
    /// Create a plan step for an existing plan
    Create {
        /// Plan ID to attach the step to
        #[arg(long)]
        plan_id: Uuid,

        /// Owner user ID (used to validate goal ownership)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,

        /// Step sequence number within the plan
        #[arg(long)]
        sequence_num: i32,

        /// Step kind (tool_call|evidence_gather|verification|ask_user)
        #[arg(long)]
        kind: String,

        /// Step title
        #[arg(long)]
        title: String,

        /// Step description
        #[arg(long)]
        description: String,

        /// JSON for tool candidates (default: {})
        #[arg(long)]
        tool_candidates_json: Option<String>,

        /// JSON for inputs (default: {})
        #[arg(long)]
        inputs_json: Option<String>,

        /// JSON for preconditions (default: {})
        #[arg(long)]
        preconditions_json: Option<String>,

        /// JSON for postconditions (default: {})
        #[arg(long)]
        postconditions_json: Option<String>,

        /// JSON for policy requirements (default: {})
        #[arg(long)]
        policy_requirements_json: Option<String>,

        /// Optional JSON rollback payload
        #[arg(long)]
        rollback_json: Option<String>,
    },

    /// Replace all plan steps for a plan atomically (replan workflow)
    Replace {
        /// Plan ID to replace steps for
        #[arg(long)]
        plan_id: Uuid,

        /// Owner user ID (used to validate goal ownership)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,

        /// JSON file containing either {\"steps\": [...]} or [...] step objects
        #[arg(
            long,
            conflicts_with = "steps_json",
            required_unless_present = "steps_json"
        )]
        steps_file: Option<PathBuf>,

        /// Inline JSON containing either {\"steps\": [...]} or [...] step objects
        #[arg(
            long,
            conflicts_with = "steps_file",
            required_unless_present = "steps_file"
        )]
        steps_json: Option<String>,
    },

    /// List plan steps for a plan
    List {
        /// Plan ID
        #[arg(long)]
        plan_id: Uuid,

        /// Owner user ID (used to validate goal ownership)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Show one plan step as JSON
    Show {
        /// Plan step ID
        id: Uuid,

        /// Owner user ID (used to validate plan ownership)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },

    /// Update plan-step status
    SetStatus {
        /// Plan step ID
        id: Uuid,

        /// New status (pending|running|succeeded|failed|blocked|skipped)
        status: String,

        /// Owner user ID (used to validate plan ownership)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },
}

pub async fn run_plan_step_command(cmd: PlanStepCommand) -> anyhow::Result<()> {
    let db = connect_db().await?;

    match cmd {
        PlanStepCommand::Create {
            plan_id,
            user_id,
            sequence_num,
            kind,
            title,
            description,
            tool_candidates_json,
            inputs_json,
            preconditions_json,
            postconditions_json,
            policy_requirements_json,
            rollback_json,
        } => {
            create_plan_step(
                db.as_ref(),
                plan_id,
                &user_id,
                sequence_num,
                &kind,
                title,
                description,
                tool_candidates_json,
                inputs_json,
                preconditions_json,
                postconditions_json,
                policy_requirements_json,
                rollback_json,
            )
            .await
        }
        PlanStepCommand::Replace {
            plan_id,
            user_id,
            steps_file,
            steps_json,
        } => replace_plan_steps(db.as_ref(), plan_id, &user_id, steps_file, steps_json).await,
        PlanStepCommand::List { plan_id, user_id } => {
            list_plan_steps(db.as_ref(), plan_id, &user_id).await
        }
        PlanStepCommand::Show { id, user_id } => show_plan_step(db.as_ref(), id, &user_id).await,
        PlanStepCommand::SetStatus {
            id,
            status,
            user_id,
        } => set_plan_step_status(db.as_ref(), id, &status, &user_id).await,
    }
}

#[derive(Debug, Deserialize)]
struct ReplacePlanStepInput {
    sequence_num: i32,
    kind: String,
    title: String,
    description: String,
    #[serde(default)]
    tool_candidates: Value,
    #[serde(default)]
    inputs: Value,
    #[serde(default)]
    preconditions: Value,
    #[serde(default)]
    postconditions: Value,
    rollback: Option<Value>,
    #[serde(default)]
    policy_requirements: Value,
}

#[derive(Debug, Deserialize)]
struct ReplacePlanStepsWrapper {
    steps: Vec<ReplacePlanStepInput>,
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

async fn create_plan_step(
    db: &dyn crate::db::Database,
    plan_id: Uuid,
    user_id: &str,
    sequence_num: i32,
    kind_raw: &str,
    title: String,
    description: String,
    tool_candidates_json: Option<String>,
    inputs_json: Option<String>,
    preconditions_json: Option<String>,
    postconditions_json: Option<String>,
    policy_requirements_json: Option<String>,
    rollback_json: Option<String>,
) -> anyhow::Result<()> {
    if sequence_num <= 0 {
        anyhow::bail!("sequence_num must be a positive integer");
    }
    let title = title.trim();
    let description = description.trim();
    if title.is_empty() {
        anyhow::bail!("title is required");
    }
    if description.is_empty() {
        anyhow::bail!("description is required");
    }

    let plan = load_owned_plan(db, plan_id, user_id).await?;
    let kind = parse_plan_step_kind(kind_raw)?;

    let now = Utc::now();
    let step = PlanStep {
        id: Uuid::new_v4(),
        plan_id: plan.id,
        sequence_num,
        kind,
        status: PlanStepStatus::Pending,
        title: title.to_string(),
        description: description.to_string(),
        tool_candidates: parse_json_arg_or_default("tool_candidates_json", tool_candidates_json)?,
        inputs: parse_json_arg_or_default("inputs_json", inputs_json)?,
        preconditions: parse_json_arg_or_default("preconditions_json", preconditions_json)?,
        postconditions: parse_json_arg_or_default("postconditions_json", postconditions_json)?,
        rollback: parse_optional_json_arg("rollback_json", rollback_json)?,
        policy_requirements: parse_json_arg_or_default(
            "policy_requirements_json",
            policy_requirements_json,
        )?,
        started_at: None,
        completed_at: None,
        created_at: now,
        updated_at: now,
    };

    db.create_plan_steps(std::slice::from_ref(&step))
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to create plan step for plan {}", plan.id))?;

    println!("Created plan step {} for plan {}", step.id, step.plan_id);
    println!("{}", serde_json::to_string_pretty(&step)?);
    Ok(())
}

async fn replace_plan_steps(
    db: &dyn crate::db::Database,
    plan_id: Uuid,
    user_id: &str,
    steps_file: Option<PathBuf>,
    steps_json: Option<String>,
) -> anyhow::Result<()> {
    let plan = load_owned_plan(db, plan_id, user_id).await?;
    let raw = match (steps_file, steps_json) {
        (Some(path), None) => tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("failed to read steps file {}", path.display()))?,
        (None, Some(json)) => json,
        _ => anyhow::bail!("exactly one of --steps-file or --steps-json is required"),
    };

    let inputs = parse_replace_steps_payload(&raw)?;
    let mut seen = std::collections::HashSet::new();
    let now = Utc::now();
    let mut steps = Vec::with_capacity(inputs.len());

    for input in inputs {
        if input.sequence_num <= 0 {
            anyhow::bail!("sequence_num must be a positive integer");
        }
        if !seen.insert(input.sequence_num) {
            anyhow::bail!("duplicate sequence_num {}", input.sequence_num);
        }
        let title = input.title.trim();
        let description = input.description.trim();
        if title.is_empty() {
            anyhow::bail!("step title is required");
        }
        if description.is_empty() {
            anyhow::bail!("step description is required");
        }

        steps.push(PlanStep {
            id: Uuid::new_v4(),
            plan_id: plan.id,
            sequence_num: input.sequence_num,
            kind: parse_plan_step_kind(&input.kind)?,
            status: PlanStepStatus::Pending,
            title: title.to_string(),
            description: description.to_string(),
            tool_candidates: normalize_json_value(input.tool_candidates),
            inputs: normalize_json_value(input.inputs),
            preconditions: normalize_json_value(input.preconditions),
            postconditions: normalize_json_value(input.postconditions),
            rollback: input.rollback.filter(|v| !v.is_null()),
            policy_requirements: normalize_json_value(input.policy_requirements),
            started_at: None,
            completed_at: None,
            created_at: now,
            updated_at: now,
        });
    }

    db.replace_plan_steps_for_plan(plan.id, &steps)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to replace plan steps for plan {}", plan.id))?;

    println!(
        "Replaced plan steps for plan {} ({} step{})",
        plan.id,
        steps.len(),
        if steps.len() == 1 { "" } else { "s" }
    );
    println!("{}", serde_json::to_string_pretty(&steps)?);
    Ok(())
}

async fn list_plan_steps(
    db: &dyn crate::db::Database,
    plan_id: Uuid,
    user_id: &str,
) -> anyhow::Result<()> {
    let plan = load_owned_plan(db, plan_id, user_id).await?;

    let mut steps = db
        .list_plan_steps_for_plan(plan.id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to list plan steps for plan {}", plan.id))?;
    steps.sort_by_key(|s| (s.sequence_num, s.created_at, s.id));

    println!("Plan Steps (plan: {}, user: {})", plan.id, user_id);
    if steps.is_empty() {
        println!();
        println!("  (none)");
        return Ok(());
    }

    println!();
    for s in steps {
        let kind = serde_json::to_string(&s.kind)?;
        let status = serde_json::to_string(&s.status)?;
        println!(
            "  seq={:<3} {:<15} {:<10} {} {}",
            s.sequence_num,
            kind.trim_matches('"'),
            status.trim_matches('"'),
            s.id,
            truncate(&s.title, 48)
        );
    }

    Ok(())
}

async fn show_plan_step(
    db: &dyn crate::db::Database,
    step_id: Uuid,
    user_id: &str,
) -> anyhow::Result<()> {
    let step = db
        .get_plan_step(step_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load plan step {}", step_id))?
        .ok_or_else(|| anyhow::anyhow!("plan step not found: {}", step_id))?;

    let _plan = load_owned_plan(db, step.plan_id, user_id).await?;

    println!("{}", serde_json::to_string_pretty(&step)?);
    Ok(())
}

async fn set_plan_step_status(
    db: &dyn crate::db::Database,
    step_id: Uuid,
    status_raw: &str,
    user_id: &str,
) -> anyhow::Result<()> {
    let step = db
        .get_plan_step(step_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to load plan step {}", step_id))?
        .ok_or_else(|| anyhow::anyhow!("plan step not found: {}", step_id))?;

    let _plan = load_owned_plan(db, step.plan_id, user_id).await?;
    let status = parse_plan_step_status(status_raw)?;

    db.update_plan_step_status(step_id, status)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to update plan step {}", step_id))?;

    let updated = db
        .get_plan_step(step_id)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .with_context(|| format!("failed to reload plan step {}", step_id))?
        .ok_or_else(|| anyhow::anyhow!("plan step disappeared after update: {}", step_id))?;

    println!("Updated plan step {}", step_id);
    println!("{}", serde_json::to_string_pretty(&updated)?);
    Ok(())
}

async fn load_owned_plan(
    db: &dyn crate::db::Database,
    plan_id: Uuid,
    user_id: &str,
) -> anyhow::Result<Plan> {
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

    Ok(plan)
}

fn parse_replace_steps_payload(raw: &str) -> anyhow::Result<Vec<ReplacePlanStepInput>> {
    if let Ok(wrapper) = serde_json::from_str::<ReplacePlanStepsWrapper>(raw) {
        return Ok(wrapper.steps);
    }
    serde_json::from_str::<Vec<ReplacePlanStepInput>>(raw)
        .map_err(|e| anyhow::anyhow!("invalid replace payload JSON: {}", e))
}

fn normalize_json_value(value: Value) -> Value {
    if value.is_null() {
        serde_json::json!({})
    } else {
        value
    }
}

fn parse_json_arg_or_default(name: &str, raw: Option<String>) -> anyhow::Result<Value> {
    match raw {
        None => Ok(serde_json::json!({})),
        Some(raw) => {
            serde_json::from_str(&raw).map_err(|e| anyhow::anyhow!("invalid {} JSON: {}", name, e))
        }
    }
}

fn parse_optional_json_arg(name: &str, raw: Option<String>) -> anyhow::Result<Option<Value>> {
    match raw {
        None => Ok(None),
        Some(raw) => {
            let value: Value = serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("invalid {} JSON: {}", name, e))?;
            if value.is_null() {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        }
    }
}

fn parse_plan_step_kind(s: &str) -> anyhow::Result<PlanStepKind> {
    match s.trim().to_ascii_lowercase().as_str() {
        "tool_call" => Ok(PlanStepKind::ToolCall),
        "evidence_gather" => Ok(PlanStepKind::EvidenceGather),
        "verification" => Ok(PlanStepKind::Verification),
        "ask_user" => Ok(PlanStepKind::AskUser),
        _ => anyhow::bail!(
            "invalid plan-step kind '{}'; expected tool_call|evidence_gather|verification|ask_user",
            s
        ),
    }
}

fn parse_plan_step_status(s: &str) -> anyhow::Result<PlanStepStatus> {
    match s.trim().to_ascii_lowercase().as_str() {
        "pending" => Ok(PlanStepStatus::Pending),
        "running" => Ok(PlanStepStatus::Running),
        "succeeded" => Ok(PlanStepStatus::Succeeded),
        "failed" => Ok(PlanStepStatus::Failed),
        "blocked" => Ok(PlanStepStatus::Blocked),
        "skipped" => Ok(PlanStepStatus::Skipped),
        _ => anyhow::bail!(
            "invalid plan-step status '{}'; expected pending|running|succeeded|failed|blocked|skipped",
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_replace_steps_payload_accepts_wrapper() {
        let raw = r#"{
            "steps": [
                {
                    "sequence_num": 1,
                    "kind": "tool_call",
                    "title": "Run",
                    "description": "Execute",
                    "tool_candidates": null,
                    "inputs": {"cmd": "echo hi"},
                    "preconditions": null,
                    "postconditions": null,
                    "rollback": null,
                    "policy_requirements": null
                }
            ]
        }"#;

        let parsed = parse_replace_steps_payload(raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].sequence_num, 1);
        assert_eq!(parsed[0].kind, "tool_call");
        assert_eq!(parsed[0].inputs, json!({"cmd":"echo hi"}));
    }

    #[test]
    fn test_parse_replace_steps_payload_accepts_array() {
        let raw = r#"[
            {
                "sequence_num": 2,
                "kind": "verification",
                "title": "Verify",
                "description": "Check result"
            }
        ]"#;

        let parsed = parse_replace_steps_payload(raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].sequence_num, 2);
        assert_eq!(parsed[0].kind, "verification");
        assert_eq!(parsed[0].tool_candidates, Value::Null);
    }

    #[test]
    fn test_parse_replace_steps_payload_invalid() {
        let err = parse_replace_steps_payload("{not json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid replace payload JSON"));
    }

    #[test]
    fn test_json_helpers_normalize_and_parse() {
        assert_eq!(normalize_json_value(Value::Null), json!({}));
        assert_eq!(normalize_json_value(json!([1, 2])), json!([1, 2]));

        assert_eq!(parse_json_arg_or_default("x", None).unwrap(), json!({}));
        assert_eq!(
            parse_json_arg_or_default("x", Some("{\"a\":1}".to_string())).unwrap(),
            json!({"a":1})
        );
        let err = parse_json_arg_or_default("x", Some("{".to_string()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid x JSON"));

        assert_eq!(parse_optional_json_arg("y", None).unwrap(), None);
        assert_eq!(
            parse_optional_json_arg("y", Some("null".to_string())).unwrap(),
            None
        );
        assert_eq!(
            parse_optional_json_arg("y", Some("[1]".to_string())).unwrap(),
            Some(json!([1]))
        );
    }

    #[test]
    fn test_parse_plan_step_kind_and_status_normalize_case() {
        assert_eq!(
            parse_plan_step_kind("  TOOL_Call ").unwrap(),
            PlanStepKind::ToolCall
        );
        assert_eq!(
            parse_plan_step_kind("Evidence_Gather").unwrap(),
            PlanStepKind::EvidenceGather
        );
        assert_eq!(
            parse_plan_step_status(" Running ").unwrap(),
            PlanStepStatus::Running
        );
        assert_eq!(
            parse_plan_step_status("succeeded").unwrap(),
            PlanStepStatus::Succeeded
        );

        let kind_err = parse_plan_step_kind("bad_kind").unwrap_err().to_string();
        assert!(kind_err.contains("invalid plan-step kind"));
        let status_err = parse_plan_step_status("bad_status")
            .unwrap_err()
            .to_string();
        assert!(status_err.contains("invalid plan-step status"));
    }

    #[test]
    fn test_truncate_behavior() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exact", 5), "exact");
        assert_eq!(truncate("abcdefghij", 8), "abcde...");
    }
}
