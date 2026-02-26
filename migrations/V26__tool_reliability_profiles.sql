-- ==================== Tool Reliability Profiles ====================

CREATE TABLE IF NOT EXISTS tool_reliability_profiles (
    tool_name TEXT PRIMARY KEY,
    window_start TIMESTAMPTZ NOT NULL,
    window_end TIMESTAMPTZ NOT NULL,
    sample_count BIGINT NOT NULL DEFAULT 0,
    success_count BIGINT NOT NULL DEFAULT 0,
    failure_count BIGINT NOT NULL DEFAULT 0,
    timeout_count BIGINT NOT NULL DEFAULT 0,
    blocked_count BIGINT NOT NULL DEFAULT 0,
    success_rate DOUBLE PRECISION NOT NULL DEFAULT 0,
    p50_latency_ms BIGINT,
    p95_latency_ms BIGINT,
    common_failure_modes JSONB NOT NULL DEFAULT '[]'::jsonb,
    recent_incident_count BIGINT NOT NULL DEFAULT 0,
    reliability_score DOUBLE PRECISION NOT NULL DEFAULT 0,
    safe_fallback_options JSONB NOT NULL DEFAULT '[]'::jsonb,
    breaker_state TEXT NOT NULL DEFAULT 'closed',
    cooldown_until TIMESTAMPTZ,
    last_failure_at TIMESTAMPTZ,
    last_success_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
