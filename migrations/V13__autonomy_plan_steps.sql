-- ==================== Autonomy Control Plane v1: Plan Steps ====================

CREATE TABLE IF NOT EXISTS autonomy_plan_steps (
    id UUID PRIMARY KEY,
    plan_id UUID NOT NULL REFERENCES autonomy_plans(id) ON DELETE CASCADE,
    sequence_num INTEGER NOT NULL,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    tool_candidates JSONB NOT NULL DEFAULT '[]'::jsonb,
    inputs JSONB NOT NULL DEFAULT '{}'::jsonb,
    preconditions JSONB NOT NULL DEFAULT '[]'::jsonb,
    postconditions JSONB NOT NULL DEFAULT '[]'::jsonb,
    "rollback" JSONB,
    policy_requirements JSONB NOT NULL DEFAULT '{}'::jsonb,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (plan_id, sequence_num)
);

CREATE INDEX IF NOT EXISTS idx_autonomy_plan_steps_plan_status
    ON autonomy_plan_steps(plan_id, status);
