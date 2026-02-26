use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::agent::{
    ConsolidationAction, ConsolidationRun, ConsolidationRunStatus, MemoryEvent, MemoryRecord,
    MemoryRecordStatus, MemorySourceKind, MemoryType, ProceduralPlaybook, ProceduralPlaybookStatus,
};
use crate::db::Database;

#[derive(Debug, Clone)]
pub(crate) struct MemoryConsolidatorConfig {
    pub enabled: bool,
    pub interval: Duration,
    pub batch_size: u32,
}

pub(crate) struct MemoryConsolidator {
    cfg: MemoryConsolidatorConfig,
    store: Arc<dyn Database>,
}

impl MemoryConsolidator {
    pub(crate) fn new(cfg: MemoryConsolidatorConfig, store: Arc<dyn Database>) -> Arc<Self> {
        Arc::new(Self { cfg, store })
    }

    pub(crate) fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    pub(crate) fn interval(&self) -> Duration {
        self.cfg.interval
    }

    pub(crate) async fn tick(&self) {
        if !self.cfg.enabled {
            return;
        }
        if let Err(e) = self.run_once().await {
            tracing::warn!("Memory consolidation tick failed: {}", e);
        }
    }

    pub(crate) async fn run_once(&self) -> Result<ConsolidationRun, String> {
        let now = Utc::now();
        let mut run = ConsolidationRun {
            id: Uuid::new_v4(),
            owner_user_id: None,
            status: ConsolidationRunStatus::Running,
            started_at: now,
            finished_at: None,
            batch_size: self.cfg.batch_size as i32,
            processed_count: 0,
            promoted_count: 0,
            playbooks_created_count: 0,
            archived_count: 0,
            error_count: 0,
            checkpoint_cursor: None,
            notes: None,
        };

        self.store
            .record_consolidation_run(&run)
            .await
            .map_err(|e| format!("record run: {e}"))?;

        let records = self
            .store
            .list_pending_episodic_for_consolidation(self.cfg.batch_size as i64)
            .await
            .map_err(|e| format!("list pending episodic: {e}"))?;

        for record in records {
            let last_cursor = format!("{}/{}", record.created_at.to_rfc3339(), record.id);
            match self.process_record(&record).await {
                Ok(ProcessOutcome {
                    promoted,
                    archived,
                    playbook_created,
                    action,
                }) => {
                    run.processed_count += 1;
                    if promoted {
                        run.promoted_count += 1;
                    }
                    if playbook_created {
                        run.playbooks_created_count += 1;
                    }
                    if archived {
                        run.archived_count += 1;
                    }
                    run.checkpoint_cursor = Some(last_cursor);
                    tracing::debug!(
                        memory_record_id = %record.id,
                        ?action,
                        "Memory consolidator processed record"
                    );
                }
                Err(e) => {
                    run.error_count += 1;
                    tracing::warn!(memory_record_id = %record.id, "Memory consolidator record error: {}", e);
                }
            }
        }

        run.finished_at = Some(Utc::now());
        run.status = if run.error_count == 0 {
            ConsolidationRunStatus::Completed
        } else if run.processed_count > 0 {
            ConsolidationRunStatus::Partial
        } else {
            ConsolidationRunStatus::Failed
        };
        if run.processed_count == 0 && run.error_count == 0 {
            run.notes = Some("No pending episodic records".to_string());
        }

        self.store
            .update_consolidation_run(&run)
            .await
            .map_err(|e| format!("update run: {e}"))?;

        Ok(run)
    }

