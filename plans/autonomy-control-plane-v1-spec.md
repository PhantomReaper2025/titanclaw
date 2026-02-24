# TitanClaw Autonomy Control Plane v1 Spec

Status: Draft (implementation-ready baseline)
Date: 2026-02-24
Scope: Phase 1 roadmap prerequisite and execution contract

## Purpose

This spec is the immediate follow-on artifact to `plans/titanclaw-jarvis-grade-roadmap.md`.

It defines the minimum decision-complete design for:

1. Goal/plan schema and DB migrations
2. Policy engine decision matrix
3. Planner I/O contract
4. Verifier evidence model
5. Dispatcher integration points
6. Phase 1 acceptance benchmark set

## Baseline (Code-Verified)

TitanClaw already has pieces that should be reused rather than replaced:

1. Tool-loop policy and telemetry hooks already exist in `src/agent/dispatcher.rs`.
   - Approval gating emits `PolicyDecisionRecord` in the tool call loop (`src/agent/dispatcher.rs:768`, `src/agent/dispatcher.rs:791`, `src/agent/dispatcher.rs:813`, `src/agent/dispatcher.rs:831`, `src/agent/dispatcher.rs:852`).
   - Tool execution emits `ExecutionAttemptRecord` (`src/agent/dispatcher.rs:938`).
2. Structured telemetry types already exist in `src/agent/autonomy_telemetry.rs`.
   - `PolicyDecisionRecord`, `ExecutionAttemptRecord`, `PolicyDecisionKind`, `FailureClass`.
3. Worker-side planning already exists as an LLM planning primitive.
   - `ActionPlan` / `PlannedAction` in `src/llm/reasoning.rs:76` and `src/llm/reasoning.rs:89`.
   - Worker creates and executes plans in `src/agent/worker.rs:175`, `src/agent/worker.rs:640`.
4. Routine-triggered and scheduled work already uses a durable job path.
   - `RoutineEngine` (`src/agent/routine_engine.rs`) creates job records and schedules execution.
   - `Scheduler::schedule(&self, job_id)` remains the stable execution entry (`src/agent/scheduler.rs:113`).
5. Dual DB backend exists and must be preserved.
   - Shared trait surface in `src/db/mod.rs`
   - Postgres backend in `src/db/postgres.rs`
   - libSQL backend in `src/db/libsql/*`
   - libSQL schema is embedded in `src/db/libsql_migrations.rs`

Implication: Control Plane v1 should extend current telemetry and worker planning paths, not introduce a parallel autonomy stack.

## Non-Goals (v1)

1. No autonomous self-deploy.
2. No memory tiering overhaul (Phase 2).
3. No tool contract v2 migration (Phase 3).
4. No cross-goal attention manager (Phase 4).
5. No hard requirement to replace `ActionPlan` immediately (bridge adapter first).

## v1 Architecture Slice

Control Plane v1 adds a goal-centric state layer above the current job/tool execution pipeline:

1. `GoalManager` (persisted goals and status)
2. `PlannerV1` (structured plans; bridge from existing `Reasoning::plan`)
3. `PolicyEngineV1` (centralized allow/approval/deny decisioning)
4. `VerifierV1` (evidence gate for completion)
5. `AutonomyStore` (goal/plan/policy/execution persistence)

Execution remains on current paths:

1. `dispatcher` for interactive tool loops
2. `worker` for sandbox/scheduled autonomous jobs
3. `scheduler` / `routine_engine` for orchestration

## Domain Model (Rust Types)

Create a new module (recommended: `src/agent/autonomy.rs`) with v1 types. Keep serialization stable (`serde`) and DB-friendly enums (`snake_case`).

### 1. Goal

```rust
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
    pub acceptance_criteria: serde_json::Value,
    pub constraints: serde_json::Value,
    pub source: GoalSource,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}
```

`GoalStatus` (v1):

1. `proposed`
2. `active`
3. `blocked`
4. `waiting`
5. `completed`
6. `abandoned`

`GoalSource` (v1):

