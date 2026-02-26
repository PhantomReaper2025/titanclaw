use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::agent::{
    ConsolidationAction, ConsolidationRun, ConsolidationRunStatus, MemoryEvent, MemoryRecord,
    MemoryRecordStatus, MemorySourceKind, MemoryType,
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
                    action,
                }) => {
                    run.processed_count += 1;
                    if promoted {
                        run.promoted_count += 1;
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
        let action;

        if record.category == "routine_run_summary" {
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
            action,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct ProcessOutcome {
    promoted: bool,
    archived: bool,
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
}
