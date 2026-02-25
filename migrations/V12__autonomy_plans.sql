-- ==================== Autonomy Control Plane v1: Plans ====================

CREATE TABLE IF NOT EXISTS autonomy_plans (
    id UUID PRIMARY KEY,
    goal_id UUID NOT NULL REFERENCES autonomy_goals(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    status TEXT NOT NULL,
    planner_kind TEXT NOT NULL,
    source_action_plan JSONB,
    assumptions JSONB NOT NULL DEFAULT '{}'::jsonb,
    confidence DOUBLE PRECISION NOT NULL,
    estimated_cost DOUBLE PRECISION,
    estimated_time_secs BIGINT,
    summary TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (goal_id, revision)
);

CREATE INDEX IF NOT EXISTS idx_autonomy_plans_goal_status
    ON autonomy_plans(goal_id, status);
