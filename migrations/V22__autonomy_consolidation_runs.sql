-- ==================== Memory Plane v2: Consolidation Runs ====================
-- Background consolidation loop runs and checkpoints.

CREATE TABLE IF NOT EXISTS autonomy_consolidation_runs (
    id UUID PRIMARY KEY,
    owner_user_id TEXT,
    status TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ,
    batch_size INTEGER NOT NULL DEFAULT 0,
    processed_count INTEGER NOT NULL DEFAULT 0,
    promoted_count INTEGER NOT NULL DEFAULT 0,
    playbooks_created_count INTEGER NOT NULL DEFAULT 0,
    archived_count INTEGER NOT NULL DEFAULT 0,
    error_count INTEGER NOT NULL DEFAULT 0,
    checkpoint_cursor TEXT,
    notes TEXT
);
