//! PostgreSQL backend for the Database trait.
//!
//! Delegates to the existing `Store` (history) and `Repository` (workspace)
//! implementations, avoiding SQL duplication.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use rust_decimal::Decimal;
use serde_json::Value;
use tokio_postgres::Row;
use uuid::Uuid;

use crate::agent::BrokenTool;
use crate::agent::autonomy::v1::{
    ExecutionAttempt, ExecutionAttemptStatus, Goal, GoalRiskClass, GoalSource, GoalStatus, Plan,
    PlanStatus, PlanStep, PlanStepKind, PlanStepStatus, PlannerKind, PolicyDecision,
    PolicyDecisionKind,
};
use crate::agent::routine::{Routine, RoutineRun, RunStatus};
use crate::config::DatabaseConfig;
use crate::context::{ActionRecord, JobContext, JobState};
use crate::db::{
    AutonomyExecutionStore, ConversationStore, Database, GoalStore, JobStore, PlanStore,
    RoutineStore, SandboxStore, SettingsStore, ToolFailureStore, WorkspaceStore,
};
use crate::error::{DatabaseError, WorkspaceError};
use crate::history::{
    ConversationMessage, ConversationSummary, JobEventRecord, LlmCallRecord, SandboxJobRecord,
    SandboxJobSummary, SettingRow, Store,
};
use crate::workspace::{
    MemoryChunk, MemoryDocument, Repository, SearchConfig, SearchResult, WorkspaceEntry,
};

/// PostgreSQL database backend.
///
/// Wraps the existing `Store` (for history/conversations/jobs/routines/settings)
/// and `Repository` (for workspace documents/chunks/search) to implement the
/// unified `Database` trait.
pub struct PgBackend {
    store: Store,
    repo: Repository,
}

impl PgBackend {
    /// Create a new PostgreSQL backend from configuration.
    pub async fn new(config: &DatabaseConfig) -> Result<Self, DatabaseError> {
        let store = Store::new(config).await?;
        let repo = Repository::new(store.pool());
        Ok(Self { store, repo })
    }

    /// Get a clone of the connection pool.
    ///
    /// Useful for sharing with components that still need raw pool access.
    pub fn pool(&self) -> Pool {
        self.store.pool()
    }
}

// ==================== Database (supertrait) ====================

#[async_trait]
impl Database for PgBackend {
    async fn run_migrations(&self) -> Result<(), DatabaseError> {
        self.store.run_migrations().await
    }
}

#[async_trait]
impl GoalStore for PgBackend {
    async fn create_goal(&self, goal: &Goal) -> Result<(), DatabaseError> {
        let conn = self.store.conn().await?;

        conn.execute(
            r#"
            INSERT INTO autonomy_goals (
                id, owner_user_id, channel, thread_id, title, intent, priority, status,
                risk_class, acceptance_criteria, constraints, source, created_at, updated_at,
                completed_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8,
                $9, $10, $11, $12, $13, $14, $15
            )
            "#,
            &[
                &goal.id,
                &goal.owner_user_id,
                &goal.channel,
                &goal.thread_id,
                &goal.title,
                &goal.intent,
                &goal.priority,
                &goal_status_to_str(goal.status),
                &goal_risk_class_to_str(goal.risk_class),
                &goal.acceptance_criteria,
                &goal.constraints,
                &goal_source_to_str(goal.source),
                &goal.created_at,
                &goal.updated_at,
                &goal.completed_at,
            ],
        )
        .await?;

        Ok(())
    }

    async fn get_goal(&self, id: Uuid) -> Result<Option<Goal>, DatabaseError> {
        let conn = self.store.conn().await?;
        let row = conn
            .query_opt("SELECT * FROM autonomy_goals WHERE id = $1", &[&id])
            .await?;
        row.map(|r| row_to_goal(&r)).transpose()
    }

    async fn list_goals(&self) -> Result<Vec<Goal>, DatabaseError> {
        let conn = self.store.conn().await?;
        let rows = conn
            .query(
                "SELECT * FROM autonomy_goals ORDER BY updated_at DESC, created_at DESC, id DESC",
                &[],
            )
            .await?;
        rows.iter().map(row_to_goal).collect()
    }

    async fn update_goal_status(&self, id: Uuid, status: GoalStatus) -> Result<(), DatabaseError> {
        let conn = self.store.conn().await?;
        let status_str = goal_status_to_str(status);
        conn.execute(
            r#"
            UPDATE autonomy_goals
            SET status = $2,
                updated_at = NOW(),
                completed_at = CASE
                    WHEN $2 = 'completed' THEN COALESCE(completed_at, NOW())
                    ELSE NULL
                END
            WHERE id = $1
            "#,
            &[&id, &status_str],
        )
        .await?;
        Ok(())
    }

    async fn update_goal_priority(&self, id: Uuid, priority: i32) -> Result<(), DatabaseError> {
        let conn = self.store.conn().await?;
        conn.execute(
            r#"
            UPDATE autonomy_goals
            SET priority = $2,
                updated_at = NOW()
            WHERE id = $1
            "#,
            &[&id, &priority],
        )
        .await?;
        Ok(())
    }
}

#[async_trait]
impl PlanStore for PgBackend {
    async fn create_plan(&self, plan: &Plan) -> Result<(), DatabaseError> {
        let conn = self.store.conn().await?;
        let estimated_time_secs = opt_u64_to_i64(plan.estimated_time_secs, "estimated_time_secs")?;

        conn.execute(
            r#"
            INSERT INTO autonomy_plans (
                id, goal_id, revision, status, planner_kind, source_action_plan, assumptions,
                confidence, estimated_cost, estimated_time_secs, summary, created_at, updated_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7,
                $8, $9, $10, $11, $12, $13
            )
            "#,
            &[
                &plan.id,
                &plan.goal_id,
                &plan.revision,
                &plan_status_to_str(plan.status),
                &planner_kind_to_str(plan.planner_kind),
                &plan.source_action_plan,
                &plan.assumptions,
                &plan.confidence,
                &plan.estimated_cost,
                &estimated_time_secs,
                &plan.summary,
                &plan.created_at,
                &plan.updated_at,
            ],
        )
        .await?;

        Ok(())
    }

    async fn list_plans_for_goal(&self, goal_id: Uuid) -> Result<Vec<Plan>, DatabaseError> {
        let conn = self.store.conn().await?;
        let rows = conn
            .query(
                "SELECT * FROM autonomy_plans WHERE goal_id = $1 ORDER BY revision DESC, created_at DESC, id DESC",
                &[&goal_id],
            )
            .await?;
        rows.iter().map(row_to_plan).collect()
    }

