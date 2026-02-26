//! Memory Plane v2 CLI commands.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Subcommand;
use serde_json::json;
use uuid::Uuid;

use crate::agent::memory_consolidator::{MemoryConsolidator, MemoryConsolidatorConfig};
use crate::agent::memory_retrieval_composer::{
    MemoryRetrievalComposer, MemoryRetrievalRequest, RetrievalTaskClass,
};
use crate::agent::{
    ConsolidationRun, ConsolidationRunStatus, MemoryRecord, MemoryRecordStatus, MemoryType,
    ProceduralPlaybook, ProceduralPlaybookStatus,
};

const DEFAULT_USER_ID: &str = "default";

#[derive(Subcommand, Debug, Clone)]
pub enum MemoryPlaneCommand {
    /// Inspect memory-plane records
    #[command(subcommand)]
    Records(MemoryPlaneRecordsCommand),
    /// Inspect and manage procedural playbooks
    #[command(subcommand)]
    Playbooks(MemoryPlanePlaybooksCommand),
    /// Run or inspect consolidation
    #[command(subcommand)]
    Consolidation(MemoryPlaneConsolidationCommand),
    /// Preview retrieval-composer output for a task
    #[command(subcommand)]
    Retrieval(MemoryPlaneRetrievalCommand),
}

