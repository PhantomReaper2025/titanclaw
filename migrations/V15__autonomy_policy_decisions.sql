-- ==================== Autonomy Control Plane v1: Policy Decisions ====================

CREATE TABLE IF NOT EXISTS autonomy_policy_decisions (
    id UUID PRIMARY KEY,
    goal_id UUID REFERENCES autonomy_goals(id) ON DELETE SET NULL,
    plan_id UUID REFERENCES autonomy_plans(id) ON DELETE SET NULL,
    plan_step_id UUID REFERENCES autonomy_plan_steps(id) ON DELETE SET NULL,
    execution_attempt_id UUID REFERENCES autonomy_execution_attempts(id) ON DELETE SET NULL,
    user_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    tool_name TEXT,
    tool_call_id TEXT,
    action_kind TEXT NOT NULL,
    decision TEXT NOT NULL,
    reason_codes JSONB NOT NULL DEFAULT '[]'::jsonb,
    risk_score REAL,
    confidence REAL,
    requires_approval BOOLEAN NOT NULL DEFAULT FALSE,
    auto_approved BOOLEAN,
    evidence_required JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_autonomy_policy_decisions_goal_created
    ON autonomy_policy_decisions(goal_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_policy_decisions_execution_attempt
    ON autonomy_policy_decisions(execution_attempt_id);

CREATE INDEX IF NOT EXISTS idx_autonomy_policy_decisions_decision_created
    ON autonomy_policy_decisions(decision, created_at DESC);
