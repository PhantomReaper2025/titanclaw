-- ==================== Autonomy Control Plane v1: Goals ====================

CREATE TABLE IF NOT EXISTS autonomy_goals (
    id UUID PRIMARY KEY,
    owner_user_id TEXT NOT NULL,
    channel TEXT,
    thread_id UUID,
    title TEXT NOT NULL,
    intent TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    risk_class TEXT NOT NULL,
    acceptance_criteria JSONB NOT NULL DEFAULT '{}'::jsonb,
    constraints JSONB NOT NULL DEFAULT '{}'::jsonb,
    source TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_autonomy_goals_owner_status
    ON autonomy_goals(owner_user_id, status);

CREATE INDEX IF NOT EXISTS idx_autonomy_goals_status_updated
    ON autonomy_goals(status, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_goals_thread
    ON autonomy_goals(thread_id)
    WHERE thread_id IS NOT NULL;