    async fn get_plan(&self, id: Uuid) -> Result<Option<Plan>, DatabaseError> {
        let conn = self.store.conn().await?;
        let row = conn
            .query_opt("SELECT * FROM autonomy_plans WHERE id = $1", &[&id])
            .await?;
        row.map(|r| row_to_plan(&r)).transpose()
    }

    async fn update_plan_status(&self, id: Uuid, status: PlanStatus) -> Result<(), DatabaseError> {
        let conn = self.store.conn().await?;
        let status_str = plan_status_to_str(status);
        conn.execute(
            r#"
            UPDATE autonomy_plans
            SET status = $2,
                updated_at = NOW()
            WHERE id = $1
            "#,
            &[&id, &status_str],
        )
        .await?;
        Ok(())
    }

    async fn create_plan_steps(&self, steps: &[PlanStep]) -> Result<(), DatabaseError> {
        if steps.is_empty() {
            return Ok(());
        }

        let mut conn = self.store.conn().await?;
        let tx = conn.transaction().await?;

        for step in steps {
            tx.execute(
                r#"
                INSERT INTO autonomy_plan_steps (
                    id, plan_id, sequence_num, kind, status, title, description,
                    tool_candidates, inputs, preconditions, postconditions, "rollback",
                    policy_requirements, started_at, completed_at, created_at, updated_at
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7,
                    $8, $9, $10, $11, $12,
                    $13, $14, $15, $16, $17
                )
                "#,
                &[
                    &step.id,
                    &step.plan_id,
                    &step.sequence_num,
                    &plan_step_kind_to_str(step.kind),
                    &plan_step_status_to_str(step.status),
                    &step.title,
                    &step.description,
                    &step.tool_candidates,
                    &step.inputs,
                    &step.preconditions,
                    &step.postconditions,
                    &step.rollback,
                    &step.policy_requirements,
                    &step.started_at,
                    &step.completed_at,
                    &step.created_at,
                    &step.updated_at,
                ],
            )
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn replace_plan_steps_for_plan(
        &self,
        plan_id: Uuid,
        steps: &[PlanStep],
    ) -> Result<(), DatabaseError> {
        if steps.iter().any(|s| s.plan_id != plan_id) {
            return Err(DatabaseError::Query(
                "replace_plan_steps_for_plan received step with mismatched plan_id".to_string(),
            ));
        }

        let mut conn = self.store.conn().await?;
        let tx = conn.transaction().await?;

        tx.execute(
            "DELETE FROM autonomy_plan_steps WHERE plan_id = $1",
            &[&plan_id],
        )
        .await?;

        for step in steps {
            tx.execute(
                r#"
                INSERT INTO autonomy_plan_steps (
                    id, plan_id, sequence_num, kind, status, title, description,
                    tool_candidates, inputs, preconditions, postconditions, "rollback",
                    policy_requirements, started_at, completed_at, created_at, updated_at
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7,
                    $8, $9, $10, $11, $12,
                    $13, $14, $15, $16, $17
                )
                "#,
                &[
                    &step.id,
                    &step.plan_id,
                    &step.sequence_num,
                    &plan_step_kind_to_str(step.kind),
                    &plan_step_status_to_str(step.status),
                    &step.title,
                    &step.description,
                    &step.tool_candidates,
                    &step.inputs,
                    &step.preconditions,
                    &step.postconditions,
                    &step.rollback,
                    &step.policy_requirements,
                    &step.started_at,
                    &step.completed_at,
                    &step.created_at,
                    &step.updated_at,
                ],
            )
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_plan_step(&self, id: Uuid) -> Result<Option<PlanStep>, DatabaseError> {
        let conn = self.store.conn().await?;
        let row = conn
            .query_opt("SELECT * FROM autonomy_plan_steps WHERE id = $1", &[&id])
            .await?;
        row.map(|r| row_to_plan_step(&r)).transpose()
    }

    async fn list_plan_steps_for_plan(
        &self,
        plan_id: Uuid,
    ) -> Result<Vec<PlanStep>, DatabaseError> {
        let conn = self.store.conn().await?;
        let rows = conn
            .query(
                r#"
                SELECT *
                FROM autonomy_plan_steps
                WHERE plan_id = $1
                ORDER BY sequence_num ASC, created_at ASC, id ASC
                "#,
                &[&plan_id],
            )
            .await?;
        rows.iter().map(row_to_plan_step).collect()
    }

    async fn update_plan_step_status(
        &self,
        id: Uuid,
        status: PlanStepStatus,
    ) -> Result<(), DatabaseError> {
        let conn = self.store.conn().await?;
        let status_str = plan_step_status_to_str(status);
        let is_running = matches!(status, PlanStepStatus::Running);
        let is_terminal = matches!(
            status,
            PlanStepStatus::Succeeded
                | PlanStepStatus::Failed
                | PlanStepStatus::Blocked
                | PlanStepStatus::Skipped
        );

        conn.execute(
            r#"
            UPDATE autonomy_plan_steps
            SET status = $2,
                updated_at = NOW(),
                started_at = CASE
                    WHEN $3 THEN COALESCE(started_at, NOW())
                    ELSE started_at
                END,
                completed_at = CASE
                    WHEN $4 THEN COALESCE(completed_at, NOW())
                    ELSE NULL
                END
            WHERE id = $1
            "#,
            &[&id, &status_str, &is_running, &is_terminal],
        )
        .await?;

        Ok(())
    }
}

#[async_trait]
impl AutonomyExecutionStore for PgBackend {
    async fn record_execution_attempt(
        &self,
        attempt: &ExecutionAttempt,
    ) -> Result<(), DatabaseError> {
        let conn = self.store.conn().await?;

        conn.execute(
            r#"
            INSERT INTO autonomy_execution_attempts (
                id, goal_id, plan_id, plan_step_id, job_id, thread_id, user_id, channel,
                tool_name, tool_call_id, tool_args, status, failure_class, retry_count,
                started_at, finished_at, elapsed_ms, result_summary, error_preview
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8,
                $9, $10, $11, $12, $13, $14,
                $15, $16, $17, $18, $19
            )
            "#,
            &[
                &attempt.id,
                &attempt.goal_id,
                &attempt.plan_id,
                &attempt.plan_step_id,
                &attempt.job_id,
                &attempt.thread_id,
                &attempt.user_id,
                &attempt.channel,
                &attempt.tool_name,
                &attempt.tool_call_id,
                &attempt.tool_args,
                &execution_attempt_status_to_str(attempt.status),
                &attempt.failure_class,
                &attempt.retry_count,
                &attempt.started_at,
                &attempt.finished_at,
                &attempt.elapsed_ms,
                &attempt.result_summary,
                &attempt.error_preview,
            ],
        )
        .await?;

        Ok(())
    }

    async fn update_execution_attempt(
        &self,
        attempt: &ExecutionAttempt,
    ) -> Result<(), DatabaseError> {
        let conn = self.store.conn().await?;
        let status_str = execution_attempt_status_to_str(attempt.status);

        conn.execute(
            r#"
            UPDATE autonomy_execution_attempts
            SET goal_id = $2,
                plan_id = $3,
                plan_step_id = $4,
                job_id = $5,
                thread_id = $6,
                user_id = $7,
                channel = $8,
                tool_name = $9,
                tool_call_id = $10,
                tool_args = $11,
                status = $12,
                failure_class = $13,
                retry_count = $14,
                started_at = $15,
                finished_at = $16,
                elapsed_ms = $17,
                result_summary = $18,
                error_preview = $19
            WHERE id = $1
            "#,
            &[
                &attempt.id,
                &attempt.goal_id,
                &attempt.plan_id,
                &attempt.plan_step_id,
                &attempt.job_id,
                &attempt.thread_id,
                &attempt.user_id,
                &attempt.channel,
                &attempt.tool_name,
                &attempt.tool_call_id,
                &attempt.tool_args,
                &status_str,
                &attempt.failure_class,
                &attempt.retry_count,
                &attempt.started_at,
                &attempt.finished_at,
                &attempt.elapsed_ms,
                &attempt.result_summary,
                &attempt.error_preview,
            ],
        )
        .await?;

        Ok(())
    }

    async fn record_policy_decision(&self, decision: &PolicyDecision) -> Result<(), DatabaseError> {
        let conn = self.store.conn().await?;
        let reason_codes = serde_json::to_value(&decision.reason_codes)
            .map_err(|e| DatabaseError::Serialization(e.to_string()))?;
        let decision_str = policy_decision_kind_to_str(decision.decision);

        conn.execute(
            r#"
            INSERT INTO autonomy_policy_decisions (
                id, goal_id, plan_id, plan_step_id, execution_attempt_id, user_id, channel,
                tool_name, tool_call_id, action_kind, decision, reason_codes, risk_score,
                confidence, requires_approval, auto_approved, evidence_required, created_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7,
                $8, $9, $10, $11, $12, $13,
                $14, $15, $16, $17, $18
            )
            "#,
            &[
                &decision.id,
                &decision.goal_id,
                &decision.plan_id,
                &decision.plan_step_id,
                &decision.execution_attempt_id,
                &decision.user_id,
                &decision.channel,
                &decision.tool_name,
                &decision.tool_call_id,
                &decision.action_kind,
                &decision_str,
                &reason_codes,
                &decision.risk_score,
                &decision.confidence,
                &decision.requires_approval,
                &decision.auto_approved,
                &decision.evidence_required,
                &decision.created_at,
            ],
        )
        .await?;

        Ok(())
    }

    async fn list_execution_attempts_for_plan(
        &self,
        plan_id: Uuid,
    ) -> Result<Vec<ExecutionAttempt>, DatabaseError> {
        let conn = self.store.conn().await?;
        let rows = conn
            .query(
                "SELECT * FROM autonomy_execution_attempts WHERE plan_id = $1 ORDER BY started_at DESC, id DESC",
                &[&plan_id],
            )
            .await?;
        rows.iter().map(row_to_execution_attempt).collect()
    }

    async fn list_policy_decisions_for_goal(
        &self,
        goal_id: Uuid,
    ) -> Result<Vec<PolicyDecision>, DatabaseError> {
        let conn = self.store.conn().await?;
        let rows = conn
            .query(
                "SELECT * FROM autonomy_policy_decisions WHERE goal_id = $1 ORDER BY created_at DESC, id DESC",
                &[&goal_id],
            )
            .await?;
        rows.iter().map(row_to_policy_decision).collect()
    }
}

fn enum_parse_error(kind: &str, value: &str) -> DatabaseError {
    DatabaseError::Serialization(format!("invalid autonomy {} value: {}", kind, value))
}

fn goal_status_to_str(value: GoalStatus) -> &'static str {
    match value {
        GoalStatus::Proposed => "proposed",
        GoalStatus::Active => "active",
        GoalStatus::Blocked => "blocked",
        GoalStatus::Waiting => "waiting",
        GoalStatus::Completed => "completed",
        GoalStatus::Abandoned => "abandoned",
    }
}

fn goal_status_from_str(value: &str) -> Result<GoalStatus, DatabaseError> {
    match value {
        "proposed" => Ok(GoalStatus::Proposed),
        "active" => Ok(GoalStatus::Active),
        "blocked" => Ok(GoalStatus::Blocked),
        "waiting" => Ok(GoalStatus::Waiting),
        "completed" => Ok(GoalStatus::Completed),
        "abandoned" => Ok(GoalStatus::Abandoned),
        _ => Err(enum_parse_error("goal_status", value)),
    }
}

fn goal_risk_class_to_str(value: GoalRiskClass) -> &'static str {
    match value {
        GoalRiskClass::Low => "low",
        GoalRiskClass::Medium => "medium",
        GoalRiskClass::High => "high",
        GoalRiskClass::Critical => "critical",
    }
}

fn goal_risk_class_from_str(value: &str) -> Result<GoalRiskClass, DatabaseError> {
    match value {
        "low" => Ok(GoalRiskClass::Low),
        "medium" => Ok(GoalRiskClass::Medium),
        "high" => Ok(GoalRiskClass::High),
        "critical" => Ok(GoalRiskClass::Critical),
        _ => Err(enum_parse_error("goal_risk_class", value)),
    }
}

fn goal_source_to_str(value: GoalSource) -> &'static str {
    match value {
        GoalSource::UserRequest => "user_request",
        GoalSource::RoutineTrigger => "routine_trigger",
        GoalSource::SystemMaintenance => "system_maintenance",
        GoalSource::FollowUp => "follow_up",
    }
}

fn goal_source_from_str(value: &str) -> Result<GoalSource, DatabaseError> {
    match value {
        "user_request" => Ok(GoalSource::UserRequest),
        "routine_trigger" => Ok(GoalSource::RoutineTrigger),
        "system_maintenance" => Ok(GoalSource::SystemMaintenance),
        "follow_up" => Ok(GoalSource::FollowUp),
        _ => Err(enum_parse_error("goal_source", value)),
    }
}

fn plan_status_to_str(value: PlanStatus) -> &'static str {
    match value {
        PlanStatus::Draft => "draft",
        PlanStatus::Ready => "ready",
        PlanStatus::Running => "running",
        PlanStatus::Paused => "paused",
        PlanStatus::Failed => "failed",
        PlanStatus::Completed => "completed",
        PlanStatus::Superseded => "superseded",
    }
}

