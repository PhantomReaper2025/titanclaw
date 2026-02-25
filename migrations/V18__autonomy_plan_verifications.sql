-- ==================== Autonomy Control Plane v1: Plan Verifications ====================
-- Stores verifier/evidence-gate outcomes tied to plans (worker/dispatcher hooks).

CREATE TABLE IF NOT EXISTS autonomy_plan_verifications (
    id UUID PRIMARY KEY,
    goal_id UUID REFERENCES autonomy_goals(id) ON DELETE SET NULL,
    plan_id UUID NOT NULL REFERENCES autonomy_plans(id) ON DELETE CASCADE,
    job_id UUID,
    user_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    verifier_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    completion_claimed BOOLEAN NOT NULL DEFAULT FALSE,
    evidence_count INTEGER NOT NULL DEFAULT 0,
    summary TEXT NOT NULL,
    checks JSONB NOT NULL DEFAULT '{}'::jsonb,
    evidence JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_autonomy_plan_verifications_plan_created
    ON autonomy_plan_verifications(plan_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_plan_verifications_goal_created
    ON autonomy_plan_verifications(goal_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_plan_verifications_status_created
    ON autonomy_plan_verifications(status, created_at DESC);
