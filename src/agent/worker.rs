//! Per-job worker execution.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::future::join_all;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::agent::memory_plane::{MemoryRecordWriteRequest, persist_memory_record_best_effort};
use crate::agent::memory_retrieval_composer::{
    MemoryRetrievalComposer, MemoryRetrievalRequest, RetrievalTaskClass,
};
use crate::agent::memory_write_policy::MemoryWriteIntent;
use crate::agent::planner_v1::PlannerV1;
use crate::agent::policy_engine::{
    ApprovalPolicyOutcome, HookToolPolicyOutcome, evaluate_tool_call_hook,
    evaluate_worker_tool_approval,
};
use crate::agent::replanner_v1::{ReplanBudgets, ReplanReason, ReplanRuntimeState, ReplannerV1};
use crate::agent::scheduler::WorkerMessage;
use crate::agent::task::TaskOutput;
use crate::agent::verifier_v1::{VerificationNextAction, VerificationRequest, VerifierV1};
use crate::agent::{
    Evidence as AutonomyEvidence, EvidenceKind as AutonomyEvidenceKind,
    ExecutionAttempt as AutonomyExecutionAttempt,
    ExecutionAttemptStatus as AutonomyExecutionAttemptStatus, Goal, GoalRiskClass, GoalSource,
    GoalStatus, MemorySourceKind, Plan, PlanStatus, PlanStep, PlanStepKind, PlanStepStatus,
    PlanVerification as AutonomyPlanVerification,
    PlanVerificationStatus as AutonomyPlanVerificationStatus, PlannerKind,
    PolicyDecision as AutonomyPolicyDecision, PolicyDecisionKind as AutonomyPolicyDecisionKind,
};
use crate::context::{ContextManager, JobContext, JobState};
use crate::db::Database;
use crate::error::{Error, ToolError};
use crate::hooks::HookRegistry;
use crate::llm::{
    ActionPlan, ChatMessage, LlmProvider, Reasoning, ReasoningContext, RespondResult, ToolSelection,
};
use crate::safety::SafetyLayer;
use crate::tools::ToolRegistry;

/// Shared dependencies for worker execution.
///
/// This bundles the dependencies that are shared across all workers,
/// reducing the number of arguments to `Worker::new`.
#[derive(Clone)]
pub struct WorkerDeps {
    pub context_manager: Arc<ContextManager>,
    pub llm: Arc<dyn LlmProvider>,
    pub safety: Arc<SafetyLayer>,
    pub tools: Arc<ToolRegistry>,
    pub store: Option<Arc<dyn Database>>,
    pub hooks: Arc<HookRegistry>,
    pub timeout: Duration,
    pub use_planning: bool,
    pub autonomy_policy_engine_v1: bool,
    pub autonomy_verifier_v1: bool,
    pub autonomy_replanner_v1: bool,
    pub autonomy_memory_plane_v2: bool,
    pub autonomy_memory_retrieval_v2: bool,
}

/// Worker that executes a single job.
pub struct Worker {
    job_id: Uuid,
    deps: WorkerDeps,
}

/// Result of a tool execution with metadata for context building.
struct ToolExecResult {
    result: Result<String, Error>,
}

struct PersistedPlanRun {
    goal_id: Uuid,
    plan_id: Uuid,
    step_ids: Vec<Uuid>,
}

enum PlanExecutionOutcome {
    Finished,
    ReplanRequested {
        reason: ReplanReason,
        detail: String,
    },
}