#[derive(Subcommand, Debug, Clone)]
pub enum MemoryPlaneRecordsCommand {
    /// List memory records (user-scoped)
    List {
        /// Owner user ID
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
        /// Optional memory type filter (working|episodic|semantic|procedural|profile)
        #[arg(long = "type")]
        memory_type: Option<String>,
        /// Optional record status filter (active|demoted|expired|archived|rejected)
        #[arg(long)]
        status: Option<String>,
        /// Optional goal filter
        #[arg(long)]
        goal_id: Option<Uuid>,
        /// Optional plan filter
        #[arg(long)]
        plan_id: Option<Uuid>,
        /// Optional exact category filter
        #[arg(long)]
        category: Option<String>,
        /// Sort order (created_desc|created_asc|updated_desc|updated_asc|confidence_desc)
        #[arg(long)]
        sort: Option<String>,
        /// Number of rows to skip after sorting
        #[arg(long)]
        offset: Option<usize>,
        /// Max number of rows to print
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show a memory record and its events
    Show {
        /// Memory record ID
        id: Uuid,
        /// Owner user ID
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum MemoryPlanePlaybooksCommand {
    /// List procedural playbooks (user-scoped)
    List {
        /// Owner user ID
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
        /// Optional task-class filter (e.g. routine_automation)
        #[arg(long)]
        task_class: Option<String>,
        /// Optional playbook status filter (draft|active|paused|retired)
        #[arg(long)]
        status: Option<String>,
        /// Sort order (updated_desc|updated_asc|confidence_desc|success_desc)
        #[arg(long)]
        sort: Option<String>,
        /// Number of rows to skip after sorting
        #[arg(long)]
        offset: Option<usize>,
        /// Max number of rows to print
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show a procedural playbook
    Show {
        /// Playbook ID
        id: Uuid,
        /// Owner user ID
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },
    /// Update procedural playbook status
    SetStatus {
        /// Playbook ID
        id: Uuid,
        /// New status (draft|active|paused|retired)
        status: String,
        /// Owner user ID
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum MemoryPlaneConsolidationCommand {
    /// List consolidation runs
    Runs {
        /// Owner user ID (filters owned runs plus global runs)
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
        /// Optional run status filter (running|completed|failed|partial)
        #[arg(long)]
        status: Option<String>,
        /// Sort order (started_desc|started_asc)
        #[arg(long)]
        sort: Option<String>,
        /// Number of rows to skip after sorting
        #[arg(long)]
        offset: Option<usize>,
        /// Max number of rows to print
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Trigger a one-shot consolidation run immediately
    Run {
        /// Batch size override
        #[arg(long, default_value_t = 50)]
        batch_size: u32,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum MemoryPlaneRetrievalCommand {
    /// Preview memory retrieval composer output
    Preview {
        /// Owner user ID
        #[arg(long, default_value = DEFAULT_USER_ID)]
        user_id: String,
        /// Task class (coding|troubleshooting|research|routine_automation|system_admin|general)
        #[arg(long)]
        task_class: Option<String>,
        /// Optional linked goal
        #[arg(long)]
        goal_id: Option<Uuid>,
        /// Optional linked plan
        #[arg(long)]
        plan_id: Option<Uuid>,
        /// Optional linked job
        #[arg(long)]
        job_id: Option<Uuid>,
        /// Retrieval query
        #[arg(long)]
        query: Option<String>,
        /// Retrieval limit budget
        #[arg(long, default_value_t = 12)]
        limit_budget: usize,
    },
}

pub async fn run_memory_plane_command(cmd: MemoryPlaneCommand) -> anyhow::Result<()> {
    let db = connect_db().await?;
    match cmd {
        MemoryPlaneCommand::Records(cmd) => run_records_command(db.as_ref(), cmd).await,
        MemoryPlaneCommand::Playbooks(cmd) => run_playbooks_command(db.as_ref(), cmd).await,
        MemoryPlaneCommand::Consolidation(cmd) => run_consolidation_command(db.clone(), cmd).await,
        MemoryPlaneCommand::Retrieval(cmd) => run_retrieval_command(db.clone(), cmd).await,
    }
}

async fn run_records_command(
    db: &dyn crate::db::Database,
    cmd: MemoryPlaneRecordsCommand,
) -> anyhow::Result<()> {
    match cmd {
        MemoryPlaneRecordsCommand::List {
            user_id,
            memory_type,
            status,
            goal_id,
            plan_id,
            category,
            sort,
            offset,
            limit,
        } => {
            let memory_type = memory_type.as_deref().map(parse_memory_type).transpose()?;
            let status = status
                .as_deref()
                .map(parse_memory_record_status)
                .transpose()?;
            let sort = parse_memory_record_list_sort(sort.as_deref())?;
            if let Some(limit) = limit
                && limit == 0
            {
                anyhow::bail!("limit must be >= 1");
            }

            let mut records = if let Some(goal_id) = goal_id {
                db.list_memory_records_for_goal(goal_id)
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e))
                    .with_context(|| {
                        format!("failed to list memory records for goal {}", goal_id)
                    })?
            } else {
                db.list_memory_records_for_user(&user_id)
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e))
                    .with_context(|| {
                        format!("failed to list memory records for user {}", user_id)
                    })?
            };

            records.retain(|r| r.owner_user_id == user_id);
            if let Some(memory_type) = memory_type {
                records.retain(|r| r.memory_type == memory_type);
            }
            if let Some(status) = status {
                records.retain(|r| r.status == status);
            }
            if let Some(goal_id) = goal_id {
                records.retain(|r| r.goal_id == Some(goal_id));
            }
            if let Some(plan_id) = plan_id {
                records.retain(|r| r.plan_id == Some(plan_id));
            }
            if let Some(category) = category.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                let category = category.to_ascii_lowercase();
                records.retain(|r| r.category.to_ascii_lowercase() == category);
            }
            apply_memory_record_list_sort(&mut records, sort);
            if let Some(offset) = offset
                && offset > 0
            {
                records = records.into_iter().skip(offset).collect();
            }
            if let Some(limit) = limit {
                records.truncate(limit);
            }

            println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "memory_records": records }))?
            );
            Ok(())
        }
        MemoryPlaneRecordsCommand::Show { id, user_id } => {
            let record = db
                .get_memory_record(id)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .with_context(|| format!("failed to load memory record {}", id))?
                .ok_or_else(|| anyhow::anyhow!("memory record not found: {}", id))?;
            if record.owner_user_id != user_id {
                anyhow::bail!("memory record {} not found for user {}", id, user_id);
            }
            let events = db
                .list_memory_events_for_record(id)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .with_context(|| format!("failed to list memory events for {}", id))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "memory_record": record,
                    "memory_events": events
                }))?
            );
            Ok(())
        }
    }
}

