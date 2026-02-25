//! Autonomy domain types.
//!
//! Versioned, serde-friendly data structures for representing autonomous
//! planning and execution state.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Version 1 autonomy domain schema.
pub mod v1 {
    use super::*;

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum GoalStatus {
        Proposed,
        Active,
        Blocked,
        Waiting,
        Completed,
        Abandoned,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum GoalRiskClass {
        Low,
        Medium,
        High,
        Critical,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum GoalSource {
        UserRequest,
        RoutineTrigger,
        SystemMaintenance,
        FollowUp,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum PlanStatus {
        Draft,
        Ready,
        Running,
        Paused,
        Failed,
        Completed,
        Superseded,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum PlannerKind {
        ReasoningV1,
        RuleBased,
        Hybrid,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum PlanStepKind {
        ToolCall,
        EvidenceGather,
        Verification,
        AskUser,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum PlanStepStatus {
        Pending,
        Running,
        Succeeded,
        Failed,
        Blocked,
        Skipped,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum ExecutionAttemptStatus {
        Running,
        Succeeded,
        Failed,
        Timeout,
        Blocked,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum PolicyDecisionKind {
        Allow,
        AllowWithLogging,
        RequireApproval,
        Deny,
        RequireMoreEvidence,
        Modify,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum PlanVerificationStatus {
        Passed,
        Failed,
        Inconclusive,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum EvidenceKind {
        ToolResult,
        FileDiff,
        TestRun,
        CommandOutput,
        Observation,
        UserConfirmation,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Goal {
        pub id: Uuid,
        pub owner_user_id: String,
        pub channel: Option<String>,
        pub thread_id: Option<Uuid>,
        pub title: String,
        pub intent: String,
        pub priority: i32,
        pub status: GoalStatus,
        pub risk_class: GoalRiskClass,
        #[serde(default)]
        pub acceptance_criteria: Value,
        #[serde(default)]
        pub constraints: Value,
        pub source: GoalSource,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
        pub completed_at: Option<DateTime<Utc>>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Plan {
        pub id: Uuid,
        pub goal_id: Uuid,
        pub revision: i32,
        pub status: PlanStatus,
        pub planner_kind: PlannerKind,
        pub source_action_plan: Option<Value>,
        #[serde(default)]
        pub assumptions: Value,
        pub confidence: f64,
        pub estimated_cost: Option<f64>,
        pub estimated_time_secs: Option<u64>,
        pub summary: Option<String>,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PlanStep {
        pub id: Uuid,
        pub plan_id: Uuid,
        pub sequence_num: i32,
        pub kind: PlanStepKind,
        pub status: PlanStepStatus,
        pub title: String,
        pub description: String,
        #[serde(default)]
        pub tool_candidates: Value,
        #[serde(default)]
        pub inputs: Value,
        #[serde(default)]
        pub preconditions: Value,
        #[serde(default)]
        pub postconditions: Value,
        pub rollback: Option<Value>,
        #[serde(default)]
        pub policy_requirements: Value,
        pub started_at: Option<DateTime<Utc>>,
        pub completed_at: Option<DateTime<Utc>>,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ExecutionAttempt {
        pub id: Uuid,
        pub goal_id: Option<Uuid>,
        pub plan_id: Option<Uuid>,
        pub plan_step_id: Option<Uuid>,
        pub job_id: Option<Uuid>,
        pub thread_id: Option<Uuid>,
        pub user_id: String,
        pub channel: String,
        pub tool_name: String,
        pub tool_call_id: Option<String>,
        pub tool_args: Option<Value>,
        pub status: ExecutionAttemptStatus,
        pub failure_class: Option<String>,
        pub retry_count: i32,
        pub started_at: DateTime<Utc>,
        pub finished_at: Option<DateTime<Utc>>,
        pub elapsed_ms: Option<i64>,
        pub result_summary: Option<String>,
        pub error_preview: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PolicyDecision {
        pub id: Uuid,
        pub goal_id: Option<Uuid>,
        pub plan_id: Option<Uuid>,
        pub plan_step_id: Option<Uuid>,
        pub execution_attempt_id: Option<Uuid>,
        pub user_id: String,
        pub channel: String,
        pub tool_name: Option<String>,
        pub tool_call_id: Option<String>,
        pub action_kind: String,
        pub decision: PolicyDecisionKind,
        #[serde(default)]
        pub reason_codes: Vec<String>,
        pub risk_score: Option<f32>,
        pub confidence: Option<f32>,
        pub requires_approval: bool,
        pub auto_approved: Option<bool>,
        #[serde(default)]
        pub evidence_required: Value,
        pub created_at: DateTime<Utc>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PlanVerification {
        pub id: Uuid,
        pub goal_id: Option<Uuid>,
        pub plan_id: Uuid,
        pub job_id: Option<Uuid>,
        pub user_id: String,
        pub channel: String,
        pub verifier_kind: String,
        pub status: PlanVerificationStatus,
        pub completion_claimed: bool,
        pub evidence_count: i32,
        pub summary: String,
        #[serde(default)]
        pub checks: Value,
        #[serde(default)]
        pub evidence: Value,
        pub created_at: DateTime<Utc>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Incident {
        pub id: Uuid,
        pub goal_id: Option<Uuid>,
        pub plan_id: Option<Uuid>,
        pub plan_step_id: Option<Uuid>,
        pub execution_attempt_id: Option<Uuid>,
        pub policy_decision_id: Option<Uuid>,
        pub job_id: Option<Uuid>,
        pub thread_id: Option<Uuid>,
        pub user_id: String,
        pub channel: Option<String>,
        pub incident_type: String,
        pub severity: String,
        pub status: String,
        pub summary: String,
        #[serde(default)]
        pub details: Value,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
        pub resolved_at: Option<DateTime<Utc>>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Evidence {
        pub kind: EvidenceKind,
        pub source: String,
        pub summary: String,
        pub confidence: f32,
        #[serde(default)]
        pub payload: Value,
        pub created_at: DateTime<Utc>,
    }
}