1. `user_request`
2. `routine_trigger`
3. `system_maintenance`
4. `follow_up`

### 2. Plan

Bridge from existing `ActionPlan` (`src/llm/reasoning.rs`) without breaking worker flow.

```rust
pub struct Plan {
    pub id: Uuid,
    pub goal_id: Uuid,
    pub revision: i32,
    pub status: PlanStatus,
    pub planner_kind: PlannerKind,
    pub source_action_plan: Option<serde_json::Value>,
    pub assumptions: serde_json::Value,
    pub confidence: f64,
    pub estimated_cost: Option<f64>,
    pub estimated_time_secs: Option<u64>,
    pub summary: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

`PlanStatus` (v1):

1. `draft`
2. `ready`
3. `running`
4. `paused`
5. `failed`
6. `completed`
7. `superseded`

### 3. PlanStep

```rust
pub struct PlanStep {
    pub id: Uuid,
    pub plan_id: Uuid,
    pub sequence_num: i32,
    pub kind: PlanStepKind,
    pub status: PlanStepStatus,
    pub title: String,
    pub description: String,
    pub tool_candidates: serde_json::Value,
    pub inputs: serde_json::Value,
    pub preconditions: serde_json::Value,
    pub postconditions: serde_json::Value,
    pub rollback: Option<serde_json::Value>,
    pub policy_requirements: serde_json::Value,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

`PlanStepKind` (v1):

1. `tool_call`
2. `evidence_gather`
3. `verification`
4. `ask_user`

`PlanStepStatus` (v1):

1. `pending`
2. `running`
3. `succeeded`
4. `failed`
5. `blocked`
6. `skipped`

### 4. PolicyDecision (Persistent Record)

Do not replace `src/agent/autonomy_telemetry.rs::PolicyDecisionRecord`; extend it and persist it.

```rust
pub struct PolicyDecisionV1 {
    pub id: Uuid,
    pub goal_id: Option<Uuid>,
    pub plan_id: Option<Uuid>,
    pub plan_step_id: Option<Uuid>,
    pub execution_attempt_id: Option<Uuid>,
    pub channel: String,
    pub user_id: String,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    pub action_kind: String,
    pub decision: PolicyDecisionKindV1,
    pub reason_codes: Vec<String>,
    pub risk_score: Option<f32>,
    pub confidence: Option<f32>,
    pub requires_approval: bool,
    pub auto_approved: Option<bool>,
    pub evidence_required: serde_json::Value,
    pub created_at: DateTime<Utc>,
}
```

### 5. ExecutionAttempt (Persistent Record)

Do not replace `ExecutionAttemptRecord`; extend and persist it.

```rust
pub struct ExecutionAttemptV1 {
    pub id: Uuid,
    pub goal_id: Option<Uuid>,
    pub plan_id: Option<Uuid>,
    pub plan_step_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub channel: String,
    pub user_id: String,
    pub tool_name: String,
    pub tool_call_id: Option<String>,
    pub tool_args: Option<serde_json::Value>,
    pub status: ExecutionAttemptStatusV1,
    pub failure_class: Option<String>,
    pub retry_count: i32,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub elapsed_ms: Option<i64>,
    pub result_summary: Option<String>,
    pub error_preview: Option<String>,
}
```

### 6. Verifier Evidence

```rust
pub struct EvidenceItem {
    pub kind: EvidenceKind,
    pub source: String,
    pub summary: String,
    pub confidence: f32,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}
```

`EvidenceKind` (v1):

1. `tool_result`
2. `file_diff`
3. `test_run`
4. `command_output`
5. `observation`
6. `user_confirmation`

## Database and Storage Design

## Backend Strategy (Required)

TitanClaw currently supports Postgres and libSQL. v1 must preserve both:

1. Postgres: add versioned SQL migrations in `migrations/`.
2. libSQL: extend embedded `SCHEMA` in `src/db/libsql_migrations.rs`.
3. Trait surface: extend `src/db/mod.rs` so handlers/services can use `Arc<dyn Database>`.

Do not ship v1 features in Postgres-only mode.

## New DB Traits

Add subtraits in `src/db/mod.rs` and compose into `Database`:

1. `GoalStore`
2. `PlanStore`
3. `AutonomyExecutionStore` (execution attempts, policy decisions, incidents)

Minimum methods (v1):

1. `create_goal`, `get_goal`, `list_goals`, `update_goal_status`
2. `create_plan`, `list_plans_for_goal`, `get_plan`, `update_plan_status`, `create_plan_steps`, `update_plan_step_status`
3. `record_execution_attempt`, `update_execution_attempt`, `record_policy_decision`
4. `list_execution_attempts_for_plan`, `list_policy_decisions_for_goal`

## Postgres Migration Sequence (v1)

Use `refinery` versioned files in lexical order.

1. `V11__autonomy_goals.sql`
2. `V12__autonomy_plans.sql`
3. `V13__autonomy_plan_steps.sql`
4. `V14__autonomy_execution_attempts.sql`
5. `V15__autonomy_policy_decisions.sql`
6. `V16__autonomy_incidents.sql` (optional in v1 MVP, but recommended to align telemetry with roadmap Phase 0/1)

### Table Requirements

`autonomy_goals`

1. `id UUID PRIMARY KEY`
2. `owner_user_id TEXT NOT NULL`
3. `channel TEXT`
4. `thread_id UUID`
5. `title TEXT NOT NULL`
6. `intent TEXT NOT NULL`
7. `priority INTEGER NOT NULL DEFAULT 0`
8. `status TEXT NOT NULL`
9. `risk_class TEXT NOT NULL`
10. `acceptance_criteria JSONB NOT NULL DEFAULT '{}'::jsonb`
11. `constraints JSONB NOT NULL DEFAULT '{}'::jsonb`
12. `source TEXT NOT NULL`
13. `created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()`
14. `updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()`
15. `completed_at TIMESTAMPTZ`

Indexes:

1. `(owner_user_id, status)`
2. `(status, updated_at DESC)`
3. `(thread_id)` partial where not null

`autonomy_plans`

1. `id UUID PRIMARY KEY`
2. `goal_id UUID NOT NULL REFERENCES autonomy_goals(id) ON DELETE CASCADE`
3. `revision INTEGER NOT NULL`
4. `status TEXT NOT NULL`
5. `planner_kind TEXT NOT NULL`
6. `source_action_plan JSONB`
7. `assumptions JSONB NOT NULL DEFAULT '{}'::jsonb`
8. `confidence DOUBLE PRECISION NOT NULL`
9. `estimated_cost DOUBLE PRECISION`
10. `estimated_time_secs BIGINT`
11. `summary TEXT`
12. `created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()`
13. `updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()`

Indexes/constraints:

1. `UNIQUE(goal_id, revision)`
2. `(goal_id, status)`

`autonomy_plan_steps`

1. `id UUID PRIMARY KEY`
2. `plan_id UUID NOT NULL REFERENCES autonomy_plans(id) ON DELETE CASCADE`
3. `sequence_num INTEGER NOT NULL`
4. `kind TEXT NOT NULL`
5. `status TEXT NOT NULL`
6. `title TEXT NOT NULL`
7. `description TEXT NOT NULL`
8. `tool_candidates JSONB NOT NULL DEFAULT '[]'::jsonb`
9. `inputs JSONB NOT NULL DEFAULT '{}'::jsonb`
10. `preconditions JSONB NOT NULL DEFAULT '[]'::jsonb`
11. `postconditions JSONB NOT NULL DEFAULT '[]'::jsonb`
12. `rollback JSONB`
13. `policy_requirements JSONB NOT NULL DEFAULT '{}'::jsonb`
14. timestamps (`started_at`, `completed_at`, `created_at`, `updated_at`)

Indexes/constraints:

1. `UNIQUE(plan_id, sequence_num)`
2. `(plan_id, status)`

`autonomy_execution_attempts`

1. `id UUID PRIMARY KEY`
2. `goal_id UUID REFERENCES autonomy_goals(id) ON DELETE SET NULL`
3. `plan_id UUID REFERENCES autonomy_plans(id) ON DELETE SET NULL`
4. `plan_step_id UUID REFERENCES autonomy_plan_steps(id) ON DELETE SET NULL`
5. `job_id UUID`
6. `thread_id UUID`
7. `user_id TEXT NOT NULL`
8. `channel TEXT NOT NULL`
9. `tool_name TEXT NOT NULL`
10. `tool_call_id TEXT`
11. `tool_args JSONB`
12. `status TEXT NOT NULL`
13. `failure_class TEXT`
14. `retry_count INTEGER NOT NULL DEFAULT 0`
15. `started_at TIMESTAMPTZ NOT NULL DEFAULT NOW()`
16. `finished_at TIMESTAMPTZ`
17. `elapsed_ms BIGINT`
18. `result_summary TEXT`
19. `error_preview TEXT`

Indexes:

1. `(plan_step_id, started_at DESC)`
2. `(goal_id, started_at DESC)`
3. `(tool_name, status, started_at DESC)`

`autonomy_policy_decisions`

1. `id UUID PRIMARY KEY`
2. `goal_id UUID REFERENCES autonomy_goals(id) ON DELETE SET NULL`
3. `plan_id UUID REFERENCES autonomy_plans(id) ON DELETE SET NULL`
4. `plan_step_id UUID REFERENCES autonomy_plan_steps(id) ON DELETE SET NULL`
5. `execution_attempt_id UUID REFERENCES autonomy_execution_attempts(id) ON DELETE SET NULL`
6. `user_id TEXT NOT NULL`
7. `channel TEXT NOT NULL`
8. `tool_name TEXT`
9. `tool_call_id TEXT`
10. `action_kind TEXT NOT NULL`
11. `decision TEXT NOT NULL`
12. `reason_codes JSONB NOT NULL DEFAULT '[]'::jsonb`
13. `risk_score REAL`
14. `confidence REAL`
15. `requires_approval BOOLEAN NOT NULL DEFAULT FALSE`
16. `auto_approved BOOLEAN`
17. `evidence_required JSONB NOT NULL DEFAULT '{}'::jsonb`
18. `created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()`

Indexes:

1. `(goal_id, created_at DESC)`
2. `(execution_attempt_id)`
3. `(decision, created_at DESC)`

## libSQL Mirror

The same logical tables/columns must be mirrored into `src/db/libsql_migrations.rs::SCHEMA` using SQLite-compatible types:

1. `UUID` -> `TEXT`
2. `TIMESTAMPTZ` -> `TEXT` ISO timestamps
3. `JSONB` -> `TEXT` JSON
4. `TEXT[]` / array fields -> JSON text

Use existing libSQL helper patterns in `src/db/libsql/*` for serialization/deserialization.

## Planner I/O Contract (v1)

## Goal

Standardize planner I/O while reusing current `Reasoning::plan` output.

## Planner Input

```rust
pub struct PlannerRequestV1 {
    pub goal_id: Uuid,
    pub task_class: PlannerTaskClass,
    pub user_id: String,
    pub channel: Option<String>,
    pub thread_id: Option<Uuid>,
    pub prompt: String,
    pub context_messages: Vec<ChatMessage>,
    pub constraints: serde_json::Value,
    pub risk_class: GoalRiskClass,
    pub budget: PlannerBudget,
    pub prior_observations: Vec<serde_json::Value>,
}
```

`PlannerTaskClass` (v1):

1. `coding_change`
2. `research_summary`
3. `system_admin`
4. `routine_automation`

`PlannerBudget`:

1. `max_steps`
2. `max_estimated_cost`
3. `max_runtime_secs`
4. `max_replans`

## Planner Output

```rust
pub struct PlannerResultV1 {
    pub goal: Goal,
    pub plan: Plan,
    pub steps: Vec<PlanStep>,
    pub planner_trace: PlannerTraceSummary,
    pub warnings: Vec<String>,
}
```

`PlannerTraceSummary` is intentionally summary-only (no chain-of-thought persistence):

1. `planner_kind`
2. `model`
3. `task_class`
4. `step_count`
5. `confidence`
6. `uncertainty_sources` (labels only)

## Bridge Adapter (Required)

Create a bridge adapter from `ActionPlan` (`src/llm/reasoning.rs`) to `Plan + PlanStep[]`:

1. `ActionPlan.goal` -> `Goal.intent` or `Plan.summary`
2. `ActionPlan.confidence` -> `Plan.confidence`
3. `ActionPlan.estimated_cost` -> `Plan.estimated_cost`
4. `ActionPlan.estimated_time_secs` -> `Plan.estimated_time_secs`
5. `PlannedAction` -> `PlanStep(kind=tool_call, tool_candidates=[{tool_name,...}])`

This allows v1 persistence without blocking on a new planner implementation.

## Replan Triggers (v1)

A replan must be attempted (or user asked) when any of the following occurs:

1. plan step fails with non-retryable `failure_class`
2. policy decision = `deny`
3. repeated `require_approval` blocks a step beyond budget
4. verifier evidence is insufficient after nominal completion
5. environment mismatch invalidates preconditions
6. user interrupt or follow-up modifies acceptance criteria

## Policy Engine v1

## Contract

Centralize decisioning for high-level actions and tool calls.

```rust
pub struct PolicyRequestV1 {
    pub goal_id: Option<Uuid>,
    pub plan_id: Option<Uuid>,
    pub plan_step_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub user_id: String,
    pub channel: String,
    pub action_kind: ActionKindV1,
    pub tool_name: Option<String>,
    pub tool_args: Option<serde_json::Value>,
    pub risk_class: GoalRiskClass,
    pub confidence: Option<f32>,
    pub environment_mode: EnvironmentModeV1,
    pub evidence_state: EvidenceStateV1,
}
```

Output:

```rust
pub struct PolicyResultV1 {
    pub decision: PolicyDecisionKindV1,
    pub reason_codes: Vec<String>,
    pub requires_approval: bool,
    pub risk_score: f32,
    pub evidence_required: serde_json::Value,
    pub allow_with_constraints: serde_json::Value,
}
```

`PolicyDecisionKindV1`:

1. `allow`
2. `allow_with_logging`
3. `require_approval`
4. `deny`
5. `require_more_evidence`
6. `modify` (carry forward existing dispatcher hook semantics)

## Decision Matrix (v1 Default Policy)

| Action profile | Risk class | Confidence | Dry-run/evidence | Decision |
|---|---:|---:|---|---|
| Read-only tool / introspection | low | any | none | `allow_with_logging` |
| Workspace write (non-destructive) | low | >=0.7 | evidence optional | `allow_with_logging` |
| Workspace write (destructive params detected) | any | any | none | `require_approval` |
| System mutation / external side effect | med/high | <0.8 | no dry-run | `require_approval` |
| System mutation / external side effect | high | any | weak evidence | `require_more_evidence` or `require_approval` |
| Credential/auth flow | any | any | N/A | `require_approval` (unless existing auth interception path handles token exchange) |
| Unsafe / policy-forbidden action | any | any | any | `deny` |

Notes:

1. Existing per-tool approval behavior in `src/agent/dispatcher.rs` remains active during migration.
2. PolicyEngine v1 wraps current checks and produces a normalized record before/after existing dispatcher decisions.
3. `modify` is reserved for hook-driven parameter rewrites already present in dispatcher.

## Verifier Evidence Model (v1)

## Objective

Prevent "completion theater" by requiring evidence aligned with goal acceptance criteria.

## Verifier Input

```rust
pub struct VerifierRequestV1 {
    pub goal: Goal,
    pub plan: Plan,
    pub steps: Vec<PlanStep>,
    pub execution_attempts: Vec<ExecutionAttemptV1>,
    pub policy_decisions: Vec<PolicyDecisionV1>,
    pub evidence: Vec<EvidenceItem>,
}
```

## Verifier Output

```rust
pub struct VerifierResultV1 {
    pub status: VerifierStatusV1,
    pub matched_criteria: Vec<String>,
    pub missing_criteria: Vec<String>,
    pub contradictions: Vec<String>,
    pub recommended_action: VerifierActionV1,
}
```

`VerifierStatusV1`:

1. `pass`
2. `inconclusive`
3. `fail`

`VerifierActionV1`:

1. `complete_goal`
2. `gather_evidence`
3. `retry_step`
4. `replan`
5. `ask_user`
6. `rollback`

## v1 Evidence Rules by Task Class

`coding_change`

1. At least one execution attempt or diff-producing action
2. Changed files evidence present
3. Test/lint evidence present or explicit skip reason evidence
4. No unresolved `deny` policy decisions tied to the final plan revision

`research_summary`

1. Source observations recorded
2. Summary evidence recorded
3. Confidence threshold or explicit uncertainty note

`system_admin`

1. Pre-check evidence
2. Action execution evidence
3. Post-check evidence
4. Approval record for any protected action

`routine_automation`

1. Trigger provenance (cron/event/webhook/manual)
2. Plan execution trace
3. Outcome summary evidence

## Integration Points (Code Mapping)

## 1. Interactive Dispatcher (`src/agent/dispatcher.rs`)

Primary integration function:

1. `impl Agent/run_agentic_loop` (`src/agent/dispatcher.rs:158`)

Hook points:

1. Tool approval branch (`~741-862`)
   - Wrap current `requires_approval` and hook-policy checks with `PolicyEngineV1::evaluate`.
   - Persist `PolicyDecisionV1`.
   - Continue emitting existing `PolicyDecisionRecord` for log compatibility.
2. Tool completion branch (`~938`)
   - Generate/persist `ExecutionAttemptV1`.
   - Continue emitting existing `ExecutionAttemptRecord`.

Migration rule:

1. Do not break current approval UX or session auto-approval behavior.

## 2. Worker Planning + Execution (`src/agent/worker.rs`)

Hook points (verified by code and explorer scan):

1. Plan creation log path (`src/agent/worker.rs:175`)
   - After `Reasoning::plan` succeeds and before execution, create `Goal` + `Plan` + `PlanStep[]`.
2. `execute_plan` loop (`src/agent/worker.rs:640`)
   - Mark step start/success/failure.
   - Attach step-level policy and execution records.
3. `execute_tool_inner` shared tool path (`src/agent/worker.rs` around explorer-identified `~370`)
   - Normalize worker telemetry to match dispatcher execution attempt schema.

## 3. Autonomy Telemetry (`src/agent/autonomy_telemetry.rs`)

Short-term:

1. Keep current tracing JSON emitters unchanged for compatibility.
2. Add conversion helpers from persistent v1 records to log records.

Medium-term:

1. Add DB sink calls behind feature/flag or service boundary after v1 schema lands.

## 4. Scheduler / Routines / Context

Current stable path:

1. `RoutineEngine` triggers work and uses `ContextManager::create_job_for_user` + `Scheduler::schedule`.
2. `Scheduler` remains job-ID-based (`src/agent/scheduler.rs:113`).

Required v1 propagation:

1. Extend `JobContext` in `src/context/state.rs` with optional `goal_id`, `plan_id`, `plan_step_id`.
2. Add an overload/variant of `ContextManager::create_job_for_user` in `src/context/manager.rs` that accepts autonomy metadata (or a `JobCreateOptions` struct).
3. For routine-triggered autonomous work, create a goal before scheduling and store `goal_id` on the job context.

## 5. CLI and Web APIs

### CLI (`src/cli/mod.rs`, `src/main.rs`)

Add subcommands:

1. `Goal(GoalCommand)`
2. `Plan(PlanCommand)`

New modules:

1. `src/cli/goal.rs`
2. `src/cli/plan.rs`

Pattern to follow:

1. `src/cli/memory.rs` (`Subcommand` enum + `run_*_command[_with_db]`)
2. `src/main.rs` top-level `match &cli.command` for logging + DB setup before dispatch

### Web (`src/channels/web/server.rs`, `src/channels/web/handlers/*`)

Add routes in `src/channels/web/server.rs` near existing `/api/routines` block:

1. `GET /api/goals`
2. `POST /api/goals`
3. `GET /api/goals/{id}`
4. `POST /api/goals/{id}/cancel`
5. `GET /api/plans`
6. `POST /api/plans`
7. `GET /api/plans/{id}`
8. `POST /api/plans/{id}/replan`
9. `GET /api/executions/{id}` (Phase 1 or 1.5)

New handler modules:

1. `src/channels/web/handlers/goals.rs`
2. `src/channels/web/handlers/plans.rs`

Update exports:

1. `src/channels/web/handlers/mod.rs`

Handler pattern:

1. Follow `routines_list_handler` / `routines_trigger_handler` in `src/channels/web/handlers/routines.rs`
2. Use `State(Arc<GatewayState>)`
3. Require `state.store`
4. Reuse `state.msg_tx` when plan execution should enqueue work

## Phase 1 Benchmark / Acceptance Set

## Benchmarks (Minimum)

Run against a reproducible local environment with tracing and DB persistence enabled.

1. Coding change benchmark (10 tasks)
   - requires file edits + tests
   - at least 2 include initial test failure and replan
2. Research/summarization benchmark (10 tasks)
   - multi-source evidence capture
3. System admin benchmark (8 tasks)
   - includes dry-run/approval boundary checks
4. Routine automation benchmark (8 tasks)
   - cron/event/manual triggers via existing routine paths

## Required Metrics

1. goal creation rate (for autonomous actions) = 100%
2. plan trace persistence coverage >= 95%
3. policy decision persistence coverage >= 95%
4. explicit verifier result coverage >= 90%
5. multi-step local task completion with explicit plan trace >= 80%

## Acceptance Criteria (Phase 1)

1. New autonomous actions (worker/routine initiated) require a persisted `Goal`.
2. `Reasoning::plan` outputs are persisted as `Plan` + `PlanStep[]` via bridge adapter.
3. Dispatcher and worker tool calls persist structured `ExecutionAttemptV1`.
4. Approval and hook decisions persist structured `PolicyDecisionV1`.
5. Goal/plan status can pause/resume/replan after a failed step.
6. CLI and web APIs can create/list/inspect goals and plans.
7. Verifier emits explicit pass/fail/inconclusive on task completion attempt.

## Delivery Sequence (Recommended PR Order)

1. PR1: domain types + DB traits + Postgres/libSQL schema (no runtime wiring)
2. PR2: store implementations + CLI read/list endpoints
3. PR3: worker plan bridge (`ActionPlan` -> `Plan/PlanStep`) + status updates
4. PR4: dispatcher/worker policy + execution persistence integration
5. PR5: verifier v1 + completion gating
6. PR6: web create/replan endpoints + benchmark harness

## Risks and Mitigations

1. Risk: dual-backend drift (Postgres vs libSQL)
   - Mitigation: define shared Rust DTOs first; add parity tests for CRUD and serialization.
2. Risk: telemetry duplication/conflicting IDs during migration
   - Mitigation: preserve existing log emitters and introduce persistent record IDs independently.
3. Risk: goal requirement breaks legacy flows
   - Mitigation: create implicit goals for worker/routine paths during transition.
4. Risk: verifier blocks legitimate completions
   - Mitigation: start with `inconclusive -> ask_user` fallback and task-class-specific thresholds.

## Immediate Implementation Start (Next Task)

Implement PR1 from the delivery sequence:

1. add autonomy domain types module
2. extend `src/db/mod.rs` traits
3. add `migrations/V11__...` through `V16__...`
4. mirror tables in `src/db/libsql_migrations.rs`

This unlocks all later wiring without forcing behavioral changes yet.
