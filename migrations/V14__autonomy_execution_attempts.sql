-- ==================== Autonomy Control Plane v1: Execution Attempts ====================

CREATE TABLE IF NOT EXISTS autonomy_execution_attempts (
    id UUID PRIMARY KEY,
    goal_id UUID REFERENCES autonomy_goals(id) ON DELETE SET NULL,
    plan_id UUID REFERENCES autonomy_plans(id) ON DELETE SET NULL,
    plan_step_id UUID REFERENCES autonomy_plan_steps(id) ON DELETE SET NULL,
    job_id UUID,
    thread_id UUID,
    user_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    tool_call_id TEXT,
    tool_args JSONB,
    status TEXT NOT NULL,
    failure_class TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ,
    elapsed_ms BIGINT,
    result_summary TEXT,
    error_preview TEXT
);

CREATE INDEX IF NOT EXISTS idx_autonomy_execution_attempts_step_started
    ON autonomy_execution_attempts(plan_step_id, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_execution_attempts_goal_started
    ON autonomy_execution_attempts(goal_id, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_execution_attempts_tool_status_started
    ON autonomy_execution_attempts(tool_name, status, started_at DESC);