    async fn process_record(&self, record: &MemoryRecord) -> Result<ProcessOutcome, String> {
        let now = Utc::now();
        let mut promoted = false;
        let mut archived = false;
        let mut playbook_created = false;
        let action;

        if record.category == "routine_playbook_candidate" {
            playbook_created = self.promote_routine_playbook_candidate(record, now).await?;
            promoted = true;
            archived = true;
            action = ConsolidationAction::GeneratePlaybook;
            self.store
                .update_memory_record_status(record.id, MemoryRecordStatus::Archived)
                .await
                .map_err(|e| format!("archive playbook candidate source record: {e}"))?;
        } else if record.category == "routine_run_summary" {
            let semantic = MemoryRecord {
                id: Uuid::new_v4(),
                owner_user_id: record.owner_user_id.clone(),
                goal_id: record.goal_id,
                plan_id: record.plan_id,
                plan_step_id: None,
                job_id: record.job_id,
                thread_id: record.thread_id,
                memory_type: MemoryType::Semantic,
                source_kind: MemorySourceKind::Consolidator,
                category: "semantic_summary".to_string(),
                title: format!("Consolidated: {}", record.title),
                summary: record.summary.clone(),
                payload: json!({
                    "source_memory_record_id": record.id,
                    "source_category": record.category,
                    "summary": record.summary,
                    "payload": record.payload,
                }),
                provenance: json!({
                    "source": "memory_consolidator.run_once",
                    "timestamp": now,
                    "source_memory_record_id": record.id,
                }),
                confidence: (record.confidence.max(0.6)).min(0.95),
                sensitivity: record.sensitivity,
                ttl_secs: None,
                status: MemoryRecordStatus::Active,
                workspace_doc_path: None,
                workspace_document_id: None,
                created_at: now,
                updated_at: now,
                expires_at: None,
            };
            self.store
                .create_memory_record(&semantic)
                .await
                .map_err(|e| format!("create semantic record: {e}"))?;
            promoted = true;
            archived = true;
            action = ConsolidationAction::PromoteSemantic;
            self.store
                .update_memory_record_status(record.id, MemoryRecordStatus::Archived)
                .await
                .map_err(|e| format!("archive source record: {e}"))?;
        } else {
            self.store
                .update_memory_record_status(record.id, MemoryRecordStatus::Demoted)
                .await
                .map_err(|e| format!("demote source record: {e}"))?;
            action = ConsolidationAction::Demote;
        }

        let event = MemoryEvent {
            id: Uuid::new_v4(),
            memory_record_id: record.id,
            event_kind: "consolidated".to_string(),
            actor: "memory_consolidator_v2".to_string(),
            reason_codes: vec!["phase2_consolidation".to_string()],
            action: Some(action),
            before: json!({
                "status": record.status,
                "memory_type": record.memory_type,
            }),
            after: json!({
                "status": if archived { MemoryRecordStatus::Archived } else { MemoryRecordStatus::Demoted },
                "promoted": promoted,
            }),
            created_at: now,
        };
        self.store
            .record_memory_event(&event)
            .await
            .map_err(|e| format!("record memory event: {e}"))?;

        Ok(ProcessOutcome {
            promoted,
            archived,
            playbook_created,
            action,
        })
    }

    async fn promote_routine_playbook_candidate(
        &self,
        record: &MemoryRecord,
        now: chrono::DateTime<Utc>,
    ) -> Result<bool, String> {
        let payload = &record.payload;
        let task_class = payload
            .get("task_class")
            .and_then(|v| v.as_str())
            .unwrap_or("routine_automation")
            .to_string();
        let routine_name = payload
            .get("routine_name")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| record.title.as_str())
            .to_string();
        let playbook_name = format!("Routine: {}", routine_name);

        let existing = self
            .store
            .list_procedural_playbooks_for_user(&record.owner_user_id)
            .await
            .map_err(|e| format!("list procedural playbooks: {e}"))?;

        let maybe_existing = existing
            .into_iter()
            .find(|p| p.name == playbook_name && p.task_class == task_class);