fn plan_status_from_str(value: &str) -> Result<PlanStatus, DatabaseError> {
    match value {
        "draft" => Ok(PlanStatus::Draft),
        "ready" => Ok(PlanStatus::Ready),
        "running" => Ok(PlanStatus::Running),
        "paused" => Ok(PlanStatus::Paused),
        "failed" => Ok(PlanStatus::Failed),
        "completed" => Ok(PlanStatus::Completed),
        "superseded" => Ok(PlanStatus::Superseded),
        _ => Err(enum_parse_error("plan_status", value)),
    }
}

fn planner_kind_to_str(value: PlannerKind) -> &'static str {
    match value {
        PlannerKind::ReasoningV1 => "reasoning_v1",
        PlannerKind::RuleBased => "rule_based",
        PlannerKind::Hybrid => "hybrid",
    }
}

fn planner_kind_from_str(value: &str) -> Result<PlannerKind, DatabaseError> {
    match value {
        "reasoning_v1" => Ok(PlannerKind::ReasoningV1),
        "rule_based" => Ok(PlannerKind::RuleBased),
        "hybrid" => Ok(PlannerKind::Hybrid),
        _ => Err(enum_parse_error("planner_kind", value)),
    }
}

fn plan_step_kind_to_str(value: PlanStepKind) -> &'static str {
    match value {
        PlanStepKind::ToolCall => "tool_call",
        PlanStepKind::EvidenceGather => "evidence_gather",
        PlanStepKind::Verification => "verification",
        PlanStepKind::AskUser => "ask_user",
    }
}

