-- ==================== Memory Plane v2: Memory Events ====================
-- Audit trail for memory writes, consolidations, status changes, and forgetting.

CREATE TABLE IF NOT EXISTS autonomy_memory_events (
    id UUID PRIMARY KEY,
    memory_record_id UUID NOT NULL REFERENCES autonomy_memory_records(id) ON DELETE CASCADE,
    event_kind TEXT NOT NULL,
    actor TEXT NOT NULL,
    reason_codes JSONB NOT NULL DEFAULT '[]'::jsonb,
    action TEXT,
    before JSONB NOT NULL DEFAULT '{}'::jsonb,
    after JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
