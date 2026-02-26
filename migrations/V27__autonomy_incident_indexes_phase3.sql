-- ==================== Autonomy / Tool Reliability Phase 3 indexes ====================

CREATE INDEX IF NOT EXISTS idx_autonomy_incidents_fingerprint_status_last_seen
    ON autonomy_incidents(fingerprint, status, last_seen_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_incidents_tool_name_status_last_seen
    ON autonomy_incidents(tool_name, status, last_seen_at DESC);

CREATE INDEX IF NOT EXISTS idx_autonomy_incidents_user_created
    ON autonomy_incidents(user_id, created_at DESC);

CREATE UNIQUE INDEX IF NOT EXISTS idx_tool_contract_v2_overrides_tool_owner_unique
    ON tool_contract_v2_overrides(tool_name, owner_user_id)
    WHERE owner_user_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_tool_contract_v2_overrides_tool_global_unique
    ON tool_contract_v2_overrides(tool_name)
    WHERE owner_user_id IS NULL;

CREATE INDEX IF NOT EXISTS idx_tool_contract_v2_overrides_tool_owner_enabled
    ON tool_contract_v2_overrides(tool_name, owner_user_id, enabled);

CREATE INDEX IF NOT EXISTS idx_tool_reliability_profiles_updated
    ON tool_reliability_profiles(updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_tool_reliability_profiles_score
    ON tool_reliability_profiles(reliability_score ASC);

CREATE INDEX IF NOT EXISTS idx_tool_reliability_profiles_breaker_state
    ON tool_reliability_profiles(breaker_state, updated_at DESC);
