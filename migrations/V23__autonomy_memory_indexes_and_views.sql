-- ==================== Memory Plane v2: Indexes ====================

CREATE INDEX IF NOT EXISTS idx_autonomy_memory_records_owner_type_created
    ON autonomy_memory_records(owner_user_id, memory_type, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_memory_records_owner_status_created
    ON autonomy_memory_records(owner_user_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_memory_records_goal_created
    ON autonomy_memory_records(goal_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_memory_records_plan_created
    ON autonomy_memory_records(plan_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_memory_records_expires_at
    ON autonomy_memory_records(expires_at);

CREATE INDEX IF NOT EXISTS idx_autonomy_memory_records_type_status_created
    ON autonomy_memory_records(memory_type, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_memory_events_record_created
    ON autonomy_memory_events(memory_record_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_memory_events_kind_created
    ON autonomy_memory_events(event_kind, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_procedural_playbooks_owner_task_status
    ON autonomy_procedural_playbooks(owner_user_id, task_class, status);

CREATE INDEX IF NOT EXISTS idx_autonomy_procedural_playbooks_status_updated
    ON autonomy_procedural_playbooks(status, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_consolidation_runs_owner_started
    ON autonomy_consolidation_runs(owner_user_id, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_consolidation_runs_status_started
    ON autonomy_consolidation_runs(status, started_at DESC);