fn classify_replan_reason_for_step_error(err: &Error) -> Option<ReplanReason> {
    match err {
        Error::Tool(ToolError::AuthRequired { .. }) => Some(ReplanReason::PolicyDenied),
        Error::Tool(ToolError::ExecutionFailed { reason, .. })
            if reason.starts_with("Blocked by hook:") =>
        {
            Some(ReplanReason::PolicyDenied)
        }
        Error::Tool(ToolError::NotFound { .. }) => Some(ReplanReason::ToolUnavailable),
        Error::Tool(ToolError::Timeout { .. }) => Some(ReplanReason::StepFailure),
        Error::Tool(ToolError::ExecutionFailed { .. }) => Some(ReplanReason::StepFailure),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
struct StepEvidenceClassification {
    kind: AutonomyEvidenceKind,
    check_kind: &'static str,
    command_hint: Option<&'static str>,
}

fn classify_step_evidence(
    tool_name: &str,
    parameters: &serde_json::Value,
    reasoning: &str,
) -> StepEvidenceClassification {
    let tool_name_lower = tool_name.to_ascii_lowercase();
    let reasoning_lower = reasoning.to_ascii_lowercase();
    let command_lower = parameters
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let looks_like_test = tool_name_lower.contains("test")
        || command_lower.contains("cargo test")
        || command_lower.contains("pytest")
        || command_lower.contains("npm test")
        || command_lower.contains("pnpm test")
        || command_lower.contains("yarn test")
        || reasoning_lower.contains("test");
    if looks_like_test {
        return StepEvidenceClassification {
            kind: AutonomyEvidenceKind::TestRun,
            check_kind: "test",
            command_hint: (!command_lower.is_empty()).then_some("command"),
        };
    }

    let looks_like_lint = tool_name_lower.contains("lint")
        || command_lower.contains("cargo clippy")
        || command_lower.contains("cargo check")
        || command_lower.contains("eslint")
        || command_lower.contains("ruff")
        || command_lower.contains("golangci-lint")
        || reasoning_lower.contains("lint")
        || reasoning_lower.contains("compile check");
    if looks_like_lint {
        return StepEvidenceClassification {
            kind: AutonomyEvidenceKind::TestRun,
            check_kind: "lint_or_check",
            command_hint: (!command_lower.is_empty()).then_some("command"),
        };
    }

    let looks_like_diff = tool_name_lower.contains("diff")
        || command_lower.contains("git diff")
        || command_lower.contains("diff --")
        || reasoning_lower.contains("diff")
        || reasoning_lower.contains("patch");
    if looks_like_diff {
        return StepEvidenceClassification {
            kind: AutonomyEvidenceKind::FileDiff,
            check_kind: "diff",
            command_hint: (!command_lower.is_empty()).then_some("command"),
        };
    }

    if tool_name_lower.contains("shell")
        || tool_name_lower.contains("command")
        || tool_name_lower.contains("exec")
    {
        return StepEvidenceClassification {
            kind: AutonomyEvidenceKind::CommandOutput,
            check_kind: "command_output",
            command_hint: (!command_lower.is_empty()).then_some("command"),
        };
    }

    StepEvidenceClassification {
        kind: AutonomyEvidenceKind::ToolResult,
        check_kind: "tool_result",
        command_hint: None,
    }
}

fn persist_worker_policy_decision_best_effort(
    deps: &WorkerDeps,
    job_ctx: &JobContext,
    tool_name: &str,
    tool_call_id: Option<&str>,
    decision: AutonomyPolicyDecisionKind,
    reason_codes: Vec<String>,
    requires_approval: bool,
    auto_approved: Option<bool>,
    action_kind: &str,
    evidence_required: serde_json::Value,
) {
    let Some(store) = deps.store.clone() else {
        return;
    };
    if deps.autonomy_memory_plane_v2 {
        persist_memory_record_best_effort(
            store.clone(),
            MemoryRecordWriteRequest {
                owner_user_id: job_ctx.user_id.clone(),
                goal_id: job_ctx.autonomy_goal_id,
                plan_id: job_ctx.autonomy_plan_id,
                plan_step_id: job_ctx.autonomy_plan_step_id,
                job_id: Some(job_ctx.job_id),
                thread_id: None,
                source_kind: MemorySourceKind::WorkerPlanExecution,
                category: "policy_decision".to_string(),
                title: format!("Worker policy {:?} for {}", decision, tool_name),
                summary: format!(
                    "Worker policy {:?} on tool '{}' ({})",
                    decision, tool_name, action_kind
                ),
                payload: serde_json::json!({
                    "tool_name": tool_name,
                    "tool_call_id": tool_call_id,
                    "decision": decision,
                    "reason_codes": reason_codes.clone(),
                    "requires_approval": requires_approval,
                    "auto_approved": auto_approved,
                    "action_kind": action_kind,
                    "evidence_required": evidence_required,
                }),
                provenance: serde_json::json!({
                    "source": "worker.persist_worker_policy_decision_best_effort",
                    "timestamp": Utc::now(),
                }),
                desired_memory_type: None,
                confidence_hint: None,
                sensitivity_hint: None,
                ttl_secs_hint: None,
                high_impact: false,
                intent: MemoryWriteIntent::PolicyOutcome {
                    denied: matches!(decision, AutonomyPolicyDecisionKind::Deny),
                    requires_approval,
                },
            },
        );
    }
    let record = AutonomyPolicyDecision {
        id: Uuid::new_v4(),
        goal_id: job_ctx.autonomy_goal_id,
        plan_id: job_ctx.autonomy_plan_id,
        plan_step_id: job_ctx.autonomy_plan_step_id,
        execution_attempt_id: None,
        user_id: job_ctx.user_id.clone(),
        channel: "worker".to_string(),
        tool_name: Some(tool_name.to_string()),
        tool_call_id: tool_call_id.map(ToString::to_string),
        action_kind: action_kind.to_string(),
        decision,
        reason_codes,
        risk_score: None,
        confidence: None,
        requires_approval,
        auto_approved,
        evidence_required,
        created_at: Utc::now(),
    };
    tokio::spawn(async move {
        if let Err(e) = store.record_policy_decision(&record).await {
            tracing::warn!("Failed to persist worker autonomy policy decision: {}", e);
        }
    });
}

impl Worker {
    /// Create a new worker for a specific job.
    pub fn new(job_id: Uuid, deps: WorkerDeps) -> Self {
        Self { job_id, deps }
    }

    // Convenience accessors to avoid deps.field everywhere
    fn context_manager(&self) -> &Arc<ContextManager> {
        &self.deps.context_manager
    }

    fn llm(&self) -> &Arc<dyn LlmProvider> {
        &self.deps.llm
    }

    fn safety(&self) -> &Arc<SafetyLayer> {
        &self.deps.safety
    }

    fn tools(&self) -> &Arc<ToolRegistry> {
        &self.deps.tools
    }

    fn store(&self) -> Option<&Arc<dyn Database>> {
        self.deps.store.as_ref()
    }

    fn timeout(&self) -> Duration {
        self.deps.timeout
    }

    fn use_planning(&self) -> bool {
        self.deps.use_planning
    }

    fn autonomy_verifier_v1(&self) -> bool {
        self.deps.autonomy_verifier_v1
    }

    fn autonomy_memory_plane_v2(&self) -> bool {
        self.deps.autonomy_memory_plane_v2
    }

    fn autonomy_memory_retrieval_v2(&self) -> bool {
        self.deps.autonomy_memory_retrieval_v2
    }

    fn autonomy_replanner_v1(&self) -> bool {
        self.deps.autonomy_replanner_v1
    }

    async fn build_planner_memory_retrieval_context(
        &self,
        reason_ctx: &ReasoningContext,
        goal_id_hint: Option<Uuid>,
        plan_id_hint: Option<Uuid>,
    ) -> Option<String> {
        if !self.autonomy_memory_retrieval_v2() {
            return None;
        }
        let store = self.store()?.clone();
        let job_ctx = match self.context_manager().get_context(self.job_id).await {
            Ok(ctx) => ctx,
            Err(e) => {
                tracing::warn!(
                    job = %self.job_id,
                    "Memory retrieval composer skipped (job context load failed): {}",
                    e
                );
                return None;
            }
        };

        let task_class = MemoryRetrievalComposer::infer_task_class(
            job_ctx.category.as_deref(),
            &job_ctx.title,
            &job_ctx.description,
        );
        let query = match reason_ctx.job_description.as_deref() {
            Some(job_desc) if !job_desc.trim().is_empty() => {
                format!("{}\n{}", job_ctx.title, job_desc.trim())
            }
            _ => format!("{}\n{}", job_ctx.title, job_ctx.description),
        };

        let req = MemoryRetrievalRequest {
            user_id: job_ctx.user_id.clone(),
            goal_id: goal_id_hint.or(job_ctx.autonomy_goal_id),
            plan_id: plan_id_hint.or(job_ctx.autonomy_plan_id),
            job_id: Some(self.job_id),
            task_class: match task_class {
                RetrievalTaskClass::General => {
                    // Bias planned worker execution toward coding retrieval when
                    // no clearer task signal is available.
                    RetrievalTaskClass::Coding
                }
                other => other,
            },
            query,
            limit_budget: 12,
        };

        match MemoryRetrievalComposer::compose(store, req).await {
            Ok(bundle) if !bundle.prompt_context_block.trim().is_empty() => {
                tracing::debug!(
                    job = %self.job_id,
                    working_hits = bundle.working_context.len(),
                    episodic_hits = bundle.episodic_hits.len(),
                    semantic_hits = bundle.semantic_hits.len(),
                    playbooks = bundle.procedural_playbooks.len(),
                    "Built Memory Plane v2 retrieval context for planner"
                );
                Some(bundle.prompt_context_block)
            }
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(
                    job = %self.job_id,
                    "Memory retrieval composer failed (continuing without retrieval context): {}",
                    e
                );
                None
            }
        }
    }

    async fn persist_action_plan(
        &self,
        job_ctx: &crate::context::JobContext,
        plan: &ActionPlan,
    ) -> Option<PersistedPlanRun> {
        let store = self.store()?.clone();
        let now = Utc::now();
        let goal_id = Uuid::new_v4();
        let plan_id = Uuid::new_v4();

        let goal = Goal {
            id: goal_id,
            owner_user_id: job_ctx.user_id.clone(),
            channel: None,
            thread_id: None,
            title: job_ctx.title.clone(),
            intent: plan.goal.clone(),
            priority: 0,
            status: GoalStatus::Active,
            risk_class: GoalRiskClass::Medium,
            acceptance_criteria: serde_json::json!({}),
            constraints: serde_json::json!({}),
            source: GoalSource::UserRequest,
            created_at: now,
            updated_at: now,
            completed_at: None,
        };
        if let Err(e) = store.create_goal(&goal).await {
            tracing::warn!(
                job = %self.job_id,
                "Failed to persist autonomy goal scaffold: {}",
                e
            );
            return None;
        }

        let db_plan = Plan {
            id: plan_id,
            goal_id,
            revision: 1,
            status: PlanStatus::Ready,
            planner_kind: PlannerKind::ReasoningV1,
            source_action_plan: serde_json::to_value(plan).ok(),
            assumptions: serde_json::json!({}),
            confidence: plan.confidence,
            estimated_cost: plan.estimated_cost,
            estimated_time_secs: plan.estimated_time_secs,
            summary: Some(plan.goal.clone()),
            created_at: now,
            updated_at: now,
        };
        if let Err(e) = store.create_plan(&db_plan).await {
            tracing::warn!(
                job = %self.job_id,
                goal_id = %goal_id,
                "Failed to persist autonomy plan scaffold: {}",
                e
            );
            return None;
        }

        let mut step_ids = Vec::with_capacity(plan.actions.len());
        let steps: Vec<PlanStep> = plan
            .actions
            .iter()
            .enumerate()
            .map(|(i, action)| {
                let step_id = Uuid::new_v4();
                step_ids.push(step_id);
                PlanStep {
                    id: step_id,
                    plan_id,
                    sequence_num: (i as i32) + 1,
                    kind: PlanStepKind::ToolCall,
                    status: PlanStepStatus::Pending,
                    title: action.tool_name.clone(),
                    description: action.reasoning.clone(),
                    tool_candidates: serde_json::json!([
                        {
                            "tool_name": action.tool_name,
                        }
                    ]),
                    inputs: action.parameters.clone(),
                    preconditions: serde_json::json!([]),
                    postconditions: serde_json::json!([]),
                    rollback: None,
                    policy_requirements: serde_json::json!({}),
                    started_at: None,
                    completed_at: None,
                    created_at: now,
                    updated_at: now,
                }
            })
            .collect();

        if let Err(e) = store.create_plan_steps(&steps).await {
            tracing::warn!(
                job = %self.job_id,
                plan_id = %plan_id,
                "Failed to persist autonomy plan steps scaffold: {}",
                e
            );
            return None;
        }

        if let Err(e) = self
            .context_manager()
            .update_context(self.job_id, |ctx| {
                ctx.set_autonomy_links(Some(goal_id), Some(plan_id));
                ctx.set_autonomy_plan_step(None);
            })
            .await
        {
            tracing::warn!(
                job = %self.job_id,
                goal_id = %goal_id,
                plan_id = %plan_id,
                "Failed to attach autonomy linkage to job context: {}",
                e
            );
        }

        Some(PersistedPlanRun {
            goal_id,
            plan_id,
            step_ids,
        })
    }

    async fn persist_replanned_action_plan(
        &self,
        goal_id: Uuid,
        supersedes_plan_id: Uuid,
        replan_reason: ReplanReason,
        replan_detail: &str,
        plan: &ActionPlan,
    ) -> Option<PersistedPlanRun> {
        let store = self.store()?.clone();
        let now = Utc::now();
        let plan_id = Uuid::new_v4();

        let revision = match store.list_plans_for_goal(goal_id).await {
            Ok(plans) => plans.iter().map(|p| p.revision).max().unwrap_or(0) + 1,
            Err(e) => {
                tracing::warn!(
                    job = %self.job_id,
                    goal_id = %goal_id,
                    "Failed to load prior plans for revisioning, defaulting to revision=2: {}",
                    e
                );
                2
            }
        };

        let db_plan = Plan {
            id: plan_id,
            goal_id,
            revision,
            status: PlanStatus::Ready,
            planner_kind: PlannerKind::ReasoningV1,
            source_action_plan: serde_json::to_value(plan).ok(),
            assumptions: serde_json::json!({
                "replan": true,
                "supersedes_plan_id": supersedes_plan_id,
                "replan_reason": replan_reason.code(),
                "replan_detail": replan_detail.chars().take(500).collect::<String>(),
            }),
            confidence: plan.confidence,
            estimated_cost: plan.estimated_cost,
            estimated_time_secs: plan.estimated_time_secs,
            summary: Some(plan.goal.clone()),
            created_at: now,
            updated_at: now,
        };
        if let Err(e) = store.create_plan(&db_plan).await {
            tracing::warn!(
                job = %self.job_id,
                goal_id = %goal_id,
                plan_id = %plan_id,
                "Failed to persist autonomy replanned plan scaffold: {}",
                e
            );
            return None;
        }

        let mut step_ids = Vec::with_capacity(plan.actions.len());
        let steps: Vec<PlanStep> = plan
            .actions
            .iter()
            .enumerate()
            .map(|(i, action)| {
                let step_id = Uuid::new_v4();
                step_ids.push(step_id);
                PlanStep {
                    id: step_id,
                    plan_id,
                    sequence_num: (i as i32) + 1,
                    kind: PlanStepKind::ToolCall,
                    status: PlanStepStatus::Pending,
                    title: action.tool_name.clone(),
                    description: action.reasoning.clone(),
                    tool_candidates: serde_json::json!([
                        {
                            "tool_name": action.tool_name,
                        }
                    ]),
                    inputs: action.parameters.clone(),
                    preconditions: serde_json::json!([]),
                    postconditions: serde_json::json!([]),
                    rollback: None,
                    policy_requirements: serde_json::json!({}),
                    started_at: None,
                    completed_at: None,
                    created_at: now,
                    updated_at: now,
                }
            })
            .collect();

        if let Err(e) = store.create_plan_steps(&steps).await {
            tracing::warn!(
                job = %self.job_id,
                plan_id = %plan_id,
                "Failed to persist autonomy replanned plan steps scaffold: {}",
                e
            );
            return None;
        }

        if let Err(e) = self
            .context_manager()
            .update_context(self.job_id, |ctx| {
                ctx.set_autonomy_links(Some(goal_id), Some(plan_id));
                ctx.set_autonomy_plan_step(None);
            })
            .await
        {
            tracing::warn!(
                job = %self.job_id,
                goal_id = %goal_id,
                plan_id = %plan_id,
                "Failed to attach replanned autonomy linkage to job context: {}",
                e
            );
        }

        Some(PersistedPlanRun {
            goal_id,
            plan_id,
            step_ids,
        })
    }

    async fn set_plan_status_best_effort(&self, plan_id: Uuid, status: PlanStatus) {
        let Some(store) = self.store() else {
            return;
        };
        if let Err(e) = store.update_plan_status(plan_id, status).await {
            tracing::warn!(plan_id = %plan_id, "Failed to update plan status: {}", e);
        }
    }

    async fn set_goal_status_best_effort(&self, goal_id: Uuid, status: GoalStatus) {
        let Some(store) = self.store() else {
            return;
        };
        if let Err(e) = store.update_goal_status(goal_id, status).await {
            tracing::warn!(goal_id = %goal_id, "Failed to update goal status: {}", e);
        }
    }

    async fn set_plan_step_status_best_effort(&self, step_id: Uuid, status: PlanStepStatus) {
        let Some(store) = self.store() else {
            return;
        };
        if let Err(e) = store.update_plan_step_status(step_id, status).await {
            tracing::warn!(step_id = %step_id, "Failed to update plan step status: {}", e);
        }
    }

    async fn record_worker_execution_attempt_best_effort(
        &self,
        persisted: &PersistedPlanRun,
        step_index: usize,
        tool_name: &str,
        tool_args: &serde_json::Value,
        tool_call_id: &str,
        started_at: chrono::DateTime<Utc>,
        result: &Result<String, Error>,
        elapsed_ms: i64,
        retry_count: i32,
    ) {
        let Some(store) = self.store() else {
            return;
        };
        let step_id = match persisted.step_ids.get(step_index) {
            Some(id) => *id,
            None => return,
        };
        let now = Utc::now();
        let (status, failure_class, error_preview) = match result {
            Ok(_) => (AutonomyExecutionAttemptStatus::Succeeded, None, None),
            Err(err) => {
                let fc = crate::agent::autonomy_telemetry::classify_failure(err);
                let fc_str = serde_json::to_value(fc)
                    .ok()
                    .and_then(|v| v.as_str().map(ToString::to_string));
                (
                    match fc {
                        crate::agent::autonomy_telemetry::FailureClass::ToolTimeout => {
                            AutonomyExecutionAttemptStatus::Timeout
                        }
                        _ => AutonomyExecutionAttemptStatus::Failed,
                    },
                    fc_str,
                    Some(err.to_string().chars().take(400).collect::<String>()),
                )
            }
        };

        let attempt = AutonomyExecutionAttempt {
            id: Uuid::new_v4(),
            goal_id: Some(persisted.goal_id),
            plan_id: Some(persisted.plan_id),
            plan_step_id: Some(step_id),
            job_id: Some(self.job_id),
            thread_id: None,
            user_id: self
                .context_manager()
                .get_context(self.job_id)
                .await
                .map(|c| c.user_id)
                .unwrap_or_else(|_| "default".to_string()),
            channel: "worker".to_string(),
            tool_name: tool_name.to_string(),
            tool_call_id: Some(tool_call_id.to_string()),
            tool_args: Some(tool_args.clone()),
            status,
            failure_class,
            retry_count,
            started_at,
            finished_at: Some(now),
            elapsed_ms: Some(elapsed_ms),
            result_summary: None,
            error_preview,
        };

        if let Err(e) = store.record_execution_attempt(&attempt).await {
            tracing::warn!(
                job = %self.job_id,
                plan_id = %persisted.plan_id,
                step_id = %step_id,
                "Failed to persist worker execution attempt: {}",
                e
            );
        }
    }

    async fn record_plan_verification_best_effort(
        &self,
        persisted: &PersistedPlanRun,
        verifier_kind: &str,
        status: AutonomyPlanVerificationStatus,
        completion_claimed: bool,
        summary: &str,
        checks: serde_json::Value,
        evidence_items: Vec<AutonomyEvidence>,
    ) {
        let Some(store) = self.store() else {
            return;
        };
        let user_id = self
            .context_manager()
            .get_context(self.job_id)
            .await
            .map(|c| c.user_id)
            .unwrap_or_else(|_| "default".to_string());

        let evidence_count = i32::try_from(evidence_items.len()).unwrap_or(i32::MAX);
        let evidence =
            serde_json::to_value(&evidence_items).unwrap_or_else(|_| serde_json::json!([]));
        let verification = AutonomyPlanVerification {
            id: Uuid::new_v4(),
            goal_id: Some(persisted.goal_id),
            plan_id: persisted.plan_id,
            job_id: Some(self.job_id),
            user_id: user_id.clone(),
            channel: "worker".to_string(),
            verifier_kind: verifier_kind.to_string(),
            status,
            completion_claimed,
            evidence_count,
            summary: summary.to_string(),
            checks,
            evidence,
            created_at: Utc::now(),
        };

        if let Err(e) = store.record_plan_verification(&verification).await {
            tracing::warn!(
                job = %self.job_id,
                plan_id = %persisted.plan_id,
                "Failed to persist worker plan verification: {}",
                e
            );
        }

        if self.autonomy_memory_plane_v2() {
            persist_memory_record_best_effort(
                store.clone(),
                MemoryRecordWriteRequest {
                    owner_user_id: user_id,
                    goal_id: Some(persisted.goal_id),
                    plan_id: Some(persisted.plan_id),
                    plan_step_id: None,
                    job_id: Some(self.job_id),
                    thread_id: None,
                    source_kind: MemorySourceKind::WorkerPlanExecution,
                    category: "verifier_outcome".to_string(),
                    title: format!("Verifier {}: {}", verifier_kind, verification.summary),
                    summary: verification.summary.clone(),
                    payload: serde_json::json!({
                        "verifier_kind": verifier_kind,
                        "status": verification.status,
                        "completion_claimed": completion_claimed,
                        "evidence_count": evidence_count,
                        "checks": verification.checks,
                        "evidence": verification.evidence,
                    }),
                    provenance: serde_json::json!({
                        "source": "worker.record_plan_verification_best_effort",
                        "timestamp": verification.created_at,
                        "plan_verification_id": verification.id,
                    }),
                    desired_memory_type: None,
                    confidence_hint: None,
                    sensitivity_hint: None,
                    ttl_secs_hint: None,
                    high_impact: false,
                    intent: MemoryWriteIntent::VerifierOutcome {
                        passed: matches!(status, AutonomyPlanVerificationStatus::Passed),
                        high_risk: false,
                        contradiction_detected: matches!(
                            status,
                            AutonomyPlanVerificationStatus::Failed
                                | AutonomyPlanVerificationStatus::Inconclusive
                        ),
                    },
                },
            );
        }
    }

    fn persist_worker_memory_record_best_effort(&self, request: MemoryRecordWriteRequest) {
        if !self.autonomy_memory_plane_v2() {
            return;
        }
        let Some(store) = self.store().cloned() else {
            return;
        };
        persist_memory_record_best_effort(store, request);
    }

    async fn load_goal_verifier_context(
        &self,
        goal_id: Uuid,
    ) -> (GoalRiskClass, serde_json::Value) {
        let Some(store) = self.store() else {
            return (GoalRiskClass::Medium, serde_json::json!({}));
        };
        match store.get_goal(goal_id).await {
            Ok(Some(goal)) => (goal.risk_class, goal.acceptance_criteria),
            Ok(None) => (GoalRiskClass::Medium, serde_json::json!({})),
            Err(e) => {
                tracing::warn!(
                    job = %self.job_id,
                    goal_id = %goal_id,
                    "Failed to load goal for verifier context: {}",
                    e
                );
                (GoalRiskClass::Medium, serde_json::json!({}))
            }
        }
    }

    /// Fire-and-forget persistence of job status.
    fn persist_status(&self, status: JobState, reason: Option<String>) {
        if let Some(store) = self.store() {
            let store = store.clone();
            let job_id = self.job_id;
            tokio::spawn(async move {
                if let Err(e) = store
                    .update_job_status(job_id, status, reason.as_deref())
                    .await
                {
                    tracing::warn!("Failed to persist status for job {}: {}", job_id, e);
                }
            });
        }
    }

    /// Run the worker until the job is complete or stopped.
    pub async fn run(self, mut rx: mpsc::Receiver<WorkerMessage>) -> Result<(), Error> {
        tracing::info!("Worker starting for job {}", self.job_id);

        // Wait for start signal
        match rx.recv().await {
            Some(WorkerMessage::Start) => {}
            Some(WorkerMessage::Stop) | None => {
                tracing::debug!("Worker for job {} stopped before starting", self.job_id);
                return Ok(());
            }
            Some(WorkerMessage::Ping) => {}
        }

        // Get job context
        let job_ctx = self.context_manager().get_context(self.job_id).await?;

        // Create reasoning engine
        let reasoning = Reasoning::new(self.llm().clone(), self.safety().clone());

        // Build initial reasoning context (tool definitions refreshed each iteration in execution_loop)
        let mut reason_ctx = ReasoningContext::new().with_job(&job_ctx.description);

        // Add system message
        reason_ctx.messages.push(ChatMessage::system(format!(
            r#"You are an autonomous agent working on a job.

Job: {}
Description: {}

You have access to tools to complete this job. Plan your approach and execute tools as needed.
You may request multiple tools at once if they can be executed in parallel.
Report when the job is complete or if you encounter issues you cannot resolve."#,
            job_ctx.title, job_ctx.description
        )));

        // Main execution loop with timeout
        let result = tokio::time::timeout(self.timeout(), async {
            self.execution_loop(&mut rx, &reasoning, &mut reason_ctx)
                .await
        })
        .await;

        match result {
            Ok(Ok(())) => {
                tracing::info!("Worker for job {} completed successfully", self.job_id);
            }
            Ok(Err(e)) => {
                tracing::error!("Worker for job {} failed: {}", self.job_id, e);
                self.mark_failed(&e.to_string()).await?;
            }
            Err(_) => {
                tracing::warn!("Worker for job {} timed out", self.job_id);
                self.mark_stuck("Execution timeout").await?;
            }
        }

        Ok(())
    }

    async fn execution_loop(
        &self,
        rx: &mut mpsc::Receiver<WorkerMessage>,
        reasoning: &Reasoning,
        reason_ctx: &mut ReasoningContext,
    ) -> Result<(), Error> {
        let max_iterations = 50;
        let mut iteration = 0;

        // Initial tool definitions for planning (will be refreshed in loop)
        reason_ctx.available_tools = self.tools().tool_definitions().await;

        // Generate plan if planning is enabled
        let plan = if self.use_planning() {
            let retrieval_context_block = self
                .build_planner_memory_retrieval_context(reason_ctx, None, None)
                .await;
            match PlannerV1::plan_initial_with_retrieval(
                reasoning,
                reason_ctx,
                retrieval_context_block.as_deref(),
            )
            .await
            {
                Ok(planner_out) => {
                    let crate::agent::planner_v1::PlannerOutput {
                        action_plan: p,
                        plan_trace_summary,
                    } = planner_out;
                    tracing::info!(
                        "Created plan for job {}: {} actions, {:.0}% confidence",
                        self.job_id,
                        p.actions.len(),
                        p.confidence * 100.0
                    );

                    // Add plan to context as assistant message
                    reason_ctx
                        .messages
                        .push(ChatMessage::assistant(plan_trace_summary));

                    // Best-effort persistence of the generated plan into the new
                    // autonomy control-plane tables. Worker execution continues
                    // even if persistence fails.
                    let job_ctx = match self.context_manager().get_context(self.job_id).await {
                        Ok(ctx) => Some(ctx),
                        Err(e) => {
                            tracing::warn!(
                                job = %self.job_id,
                                "Failed to load job context for autonomy persistence: {}",
                                e
                            );
                            None
                        }
                    };
                    let persisted_plan = if let Some(ref job_ctx) = job_ctx {
                        self.persist_action_plan(job_ctx, &p).await
                    } else {
                        None
                    };
                    if let Some(ref persisted) = persisted_plan {
                        self.set_plan_status_best_effort(persisted.plan_id, PlanStatus::Running)
                            .await;
                    }

                    Some((p, persisted_plan))
                }
                Err(e) => {
                    tracing::warn!(
                        "Planning failed for job {}, falling back to direct selection: {}",
                        self.job_id,
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        // If we have a plan, execute it (with bounded automatic replanning).
        if let Some((mut current_plan, mut current_persisted_plan)) = plan {
            if !self.autonomy_replanner_v1() {
                let _ = self
                    .execute_plan(
                        rx,
                        reasoning,
                        reason_ctx,
                        &current_plan,
                        current_persisted_plan.as_ref(),
                    )
                    .await?;
                return Ok(());
            }

            let budgets = ReplanBudgets::default();
            let mut replan_state = ReplanRuntimeState::default();

            loop {
                match self
                    .execute_plan(
                        rx,
                        reasoning,
                        reason_ctx,
                        &current_plan,
                        current_persisted_plan.as_ref(),
                    )
                    .await?
                {
                    PlanExecutionOutcome::Finished => return Ok(()),
                    PlanExecutionOutcome::ReplanRequested { reason, detail } => {
                        let Some(previous_persisted) = current_persisted_plan.as_ref() else {
                            self.mark_stuck(&format!(
                                "Plan requires replan but autonomy linkage is unavailable: {}",
                                detail
                            ))
                            .await?;
                            return Ok(());
                        };

                        if !replan_state.can_replan(budgets) {
                            tracing::warn!(
                                job = %self.job_id,
                                replans_attempted = replan_state.replans_attempted,
                                replan_reason = reason.code(),
                                "Replan budget exhausted"
                            );
                            self.set_plan_status_best_effort(
                                previous_persisted.plan_id,
                                PlanStatus::Failed,
                            )
                            .await;
                            self.set_goal_status_best_effort(
                                previous_persisted.goal_id,
                                GoalStatus::Blocked,
                            )
                            .await;
                            self.mark_stuck(&format!(
                                "Automatic replan budget exhausted after {} attempts (last reason: {} - {})",
                                replan_state.replans_attempted,
                                reason.code(),
                                detail
                            ))
                            .await?;
                            return Ok(());
                        }

                        replan_state.record_replan();

                        let prior_plan_status = match reason {
                            ReplanReason::PolicyDenied => PlanStatus::Paused,
                            _ => PlanStatus::Failed,
                        };
                        self.set_plan_status_best_effort(
                            previous_persisted.plan_id,
                            prior_plan_status,
                        )
                        .await;
                        self.set_goal_status_best_effort(
                            previous_persisted.goal_id,
                            GoalStatus::Active,
                        )
                        .await;

                        reason_ctx
                            .messages
                            .push(ChatMessage::user(ReplannerV1::replan_prompt(
                                reason, &detail,
                            )));

                        let retrieval_context_block = self
                            .build_planner_memory_retrieval_context(
                                reason_ctx,
                                Some(previous_persisted.goal_id),
                                Some(previous_persisted.plan_id),
                            )
                            .await;
                        let planner_out = match PlannerV1::plan_initial_with_retrieval(
                            reasoning,
                            reason_ctx,
                            retrieval_context_block.as_deref(),
                        )
                        .await
                        {
                            Ok(out) => out,
                            Err(e) => {
                                self.set_goal_status_best_effort(
                                    previous_persisted.goal_id,
                                    GoalStatus::Blocked,
                                )
                                .await;
                                self.mark_stuck(&format!(
                                    "Automatic replanning failed after {} attempt(s): {}",
                                    replan_state.replans_attempted, e
                                ))
                                .await?;
                                return Ok(());
                            }
                        };
                        let crate::agent::planner_v1::PlannerOutput {
                            action_plan: next_plan,
                            plan_trace_summary,
                        } = planner_out;

                        tracing::info!(
                            job = %self.job_id,
                            prior_plan_id = %previous_persisted.plan_id,
                            goal_id = %previous_persisted.goal_id,
                            replan_reason = reason.code(),
                            replans_attempted = replan_state.replans_attempted,
                            steps = next_plan.actions.len(),
                            "Created replanned action plan"
                        );

                        reason_ctx
                            .messages
                            .push(ChatMessage::assistant(plan_trace_summary));

                        let next_persisted = self
                            .persist_replanned_action_plan(
                                previous_persisted.goal_id,
                                previous_persisted.plan_id,
                                reason,
                                &detail,
                                &next_plan,
                            )
                            .await;

                        if let Some(ref next_persisted) = next_persisted {
                            self.set_plan_status_best_effort(
                                previous_persisted.plan_id,
                                PlanStatus::Superseded,
                            )
                            .await;
                            self.set_goal_status_best_effort(
                                next_persisted.goal_id,
                                GoalStatus::Active,
                            )
                            .await;
                            self.set_plan_status_best_effort(
                                next_persisted.plan_id,
                                PlanStatus::Running,
                            )
                            .await;
                        } else {
                            tracing::warn!(
                                job = %self.job_id,
                                prior_plan_id = %previous_persisted.plan_id,
                                "Replan persistence failed; continuing execution with transient plan only"
                            );
                        }

                        current_plan = next_plan;
                        current_persisted_plan = next_persisted;
                    }
                }
            }
        }

        // Otherwise, use direct tool selection loop
        loop {
            // Check for stop signal
            if let Ok(msg) = rx.try_recv() {
                match msg {
                    WorkerMessage::Stop => {
                        tracing::debug!("Worker for job {} received stop signal", self.job_id);
                        return Ok(());
                    }
                    WorkerMessage::Ping => {
                        tracing::trace!("Worker for job {} received ping", self.job_id);
                    }
                    WorkerMessage::Start => {}
                }
            }

            // Check for cancellation
            if let Ok(ctx) = self.context_manager().get_context(self.job_id).await
                && ctx.state == JobState::Cancelled
            {
                tracing::info!("Worker for job {} detected cancellation", self.job_id);
                return Ok(());
            }

            iteration += 1;
            if iteration > max_iterations {
                self.mark_stuck("Maximum iterations exceeded").await?;
                return Ok(());
            }

            // Refresh tool definitions so newly built tools become visible
            reason_ctx.available_tools = self.tools().tool_definitions().await;

            // Select next tool(s) to use
            let selections = reasoning.select_tools(reason_ctx).await?;

            if selections.is_empty() {
                // No tools from select_tools, ask LLM directly (may still return tool calls)
                let respond_output = reasoning.respond_with_tools(reason_ctx).await?;

                match respond_output.result {
                    RespondResult::Text(response) => {
                        // Check for explicit completion phrases. Use word-boundary
                        // aware checks to avoid false positives like "incomplete",
                        // "not done", or "unfinished". Only the LLM's own response
                        // (not tool output) can trigger this.
                        if crate::util::llm_signals_completion(&response) {
                            self.mark_completed().await?;
                            return Ok(());
                        }

                        // Add assistant response to context
                        reason_ctx.messages.push(ChatMessage::assistant(&response));

                        // Give it one more chance to select a tool
                        if iteration > 3 && iteration % 5 == 0 {
                            reason_ctx.messages.push(ChatMessage::user(
                                "Are you stuck? Do you need help completing this job?",
                            ));
                        }
                    }
                    RespondResult::ToolCalls {
                        tool_calls,
                        content,
                    } => {
                        // Model returned tool calls - execute them
                        tracing::debug!(
                            "Job {} respond_with_tools returned {} tool calls",
                            self.job_id,
                            tool_calls.len()
                        );

                        // Add assistant message with tool_calls (OpenAI protocol)
                        reason_ctx
                            .messages
                            .push(ChatMessage::assistant_with_tool_calls(
                                content,
                                tool_calls.clone(),
                            ));

                        for tc in tool_calls {
                            let result = self.execute_tool(&tc.name, &tc.arguments).await;

                            // Create synthetic selection for process_tool_result
                            let selection = ToolSelection {
                                tool_name: tc.name.clone(),
                                parameters: tc.arguments.clone(),
                                reasoning: String::new(),
                                alternatives: vec![],
                                tool_call_id: tc.id.clone(),
                            };

                            self.process_tool_result(reason_ctx, &selection, result)
                                .await?;
                        }
                    }
                }
            } else if selections.len() == 1 {
                // Single tool: execute directly
                let selection = &selections[0];
                tracing::debug!(
                    "Job {} selecting tool: {} - {}",
                    self.job_id,
                    selection.tool_name,
                    selection.reasoning
                );

                let result = self
                    .execute_tool(&selection.tool_name, &selection.parameters)
                    .await;

                self.process_tool_result(reason_ctx, selection, result)
                    .await?;
            } else {
                // Multiple tools: execute in parallel
                tracing::debug!(
                    "Job {} executing {} tools in parallel",
                    self.job_id,
                    selections.len()
                );

                let results = self.execute_tools_parallel(&selections).await;

                // Process all results
                for (selection, result) in selections.iter().zip(results) {
                    self.process_tool_result(reason_ctx, selection, result.result)
                        .await?;
                }
            }

            // Small delay between iterations
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Execute multiple tools in parallel.
    async fn execute_tools_parallel(&self, selections: &[ToolSelection]) -> Vec<ToolExecResult> {
        let futures: Vec<_> = selections
            .iter()
            .map(|selection| {
                let tool_name = selection.tool_name.clone();
                let params = selection.parameters.clone();
                let deps = self.deps.clone();
                let job_id = self.job_id;

                async move {
                    let result = Self::execute_tool_inner(&deps, job_id, &tool_name, &params).await;
                    ToolExecResult { result }
                }
            })
            .collect();

        join_all(futures).await
    }

    /// Inner tool execution logic that can be called from both single and parallel paths.
    async fn execute_tool_inner(
        deps: &WorkerDeps,
        job_id: Uuid,
        tool_name: &str,
        params: &serde_json::Value,
    ) -> Result<String, Error> {
        let tool =
            deps.tools
                .get(tool_name)
                .await
                .ok_or_else(|| crate::error::ToolError::NotFound {
                    name: tool_name.to_string(),
                })?;

        // Fetch job context early so we have the real user_id for policy + hooks
        let job_ctx = deps.context_manager.get_context(job_id).await?;

        // Policy engine v1: autonomous jobs cannot execute approval-required tools.
        if let ApprovalPolicyOutcome::RequireApproval { reason_codes } =
            evaluate_worker_tool_approval(tool.as_ref())
        {
            if deps.autonomy_policy_engine_v1 {
                persist_worker_policy_decision_best_effort(
                    deps,
                    &job_ctx,
                    tool_name,
                    None,
                    AutonomyPolicyDecisionKind::RequireApproval,
                    reason_codes,
                    true,
                    Some(false),
                    "tool_call_preflight",
                    serde_json::json!({}),
                );
            }
            return Err(crate::error::ToolError::AuthRequired {
                name: tool_name.to_string(),
            }
            .into());
        }

        // Policy engine v1 hook evaluation
        let params = match evaluate_tool_call_hook(
            deps.hooks.as_ref(),
            tool_name,
            params,
            &job_ctx.user_id,
            &format!("job:{}", job_id),
        )
        .await
        {
            HookToolPolicyOutcome::Continue { parameters } => parameters,
            HookToolPolicyOutcome::Modified {
                parameters,
                reason_codes,
            } => {
                if deps.autonomy_policy_engine_v1 {
                    persist_worker_policy_decision_best_effort(
                        deps,
                        &job_ctx,
                        tool_name,
                        None,
                        AutonomyPolicyDecisionKind::Modify,
                        reason_codes,
                        false,
                        None,
                        "tool_call_hook",
                        serde_json::json!({}),
                    );
                }
                parameters
            }
            HookToolPolicyOutcome::Deny {
                reason,
                reason_codes,
            } => {
                if deps.autonomy_policy_engine_v1 {
                    persist_worker_policy_decision_best_effort(
                        deps,
                        &job_ctx,
                        tool_name,
                        None,
                        AutonomyPolicyDecisionKind::Deny,
                        reason_codes,
                        false,
                        None,
                        "tool_call_hook",
                        serde_json::json!({}),
                    );
                }
                return Err(crate::error::ToolError::ExecutionFailed {
                    name: tool_name.to_string(),
                    reason: format!("Blocked by hook: {}", reason),
                }
                .into());
            }
        };
        if job_ctx.state == JobState::Cancelled {
            return Err(crate::error::ToolError::ExecutionFailed {
                name: tool_name.to_string(),
                reason: "Job is cancelled".to_string(),
            }
            .into());
        }

        // Validate tool parameters
        let validation = deps.safety.validator().validate_tool_params(&params);
        if !validation.is_valid {
            let details = validation
                .errors
                .iter()
                .map(|e| format!("{}: {}", e.field, e.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(crate::error::ToolError::InvalidParameters {
                name: tool_name.to_string(),
                reason: format!("Invalid tool parameters: {}", details),
            }
            .into());
        }

        tracing::debug!(
            tool = %tool_name,
            params = %params,
            job = %job_id,
            "Tool call started"
        );

        // Execute with per-tool timeout and timing
        let tool_timeout = tool.execution_timeout();
        let start = std::time::Instant::now();
        let result = tokio::time::timeout(tool_timeout, async {
            tool.execute(params.clone(), &job_ctx).await
        })
        .await;
        let elapsed = start.elapsed();

        match &result {
            Ok(Ok(output)) => {
                let result_str = serde_json::to_string(&output.result)
                    .unwrap_or_else(|_| "<serialize error>".to_string());
                tracing::debug!(
                    tool = %tool_name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    result = %result_str,
                    "Tool call succeeded"
                );
            }
            Ok(Err(e)) => {
                tracing::debug!(
                    tool = %tool_name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    error = %e,
                    "Tool call failed"
                );
            }
            Err(_) => {
                tracing::debug!(
                    tool = %tool_name,
                    elapsed_ms = elapsed.as_millis() as u64,
                    timeout_secs = tool_timeout.as_secs(),
                    "Tool call timed out"
                );
            }
        }

        // Record action in memory and get the ActionRecord for persistence
        let action = match &result {
            Ok(Ok(output)) => {
                let output_str = serde_json::to_string_pretty(&output.result)
                    .ok()
                    .map(|s| deps.safety.sanitize_tool_output(tool_name, &s).content);
                deps.context_manager
                    .update_memory(job_id, |mem| {
                        let rec = mem.create_action(tool_name, params.clone()).succeed(
                            output_str.clone(),
                            output.result.clone(),
                            elapsed,
                        );
                        mem.record_action(rec.clone());
                        rec
                    })
                    .await
                    .ok()
            }
            Ok(Err(e)) => deps
                .context_manager
                .update_memory(job_id, |mem| {
                    let rec = mem
                        .create_action(tool_name, params.clone())
                        .fail(e.to_string(), elapsed);
                    mem.record_action(rec.clone());
                    rec
                })
                .await
                .ok(),
            Err(_) => deps
                .context_manager
                .update_memory(job_id, |mem| {
                    let rec = mem
                        .create_action(tool_name, params.clone())
                        .fail("Execution timeout", elapsed);
                    mem.record_action(rec.clone());
                    rec
                })
                .await
                .ok(),
        };

        // Persist action to database (fire-and-forget)
        if let (Some(action), Some(store)) = (action, deps.store.clone()) {
            tokio::spawn(async move {
                if let Err(e) = store.save_action(job_id, &action).await {
                    tracing::warn!("Failed to persist action for job {}: {}", job_id, e);
                }
            });
        }

        // Handle the result
        let output = result
            .map_err(|_| crate::error::ToolError::Timeout {
                name: tool_name.to_string(),
                timeout: tool_timeout,
            })?
            .map_err(|e| crate::error::ToolError::ExecutionFailed {
                name: tool_name.to_string(),
                reason: e.to_string(),
            })?;

        // Return result as string
        serde_json::to_string_pretty(&output.result).map_err(|e| {
            crate::error::ToolError::ExecutionFailed {
                name: tool_name.to_string(),
                reason: format!("Failed to serialize result: {}", e),
            }
            .into()
        })
    }

    /// Process a tool execution result and add it to the reasoning context.
    async fn process_tool_result(
        &self,
        reason_ctx: &mut ReasoningContext,
        selection: &ToolSelection,
        result: Result<String, Error>,
    ) -> Result<bool, Error> {
        match result {
            Ok(output) => {
                // Sanitize output
                let sanitized = self
                    .safety()
                    .sanitize_tool_output(&selection.tool_name, &output);

                // Add to context
                let wrapped = self.safety().wrap_for_llm(
                    &selection.tool_name,
                    &sanitized.content,
                    sanitized.was_modified,
                );

                reason_ctx.messages.push(ChatMessage::tool_result(
                    &selection.tool_call_id,
                    &selection.tool_name,
                    wrapped,
                ));

                // Tool output never drives job completion. A malicious tool could
                // emit "TASK_COMPLETE" to force premature completion. Only the LLM's
                // own structured response (in execution_loop) can mark a job done.
                Ok(false)
            }
            Err(e) => {
                tracing::warn!(
                    "Tool {} failed for job {}: {}",
                    selection.tool_name,
                    self.job_id,
                    e
                );

                // Record failure for self-repair tracking
                if let Some(store) = self.store() {
                    let store = store.clone();
                    let tool_name = selection.tool_name.clone();
                    let error_msg = e.to_string();
                    tokio::spawn(async move {
                        if let Err(db_err) = store.record_tool_failure(&tool_name, &error_msg).await
                        {
                            tracing::warn!("Failed to record tool failure: {}", db_err);
                        }
                    });
                }

                reason_ctx.messages.push(ChatMessage::tool_result(
                    &selection.tool_call_id,
                    &selection.tool_name,
                    format!("Error: {}", e),
                ));

                Ok(false)
            }
        }
    }

    /// Execute a pre-generated plan.
    async fn execute_plan(
        &self,
        rx: &mut mpsc::Receiver<WorkerMessage>,
        reasoning: &Reasoning,
        reason_ctx: &mut ReasoningContext,
        plan: &ActionPlan,
        persisted_plan: Option<&PersistedPlanRun>,
    ) -> Result<PlanExecutionOutcome, Error> {
        let mut executed_steps = 0usize;
        let mut succeeded_steps = 0usize;
        let mut failed_steps = 0usize;
        let mut step_outcomes = Vec::<serde_json::Value>::with_capacity(plan.actions.len());
        let mut step_evidence = Vec::<AutonomyEvidence>::with_capacity(plan.actions.len());
        let memory_user_id = if self.autonomy_memory_plane_v2() {
            self.context_manager()
                .get_context(self.job_id)
                .await
                .map(|ctx| ctx.user_id)
                .unwrap_or_else(|_| "default".to_string())
        } else {
            String::new()
        };
        for (i, action) in plan.actions.iter().enumerate() {
            // Check for stop signal
            if let Ok(msg) = rx.try_recv() {
                match msg {
                    WorkerMessage::Stop => {
                        tracing::debug!(
                            "Worker for job {} received stop signal during plan execution",
                            self.job_id
                        );
                        return Ok(PlanExecutionOutcome::Finished);
                    }
                    WorkerMessage::Ping => {
                        tracing::trace!("Worker for job {} received ping", self.job_id);
                    }
                    WorkerMessage::Start => {}
                }
            }

            tracing::debug!(
                "Job {} executing planned action {}/{}: {} - {}",
                self.job_id,
                i + 1,
                plan.actions.len(),
                action.tool_name,
                action.reasoning
            );

            if let Some(p) = persisted_plan
                && let Some(step_id) = p.step_ids.get(i)
            {
                self.set_plan_step_status_best_effort(*step_id, PlanStepStatus::Running)
                    .await;
                let _ = self
                    .context_manager()
                    .update_context(self.job_id, |ctx| {
                        ctx.set_autonomy_plan_step(Some(*step_id));
                    })
                    .await;
            }

            // Execute the planned tool
            let started_at = Utc::now();
            let started = std::time::Instant::now();
            let result = self
                .execute_tool(&action.tool_name, &action.parameters)
                .await;
            let elapsed_ms = started.elapsed().as_millis() as i64;
            let step_succeeded = result.is_ok();
            executed_steps += 1;
            if step_succeeded {
                succeeded_steps += 1;
            } else {
                failed_steps += 1;
            }
            let (failure_class, error_preview, result_preview) = match &result {
                Ok(output) => (
                    None,
                    None,
                    Some(output.chars().take(400).collect::<String>()),
                ),
                Err(err) => {
                    let fc = crate::agent::autonomy_telemetry::classify_failure(err);
                    let fc_str = serde_json::to_value(fc)
                        .ok()
                        .and_then(|v| v.as_str().map(ToString::to_string));
                    (
                        fc_str,
                        Some(err.to_string().chars().take(400).collect::<String>()),
                        None,
                    )
                }
            };
            let evidence_class =
                classify_step_evidence(&action.tool_name, &action.parameters, &action.reasoning);
            let step_payload = serde_json::json!({
                "sequence_index": i,
                "tool_name": action.tool_name,
                "reasoning": action.reasoning,
                "elapsed_ms": elapsed_ms,
                "status": if step_succeeded { "succeeded" } else { "failed" },
                "failure_class": failure_class,
                "result_preview": result_preview,
                "error_preview": error_preview,
                "verifier_evidence_kind": evidence_class.kind,
                "verifier_check_kind": evidence_class.check_kind,
                "command": action.parameters.get("command").and_then(|v| v.as_str()),
            });
            step_outcomes.push(step_payload.clone());
            if self.autonomy_memory_plane_v2() {
                let (goal_id, plan_id, plan_step_id) = persisted_plan
                    .map(|p| (Some(p.goal_id), Some(p.plan_id), p.step_ids.get(i).copied()))
                    .unwrap_or((None, None, None));
                self.persist_worker_memory_record_best_effort(MemoryRecordWriteRequest {
                    owner_user_id: memory_user_id.clone(),
                    goal_id,
                    plan_id,
                    plan_step_id,
                    job_id: Some(self.job_id),
                    thread_id: None,
                    source_kind: MemorySourceKind::WorkerPlanExecution,
                    category: "worker_step_outcome".to_string(),
                    title: format!(
                        "Planned step {} {} ({})",
                        i + 1,
                        if step_succeeded {
                            "succeeded"
                        } else {
                            "failed"
                        },
                        action.tool_name
                    ),
                    summary: format!(
                        "Step {}/{} {} in {}ms: {}",
                        i + 1,
                        plan.actions.len(),
                        if step_succeeded {
                            "succeeded"
                        } else {
                            "failed"
                        },
                        elapsed_ms,
                        action.reasoning.chars().take(160).collect::<String>()
                    ),
                    payload: step_payload.clone(),
                    provenance: serde_json::json!({
                        "source": "worker.execute_plan",
                        "timestamp": started_at,
                        "tool_call_id": format!("plan_{}_{}", self.job_id, i),
                    }),
                    desired_memory_type: None,
                    confidence_hint: None,
                    sensitivity_hint: None,
                    ttl_secs_hint: None,
                    high_impact: false,
                    intent: MemoryWriteIntent::WorkerStepOutcome {
                        success: step_succeeded,
                    },
                });
            }
            step_evidence.push(AutonomyEvidence {
                kind: evidence_class.kind,
                source: format!("worker.plan_step.{}", i),
                summary: format!(
                    "{} {} ({})",
                    if step_succeeded {
                        "Succeeded"
                    } else {
                        "Failed"
                    },
                    action.tool_name,
                    action.reasoning
                )
                .chars()
                .take(240)
                .collect(),
                confidence: if step_succeeded { 0.95 } else { 0.8 },
                payload: {
                    let mut payload = step_payload;
                    if let Some(command_hint) = evidence_class.command_hint {
                        if let Some(map) = payload.as_object_mut() {
                            map.insert(
                                "verifier_command_hint".to_string(),
                                serde_json::json!(command_hint),
                            );
                        }
                    }
                    payload
                },
                created_at: started_at,
            });

            // Create a synthetic ToolSelection for process_tool_result.
            // Plan actions don't originate from an LLM tool_call response so
            // there is no real tool_call_id; generate a unique one.
            let selection = ToolSelection {
                tool_name: action.tool_name.clone(),
                parameters: action.parameters.clone(),
                reasoning: action.reasoning.clone(),
                alternatives: vec![],
                tool_call_id: format!("plan_{}_{}", self.job_id, i),
            };

            if let Some(p) = persisted_plan {
                self.record_worker_execution_attempt_best_effort(
                    p,
                    i,
                    &action.tool_name,
                    &action.parameters,
                    &selection.tool_call_id,
                    started_at,
                    &result,
                    elapsed_ms,
                    0,
                )
                .await;
            }

            if let Some(p) = persisted_plan
                && let Some(step_id) = p.step_ids.get(i)
            {
                let status = if step_succeeded {
                    PlanStepStatus::Succeeded
                } else {
                    PlanStepStatus::Failed
                };
                self.set_plan_step_status_best_effort(*step_id, status)
                    .await;
                let _ = self
                    .context_manager()
                    .update_context(self.job_id, |ctx| {
                        ctx.set_autonomy_plan_step(None);
                    })
                    .await;
            }

            let step_replan_request = result
                .as_ref()
                .err()
                .and_then(classify_replan_reason_for_step_error)
                .map(|reason| {
                    (
                        reason,
                        format!(
                            "Planned step {}/{} ({}) failed and requires replanning: {}",
                            i + 1,
                            plan.actions.len(),
                            action.tool_name,
                            result
                                .as_ref()
                                .err()
                                .map(ToString::to_string)
                                .unwrap_or_else(|| "unknown failure".to_string())
                        ),
                    )
                });

            // Process the result
            let completed = self
                .process_tool_result(reason_ctx, &selection, result)
                .await?;

            if self.autonomy_replanner_v1()
                && let Some((reason, detail)) = step_replan_request
            {
                if self.autonomy_memory_plane_v2() {
                    let (goal_id, plan_id, plan_step_id) = persisted_plan
                        .map(|p| (Some(p.goal_id), Some(p.plan_id), p.step_ids.get(i).copied()))
                        .unwrap_or((None, None, None));
                    self.persist_worker_memory_record_best_effort(MemoryRecordWriteRequest {
                        owner_user_id: memory_user_id.clone(),
                        goal_id,
                        plan_id,
                        plan_step_id,
                        job_id: Some(self.job_id),
                        thread_id: None,
                        source_kind: MemorySourceKind::WorkerPlanExecution,
                        category: "replan_event".to_string(),
                        title: format!("Automatic replan requested after step {}", i + 1),
                        summary: detail.clone(),
                        payload: serde_json::json!({
                            "reason": reason.code(),
                            "detail": detail,
                            "step_index": i,
                            "tool_name": action.tool_name,
                        }),
                        provenance: serde_json::json!({
                            "source": "worker.execute_plan",
                            "timestamp": Utc::now(),
                        }),
                        desired_memory_type: None,
                        confidence_hint: None,
                        sensitivity_hint: None,
                        ttl_secs_hint: None,
                        high_impact: false,
                        intent: MemoryWriteIntent::Generic,
                    });
                }
                tracing::info!(
                    job = %self.job_id,
                    replan_reason = reason.code(),
                    step_index = i,
                    "Planned step requested automatic replan"
                );
                return Ok(PlanExecutionOutcome::ReplanRequested { reason, detail });
            }

            if completed {
                if let Some(p) = persisted_plan {
                    let (risk_class, acceptance_criteria) =
                        self.load_goal_verifier_context(p.goal_id).await;
                    let verifier_result = VerifierV1::verify_completion(VerificationRequest {
                        completion_path: "early_process_tool_result",
                        completion_claimed: true,
                        completion_candidate_summary:
                            "Plan execution ended early before final completion-check verification."
                                .to_string(),
                        planned_steps: plan.actions.len(),
                        executed_steps,
                        succeeded_steps,
                        failed_steps,
                        step_outcomes: step_outcomes.clone(),
                        base_evidence: step_evidence.clone(),
                        acceptance_criteria,
                        risk_class,
                    });
                    self.record_plan_verification_best_effort(
                        p,
                        "execute_plan_early_completion_path",
                        verifier_result.status,
                        true,
                        &verifier_result.summary,
                        verifier_result.checks,
                        verifier_result.evidence,
                    )
                    .await;
                }
                return Ok(PlanExecutionOutcome::Finished);
            }

            // Small delay between actions
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Plan completed, check with LLM if job is done
        reason_ctx.messages.push(ChatMessage::user(
            "All planned actions have been executed. Is the job complete? If not, what else needs to be done?",
        ));

        let response = reasoning.respond(reason_ctx).await?;
        reason_ctx.messages.push(ChatMessage::assistant(&response));

        let completion_claimed = crate::util::llm_signals_completion(&response);
        let verifier_result = if self.autonomy_verifier_v1()
            && let Some(p) = persisted_plan
        {
            let (risk_class, acceptance_criteria) =
                self.load_goal_verifier_context(p.goal_id).await;
            Some(VerifierV1::verify_completion(VerificationRequest {
                completion_path: "final_llm_completion_check",
                completion_claimed,
                completion_candidate_summary: response.clone(),
                planned_steps: plan.actions.len(),
                executed_steps,
                succeeded_steps,
                failed_steps,
                step_outcomes: step_outcomes.clone(),
                base_evidence: step_evidence,
                acceptance_criteria,
                risk_class,
            }))
        } else {
            None
        };

        if let (Some(p), Some(result)) = (persisted_plan, verifier_result.as_ref()) {
            self.record_plan_verification_best_effort(
                p,
                "execute_plan_completion_check",
                result.status,
                completion_claimed,
                &result.summary,
                result.checks.clone(),
                result.evidence.clone(),
            )
            .await;
        }

        let verifier_allows_completion = verifier_result
            .as_ref()
            .map(|r| r.allow_completion)
            .unwrap_or(completion_claimed);
        let should_complete = completion_claimed && verifier_allows_completion;

        if should_complete {
            self.mark_completed().await?;
            if let Some(p) = persisted_plan {
                self.set_plan_status_best_effort(p.plan_id, PlanStatus::Completed)
                    .await;
                self.set_goal_status_best_effort(p.goal_id, GoalStatus::Completed)
                    .await;
            }
            return Ok(PlanExecutionOutcome::Finished);
        }

        let mut stuck_reason = "Plan completed but job incomplete - needs re-planning".to_string();
        let mut replan_request: Option<(ReplanReason, String)> = None;

        if completion_claimed && let Some(result) = verifier_result.as_ref() {
            tracing::info!(
                job = %self.job_id,
                verifier_status = ?result.status,
                next_action = ?result.next_action,
                verifier_failure_reasons = result.failure_reasons.len(),
                "Verifier blocked completion for planned execution"
            );
            match result.next_action {
                VerificationNextAction::Replan | VerificationNextAction::GatherEvidence => {
                    replan_request = Some((
                        ReplanReason::VerifierInsufficientEvidence,
                        format!(
                            "Verifier blocked completion ({}): {}",
                            match result.next_action {
                                VerificationNextAction::Replan => "replan",
                                VerificationNextAction::GatherEvidence => "gather_evidence",
                                _ => "other",
                            },
                            result.summary
                        ),
                    ));
                }
                VerificationNextAction::AskUser => {
                    stuck_reason = format!(
                        "Verifier blocked completion pending user confirmation: {}",
                        result.summary
                    );
                }
                VerificationNextAction::MarkBlocked => {
                    stuck_reason = result.summary.clone();
                }
                VerificationNextAction::Complete => {
                    stuck_reason = result.summary.clone();
                }
            }
        } else if !completion_claimed {
            tracing::info!(
                job = %self.job_id,
                failed_steps,
                executed_steps,
                planned_steps = plan.actions.len(),
                "Plan completed but completion check indicates work remains"
            );
            replan_request = Some((
                if failed_steps > 0 {
                    ReplanReason::StepFailure
                } else {
                    ReplanReason::PlanExhaustedIncomplete
                },
                format!(
                    "Planned execution exhausted ({}/{}) but completion check reported remaining work. Failed steps: {}. Response: {}",
                    executed_steps,
                    plan.actions.len(),
                    failed_steps,
                    response.chars().take(280).collect::<String>()
                ),
            ));
        }

        if self.autonomy_replanner_v1()
            && let Some((reason, detail)) = replan_request
            && persisted_plan.is_some()
        {
            if self.autonomy_memory_plane_v2() {
                let (goal_id, plan_id) = persisted_plan
                    .map(|p| (Some(p.goal_id), Some(p.plan_id)))
                    .unwrap_or((None, None));
                self.persist_worker_memory_record_best_effort(MemoryRecordWriteRequest {
                    owner_user_id: memory_user_id,
                    goal_id,
                    plan_id,
                    plan_step_id: None,
                    job_id: Some(self.job_id),
                    thread_id: None,
                    source_kind: MemorySourceKind::WorkerPlanExecution,
                    category: "replan_event".to_string(),
                    title: "Automatic replan requested after planned execution".to_string(),
                    summary: detail.clone(),
                    payload: serde_json::json!({
                        "reason": reason.code(),
                        "detail": detail,
                        "completion_claimed": completion_claimed,
                        "failed_steps": failed_steps,
                        "executed_steps": executed_steps,
                        "planned_steps": plan.actions.len(),
                    }),
                    provenance: serde_json::json!({
                        "source": "worker.execute_plan",
                        "timestamp": Utc::now(),
                    }),
                    desired_memory_type: None,
                    confidence_hint: None,
                    sensitivity_hint: None,
                    ttl_secs_hint: None,
                    high_impact: false,
                    intent: MemoryWriteIntent::Generic,
                });
            }
            return Ok(PlanExecutionOutcome::ReplanRequested { reason, detail });
        }

        self.mark_stuck(&stuck_reason).await?;
        if let Some(p) = persisted_plan {
            self.set_plan_status_best_effort(p.plan_id, PlanStatus::Failed)
                .await;
            self.set_goal_status_best_effort(p.goal_id, GoalStatus::Blocked)
                .await;
        }

        Ok(PlanExecutionOutcome::Finished)
    }

    async fn execute_tool(
        &self,
        tool_name: &str,
        params: &serde_json::Value,
    ) -> Result<String, Error> {
        Self::execute_tool_inner(&self.deps, self.job_id, tool_name, params).await
    }

    async fn mark_completed(&self) -> Result<(), Error> {
        self.context_manager()
            .update_context(self.job_id, |ctx| {
                ctx.transition_to(
                    JobState::Completed,
                    Some("Job completed successfully".to_string()),
                )
            })
            .await?
            .map_err(|s| crate::error::JobError::ContextError {
                id: self.job_id,
                reason: s,
            })?;

        self.persist_status(
            JobState::Completed,
            Some("Job completed successfully".to_string()),
        );
        Ok(())
    }

    async fn mark_failed(&self, reason: &str) -> Result<(), Error> {
        self.context_manager()
            .update_context(self.job_id, |ctx| {
                ctx.transition_to(JobState::Failed, Some(reason.to_string()))
            })
            .await?
            .map_err(|s| crate::error::JobError::ContextError {
                id: self.job_id,
                reason: s,
            })?;

        self.persist_status(JobState::Failed, Some(reason.to_string()));
        Ok(())
    }

    async fn mark_stuck(&self, reason: &str) -> Result<(), Error> {
        self.context_manager()
            .update_context(self.job_id, |ctx| ctx.mark_stuck(reason))
            .await?
            .map_err(|s| crate::error::JobError::ContextError {
                id: self.job_id,
                reason: s,
            })?;

        self.persist_status(JobState::Stuck, Some(reason.to_string()));
        Ok(())
    }
}

/// Convert a TaskOutput to a string result for tool execution.
impl From<TaskOutput> for Result<String, Error> {
    fn from(output: TaskOutput) -> Self {
        serde_json::to_string_pretty(&output.result).map_err(|e| {
            crate::error::ToolError::ExecutionFailed {
                name: "task".to_string(),
                reason: format!("Failed to serialize result: {}", e),
            }
            .into()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use chrono::Utc;
    use rust_decimal::Decimal;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use crate::agent::{
        Goal, GoalRiskClass, GoalSource, GoalStatus, Plan, PlanStatus, PlanStep, PlanStepKind,
        PlanStepStatus, PlannerKind,
    };
    use crate::config::SafetyConfig;
    use crate::context::{ContextManager, JobState};
    use crate::error::{Error, LlmError, ToolError};
    use crate::hooks::HookRegistry;
    use crate::llm::{
        ActionPlan, CompletionRequest, CompletionResponse, FinishReason, LlmProvider, Reasoning,
        ReasoningContext, ToolCompletionRequest, ToolCompletionResponse, ToolSelection,
    };
    use crate::safety::SafetyLayer;
    use crate::testing::{StubLlm, TestHarnessBuilder};
    use crate::tools::{Tool, ToolOutput, ToolRegistry};
    use crate::util::llm_signals_completion;

    use super::{
        PersistedPlanRun, PlanExecutionOutcome, ReplanReason, Worker, WorkerDeps,
        classify_replan_reason_for_step_error, classify_step_evidence,
    };

    #[test]
    fn test_tool_selection_preserves_call_id() {
        let selection = ToolSelection {
            tool_name: "memory_search".to_string(),
            parameters: serde_json::json!({"query": "test"}),
            reasoning: "Need to search memory".to_string(),
            alternatives: vec![],
            tool_call_id: "call_abc123".to_string(),
        };

        assert_eq!(selection.tool_call_id, "call_abc123");
        assert_ne!(
            selection.tool_call_id, "tool_call_id",
            "tool_call_id must not be the hardcoded placeholder string"
        );
    }

    #[test]
    fn test_completion_positive_signals() {
        assert!(llm_signals_completion("The job is complete."));
        assert!(llm_signals_completion(
            "I have completed the task successfully."
        ));
        assert!(llm_signals_completion("The task is done."));
        assert!(llm_signals_completion("The task is finished."));
        assert!(llm_signals_completion(
            "All steps are complete and verified."
        ));
        assert!(llm_signals_completion(
            "I've done all the work. The work is done."
        ));
        assert!(llm_signals_completion(
            "Successfully completed the migration."
        ));
    }

    #[test]
    fn test_completion_negative_signals_block_false_positives() {
        // These contain completion keywords but also negation, should NOT trigger.
        assert!(!llm_signals_completion("The task is not complete yet."));
        assert!(!llm_signals_completion("This is not done."));
        assert!(!llm_signals_completion("The work is incomplete."));
        assert!(!llm_signals_completion(
            "The migration is not yet finished."
        ));
        assert!(!llm_signals_completion("The job isn't done yet."));
        assert!(!llm_signals_completion("This remains unfinished."));
    }

    #[test]
    fn test_completion_does_not_match_bare_substrings() {
        // Bare words embedded in other text should NOT trigger completion.
        assert!(!llm_signals_completion(
            "I need to complete more work first."
        ));
        assert!(!llm_signals_completion(
            "Let me finish the remaining steps."
        ));
        assert!(!llm_signals_completion(
            "I'm done analyzing, now let me fix it."
        ));
        assert!(!llm_signals_completion(
            "I completed step 1 but step 2 remains."
        ));
    }

    #[test]
    fn test_completion_tool_output_injection() {
        // A malicious tool output echoed by the LLM should not trigger
        // completion unless it forms a genuine completion phrase.
        assert!(!llm_signals_completion("TASK_COMPLETE"));
        assert!(!llm_signals_completion("JOB_DONE"));
        assert!(!llm_signals_completion(
            "The tool returned: TASK_COMPLETE signal"
        ));
    }

    #[test]
    fn test_classify_replan_reason_for_policy_denial() {
        let err: Error = ToolError::AuthRequired {
            name: "shell".to_string(),
        }
        .into();
        assert_eq!(
            classify_replan_reason_for_step_error(&err),
            Some(ReplanReason::PolicyDenied)
        );
    }

    #[test]
    fn test_classify_replan_reason_for_missing_tool() {
        let err: Error = ToolError::NotFound {
            name: "missing".to_string(),
        }
        .into();
        assert_eq!(
            classify_replan_reason_for_step_error(&err),
            Some(ReplanReason::ToolUnavailable)
        );
    }

    #[test]
    fn test_classify_step_evidence_detects_diff_and_test_commands() {
        let diff = classify_step_evidence(
            "shell",
            &serde_json::json!({"command":"git diff --stat"}),
            "Inspect diff",
        );
        assert_eq!(diff.kind, crate::agent::EvidenceKind::FileDiff);

        let test = classify_step_evidence(
            "shell",
            &serde_json::json!({"command":"cargo test -q"}),
            "Run tests",
        );
        assert_eq!(test.kind, crate::agent::EvidenceKind::TestRun);
    }

    struct NoopTool;

    #[async_trait]
    impl Tool for NoopTool {
        fn name(&self) -> &str {
            "noop_tool"
        }

        fn description(&self) -> &str {
            "No-op test tool"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type":"object",
                "properties": {},
                "additionalProperties": true
            })
        }

        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: &crate::context::JobContext,
        ) -> Result<ToolOutput, crate::tools::ToolError> {
            Ok(ToolOutput::success(
                serde_json::json!({"ok": true}),
                Duration::from_millis(1),
            ))
        }
    }

    struct ApprovalTool;

    #[async_trait]
    impl Tool for ApprovalTool {
        fn name(&self) -> &str {
            "approval_tool"
        }

        fn description(&self) -> &str {
            "Tool that should be blocked by worker policy preflight"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type":"object",
                "properties": {},
                "additionalProperties": true
            })
        }

        fn requires_approval(&self) -> bool {
            true
        }

        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: &crate::context::JobContext,
        ) -> Result<ToolOutput, crate::tools::ToolError> {
            Ok(ToolOutput::success(
                serde_json::json!({"unexpected": "executed"}),
                Duration::from_millis(1),
            ))
        }
    }

    enum ScriptedLlmResponse {
        Complete(String),
        CompleteWithTools(String),
    }

    struct SequenceLlm {
        responses: Mutex<VecDeque<ScriptedLlmResponse>>,
        complete_calls: AtomicU32,
        complete_with_tools_calls: AtomicU32,
    }

    impl SequenceLlm {
        fn new(responses: Vec<ScriptedLlmResponse>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                complete_calls: AtomicU32::new(0),
                complete_with_tools_calls: AtomicU32::new(0),
            }
        }

        fn pop(&self) -> Result<ScriptedLlmResponse, LlmError> {
            self.responses
                .lock()
                .expect("sequence llm mutex poisoned")
                .pop_front()
                .ok_or_else(|| LlmError::InvalidResponse {
                    provider: "sequence-llm".to_string(),
                    reason: "No scripted LLM responses remain".to_string(),
                })
        }

        fn remaining(&self) -> usize {
            self.responses
                .lock()
                .expect("sequence llm mutex poisoned")
                .len()
        }

        fn complete_calls(&self) -> u32 {
            self.complete_calls.load(Ordering::Relaxed)
        }

        fn complete_with_tools_calls(&self) -> u32 {
            self.complete_with_tools_calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl LlmProvider for SequenceLlm {
        fn model_name(&self) -> &str {
            "sequence-llm"
        }

        fn cost_per_token(&self) -> (Decimal, Decimal) {
            (Decimal::ZERO, Decimal::ZERO)
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
        ) -> Result<CompletionResponse, LlmError> {
            self.complete_calls.fetch_add(1, Ordering::Relaxed);
            match self.pop()? {
                ScriptedLlmResponse::Complete(content) => Ok(CompletionResponse {
                    content,
                    input_tokens: 10,
                    output_tokens: 10,
                    finish_reason: FinishReason::Stop,
                    response_id: None,
                }),
                ScriptedLlmResponse::CompleteWithTools(_) => Err(LlmError::InvalidResponse {
                    provider: self.model_name().to_string(),
                    reason: "Scripted response kind mismatch: expected complete()".to_string(),
                }),
            }
        }

        async fn complete_with_tools(
            &self,
            _request: ToolCompletionRequest,
        ) -> Result<ToolCompletionResponse, LlmError> {
            self.complete_with_tools_calls
                .fetch_add(1, Ordering::Relaxed);
            match self.pop()? {
                ScriptedLlmResponse::CompleteWithTools(content) => Ok(ToolCompletionResponse {
                    content: Some(content),
                    tool_calls: Vec::new(),
                    input_tokens: 10,
                    output_tokens: 10,
                    finish_reason: FinishReason::Stop,
                    response_id: None,
                }),
                ScriptedLlmResponse::Complete(_) => Err(LlmError::InvalidResponse {
                    provider: self.model_name().to_string(),
                    reason: "Scripted response kind mismatch: expected complete_with_tools()"
                        .to_string(),
                }),
            }
        }
    }

    #[cfg(feature = "libsql")]
    #[tokio::test]
    async fn test_execute_plan_verifier_block_returns_replan_request_and_persists_verification() {
        let tools = Arc::new(ToolRegistry::new());
        tools.register(Arc::new(NoopTool)).await;
        let llm: Arc<dyn crate::llm::LlmProvider> = Arc::new(StubLlm::new("The job is complete."));
        let harness = TestHarnessBuilder::new()
            .with_tools(Arc::clone(&tools))
            .with_llm(Arc::clone(&llm))
            .build()
            .await;

        let context_manager = Arc::new(ContextManager::new(4));
        let job_id = context_manager
            .create_job_for_user("user-1", "High-risk job", "Need explicit evidence")
            .await
            .expect("create job");
        context_manager
            .update_context(job_id, |ctx| {
                ctx.transition_to(JobState::InProgress, Some("test setup".to_string()))
            })
            .await
            .expect("update context")
            .expect("transition to in_progress");

        let worker = Worker::new(
            job_id,
            WorkerDeps {
                context_manager: Arc::clone(&context_manager),
                llm: Arc::clone(&llm),
                safety: harness.deps.safety.clone(),
                tools: Arc::clone(&tools),
                store: Some(harness.db.clone()),
                hooks: Arc::new(HookRegistry::new()),
                timeout: Duration::from_secs(10),
                use_planning: true,
                autonomy_policy_engine_v1: true,
                autonomy_verifier_v1: true,
                autonomy_replanner_v1: true,
                autonomy_memory_plane_v2: false,
                autonomy_memory_retrieval_v2: false,
            },
        );

        let now = Utc::now();
        let goal_id = Uuid::new_v4();
        let plan_id = Uuid::new_v4();
        let step_id = Uuid::new_v4();

        harness
            .db
            .create_goal(&Goal {
                id: goal_id,
                owner_user_id: "user-1".to_string(),
                channel: Some("worker".to_string()),
                thread_id: None,
                title: "High-risk job".to_string(),
                intent: "Demonstrate verifier block".to_string(),
                priority: 0,
                status: GoalStatus::Active,
                risk_class: GoalRiskClass::High,
                acceptance_criteria: serde_json::json!({"tests":"must pass before completion"}),
                constraints: serde_json::json!({}),
                source: GoalSource::UserRequest,
                created_at: now,
                updated_at: now,
                completed_at: None,
            })
            .await
            .expect("create goal");

        harness
            .db
            .create_plan(&Plan {
                id: plan_id,
                goal_id,
                revision: 1,
                status: PlanStatus::Running,
                planner_kind: PlannerKind::ReasoningV1,
                source_action_plan: None,
                assumptions: serde_json::json!({}),
                confidence: 0.9,
                estimated_cost: None,
                estimated_time_secs: Some(5),
                summary: Some("plan".to_string()),
                created_at: now,
                updated_at: now,
            })
            .await
            .expect("create plan");

        harness
            .db
            .create_plan_steps(&[PlanStep {
                id: step_id,
                plan_id,
                sequence_num: 1,
                kind: PlanStepKind::ToolCall,
                status: PlanStepStatus::Pending,
                title: "noop_tool".to_string(),
                description: "Run noop".to_string(),
                tool_candidates: serde_json::json!([{"tool_name":"noop_tool"}]),
                inputs: serde_json::json!({}),
                preconditions: serde_json::json!([]),
                postconditions: serde_json::json!([]),
                rollback: None,
                policy_requirements: serde_json::json!({}),
                started_at: None,
                completed_at: None,
                created_at: now,
                updated_at: now,
            }])
            .await
            .expect("create step");

        let action_plan: ActionPlan = serde_json::from_value(serde_json::json!({
            "goal": "Demonstrate verifier block",
            "actions": [{
                "tool_name": "noop_tool",
                "parameters": {},
                "reasoning": "Run a no-op step",
                "expected_outcome": "No-op result"
            }],
            "estimated_cost": null,
            "estimated_time_secs": 5,
            "confidence": 0.9
        }))
        .expect("action plan");

        let reasoning = Reasoning::new(
            Arc::clone(&llm),
            Arc::new(SafetyLayer::new(&SafetyConfig {
                max_output_length: 100_000,
                injection_check_enabled: false,
            })),
        );
        let mut reason_ctx = ReasoningContext::new();
        let (_tx, mut rx) = mpsc::channel(4);
        let persisted = PersistedPlanRun {
            goal_id,
            plan_id,
            step_ids: vec![step_id],
        };

        let outcome = worker
            .execute_plan(
                &mut rx,
                &reasoning,
                &mut reason_ctx,
                &action_plan,
                Some(&persisted),
            )
            .await
            .expect("execute plan");

        match outcome {
            PlanExecutionOutcome::ReplanRequested { reason, detail } => {
                assert_eq!(reason, ReplanReason::VerifierInsufficientEvidence);
                assert!(detail.contains("Verifier blocked completion"));
            }
            _ => panic!("expected replan request"),
        }

        let verifications = harness
            .db
            .list_plan_verifications_for_plan(plan_id)
            .await
            .expect("list verifications");
        assert_eq!(verifications.len(), 1);
        assert_eq!(
            verifications[0].status,
            crate::agent::PlanVerificationStatus::Inconclusive
        );
        assert_eq!(verifications[0].completion_claimed, true);
    }

    #[cfg(feature = "libsql")]
    #[tokio::test]
    async fn test_worker_run_step_failure_triggers_replan_and_completes_new_revision() {
        let tools = Arc::new(ToolRegistry::new());
        tools.register(Arc::new(NoopTool)).await;

        let scripted_llm = Arc::new(SequenceLlm::new(vec![
            ScriptedLlmResponse::Complete(
                serde_json::json!({
                    "goal": "Fix the issue",
                    "actions": [{
                        "tool_name": "missing_tool",
                        "parameters": {},
                        "reasoning": "Try a tool that does not exist",
                        "expected_outcome": "This should fail and trigger replan"
                    }],
                    "estimated_cost": null,
                    "estimated_time_secs": 5,
                    "confidence": 0.7
                })
                .to_string(),
            ),
            ScriptedLlmResponse::Complete(
                serde_json::json!({
                    "goal": "Fix the issue",
                    "actions": [{
                        "tool_name": "noop_tool",
                        "parameters": {},
                        "reasoning": "Run the available tool to finish the task",
                        "expected_outcome": "Successful no-op"
                    }],
                    "estimated_cost": null,
                    "estimated_time_secs": 5,
                    "confidence": 0.9
                })
                .to_string(),
            ),
            ScriptedLlmResponse::CompleteWithTools("The job is complete.".to_string()),
        ]));
        let llm: Arc<dyn LlmProvider> = scripted_llm.clone();

        let harness = TestHarnessBuilder::new()
            .with_tools(Arc::clone(&tools))
            .with_llm(Arc::clone(&llm))
            .build()
            .await;

        let context_manager = Arc::new(ContextManager::new(4));
        let job_id = context_manager
            .create_job_for_user("user-1", "Replan test", "Trigger an automatic replan")
            .await
            .expect("create job");
        context_manager
            .update_context(job_id, |ctx| {
                ctx.transition_to(JobState::InProgress, Some("test setup".to_string()))
            })
            .await
            .expect("update context")
            .expect("transition to in_progress");

        let worker = Worker::new(
            job_id,
            WorkerDeps {
                context_manager: Arc::clone(&context_manager),
                llm: Arc::clone(&llm),
                safety: harness.deps.safety.clone(),
                tools: Arc::clone(&tools),
                store: Some(harness.db.clone()),
                hooks: Arc::new(HookRegistry::new()),
                timeout: Duration::from_secs(10),
                use_planning: true,
                autonomy_policy_engine_v1: true,
                autonomy_verifier_v1: true,
                autonomy_replanner_v1: true,
                autonomy_memory_plane_v2: false,
                autonomy_memory_retrieval_v2: false,
            },
        );

        let (tx, rx) = mpsc::channel(4);
        tx.send(crate::agent::scheduler::WorkerMessage::Start)
            .await
            .expect("send start");
        worker.run(rx).await.expect("worker run");

        let ctx = context_manager
            .get_context(job_id)
            .await
            .expect("job context");
        assert_eq!(ctx.state, JobState::Completed);
        let goal_id = ctx.autonomy_goal_id.expect("goal link persisted");
        let final_plan_id = ctx.autonomy_plan_id.expect("plan link persisted");

        let goal = harness
            .db
            .get_goal(goal_id)
            .await
            .expect("get goal")
            .expect("goal exists");
        assert_eq!(goal.status, GoalStatus::Completed);

        let mut plans = harness
            .db
            .list_plans_for_goal(goal_id)
            .await
            .expect("list plans");
        plans.sort_by_key(|p| p.revision);
        assert_eq!(
            plans.len(),
            2,
            "expected initial plan plus one replan revision"
        );
        assert_eq!(plans[0].revision, 1);
        assert_eq!(plans[0].status, PlanStatus::Superseded);
        assert_eq!(plans[1].revision, 2);
        assert_eq!(plans[1].status, PlanStatus::Completed);
        assert_eq!(plans[1].id, final_plan_id);

        let rev1_steps = harness
            .db
            .list_plan_steps_for_plan(plans[0].id)
            .await
            .expect("list rev1 steps");
        let rev2_steps = harness
            .db
            .list_plan_steps_for_plan(plans[1].id)
            .await
            .expect("list rev2 steps");
        assert_eq!(rev1_steps.len(), 1);
        assert_eq!(rev2_steps.len(), 1);
        assert_eq!(rev1_steps[0].status, PlanStepStatus::Failed);
        assert_eq!(rev2_steps[0].status, PlanStepStatus::Succeeded);
        assert!(rev1_steps[0].sequence_num > 0);
        assert!(rev2_steps[0].sequence_num > 0);

        let rev1_verifications = harness
            .db
            .list_plan_verifications_for_plan(plans[0].id)
            .await
            .expect("list rev1 verifications");
        let rev2_verifications = harness
            .db
            .list_plan_verifications_for_plan(plans[1].id)
            .await
            .expect("list rev2 verifications");
        assert!(
            rev1_verifications.is_empty(),
            "step failure replan should occur before final completion verification"
        );
        assert_eq!(rev2_verifications.len(), 1);
        assert_eq!(
            rev2_verifications[0].status,
            crate::agent::PlanVerificationStatus::Passed
        );
        assert!(rev2_verifications[0].completion_claimed);

        assert_eq!(scripted_llm.complete_calls(), 2);
        assert_eq!(scripted_llm.complete_with_tools_calls(), 1);
        assert_eq!(scripted_llm.remaining(), 0);
    }

    #[cfg(feature = "libsql")]
    #[tokio::test]
    async fn test_execute_tool_inner_policy_denial_persists_autonomy_policy_record() {
        let tools = Arc::new(ToolRegistry::new());
        tools.register(Arc::new(ApprovalTool)).await;
        let llm: Arc<dyn LlmProvider> = Arc::new(StubLlm::new("unused"));
        let harness = TestHarnessBuilder::new()
            .with_tools(Arc::clone(&tools))
            .with_llm(Arc::clone(&llm))
            .build()
            .await;

        let context_manager = Arc::new(ContextManager::new(4));
        let job_id = context_manager
            .create_job_for_user("user-1", "Policy preflight test", "Should require approval")
            .await
            .expect("create job");

        let goal_id = Uuid::new_v4();
        let now = Utc::now();
        harness
            .db
            .create_goal(&Goal {
                id: goal_id,
                owner_user_id: "user-1".to_string(),
                channel: Some("worker".to_string()),
                thread_id: None,
                title: "Policy preflight test".to_string(),
                intent: "Assert worker policy preflight persistence".to_string(),
                priority: 0,
                status: GoalStatus::Active,
                risk_class: GoalRiskClass::Medium,
                acceptance_criteria: serde_json::json!({}),
                constraints: serde_json::json!({}),
                source: GoalSource::UserRequest,
                created_at: now,
                updated_at: now,
                completed_at: None,
            })
            .await
            .expect("create goal");

        context_manager
            .update_context(job_id, |ctx| -> Result<(), String> {
                ctx.transition_to(JobState::InProgress, Some("test setup".to_string()))?;
                ctx.set_autonomy_links(Some(goal_id), None);
                Ok(())
            })
            .await
            .expect("update context")
            .expect("context setup");

        let deps = WorkerDeps {
            context_manager: Arc::clone(&context_manager),
            llm: Arc::clone(&llm),
            safety: harness.deps.safety.clone(),
            tools: Arc::clone(&tools),
            store: Some(harness.db.clone()),
            hooks: Arc::new(HookRegistry::new()),
            timeout: Duration::from_secs(10),
            use_planning: true,
            autonomy_policy_engine_v1: true,
            autonomy_verifier_v1: true,
            autonomy_replanner_v1: true,
            autonomy_memory_plane_v2: false,
            autonomy_memory_retrieval_v2: false,
        };

        let err =
            Worker::execute_tool_inner(&deps, job_id, "approval_tool", &serde_json::json!({}))
                .await
                .expect_err("approval tool should be blocked before execution");
        match err {
            Error::Tool(ToolError::AuthRequired { name }) => assert_eq!(name, "approval_tool"),
            other => panic!("expected AuthRequired, got {other}"),
        }

        let record = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let rows = harness
                    .db
                    .list_policy_decisions_for_goal(goal_id)
                    .await
                    .expect("list policy decisions");
                if let Some(row) = rows
                    .iter()
                    .find(|r| r.tool_name.as_deref() == Some("approval_tool"))
                    .cloned()
                {
                    break row;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("policy decision persistence timeout");

        assert_eq!(
            record.decision,
            crate::agent::PolicyDecisionKind::RequireApproval
        );
        assert_eq!(record.action_kind, "tool_call_preflight");
        assert_eq!(record.goal_id, Some(goal_id));
        assert_eq!(record.plan_id, None);
        assert_eq!(record.user_id, "user-1");
        assert_eq!(record.channel, "worker");
        assert_eq!(record.tool_name.as_deref(), Some("approval_tool"));
        assert!(record.requires_approval);
        assert_eq!(record.auto_approved, Some(false));
        assert!(
            record
                .reason_codes
                .iter()
                .any(|c| c == "tool_requires_approval")
        );
    }
}
