-- Persistent mapping from normalized recurring intent patterns to compiled reflex tools.

CREATE TABLE IF NOT EXISTS reflex_patterns (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    normalized_pattern TEXT NOT NULL UNIQUE,
    compiled_tool_name TEXT NOT NULL,
    source_count BIGINT NOT NULL DEFAULT 0,
    last_seen_at TIMESTAMPTZ,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_reflex_patterns_enabled
    ON reflex_patterns (enabled);

CREATE INDEX IF NOT EXISTS idx_reflex_patterns_last_seen
    ON reflex_patterns (last_seen_at DESC);