        if let Some(mut playbook) = maybe_existing {
            playbook.success_count = playbook.success_count.saturating_add(1);
            playbook.confidence = playbook.confidence.max(record.confidence).min(0.95);
            playbook.updated_at = now;
            let mut source_ids = playbook
                .source_memory_record_ids
                .as_array()
                .cloned()
                .unwrap_or_default();
            let source_id_json = json!(record.id);
            if !source_ids.iter().any(|v| v == &source_id_json) {
                source_ids.push(source_id_json);
            }
            playbook.source_memory_record_ids = serde_json::Value::Array(source_ids);
            self.store
                .create_or_update_procedural_playbook(&playbook)
                .await
                .map_err(|e| format!("update procedural playbook: {e}"))?;
            Ok(false)
        } else {
            let playbook = ProceduralPlaybook {
                id: Uuid::new_v4(),
                owner_user_id: record.owner_user_id.clone(),
                name: playbook_name,
                task_class,
                trigger_signals: json!({
                    "trigger_type": payload.get("trigger_type"),
                    "routine_name": routine_name,
                }),
                steps_template: json!({
                    "candidate_steps_hint": payload.get("candidate_steps_hint"),
                    "source_category": record.category,
                }),
                tool_preferences: json!({}),
                constraints: json!({}),
                success_count: 1,
                failure_count: 0,
                confidence: record.confidence.max(0.65).min(0.9),
                status: ProceduralPlaybookStatus::Draft,
                requires_approval: false,
                source_memory_record_ids: json!([record.id]),
                created_at: now,
                updated_at: now,
            };
            self.store
                .create_or_update_procedural_playbook(&playbook)
                .await
                .map_err(|e| format!("create procedural playbook: {e}"))?;
            Ok(true)
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ProcessOutcome {
    promoted: bool,
    archived: bool,
    playbook_created: bool,
    action: ConsolidationAction,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AutonomyMemoryStore;
    use crate::db::Database;
    use crate::db::libsql::LibSqlBackend;
    use serde_json::json;

    #[tokio::test]
    async fn run_once_records_run_and_promotes_routine_summary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("memory_consolidator_test.db");
        let backend = LibSqlBackend::new_local(&db_path)
            .await
            .expect("libsql local");
        backend.run_migrations().await.expect("migrations");

        let now = Utc::now();
        let source = MemoryRecord {
            id: Uuid::new_v4(),
            owner_user_id: "user-1".to_string(),
            goal_id: None,
            plan_id: None,
            plan_step_id: None,
            job_id: None,
            thread_id: None,
            memory_type: MemoryType::Episodic,
            source_kind: MemorySourceKind::RoutineRun,
            category: "routine_run_summary".to_string(),
            title: "Routine cleanup ok".to_string(),
            summary: "Routine completed successfully".to_string(),
            payload: json!({"status":"ok"}),
            provenance: json!({"source":"test","timestamp":now}),
            confidence: 0.9,
            sensitivity: crate::agent::MemorySensitivity::Internal,
            ttl_secs: Some(3600),
            status: MemoryRecordStatus::Active,
            workspace_doc_path: None,
            workspace_document_id: None,
            created_at: now,
            updated_at: now,
            expires_at: None,
        };
        backend
            .create_memory_record(&source)
            .await
            .expect("create source memory record");

        let consolidator = MemoryConsolidator::new(
            MemoryConsolidatorConfig {
                enabled: true,
                interval: Duration::from_secs(60),
                batch_size: 10,
            },
            Arc::new(backend),
        );

        let run = consolidator.run_once().await.expect("run once");
        assert_eq!(run.status, ConsolidationRunStatus::Completed);
        assert_eq!(run.processed_count, 1);
        assert_eq!(run.promoted_count, 1);

        let records = consolidator
            .store
            .list_memory_records_for_user("user-1")
            .await
            .expect("list memory records");
        assert!(
            records
                .iter()
                .any(|r| r.id == source.id && r.status == MemoryRecordStatus::Archived)
        );
        assert!(
            records
                .iter()
                .any(|r| r.category == "semantic_summary" && r.memory_type == MemoryType::Semantic)
        );

        let events = consolidator
            .store
            .list_memory_events_for_record(source.id)
            .await
            .expect("list memory events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "consolidated");
    }

    #[tokio::test]
    async fn run_once_promotes_routine_playbook_candidate_into_procedural_playbook() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("memory_consolidator_playbook_test.db");
        let backend = LibSqlBackend::new_local(&db_path)
            .await
            .expect("libsql local");
        backend.run_migrations().await.expect("migrations");

        let consolidator = MemoryConsolidator::new(
            MemoryConsolidatorConfig {
                enabled: true,
                interval: Duration::from_secs(60),
                batch_size: 10,
            },
            Arc::new(backend),
        );

        let now = Utc::now();
        let first_candidate = MemoryRecord {
            id: Uuid::new_v4(),
            owner_user_id: "user-1".to_string(),
            goal_id: None,
            plan_id: None,
            plan_step_id: None,
            job_id: None,
            thread_id: None,
            memory_type: MemoryType::Episodic,
            source_kind: MemorySourceKind::RoutineRun,
            category: "routine_playbook_candidate".to_string(),
            title: "Routine cleanup playbook candidate".to_string(),
            summary: "Repeated successful cleanup routine".to_string(),
            payload: json!({
                "task_class": "routine_automation",
                "routine_name": "cleanup",
                "trigger_type": "cron",
                "candidate_steps_hint": ["scan", "cleanup", "verify"]
            }),
            provenance: json!({"source":"test","timestamp":now}),
            confidence: 0.82,
            sensitivity: crate::agent::MemorySensitivity::Internal,
            ttl_secs: None,
            status: MemoryRecordStatus::Active,
            workspace_doc_path: None,
            workspace_document_id: None,
            created_at: now,
            updated_at: now,
            expires_at: None,
        };
        consolidator
            .store
            .create_memory_record(&first_candidate)
            .await
            .expect("create first candidate");

        let first_run = consolidator.run_once().await.expect("first run");
        assert_eq!(first_run.status, ConsolidationRunStatus::Completed);
        assert_eq!(first_run.processed_count, 1);
        assert_eq!(first_run.promoted_count, 1);
        assert_eq!(first_run.playbooks_created_count, 1);
        assert_eq!(first_run.archived_count, 1);

        let mut playbooks = consolidator
            .store
            .list_procedural_playbooks_for_user("user-1")
            .await
            .expect("list playbooks after first run");
        assert_eq!(playbooks.len(), 1);
        let playbook_id = playbooks[0].id;
        assert_eq!(playbooks[0].name, "Routine: cleanup");
        assert_eq!(playbooks[0].task_class, "routine_automation");
        assert_eq!(playbooks[0].success_count, 1);
        assert_eq!(playbooks[0].status, ProceduralPlaybookStatus::Draft);
        assert_eq!(
            playbooks[0]
                .source_memory_record_ids
                .as_array()
                .expect("source ids array")
                .len(),
            1
        );

        let records_after_first = consolidator
            .store
            .list_memory_records_for_user("user-1")
            .await
            .expect("list memory records after first run");
        assert!(
            records_after_first.iter().any(|r| {
                r.id == first_candidate.id && r.status == MemoryRecordStatus::Archived
            })
        );

        let second_now = now + chrono::Duration::seconds(1);
        let second_candidate = MemoryRecord {
            id: Uuid::new_v4(),
            owner_user_id: "user-1".to_string(),
            goal_id: None,
            plan_id: None,
            plan_step_id: None,
            job_id: None,
            thread_id: None,
            memory_type: MemoryType::Episodic,
            source_kind: MemorySourceKind::RoutineRun,
            category: "routine_playbook_candidate".to_string(),
            title: "Routine cleanup playbook candidate".to_string(),
            summary: "Repeated successful cleanup routine again".to_string(),
            payload: json!({
                "task_class": "routine_automation",
                "routine_name": "cleanup",
                "trigger_type": "cron",
                "candidate_steps_hint": ["scan", "cleanup", "verify"]
            }),
            provenance: json!({"source":"test","timestamp":second_now}),
            confidence: 0.88,
            sensitivity: crate::agent::MemorySensitivity::Internal,
            ttl_secs: None,
            status: MemoryRecordStatus::Active,
            workspace_doc_path: None,
            workspace_document_id: None,
            created_at: second_now,
            updated_at: second_now,
            expires_at: None,
        };
        consolidator
            .store
            .create_memory_record(&second_candidate)
            .await
            .expect("create second candidate");

        let second_run = consolidator.run_once().await.expect("second run");
        assert_eq!(second_run.status, ConsolidationRunStatus::Completed);
        assert_eq!(second_run.processed_count, 1);
        assert_eq!(second_run.promoted_count, 1);
        assert_eq!(second_run.playbooks_created_count, 0);
        assert_eq!(second_run.archived_count, 1);

        playbooks = consolidator
            .store
            .list_procedural_playbooks_for_user("user-1")
            .await
            .expect("list playbooks after second run");
        assert_eq!(playbooks.len(), 1);
        assert_eq!(playbooks[0].id, playbook_id);
        assert_eq!(playbooks[0].success_count, 2);
        assert!(playbooks[0].confidence >= 0.88);
        let source_ids = playbooks[0]
            .source_memory_record_ids
            .as_array()
            .expect("source ids array after update");
        assert_eq!(source_ids.len(), 2);
        assert!(source_ids.iter().any(|v| *v == json!(first_candidate.id)));
        assert!(source_ids.iter().any(|v| *v == json!(second_candidate.id)));

        let second_events = consolidator
            .store
            .list_memory_events_for_record(second_candidate.id)
            .await
            .expect("list memory events for second candidate");
        assert_eq!(second_events.len(), 1);
        assert_eq!(second_events[0].event_kind, "consolidated");
        assert_eq!(
            second_events[0].action,
            Some(ConsolidationAction::GeneratePlaybook)
        );
    }
}
