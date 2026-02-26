use std::cmp::Reverse;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::agent::{
    MemoryRecord, MemoryRecordStatus, MemoryType, ProceduralPlaybook, ProceduralPlaybookStatus,
};
use crate::db::Database;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetrievalTaskClass {
    Coding,
    Troubleshooting,
    Research,
    RoutineAutomation,
    SystemAdmin,
    General,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryRetrievalRequest {
    pub user_id: String,
    pub goal_id: Option<Uuid>,
    pub plan_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub task_class: RetrievalTaskClass,
    pub query: String,
    pub limit_budget: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryRetrievalBundle {
    pub working_context: Vec<MemoryRecord>,
    pub episodic_hits: Vec<MemoryRecord>,
    pub semantic_hits: Vec<MemoryRecord>,
    pub procedural_playbooks: Vec<ProceduralPlaybook>,
    pub prompt_context_block: String,
    #[allow(dead_code)]
    pub debug_meta: serde_json::Value,
}

pub(crate) struct MemoryRetrievalComposer;

impl MemoryRetrievalComposer {
    pub(crate) fn infer_task_class(
        category: Option<&str>,
        title: &str,
        description: &str,
    ) -> RetrievalTaskClass {
        let cat = category.unwrap_or_default().to_ascii_lowercase();
        let text = format!("{title}\n{description}").to_ascii_lowercase();

        if cat.contains("routine")
            || text.contains("cron")
            || text.contains("routine")
            || text.contains("scheduled")
        {
            return RetrievalTaskClass::RoutineAutomation;
        }
        if cat.contains("debug")
            || cat.contains("incident")
            || text.contains("troubleshoot")
            || text.contains("error")
            || text.contains("failed")
        {
            return RetrievalTaskClass::Troubleshooting;
        }
        if cat.contains("research")
            || text.contains("summarize")
            || text.contains("research")
            || text.contains("investigate")
        {
            return RetrievalTaskClass::Research;
        }
        if cat.contains("ops")
            || cat.contains("admin")
            || text.contains("deploy")
            || text.contains("service")
            || text.contains("systemd")
        {
            return RetrievalTaskClass::SystemAdmin;
        }
        if cat.contains("code")
            || cat.contains("dev")
            || text.contains("rust")
            || text.contains("compile")
            || text.contains("test")
            || text.contains("code")
            || text.contains("refactor")
        {
            return RetrievalTaskClass::Coding;
        }
        RetrievalTaskClass::General
    }

    pub(crate) async fn compose(
        store: Arc<dyn Database>,
        req: MemoryRetrievalRequest,
    ) -> Result<MemoryRetrievalBundle, String> {
        let mut records = if let Some(goal_id) = req.goal_id {
            store
                .list_memory_records_for_goal(goal_id)
                .await
                .map_err(|e| format!("list memory records for goal: {e}"))?
        } else {
            store
                .list_memory_records_for_user(&req.user_id)
                .await
                .map_err(|e| format!("list memory records for user: {e}"))?
        };
        records.retain(|r| {
            r.owner_user_id == req.user_id
                && r.status == MemoryRecordStatus::Active
                && matches!(
                    r.memory_type,
                    MemoryType::Working | MemoryType::Episodic | MemoryType::Semantic
                )
        });
        records.sort_by_key(|r| Reverse(r.created_at));

        let mut playbooks = store
            .list_procedural_playbooks_for_user(&req.user_id)
            .await
            .map_err(|e| format!("list procedural playbooks: {e}"))?;
        playbooks.retain(|p| {
            matches!(
                p.status,
                ProceduralPlaybookStatus::Draft | ProceduralPlaybookStatus::Active
            )
        });

        let limits = Limits::for_task(req.task_class, req.limit_budget);

        let working_context =
            select_records(&records, &req, MemoryType::Working, limits.working, |r| {
                score_record(r, &req, MemoryType::Working)
            });
        let episodic_hits =
            select_records(&records, &req, MemoryType::Episodic, limits.episodic, |r| {
                score_record(r, &req, MemoryType::Episodic)
            });
        let semantic_hits =
            select_records(&records, &req, MemoryType::Semantic, limits.semantic, |r| {
                score_record(r, &req, MemoryType::Semantic)
            });

        playbooks.sort_by(|a, b| {
            score_playbook(b, &req)
                .cmp(&score_playbook(a, &req))
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        let procedural_playbooks = playbooks
            .into_iter()
            .take(limits.playbooks)
            .collect::<Vec<_>>();

        let prompt_context_block = render_prompt_context_block(
            &req,
            &working_context,
            &episodic_hits,
            &semantic_hits,
            &procedural_playbooks,
        );

        Ok(MemoryRetrievalBundle {
            working_context,
            episodic_hits,
            semantic_hits,
            procedural_playbooks,
            prompt_context_block,
            debug_meta: json!({
                "task_class": format!("{:?}", req.task_class).to_ascii_lowercase(),
                "goal_id": req.goal_id,
                "plan_id": req.plan_id,
                "job_id": req.job_id,
                "query": truncate_chars(&req.query, 160),
                "limit_budget": req.limit_budget,
            }),
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct Limits {
    working: usize,
    episodic: usize,
    semantic: usize,
    playbooks: usize,
}

impl Limits {
    fn for_task(task_class: RetrievalTaskClass, limit_budget: usize) -> Self {
        let budget = limit_budget.clamp(4, 20);
        let base = match task_class {
            RetrievalTaskClass::Coding => Self {
                working: 2,
                episodic: 4,
                semantic: 4,
                playbooks: 2,
            },
            RetrievalTaskClass::Troubleshooting => Self {
                working: 2,
                episodic: 5,
                semantic: 2,
                playbooks: 3,
            },
            RetrievalTaskClass::RoutineAutomation => Self {
                working: 2,
                episodic: 4,
                semantic: 2,
                playbooks: 4,
            },
            RetrievalTaskClass::Research => Self {
                working: 1,
                episodic: 2,
                semantic: 5,
                playbooks: 1,
            },
            RetrievalTaskClass::SystemAdmin => Self {
                working: 2,
                episodic: 4,
                semantic: 3,
                playbooks: 3,
            },
            RetrievalTaskClass::General => Self {
                working: 2,
                episodic: 3,
                semantic: 3,
                playbooks: 2,
            },
        };
        let total = base.working + base.episodic + base.semantic + base.playbooks;
        if total <= budget {
            return base;
        }
        let scale = budget as f32 / total as f32;
        Self {
            working: ((base.working as f32 * scale).round() as usize).max(1),
            episodic: ((base.episodic as f32 * scale).round() as usize).max(1),
            semantic: ((base.semantic as f32 * scale).round() as usize).max(1),
            playbooks: ((base.playbooks as f32 * scale).round() as usize).max(1),
        }
    }
}

fn select_records<F>(
    records: &[MemoryRecord],
    req: &MemoryRetrievalRequest,
    ty: MemoryType,
    limit: usize,
    score_fn: F,
) -> Vec<MemoryRecord>
where
    F: Fn(&MemoryRecord) -> i64,
{
    if limit == 0 {
        return Vec::new();
    }
    let mut typed = records
        .iter()
        .filter(|r| r.memory_type == ty)
        .cloned()
        .collect::<Vec<_>>();
    typed.sort_by(|a, b| {
        score_fn(b)
            .cmp(&score_fn(a))
            .then_with(|| b.created_at.cmp(&a.created_at))
    });
    typed.truncate(limit);

    // Keep deterministic recent ordering within the selected slice.
    let mut ordered = typed;
    ordered.sort_by_key(|r| Reverse(r.created_at));

    // Light goal/plan tie-break: if nothing selected and a specific goal was requested, allow any matching goal items.
    if ordered.is_empty() && req.goal_id.is_some() {
        return records
            .iter()
            .filter(|r| r.memory_type == ty)
            .take(limit)
            .cloned()
            .collect();
    }
    ordered
}

fn score_record(
    record: &MemoryRecord,
    req: &MemoryRetrievalRequest,
    memory_type: MemoryType,
) -> i64 {
    let mut score = 0_i64;
    if req.goal_id.is_some() && record.goal_id == req.goal_id {
        score += 20;
    }
    if req.plan_id.is_some() && record.plan_id == req.plan_id {
        score += 20;
    }
    if req.job_id.is_some() && record.job_id == req.job_id {
        score += 10;
    }
    score += (record.confidence * 10.0) as i64;

    let text = format!(
        "{} {} {}",
        record.category.to_ascii_lowercase(),
        record.title.to_ascii_lowercase(),
        record.summary.to_ascii_lowercase()
    );

    match req.task_class {
        RetrievalTaskClass::RoutineAutomation => {
            if text.contains("routine") {
                score += 25;
            }
            if memory_type == MemoryType::Episodic && record.category == "routine_run_summary" {
                score += 15;
            }
        }
        RetrievalTaskClass::Troubleshooting => {
            if text.contains("fail") || text.contains("error") || text.contains("deny") {
                score += 25;
            }
            if memory_type == MemoryType::Episodic {
                score += 10;
            }
        }
        RetrievalTaskClass::Coding => {
            if text.contains("verifier")
                || text.contains("step")
                || text.contains("lint")
                || text.contains("test")
                || text.contains("diff")
                || text.contains("replan")
            {
                score += 20;
            }
            if memory_type == MemoryType::Semantic {
                score += 10;
            }
        }
        RetrievalTaskClass::Research => {
            if memory_type == MemoryType::Semantic {
                score += 20;
            }
        }
        RetrievalTaskClass::SystemAdmin => {
            if text.contains("deploy")
                || text.contains("service")
                || text.contains("timeout")
                || text.contains("system")
            {
                score += 20;
            }
        }
        RetrievalTaskClass::General => {}
    }

    score
}

fn score_playbook(playbook: &ProceduralPlaybook, req: &MemoryRetrievalRequest) -> i64 {
    let mut score = 0_i64;
    score += (playbook.success_count as i64) * 5;
    score += (playbook.confidence * 10.0) as i64;
    if matches!(playbook.status, ProceduralPlaybookStatus::Active) {
        score += 10;
    }

    let task = playbook.task_class.to_ascii_lowercase();
    match req.task_class {
        RetrievalTaskClass::RoutineAutomation if task.contains("routine") => score += 25,
        RetrievalTaskClass::Troubleshooting if task.contains("troubleshoot") => score += 20,
        RetrievalTaskClass::Coding if task.contains("coding") || task.contains("code") => {
            score += 20
        }
        RetrievalTaskClass::SystemAdmin if task.contains("admin") || task.contains("ops") => {
            score += 20
        }
        _ => {}
    }
    score
}

fn render_prompt_context_block(
    req: &MemoryRetrievalRequest,
    working: &[MemoryRecord],
    episodic: &[MemoryRecord],
    semantic: &[MemoryRecord],
    playbooks: &[ProceduralPlaybook],
) -> String {
    if working.is_empty() && episodic.is_empty() && semantic.is_empty() && playbooks.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "Memory Retrieval Context (Phase 2, best-effort; verify before high-impact actions)\n",
    );
    out.push_str(&format!(
        "Task class: {:?}\nQuery: {}\n",
        req.task_class,
        truncate_chars(&req.query, 200)
    ));

    if !playbooks.is_empty() {
        out.push_str("\nProcedural Playbooks:\n");
        for p in playbooks {
            out.push_str(&format!(
                "- {} [{}] success={} conf={:.2} status={:?}\n",
                truncate_chars(&p.name, 80),
                p.task_class,
                p.success_count,
                p.confidence,
                p.status
            ));
        }
    }

    append_record_section(&mut out, "Working Memory", working);
    append_record_section(&mut out, "Episodic Memory", episodic);
    append_record_section(&mut out, "Semantic Memory", semantic);

    out
}

fn append_record_section(out: &mut String, title: &str, records: &[MemoryRecord]) {
    if records.is_empty() {
        return;
    }
    out.push_str(&format!("\n{}:\n", title));
    for r in records {
        out.push_str(&format!(
            "- [{}] {} :: {} (conf={:.2}, at={})\n",
            r.category,
            truncate_chars(&r.title, 80),
            truncate_chars(&r.summary, 160),
            r.confidence,
            short_ts(r.created_at)
        ));
    }
}

fn short_ts(ts: DateTime<Utc>) -> String {
    ts.format("%Y-%m-%dT%H:%MZ").to_string()
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated = s.chars().take(max_chars).collect::<String>();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{MemorySensitivity, MemorySourceKind};
    use crate::db::AutonomyMemoryStore;
    use crate::db::Database;
    use crate::db::libsql::LibSqlBackend;
    use chrono::Duration;

    fn sample_record(
        user_id: &str,
        memory_type: MemoryType,
        category: &str,
        title: &str,
        summary: &str,
        created_at: DateTime<Utc>,
    ) -> MemoryRecord {
        MemoryRecord {
            id: Uuid::new_v4(),
            owner_user_id: user_id.to_string(),
            goal_id: None,
            plan_id: None,
            plan_step_id: None,
            job_id: None,
            thread_id: None,
            memory_type,
            source_kind: MemorySourceKind::WorkerPlanExecution,
            category: category.to_string(),
            title: title.to_string(),
            summary: summary.to_string(),
            payload: json!({}),
            provenance: json!({"source":"test"}),
            confidence: 0.8,
            sensitivity: MemorySensitivity::Internal,
            ttl_secs: None,
            status: MemoryRecordStatus::Active,
            workspace_doc_path: None,
            workspace_document_id: None,
            created_at,
            updated_at: created_at,
            expires_at: None,
        }
    }

    #[test]
    fn infer_task_class_prefers_routine_signals() {
        let task_class = MemoryRetrievalComposer::infer_task_class(
            Some("maintenance_routine"),
            "Nightly cleanup",
            "Scheduled cron cleanup run",
        );
        assert_eq!(task_class, RetrievalTaskClass::RoutineAutomation);
    }

    #[tokio::test]
    async fn compose_prefers_routine_playbooks_and_scopes_to_user() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("memory_retrieval_composer_test.db");
        let backend = LibSqlBackend::new_local(&db_path)
            .await
            .expect("libsql local");
        backend.run_migrations().await.expect("migrations");

        let now = Utc::now();
        backend
            .create_memory_record(&sample_record(
                "user-1",
                MemoryType::Episodic,
                "routine_run_summary",
                "Cleanup completed",
                "Routine completed successfully",
                now,
            ))
            .await
            .expect("create episodic");
        backend
            .create_memory_record(&sample_record(
                "user-1",
                MemoryType::Semantic,
                "semantic_summary",
                "Cleanup policy",
                "Prefer dry-run before deletion",
                now - Duration::minutes(1),
            ))
            .await
            .expect("create semantic");
        backend
            .create_memory_record(&sample_record(
                "user-2",
                MemoryType::Episodic,
                "routine_run_summary",
                "Other user routine",
                "Should not appear",
                now - Duration::minutes(2),
            ))
            .await
            .expect("create other user record");

        backend
            .create_or_update_procedural_playbook(&ProceduralPlaybook {
                id: Uuid::new_v4(),
                owner_user_id: "user-1".to_string(),
                name: "Routine: cleanup".to_string(),
                task_class: "routine_automation".to_string(),
                trigger_signals: json!({"trigger":"cron"}),
                steps_template: json!({"steps":["scan","cleanup","verify"]}),
                tool_preferences: json!({}),
                constraints: json!({}),
                success_count: 3,
                failure_count: 0,
                confidence: 0.84,
                status: ProceduralPlaybookStatus::Active,
                requires_approval: false,
                source_memory_record_ids: json!([]),
                created_at: now,
                updated_at: now,
            })
            .await
            .expect("create playbook");

        let bundle = MemoryRetrievalComposer::compose(
            Arc::new(backend),
            MemoryRetrievalRequest {
                user_id: "user-1".to_string(),
                goal_id: None,
                plan_id: None,
                job_id: None,
                task_class: RetrievalTaskClass::RoutineAutomation,
                query: "Nightly cleanup routine failed yesterday; retry with safe steps"
                    .to_string(),
                limit_budget: 12,
            },
        )
        .await
        .expect("compose retrieval bundle");

        assert_eq!(bundle.procedural_playbooks.len(), 1);
        assert_eq!(bundle.procedural_playbooks[0].name, "Routine: cleanup");
        assert_eq!(bundle.episodic_hits.len(), 1);
        assert!(
            bundle
                .episodic_hits
                .iter()
                .all(|r| r.owner_user_id == "user-1")
        );
        assert!(bundle.prompt_context_block.contains("Procedural Playbooks"));
        assert!(bundle.prompt_context_block.contains("Routine: cleanup"));
        assert!(bundle.prompt_context_block.contains("routine_run_summary"));
        assert!(
            bundle
                .prompt_context_block
                .contains("Task class: RoutineAutomation")
        );
        assert!(!bundle.prompt_context_block.contains("Other user routine"));
    }
}