async fn run_playbooks_command(
    db: &dyn crate::db::Database,
    cmd: MemoryPlanePlaybooksCommand,
) -> anyhow::Result<()> {
    match cmd {
        MemoryPlanePlaybooksCommand::List {
            user_id,
            task_class,
            status,
            sort,
            offset,
            limit,
        } => {
            if let Some(limit) = limit
                && limit == 0
            {
                anyhow::bail!("limit must be >= 1");
            }
            let status = status.as_deref().map(parse_playbook_status).transpose()?;
            let sort = parse_playbook_list_sort(sort.as_deref())?;
            let task_class = task_class
                .as_deref()
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty());

            let mut playbooks = db
                .list_procedural_playbooks_for_user(&user_id)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .with_context(|| format!("failed to list playbooks for user {}", user_id))?;

            if let Some(status) = status {
                playbooks.retain(|p| p.status == status);
            }
            if let Some(task_class) = task_class.as_deref() {
                playbooks.retain(|p| p.task_class.to_ascii_lowercase() == task_class);
            }
            apply_playbook_list_sort(&mut playbooks, sort);
            if let Some(offset) = offset
                && offset > 0
            {
                playbooks = playbooks.into_iter().skip(offset).collect();
            }
            if let Some(limit) = limit {
                playbooks.truncate(limit);
            }

            println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "playbooks": playbooks }))?
            );
            Ok(())
        }
        MemoryPlanePlaybooksCommand::Show { id, user_id } => {
            let playbook = db
                .get_procedural_playbook(id)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .with_context(|| format!("failed to load playbook {}", id))?
                .ok_or_else(|| anyhow::anyhow!("playbook not found: {}", id))?;
            if playbook.owner_user_id != user_id {
                anyhow::bail!("playbook {} not found for user {}", id, user_id);
            }
            println!("{}", serde_json::to_string_pretty(&playbook)?);
            Ok(())
        }
        MemoryPlanePlaybooksCommand::SetStatus {
            id,
            status,
            user_id,
        } => {
            let playbook = db
                .get_procedural_playbook(id)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .with_context(|| format!("failed to load playbook {}", id))?
                .ok_or_else(|| anyhow::anyhow!("playbook not found: {}", id))?;
            if playbook.owner_user_id != user_id {
                anyhow::bail!("playbook {} not found for user {}", id, user_id);
            }
            let status = parse_playbook_status(&status)?;
            db.update_procedural_playbook_status(id, status)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .with_context(|| format!("failed to update playbook {}", id))?;
            let updated = db
                .get_procedural_playbook(id)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .with_context(|| format!("failed to reload playbook {}", id))?
                .ok_or_else(|| anyhow::anyhow!("playbook disappeared after update: {}", id))?;
            println!("Updated playbook {}", id);
            println!("{}", serde_json::to_string_pretty(&updated)?);
            Ok(())
        }
    }
}

async fn run_consolidation_command(
    db: Arc<dyn crate::db::Database>,
    cmd: MemoryPlaneConsolidationCommand,
) -> anyhow::Result<()> {
    match cmd {
        MemoryPlaneConsolidationCommand::Runs {
            user_id,
            status,
            sort,
            offset,
            limit,
        } => {
            if let Some(limit) = limit
                && limit == 0
            {
                anyhow::bail!("limit must be >= 1");
            }
            let status = status
                .as_deref()
                .map(parse_consolidation_run_status)
                .transpose()?;
            let sort = parse_consolidation_run_list_sort(sort.as_deref())?;
            let mut runs = db
                .list_consolidation_runs_for_user(None)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .context("failed to list consolidation runs")?;
            runs.retain(|r| r.owner_user_id.as_deref().is_none_or(|u| u == user_id));
            if let Some(status) = status {
                runs.retain(|r| r.status == status);
            }
            apply_consolidation_run_list_sort(&mut runs, sort);
            if let Some(offset) = offset
                && offset > 0
            {
                runs = runs.into_iter().skip(offset).collect();
            }
            if let Some(limit) = limit {
                runs.truncate(limit);
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "consolidation_runs": runs }))?
            );
            Ok(())
        }
        MemoryPlaneConsolidationCommand::Run { batch_size } => {
            let consolidator = MemoryConsolidator::new(
                MemoryConsolidatorConfig {
                    enabled: true,
                    interval: Duration::from_secs(60),
                    batch_size: batch_size.clamp(1, 500),
                },
                db,
            );
            let run = consolidator
                .run_once()
                .await
                .map_err(|e| anyhow::anyhow!(e))
                .context("failed to run memory consolidation")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "triggered": true,
                    "consolidation_run": run
                }))?
            );
            Ok(())
        }
    }
}

