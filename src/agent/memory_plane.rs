use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

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
