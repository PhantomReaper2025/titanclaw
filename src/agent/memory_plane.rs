use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::db::Database;

use super::memory_write_policy::{
    MemoryWriteCandidate, MemoryWriteIntent, MemoryWritePolicyEngine,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Working,
    Episodic,
    Semantic,
    Procedural,
    Profile,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemorySourceKind {
    WorkerPlanExecution,
    SchedulerSubtask,
    DispatcherToolExecution,
    ApprovalFlow,
    RoutineRun,
    ProfileSynthesis,
    ToolMemoryWrite,
    Consolidator,
    UserApi,
    Cli,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemorySensitivity {
    Public,
    Internal,
    Sensitive,
    CredentialAdjacent,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRecordStatus {
    Active,
    Demoted,
    Expired,
    Archived,
    Rejected,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConsolidationAction {
    PromoteSemantic,
    GeneratePlaybook,
    ArchiveSummary,
    Demote,
    Reject,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProceduralPlaybookStatus {
    Draft,
    Active,
    Paused,
    Retired,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConsolidationRunStatus {
    Running,
    Completed,
    Failed,
    Partial,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: Uuid,
    pub owner_user_id: String,
    pub goal_id: Option<Uuid>,
    pub plan_id: Option<Uuid>,
    pub plan_step_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub memory_type: MemoryType,
    pub source_kind: MemorySourceKind,
    pub category: String,
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub provenance: Value,
    pub confidence: f32,
    pub sensitivity: MemorySensitivity,
    pub ttl_secs: Option<i64>,
    pub status: MemoryRecordStatus,
    pub workspace_doc_path: Option<String>,
    pub workspace_document_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEvent {
    pub id: Uuid,
    pub memory_record_id: Uuid,
    pub event_kind: String,
    pub actor: String,
    #[serde(default)]
    pub reason_codes: Vec<String>,
    pub action: Option<ConsolidationAction>,
    #[serde(default)]
    pub before: Value,
    #[serde(default)]
    pub after: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProceduralPlaybook {
    pub id: Uuid,
    pub owner_user_id: String,
    pub name: String,
    pub task_class: String,
    #[serde(default)]
    pub trigger_signals: Value,
    #[serde(default)]
    pub steps_template: Value,
    #[serde(default)]
    pub tool_preferences: Value,
    #[serde(default)]
    pub constraints: Value,
    pub success_count: i32,
    pub failure_count: i32,
    pub confidence: f32,
    pub status: ProceduralPlaybookStatus,
    pub requires_approval: bool,
    #[serde(default)]
    pub source_memory_record_ids: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationRun {
    pub id: Uuid,
    pub owner_user_id: Option<String>,
    pub status: ConsolidationRunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub batch_size: i32,
    pub processed_count: i32,
    pub promoted_count: i32,
    pub playbooks_created_count: i32,
    pub archived_count: i32,
    pub error_count: i32,
    pub checkpoint_cursor: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryRecordWriteRequest {
    pub owner_user_id: String,
    pub goal_id: Option<Uuid>,
    pub plan_id: Option<Uuid>,
    pub plan_step_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub source_kind: MemorySourceKind,
    pub category: String,
    pub title: String,
    pub summary: String,
    pub payload: Value,
    pub provenance: Value,
    pub desired_memory_type: Option<MemoryType>,
    pub confidence_hint: Option<f32>,
    pub sensitivity_hint: Option<MemorySensitivity>,
    pub ttl_secs_hint: Option<i64>,
    pub high_impact: bool,
    pub intent: MemoryWriteIntent,
}

pub(crate) fn persist_memory_record_best_effort(
    store: Arc<dyn Database>,
    request: MemoryRecordWriteRequest,
) {
    tokio::spawn(async move {
        let engine = MemoryWritePolicyEngine::default();
        let candidate = MemoryWriteCandidate {
            source_kind: request.source_kind,
            category: request.category.clone(),
            title: request.title.clone(),
            summary: request.summary.clone(),
            payload: request.payload.clone(),
            provenance: request.provenance.clone(),
            desired_memory_type: request.desired_memory_type,
            confidence_hint: request.confidence_hint,
            sensitivity_hint: request.sensitivity_hint,
            ttl_secs_hint: request.ttl_secs_hint,
            high_impact: request.high_impact,
            intent: request.intent.clone(),
        };
        let decision = engine.classify(&candidate);
        let now = Utc::now();
        let expires_at = decision
            .ttl_secs
            .filter(|ttl| *ttl > 0)
            .and_then(chrono::Duration::try_seconds)
            .map(|delta| now + delta);

        let record = MemoryRecord {
            id: Uuid::new_v4(),
            owner_user_id: request.owner_user_id,
            goal_id: request.goal_id,
            plan_id: request.plan_id,
            plan_step_id: request.plan_step_id,
            job_id: request.job_id,
            thread_id: request.thread_id,
            memory_type: decision.memory_type,
            source_kind: request.source_kind,
            category: request.category,
            title: request.title,
            summary: request.summary,
            payload: request.payload,
            provenance: request.provenance,
            confidence: decision.confidence,
            sensitivity: decision.sensitivity,
            ttl_secs: decision.ttl_secs,
            status: decision.status,
            workspace_doc_path: None,
            workspace_document_id: None,
            created_at: now,
            updated_at: now,
            expires_at,
        };

        if let Err(e) = store.create_memory_record(&record).await {
            tracing::warn!(
                owner_user_id = %record.owner_user_id,
                category = %record.category,
                source_kind = ?record.source_kind,
                "Failed to persist memory-plane record: {}",
                e
            );
        }
    });
}