async fn run_retrieval_command(
    db: Arc<dyn crate::db::Database>,
    cmd: MemoryPlaneRetrievalCommand,
) -> anyhow::Result<()> {
    match cmd {
        MemoryPlaneRetrievalCommand::Preview {
            user_id,
            task_class,
            goal_id,
            plan_id,
            job_id,
            query,
            limit_budget,
        } => {
            if limit_budget == 0 {
                anyhow::bail!("limit_budget must be >= 1");
            }
            let task_class = parse_retrieval_task_class(task_class.as_deref())?;
            let query = query
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("memory retrieval preview")
                .to_string();
            let bundle = MemoryRetrievalComposer::compose(
                db,
                MemoryRetrievalRequest {
                    user_id,
                    goal_id,
                    plan_id,
                    job_id,
                    task_class,
                    query,
                    limit_budget,
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))
            .context("failed to compose memory retrieval bundle")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "working_context": bundle.working_context,
                    "episodic_hits": bundle.episodic_hits,
                    "semantic_hits": bundle.semantic_hits,
                    "procedural_playbooks": bundle.procedural_playbooks,
                    "prompt_context_block": bundle.prompt_context_block,
                    "debug_meta": bundle.debug_meta,
                }))?
            );
            Ok(())
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

fn parse_memory_type(s: &str) -> anyhow::Result<MemoryType> {
    match s.trim().to_ascii_lowercase().as_str() {
        "working" => Ok(MemoryType::Working),
        "episodic" => Ok(MemoryType::Episodic),
        "semantic" => Ok(MemoryType::Semantic),
        "procedural" => Ok(MemoryType::Procedural),
        "profile" => Ok(MemoryType::Profile),
        _ => anyhow::bail!(
            "invalid memory type '{}'; expected working|episodic|semantic|procedural|profile",
            s
        ),
    }
}

fn parse_memory_record_status(s: &str) -> anyhow::Result<MemoryRecordStatus> {
    match s.trim().to_ascii_lowercase().as_str() {
        "active" => Ok(MemoryRecordStatus::Active),
        "demoted" => Ok(MemoryRecordStatus::Demoted),
        "expired" => Ok(MemoryRecordStatus::Expired),
        "archived" => Ok(MemoryRecordStatus::Archived),
        "rejected" => Ok(MemoryRecordStatus::Rejected),
        _ => anyhow::bail!(
            "invalid memory record status '{}'; expected active|demoted|expired|archived|rejected",
            s
        ),
    }
}

fn parse_playbook_status(s: &str) -> anyhow::Result<ProceduralPlaybookStatus> {
    match s.trim().to_ascii_lowercase().as_str() {
        "draft" => Ok(ProceduralPlaybookStatus::Draft),
        "active" => Ok(ProceduralPlaybookStatus::Active),
        "paused" => Ok(ProceduralPlaybookStatus::Paused),
        "retired" => Ok(ProceduralPlaybookStatus::Retired),
        _ => anyhow::bail!(
            "invalid playbook status '{}'; expected draft|active|paused|retired",
            s
        ),
    }
}

fn parse_consolidation_run_status(s: &str) -> anyhow::Result<ConsolidationRunStatus> {
    match s.trim().to_ascii_lowercase().as_str() {
        "running" => Ok(ConsolidationRunStatus::Running),
        "completed" => Ok(ConsolidationRunStatus::Completed),
        "failed" => Ok(ConsolidationRunStatus::Failed),
        "partial" => Ok(ConsolidationRunStatus::Partial),
        _ => anyhow::bail!(
            "invalid consolidation run status '{}'; expected running|completed|failed|partial",
            s
        ),
    }
}

#[derive(Debug, Clone, Copy)]
enum MemoryRecordListSort {
    CreatedDesc,
    CreatedAsc,
    UpdatedDesc,
    UpdatedAsc,
    ConfidenceDesc,
}

