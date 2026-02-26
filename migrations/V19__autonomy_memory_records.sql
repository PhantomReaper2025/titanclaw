-- ==================== Memory Plane v2: Memory Records ====================
-- Typed memory records spanning working/episodic/semantic/procedural/profile tiers.

CREATE TABLE IF NOT EXISTS autonomy_memory_records (
    id UUID PRIMARY KEY,
    owner_user_id TEXT NOT NULL,
    goal_id UUID REFERENCES autonomy_goals(id) ON DELETE SET NULL,
    plan_id UUID REFERENCES autonomy_plans(id) ON DELETE SET NULL,
    plan_step_id UUID REFERENCES autonomy_plan_steps(id) ON DELETE SET NULL,
    job_id UUID,
    thread_id UUID,
    memory_type TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    category TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    provenance JSONB NOT NULL DEFAULT '{}'::jsonb,
    confidence REAL NOT NULL DEFAULT 0,
    sensitivity TEXT NOT NULL,
    ttl_secs BIGINT,
    status TEXT NOT NULL,
    workspace_doc_path TEXT,
    workspace_document_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ
);
