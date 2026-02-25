-- ==================== Autonomy Control Plane v1: Incidents ====================
-- v1 spec recommends this table to align failed executions / policy denials with incident tracking.

CREATE TABLE IF NOT EXISTS autonomy_incidents (
    id UUID PRIMARY KEY,
    goal_id UUID REFERENCES autonomy_goals(id) ON DELETE SET NULL,
    plan_id UUID REFERENCES autonomy_plans(id) ON DELETE SET NULL,
    plan_step_id UUID REFERENCES autonomy_plan_steps(id) ON DELETE SET NULL,
    execution_attempt_id UUID REFERENCES autonomy_execution_attempts(id) ON DELETE SET NULL,
    policy_decision_id UUID REFERENCES autonomy_policy_decisions(id) ON DELETE SET NULL,
    job_id UUID,
    thread_id UUID,
    user_id TEXT NOT NULL,
    channel TEXT,
    incident_type TEXT NOT NULL,
    severity TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    summary TEXT NOT NULL,
    details JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_autonomy_incidents_goal_created
    ON autonomy_incidents(goal_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_incidents_execution_attempt
    ON autonomy_incidents(execution_attempt_id);

CREATE INDEX IF NOT EXISTS idx_autonomy_incidents_policy_decision
    ON autonomy_incidents(policy_decision_id);

CREATE INDEX IF NOT EXISTS idx_autonomy_incidents_status_created
    ON autonomy_incidents(status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_incidents_type_severity_created
    ON autonomy_incidents(incident_type, severity, created_at DESC);