#[derive(Debug, Clone, Copy)]
enum PlaybookListSort {
    UpdatedDesc,
    UpdatedAsc,
    ConfidenceDesc,
    SuccessDesc,
}

#[derive(Debug, Clone, Copy)]
enum ConsolidationRunListSort {
    StartedDesc,
    StartedAsc,
}

fn parse_memory_record_list_sort(raw: Option<&str>) -> anyhow::Result<MemoryRecordListSort> {
    match raw
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
        .unwrap_or("created_desc")
    {
        "created_desc" => Ok(MemoryRecordListSort::CreatedDesc),
        "created_asc" => Ok(MemoryRecordListSort::CreatedAsc),
        "updated_desc" => Ok(MemoryRecordListSort::UpdatedDesc),
        "updated_asc" => Ok(MemoryRecordListSort::UpdatedAsc),
        "confidence_desc" => Ok(MemoryRecordListSort::ConfidenceDesc),
        other => anyhow::bail!(
            "invalid memory record sort '{}'; expected created_desc|created_asc|updated_desc|updated_asc|confidence_desc",
            other
        ),
    }
}

fn apply_memory_record_list_sort(records: &mut [MemoryRecord], sort: MemoryRecordListSort) {
    match sort {
        MemoryRecordListSort::CreatedDesc => records.sort_by_key(|r| {
            (
                std::cmp::Reverse(r.created_at),
                std::cmp::Reverse(r.updated_at),
                std::cmp::Reverse(r.id),
            )
        }),
        MemoryRecordListSort::CreatedAsc => {
            records.sort_by_key(|r| (r.created_at, r.updated_at, r.id))
        }
        MemoryRecordListSort::UpdatedDesc => records.sort_by_key(|r| {
            (
                std::cmp::Reverse(r.updated_at),
                std::cmp::Reverse(r.created_at),
                std::cmp::Reverse(r.id),
            )
        }),
        MemoryRecordListSort::UpdatedAsc => {
            records.sort_by_key(|r| (r.updated_at, r.created_at, r.id))
        }
        MemoryRecordListSort::ConfidenceDesc => records.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| b.id.cmp(&a.id))
        }),
    }
}

fn parse_playbook_list_sort(raw: Option<&str>) -> anyhow::Result<PlaybookListSort> {
    match raw
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
        .unwrap_or("updated_desc")
    {
        "updated_desc" => Ok(PlaybookListSort::UpdatedDesc),
        "updated_asc" => Ok(PlaybookListSort::UpdatedAsc),
        "confidence_desc" => Ok(PlaybookListSort::ConfidenceDesc),
        "success_desc" => Ok(PlaybookListSort::SuccessDesc),
        other => anyhow::bail!(
            "invalid playbook sort '{}'; expected updated_desc|updated_asc|confidence_desc|success_desc",
            other
        ),
    }
}

fn apply_playbook_list_sort(playbooks: &mut [ProceduralPlaybook], sort: PlaybookListSort) {
    match sort {
        PlaybookListSort::UpdatedDesc => playbooks.sort_by_key(|p| {
            (
                std::cmp::Reverse(p.updated_at),
                std::cmp::Reverse(p.created_at),
                std::cmp::Reverse(p.id),
            )
        }),
        PlaybookListSort::UpdatedAsc => {
            playbooks.sort_by_key(|p| (p.updated_at, p.created_at, p.id))
        }
        PlaybookListSort::ConfidenceDesc => playbooks.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| b.id.cmp(&a.id))
        }),
        PlaybookListSort::SuccessDesc => playbooks.sort_by_key(|p| {
            (
                std::cmp::Reverse(p.success_count),
                std::cmp::Reverse(p.updated_at),
                std::cmp::Reverse(p.id),
            )
        }),
    }
}

fn parse_consolidation_run_list_sort(
    raw: Option<&str>,
) -> anyhow::Result<ConsolidationRunListSort> {
    match raw
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
        .unwrap_or("started_desc")
    {
        "started_desc" => Ok(ConsolidationRunListSort::StartedDesc),
        "started_asc" => Ok(ConsolidationRunListSort::StartedAsc),
        other => anyhow::bail!(
            "invalid consolidation run sort '{}'; expected started_desc|started_asc",
            other
        ),
    }
}