#[allow(dead_code)]
fn plan_step_kind_from_str(value: &str) -> Result<PlanStepKind, DatabaseError> {
    match value {
        "tool_call" => Ok(PlanStepKind::ToolCall),
        "evidence_gather" => Ok(PlanStepKind::EvidenceGather),
        "verification" => Ok(PlanStepKind::Verification),
        "ask_user" => Ok(PlanStepKind::AskUser),
        _ => Err(enum_parse_error("plan_step_kind", value)),
    }
}

fn plan_step_status_to_str(value: PlanStepStatus) -> &'static str {
    match value {
        PlanStepStatus::Pending => "pending",
        PlanStepStatus::Running => "running",
        PlanStepStatus::Succeeded => "succeeded",
        PlanStepStatus::Failed => "failed",
        PlanStepStatus::Blocked => "blocked",
        PlanStepStatus::Skipped => "skipped",
    }
}

#[allow(dead_code)]
fn plan_step_status_from_str(value: &str) -> Result<PlanStepStatus, DatabaseError> {
    match value {
        "pending" => Ok(PlanStepStatus::Pending),
        "running" => Ok(PlanStepStatus::Running),
        "succeeded" => Ok(PlanStepStatus::Succeeded),
        "failed" => Ok(PlanStepStatus::Failed),
        "blocked" => Ok(PlanStepStatus::Blocked),
        "skipped" => Ok(PlanStepStatus::Skipped),
        _ => Err(enum_parse_error("plan_step_status", value)),
    }
}

fn execution_attempt_status_to_str(value: ExecutionAttemptStatus) -> &'static str {
    match value {
        ExecutionAttemptStatus::Running => "running",
        ExecutionAttemptStatus::Succeeded => "succeeded",
        ExecutionAttemptStatus::Failed => "failed",
        ExecutionAttemptStatus::Timeout => "timeout",
        ExecutionAttemptStatus::Blocked => "blocked",
    }
}

fn execution_attempt_status_from_str(value: &str) -> Result<ExecutionAttemptStatus, DatabaseError> {
    match value {
        "running" => Ok(ExecutionAttemptStatus::Running),
        "succeeded" => Ok(ExecutionAttemptStatus::Succeeded),
        "failed" => Ok(ExecutionAttemptStatus::Failed),
        "timeout" => Ok(ExecutionAttemptStatus::Timeout),
        "blocked" => Ok(ExecutionAttemptStatus::Blocked),
        _ => Err(enum_parse_error("execution_attempt_status", value)),
    }
}

fn policy_decision_kind_to_str(value: PolicyDecisionKind) -> &'static str {
    match value {
        PolicyDecisionKind::Allow => "allow",
        PolicyDecisionKind::AllowWithLogging => "allow_with_logging",
        PolicyDecisionKind::RequireApproval => "require_approval",
        PolicyDecisionKind::Deny => "deny",
        PolicyDecisionKind::RequireMoreEvidence => "require_more_evidence",
        PolicyDecisionKind::Modify => "modify",
    }
}

fn policy_decision_kind_from_str(value: &str) -> Result<PolicyDecisionKind, DatabaseError> {
    match value {
        "allow" => Ok(PolicyDecisionKind::Allow),
        "allow_with_logging" => Ok(PolicyDecisionKind::AllowWithLogging),
        "require_approval" => Ok(PolicyDecisionKind::RequireApproval),
        "deny" => Ok(PolicyDecisionKind::Deny),
        "require_more_evidence" => Ok(PolicyDecisionKind::RequireMoreEvidence),
        "modify" => Ok(PolicyDecisionKind::Modify),
        _ => Err(enum_parse_error("policy_decision_kind", value)),
    }
}

