-- Persist optional autonomy control-plane linkage IDs on agent jobs so
-- worker/dispatcher runtime events can be correlated after restarts.

ALTER TABLE agent_jobs ADD COLUMN IF NOT EXISTS autonomy_goal_id UUID;
ALTER TABLE agent_jobs ADD COLUMN IF NOT EXISTS autonomy_plan_id UUID;
ALTER TABLE agent_jobs ADD COLUMN IF NOT EXISTS autonomy_plan_step_id UUID;

CREATE INDEX IF NOT EXISTS idx_agent_jobs_autonomy_goal
    ON agent_jobs(autonomy_goal_id)
    WHERE autonomy_goal_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_agent_jobs_autonomy_plan
    ON agent_jobs(autonomy_plan_id)
    WHERE autonomy_plan_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_agent_jobs_autonomy_plan_step
    ON agent_jobs(autonomy_plan_step_id)
    WHERE autonomy_plan_step_id IS NOT NULL;