fn apply_consolidation_run_list_sort(
    runs: &mut [ConsolidationRun],
    sort: ConsolidationRunListSort,
) {
    match sort {
        ConsolidationRunListSort::StartedDesc => {
            runs.sort_by_key(|r| (std::cmp::Reverse(r.started_at), std::cmp::Reverse(r.id)))
        }
        ConsolidationRunListSort::StartedAsc => runs.sort_by_key(|r| (r.started_at, r.id)),
    }
}

fn parse_retrieval_task_class(raw: Option<&str>) -> anyhow::Result<RetrievalTaskClass> {
    match raw
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
        .unwrap_or("general")
    {
        "coding" => Ok(RetrievalTaskClass::Coding),
        "troubleshooting" => Ok(RetrievalTaskClass::Troubleshooting),
        "research" => Ok(RetrievalTaskClass::Research),
        "routine_automation" | "routine" => Ok(RetrievalTaskClass::RoutineAutomation),
        "system_admin" | "admin" => Ok(RetrievalTaskClass::SystemAdmin),
        "general" => Ok(RetrievalTaskClass::General),
        other => anyhow::bail!(
            "invalid retrieval task_class '{}'; expected coding|troubleshooting|research|routine_automation|system_admin|general",
            other
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(clap::Parser)]
    struct TestMemoryPlaneCli {
        #[command(subcommand)]
        cmd: MemoryPlaneCommand,
    }

    #[test]
    fn test_parse_memory_plane_records_list_subcommand() {
        let cli = TestMemoryPlaneCli::try_parse_from([
            "titanclaw",
            "records",
            "list",
            "--user-id",
            "alice",
            "--type",
            "episodic",
            "--status",
            "active",
            "--sort",
            "created_desc",
            "--offset",
            "2",
            "--limit",
            "5",
        ])
        .expect("parse memory-plane records list");

        match cli.cmd {
            MemoryPlaneCommand::Records(MemoryPlaneRecordsCommand::List {
                user_id,
                memory_type,
                status,
                sort,
                offset,
                limit,
                ..
            }) => {
                assert_eq!(user_id, "alice");
                assert_eq!(memory_type.as_deref(), Some("episodic"));
                assert_eq!(status.as_deref(), Some("active"));
                assert_eq!(sort.as_deref(), Some("created_desc"));
                assert_eq!(offset, Some(2));
                assert_eq!(limit, Some(5));
            }
            other => panic!("unexpected command parse result: {other:?}"),
        }
    }

    #[test]
    fn test_parse_memory_plane_nested_commands() {
        let playbook = TestMemoryPlaneCli::try_parse_from([
            "titanclaw",
            "playbooks",
            "set-status",
            "00000000-0000-0000-0000-000000000001",
            "paused",
        ])
        .expect("parse playbook set-status");
        assert!(matches!(
            playbook.cmd,
            MemoryPlaneCommand::Playbooks(MemoryPlanePlaybooksCommand::SetStatus { .. })
        ));

        let retrieval = TestMemoryPlaneCli::try_parse_from([
            "titanclaw",
            "retrieval",
            "preview",
            "--task-class",
            "routine_automation",
            "--limit-budget",
            "8",
        ])
        .expect("parse retrieval preview");
        assert!(matches!(
            retrieval.cmd,
            MemoryPlaneCommand::Retrieval(MemoryPlaneRetrievalCommand::Preview { .. })
        ));
    }

    #[test]
    fn test_memory_plane_parse_helpers_reject_invalid_values() {
        assert!(parse_memory_record_list_sort(Some("bad")).is_err());
        assert!(parse_playbook_list_sort(Some("bad")).is_err());
        assert!(parse_consolidation_run_list_sort(Some("bad")).is_err());
        assert!(parse_retrieval_task_class(Some("bad")).is_err());
        assert!(parse_memory_type("bad").is_err());
        assert!(parse_memory_record_status("bad").is_err());
        assert!(parse_playbook_status("bad").is_err());
        assert!(parse_consolidation_run_status("bad").is_err());
    }
}