fn opt_u64_to_i64(value: Option<u64>, field: &str) -> Result<Option<i64>, DatabaseError> {
    value.map(|v| u64_to_i64(v, field)).transpose()
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, DatabaseError> {
    if value > i64::MAX as u64 {
        return Err(DatabaseError::Serialization(format!(
            "autonomy {} out of range for BIGINT: {}",
            field, value
        )));
    }
    Ok(value as i64)
}

fn opt_i64_to_u64(value: Option<i64>, field: &str) -> Result<Option<u64>, DatabaseError> {
    value.map(|v| i64_to_u64(v, field)).transpose()
}

fn i64_to_u64(value: i64, field: &str) -> Result<u64, DatabaseError> {
    if value < 0 {
        return Err(DatabaseError::Serialization(format!(
            "autonomy {} cannot be negative: {}",
            field, value
        )));
    }
    Ok(value as u64)
}

fn json_value_to_string_vec(value: Value, field: &str) -> Result<Vec<String>, DatabaseError> {
    serde_json::from_value(value).map_err(|e| {
        DatabaseError::Serialization(format!("invalid autonomy {} JSON value: {}", field, e))
    })
}

fn row_to_goal(row: &Row) -> Result<Goal, DatabaseError> {
    let status: String = row.get("status");
    let risk_class: String = row.get("risk_class");
    let source: String = row.get("source");

    Ok(Goal {
        id: row.get("id"),
        owner_user_id: row.get("owner_user_id"),
        channel: row.get("channel"),
        thread_id: row.get("thread_id"),
        title: row.get("title"),
        intent: row.get("intent"),
        priority: row.get("priority"),
        status: goal_status_from_str(&status)?,
        risk_class: goal_risk_class_from_str(&risk_class)?,
        acceptance_criteria: row.get("acceptance_criteria"),
        constraints: row.get("constraints"),
        source: goal_source_from_str(&source)?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        completed_at: row.get("completed_at"),
    })
}

fn row_to_plan(row: &Row) -> Result<Plan, DatabaseError> {
    let status: String = row.get("status");
    let planner_kind: String = row.get("planner_kind");
    let estimated_time_secs: Option<i64> = row.get("estimated_time_secs");

    Ok(Plan {
        id: row.get("id"),
        goal_id: row.get("goal_id"),
        revision: row.get("revision"),
        status: plan_status_from_str(&status)?,
        planner_kind: planner_kind_from_str(&planner_kind)?,
        source_action_plan: row.get("source_action_plan"),
        assumptions: row.get("assumptions"),
        confidence: row.get("confidence"),
        estimated_cost: row.get("estimated_cost"),
        estimated_time_secs: opt_i64_to_u64(estimated_time_secs, "estimated_time_secs")?,
        summary: row.get("summary"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

#[allow(dead_code)]
fn row_to_plan_step(row: &Row) -> Result<PlanStep, DatabaseError> {
    let kind: String = row.get("kind");
    let status: String = row.get("status");

    Ok(PlanStep {
        id: row.get("id"),
        plan_id: row.get("plan_id"),
        sequence_num: row.get("sequence_num"),
        kind: plan_step_kind_from_str(&kind)?,
        status: plan_step_status_from_str(&status)?,
        title: row.get("title"),
        description: row.get("description"),
        tool_candidates: row.get("tool_candidates"),
        inputs: row.get("inputs"),
        preconditions: row.get("preconditions"),
        postconditions: row.get("postconditions"),
        rollback: row.get("rollback"),
        policy_requirements: row.get("policy_requirements"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn row_to_execution_attempt(row: &Row) -> Result<ExecutionAttempt, DatabaseError> {
    let status: String = row.get("status");

    Ok(ExecutionAttempt {
        id: row.get("id"),
        goal_id: row.get("goal_id"),
        plan_id: row.get("plan_id"),
        plan_step_id: row.get("plan_step_id"),
        job_id: row.get("job_id"),
        thread_id: row.get("thread_id"),
        user_id: row.get("user_id"),
        channel: row.get("channel"),
        tool_name: row.get("tool_name"),
        tool_call_id: row.get("tool_call_id"),
        tool_args: row.get("tool_args"),
        status: execution_attempt_status_from_str(&status)?,
        failure_class: row.get("failure_class"),
        retry_count: row.get("retry_count"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        elapsed_ms: row.get("elapsed_ms"),
        result_summary: row.get("result_summary"),
        error_preview: row.get("error_preview"),
    })
}

fn row_to_policy_decision(row: &Row) -> Result<PolicyDecision, DatabaseError> {
    let decision: String = row.get("decision");
    let reason_codes: Value = row.get("reason_codes");

    Ok(PolicyDecision {
        id: row.get("id"),
        goal_id: row.get("goal_id"),
        plan_id: row.get("plan_id"),
        plan_step_id: row.get("plan_step_id"),
        execution_attempt_id: row.get("execution_attempt_id"),
        user_id: row.get("user_id"),
        channel: row.get("channel"),
        tool_name: row.get("tool_name"),
        tool_call_id: row.get("tool_call_id"),
        action_kind: row.get("action_kind"),
        decision: policy_decision_kind_from_str(&decision)?,
        reason_codes: json_value_to_string_vec(reason_codes, "reason_codes")?,
        risk_score: row.get("risk_score"),
        confidence: row.get("confidence"),
        requires_approval: row.get("requires_approval"),
        auto_approved: row.get("auto_approved"),
        evidence_required: row.get("evidence_required"),
        created_at: row.get("created_at"),
    })
}

// ==================== ConversationStore ====================

#[async_trait]
impl ConversationStore for PgBackend {
    async fn create_conversation(
        &self,
        channel: &str,
        user_id: &str,
        thread_id: Option<&str>,
    ) -> Result<Uuid, DatabaseError> {
        self.store
            .create_conversation(channel, user_id, thread_id)
            .await
    }

    async fn touch_conversation(&self, id: Uuid) -> Result<(), DatabaseError> {
        self.store.touch_conversation(id).await
    }

    async fn add_conversation_message(
        &self,
        conversation_id: Uuid,
        role: &str,
        content: &str,
    ) -> Result<Uuid, DatabaseError> {
        self.store
            .add_conversation_message(conversation_id, role, content)
            .await
    }

    async fn ensure_conversation(
        &self,
        id: Uuid,
        channel: &str,
        user_id: &str,
        thread_id: Option<&str>,
    ) -> Result<(), DatabaseError> {
        self.store
            .ensure_conversation(id, channel, user_id, thread_id)
            .await
    }

    async fn list_conversations_with_preview(
        &self,
        user_id: &str,
        channel: &str,
        limit: i64,
    ) -> Result<Vec<ConversationSummary>, DatabaseError> {
        self.store
            .list_conversations_with_preview(user_id, channel, limit)
            .await
    }

    async fn get_or_create_assistant_conversation(
        &self,
        user_id: &str,
        channel: &str,
    ) -> Result<Uuid, DatabaseError> {
        self.store
            .get_or_create_assistant_conversation(user_id, channel)
            .await
    }

    async fn create_conversation_with_metadata(
        &self,
        channel: &str,
        user_id: &str,
        metadata: &serde_json::Value,
    ) -> Result<Uuid, DatabaseError> {
        self.store
            .create_conversation_with_metadata(channel, user_id, metadata)
            .await
    }

    async fn list_conversation_messages_paginated(
        &self,
        conversation_id: Uuid,
        before: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<(Vec<ConversationMessage>, bool), DatabaseError> {
        self.store
            .list_conversation_messages_paginated(conversation_id, before, limit)
            .await
    }

    async fn update_conversation_metadata_field(
        &self,
        id: Uuid,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        self.store
            .update_conversation_metadata_field(id, key, value)
            .await
    }

    async fn get_conversation_metadata(
        &self,
        id: Uuid,
    ) -> Result<Option<serde_json::Value>, DatabaseError> {
        self.store.get_conversation_metadata(id).await
    }

    async fn list_conversation_messages(
        &self,
        conversation_id: Uuid,
    ) -> Result<Vec<ConversationMessage>, DatabaseError> {
        self.store.list_conversation_messages(conversation_id).await
    }

    async fn conversation_belongs_to_user(
        &self,
        conversation_id: Uuid,
        user_id: &str,
    ) -> Result<bool, DatabaseError> {
        self.store
            .conversation_belongs_to_user(conversation_id, user_id)
            .await
    }

    async fn delete_conversation_for_user(
        &self,
        conversation_id: Uuid,
        user_id: &str,
    ) -> Result<bool, DatabaseError> {
        self.store
            .delete_conversation_for_user(conversation_id, user_id)
            .await
    }

    async fn delete_all_conversations_for_user_channel(
        &self,
        user_id: &str,
        channel: &str,
    ) -> Result<u64, DatabaseError> {
        self.store
            .delete_all_conversations_for_user_channel(user_id, channel)
            .await
    }
}

// ==================== JobStore ====================

#[async_trait]
impl JobStore for PgBackend {
    async fn save_job(&self, ctx: &JobContext) -> Result<(), DatabaseError> {
        self.store.save_job(ctx).await
    }

    async fn get_job(&self, id: Uuid) -> Result<Option<JobContext>, DatabaseError> {
        self.store.get_job(id).await
    }

    async fn update_job_status(
        &self,
        id: Uuid,
        status: JobState,
        failure_reason: Option<&str>,
    ) -> Result<(), DatabaseError> {
        self.store
            .update_job_status(id, status, failure_reason)
            .await
    }

    async fn mark_job_stuck(&self, id: Uuid) -> Result<(), DatabaseError> {
        self.store.mark_job_stuck(id).await
    }

    async fn get_stuck_jobs(&self) -> Result<Vec<Uuid>, DatabaseError> {
        self.store.get_stuck_jobs().await
    }

    async fn save_action(&self, job_id: Uuid, action: &ActionRecord) -> Result<(), DatabaseError> {
        self.store.save_action(job_id, action).await
    }

    async fn get_job_actions(&self, job_id: Uuid) -> Result<Vec<ActionRecord>, DatabaseError> {
        self.store.get_job_actions(job_id).await
    }

    async fn record_llm_call(&self, record: &LlmCallRecord<'_>) -> Result<Uuid, DatabaseError> {
        self.store.record_llm_call(record).await
    }

    async fn save_estimation_snapshot(
        &self,
        job_id: Uuid,
        category: &str,
        tool_names: &[String],
        estimated_cost: Decimal,
        estimated_time_secs: i32,
        estimated_value: Decimal,
    ) -> Result<Uuid, DatabaseError> {
        self.store
            .save_estimation_snapshot(
                job_id,
                category,
                tool_names,
                estimated_cost,
                estimated_time_secs,
                estimated_value,
            )
            .await
    }

    async fn update_estimation_actuals(
        &self,
        id: Uuid,
        actual_cost: Decimal,
        actual_time_secs: i32,
        actual_value: Option<Decimal>,
    ) -> Result<(), DatabaseError> {
        self.store
            .update_estimation_actuals(id, actual_cost, actual_time_secs, actual_value)
            .await
    }

    async fn find_recurring_job_patterns(
        &self,
        threshold: i64,
        limit: i64,
    ) -> Result<Vec<String>, DatabaseError> {
        self.store
            .find_recurring_job_patterns(threshold, limit)
            .await
    }

    async fn upsert_reflex_pattern(
        &self,
        normalized_pattern: &str,
        compiled_tool_name: &str,
    ) -> Result<(), DatabaseError> {
        self.store
            .upsert_reflex_pattern(normalized_pattern, compiled_tool_name)
            .await
    }

    async fn find_reflex_tool_for_pattern(
        &self,
        normalized_pattern: &str,
    ) -> Result<Option<String>, DatabaseError> {
        self.store
            .find_reflex_tool_for_pattern(normalized_pattern)
            .await
    }

    async fn bump_reflex_pattern_hit(&self, normalized_pattern: &str) -> Result<(), DatabaseError> {
        self.store.bump_reflex_pattern_hit(normalized_pattern).await
    }
}

// ==================== SandboxStore ====================

#[async_trait]
impl SandboxStore for PgBackend {
    async fn save_sandbox_job(&self, job: &SandboxJobRecord) -> Result<(), DatabaseError> {
        self.store.save_sandbox_job(job).await
    }

    async fn get_sandbox_job(&self, id: Uuid) -> Result<Option<SandboxJobRecord>, DatabaseError> {
        self.store.get_sandbox_job(id).await
    }

    async fn list_sandbox_jobs(&self) -> Result<Vec<SandboxJobRecord>, DatabaseError> {
        self.store.list_sandbox_jobs().await
    }

    async fn update_sandbox_job_status(
        &self,
        id: Uuid,
        status: &str,
        success: Option<bool>,
        message: Option<&str>,
        started_at: Option<DateTime<Utc>>,
        completed_at: Option<DateTime<Utc>>,
    ) -> Result<(), DatabaseError> {
        self.store
            .update_sandbox_job_status(id, status, success, message, started_at, completed_at)
            .await
    }

    async fn cleanup_stale_sandbox_jobs(&self) -> Result<u64, DatabaseError> {
        self.store.cleanup_stale_sandbox_jobs().await
    }

    async fn sandbox_job_summary(&self) -> Result<SandboxJobSummary, DatabaseError> {
        self.store.sandbox_job_summary().await
    }

    async fn list_sandbox_jobs_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<SandboxJobRecord>, DatabaseError> {
        self.store.list_sandbox_jobs_for_user(user_id).await
    }

    async fn sandbox_job_summary_for_user(
        &self,
        user_id: &str,
    ) -> Result<SandboxJobSummary, DatabaseError> {
        self.store.sandbox_job_summary_for_user(user_id).await
    }

    async fn sandbox_job_belongs_to_user(
        &self,
        job_id: Uuid,
        user_id: &str,
    ) -> Result<bool, DatabaseError> {
        self.store
            .sandbox_job_belongs_to_user(job_id, user_id)
            .await
    }

    async fn update_sandbox_job_mode(&self, id: Uuid, mode: &str) -> Result<(), DatabaseError> {
        self.store.update_sandbox_job_mode(id, mode).await
    }

    async fn get_sandbox_job_mode(&self, id: Uuid) -> Result<Option<String>, DatabaseError> {
        self.store.get_sandbox_job_mode(id).await
    }

    async fn save_job_event(
        &self,
        job_id: Uuid,
        event_type: &str,
        data: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        self.store.save_job_event(job_id, event_type, data).await
    }

    async fn list_job_events(
        &self,
        job_id: Uuid,
        limit: Option<i64>,
    ) -> Result<Vec<JobEventRecord>, DatabaseError> {
        self.store.list_job_events(job_id, limit).await
    }
}

// ==================== RoutineStore ====================

#[async_trait]
impl RoutineStore for PgBackend {
    async fn create_routine(&self, routine: &Routine) -> Result<(), DatabaseError> {
        self.store.create_routine(routine).await
    }

    async fn get_routine(&self, id: Uuid) -> Result<Option<Routine>, DatabaseError> {
        self.store.get_routine(id).await
    }

    async fn get_routine_by_name(
        &self,
        user_id: &str,
        name: &str,
    ) -> Result<Option<Routine>, DatabaseError> {
        self.store.get_routine_by_name(user_id, name).await
    }

    async fn list_routines(&self, user_id: &str) -> Result<Vec<Routine>, DatabaseError> {
        self.store.list_routines(user_id).await
    }

    async fn list_event_routines(&self) -> Result<Vec<Routine>, DatabaseError> {
        self.store.list_event_routines().await
    }

    async fn list_due_cron_routines(&self) -> Result<Vec<Routine>, DatabaseError> {
        self.store.list_due_cron_routines().await
    }

    async fn update_routine(&self, routine: &Routine) -> Result<(), DatabaseError> {
        self.store.update_routine(routine).await
    }

    async fn update_routine_runtime(
        &self,
        id: Uuid,
        last_run_at: DateTime<Utc>,
        next_fire_at: Option<DateTime<Utc>>,
        run_count: u64,
        consecutive_failures: u32,
        state: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        self.store
            .update_routine_runtime(
                id,
                last_run_at,
                next_fire_at,
                run_count,
                consecutive_failures,
                state,
            )
            .await
    }

    async fn delete_routine(&self, id: Uuid) -> Result<bool, DatabaseError> {
        self.store.delete_routine(id).await
    }

    async fn create_routine_run(&self, run: &RoutineRun) -> Result<(), DatabaseError> {
        self.store.create_routine_run(run).await
    }

    async fn complete_routine_run(
        &self,
        id: Uuid,
        status: RunStatus,
        result_summary: Option<&str>,
        tokens_used: Option<i32>,
    ) -> Result<(), DatabaseError> {
        self.store
            .complete_routine_run(id, status, result_summary, tokens_used)
            .await
    }

    async fn list_routine_runs(
        &self,
        routine_id: Uuid,
        limit: i64,
    ) -> Result<Vec<RoutineRun>, DatabaseError> {
        self.store.list_routine_runs(routine_id, limit).await
    }

    async fn count_running_routine_runs(&self, routine_id: Uuid) -> Result<i64, DatabaseError> {
        self.store.count_running_routine_runs(routine_id).await
    }
}

// ==================== ToolFailureStore ====================

#[async_trait]
impl ToolFailureStore for PgBackend {
    async fn record_tool_failure(
        &self,
        tool_name: &str,
        error_message: &str,
    ) -> Result<(), DatabaseError> {
        self.store
            .record_tool_failure(tool_name, error_message)
            .await
    }

    async fn get_broken_tools(&self, threshold: i32) -> Result<Vec<BrokenTool>, DatabaseError> {
        self.store.get_broken_tools(threshold).await
    }

    async fn mark_tool_repaired(&self, tool_name: &str) -> Result<(), DatabaseError> {
        self.store.mark_tool_repaired(tool_name).await
    }

    async fn increment_repair_attempts(&self, tool_name: &str) -> Result<(), DatabaseError> {
        self.store.increment_repair_attempts(tool_name).await
    }
}

// ==================== SettingsStore ====================

#[async_trait]
impl SettingsStore for PgBackend {
    async fn get_setting(
        &self,
        user_id: &str,
        key: &str,
    ) -> Result<Option<serde_json::Value>, DatabaseError> {
        self.store.get_setting(user_id, key).await
    }

    async fn get_setting_full(
        &self,
        user_id: &str,
        key: &str,
    ) -> Result<Option<SettingRow>, DatabaseError> {
        self.store.get_setting_full(user_id, key).await
    }

    async fn set_setting(
        &self,
        user_id: &str,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        self.store.set_setting(user_id, key, value).await
    }

    async fn delete_setting(&self, user_id: &str, key: &str) -> Result<bool, DatabaseError> {
        self.store.delete_setting(user_id, key).await
    }

    async fn list_settings(&self, user_id: &str) -> Result<Vec<SettingRow>, DatabaseError> {
        self.store.list_settings(user_id).await
    }

    async fn get_all_settings(
        &self,
        user_id: &str,
    ) -> Result<HashMap<String, serde_json::Value>, DatabaseError> {
        self.store.get_all_settings(user_id).await
    }

    async fn set_all_settings(
        &self,
        user_id: &str,
        settings: &HashMap<String, serde_json::Value>,
    ) -> Result<(), DatabaseError> {
        self.store.set_all_settings(user_id, settings).await
    }

    async fn has_settings(&self, user_id: &str) -> Result<bool, DatabaseError> {
        self.store.has_settings(user_id).await
    }
}

// ==================== WorkspaceStore ====================

#[async_trait]
impl WorkspaceStore for PgBackend {
    async fn get_document_by_path(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        path: &str,
    ) -> Result<MemoryDocument, WorkspaceError> {
        self.repo
            .get_document_by_path(user_id, agent_id, path)
            .await
    }

    async fn get_document_by_id(&self, id: Uuid) -> Result<MemoryDocument, WorkspaceError> {
        self.repo.get_document_by_id(id).await
    }

    async fn get_or_create_document_by_path(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        path: &str,
    ) -> Result<MemoryDocument, WorkspaceError> {
        self.repo
            .get_or_create_document_by_path(user_id, agent_id, path)
            .await
    }

    async fn update_document(&self, id: Uuid, content: &str) -> Result<(), WorkspaceError> {
        self.repo.update_document(id, content).await
    }

    async fn delete_document_by_path(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        path: &str,
    ) -> Result<(), WorkspaceError> {
        self.repo
            .delete_document_by_path(user_id, agent_id, path)
            .await
    }

    async fn list_directory(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        directory: &str,
    ) -> Result<Vec<WorkspaceEntry>, WorkspaceError> {
        self.repo.list_directory(user_id, agent_id, directory).await
    }

    async fn list_all_paths(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
    ) -> Result<Vec<String>, WorkspaceError> {
        self.repo.list_all_paths(user_id, agent_id).await
    }

    async fn list_documents(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
    ) -> Result<Vec<MemoryDocument>, WorkspaceError> {
        self.repo.list_documents(user_id, agent_id).await
    }

    async fn delete_chunks(&self, document_id: Uuid) -> Result<(), WorkspaceError> {
        self.repo.delete_chunks(document_id).await
    }

    async fn insert_chunk(
        &self,
        document_id: Uuid,
        chunk_index: i32,
        content: &str,
        embedding: Option<&[f32]>,
    ) -> Result<Uuid, WorkspaceError> {
        self.repo
            .insert_chunk(document_id, chunk_index, content, embedding)
            .await
    }

    async fn update_chunk_embedding(
        &self,
        chunk_id: Uuid,
        embedding: &[f32],
    ) -> Result<(), WorkspaceError> {
        self.repo.update_chunk_embedding(chunk_id, embedding).await
    }

    async fn get_chunks_without_embeddings(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<MemoryChunk>, WorkspaceError> {
        self.repo
            .get_chunks_without_embeddings(user_id, agent_id, limit)
            .await
    }

    async fn hybrid_search(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        query: &str,
        embedding: Option<&[f32]>,
        config: &SearchConfig,
    ) -> Result<Vec<SearchResult>, WorkspaceError> {
        self.repo
            .hybrid_search(user_id, agent_id, query, embedding, config)
            .await
    }
}

// ==================== AstGraphStore ====================

use crate::db::{AstGraphStore, StoredAstEdge, StoredAstNode};

#[async_trait]
impl AstGraphStore for PgBackend {
    async fn delete_ast_nodes(&self, document_id: Uuid) -> Result<(), WorkspaceError> {
        let client = self
            .store
            .pool()
            .get()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Pool error: {}", e),
            })?;
        client
            .execute(
                "DELETE FROM memory_ast_nodes WHERE document_id = $1",
                &[&document_id],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Delete AST nodes failed: {}", e),
            })?;
        Ok(())
    }

    async fn insert_ast_node(
        &self,
        document_id: Uuid,
        node_type: &str,
        name: &str,
        content_preview: &str,
        start_byte: i64,
        end_byte: i64,
    ) -> Result<Uuid, WorkspaceError> {
        let client = self
            .store
            .pool()
            .get()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Pool error: {}", e),
            })?;
        let id = Uuid::new_v4();
        client
            .execute(
                r#"INSERT INTO memory_ast_nodes
                   (id, document_id, node_type, name, content_preview, start_byte, end_byte)
                   VALUES ($1, $2, $3, $4, $5, $6, $7)
                   ON CONFLICT (document_id, name) DO UPDATE
                   SET node_type = EXCLUDED.node_type,
                       content_preview = EXCLUDED.content_preview,
                       start_byte = EXCLUDED.start_byte,
                       end_byte = EXCLUDED.end_byte"#,
                &[
                    &id,
                    &document_id,
                    &node_type,
                    &name,
                    &content_preview,
                    &(start_byte as i32),
                    &(end_byte as i32),
                ],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Insert AST node failed: {}", e),
            })?;
        Ok(id)
    }

    async fn insert_ast_edge(
        &self,
        source_node_id: Uuid,
        target_node_id: Uuid,
        edge_type: &str,
    ) -> Result<Uuid, WorkspaceError> {
        let client = self
            .store
            .pool()
            .get()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Pool error: {}", e),
            })?;
        let id = Uuid::new_v4();
        client
            .execute(
                r#"INSERT INTO memory_ast_edges (id, source_node_id, target_node_id, edge_type)
                   VALUES ($1, $2, $3, $4)"#,
                &[&id, &source_node_id, &target_node_id, &edge_type],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Insert AST edge failed: {}", e),
            })?;
        Ok(id)
    }

    async fn get_ast_nodes(&self, document_id: Uuid) -> Result<Vec<StoredAstNode>, WorkspaceError> {
        let client = self
            .store
            .pool()
            .get()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Pool error: {}", e),
            })?;
        let rows = client
            .query(
                r#"SELECT id, document_id, node_type, name, content_preview, start_byte, end_byte
                   FROM memory_ast_nodes WHERE document_id = $1"#,
                &[&document_id],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query AST nodes failed: {}", e),
            })?;
        Ok(rows
            .iter()
            .map(|row| {
                let start: i32 = row.get("start_byte");
                let end: i32 = row.get("end_byte");
                StoredAstNode {
                    id: row.get("id"),
                    document_id: row.get("document_id"),
                    node_type: row.get("node_type"),
                    name: row.get("name"),
                    content_preview: row.get("content_preview"),
                    start_byte: start as i64,
                    end_byte: end as i64,
                }
            })
            .collect())
    }

    async fn get_ast_edges_from(
        &self,
        source_node_id: Uuid,
    ) -> Result<Vec<StoredAstEdge>, WorkspaceError> {
        let client = self
            .store
            .pool()
            .get()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Pool error: {}", e),
            })?;
        let rows = client
            .query(
                "SELECT id, source_node_id, target_node_id, edge_type FROM memory_ast_edges WHERE source_node_id = $1",
                &[&source_node_id],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query AST edges failed: {}", e),
            })?;
        Ok(rows
            .iter()
            .map(|row| StoredAstEdge {
                id: row.get("id"),
                source_node_id: row.get("source_node_id"),
                target_node_id: row.get("target_node_id"),
                edge_type: row.get("edge_type"),
            })
            .collect())
    }

    async fn find_ast_nodes_by_name(
        &self,
        name: &str,
    ) -> Result<Vec<StoredAstNode>, WorkspaceError> {
        let client = self
            .store
            .pool()
            .get()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Pool error: {}", e),
            })?;
        let rows = client
            .query(
                r#"SELECT id, document_id, node_type, name, content_preview, start_byte, end_byte
                   FROM memory_ast_nodes WHERE name = $1"#,
                &[&name],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query AST nodes by name failed: {}", e),
            })?;
        Ok(rows
            .iter()
            .map(|row| {
                let start: i32 = row.get("start_byte");
                let end: i32 = row.get("end_byte");
                StoredAstNode {
                    id: row.get("id"),
                    document_id: row.get("document_id"),
                    node_type: row.get("node_type"),
                    name: row.get("name"),
                    content_preview: row.get("content_preview"),
                    start_byte: start as i64,
                    end_byte: end as i64,
                }
            })
            .collect())
    }
}
