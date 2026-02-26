-- ==================== Memory Plane v2: Procedural Playbooks ====================
-- Reusable strategies and step templates promoted from repeated successful patterns.

CREATE TABLE IF NOT EXISTS autonomy_procedural_playbooks (
    id UUID PRIMARY KEY,
    owner_user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    task_class TEXT NOT NULL,
    trigger_signals JSONB NOT NULL DEFAULT '{}'::jsonb,
    steps_template JSONB NOT NULL DEFAULT '[]'::jsonb,
    tool_preferences JSONB NOT NULL DEFAULT '{}'::jsonb,
    constraints JSONB NOT NULL DEFAULT '{}'::jsonb,
    success_count INTEGER NOT NULL DEFAULT 0,
    failure_count INTEGER NOT NULL DEFAULT 0,
    confidence REAL NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    requires_approval BOOLEAN NOT NULL DEFAULT FALSE,
    source_memory_record_ids JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
