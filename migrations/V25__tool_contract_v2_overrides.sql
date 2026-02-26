-- ==================== Tool Contract V2 overrides ====================

CREATE TABLE IF NOT EXISTS tool_contract_v2_overrides (
    id UUID PRIMARY KEY,
    tool_name TEXT NOT NULL,
    owner_user_id TEXT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    descriptor_json JSONB NOT NULL,
    source TEXT NOT NULL,
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
