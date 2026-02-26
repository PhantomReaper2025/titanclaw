-- ==================== Autonomy Incidents Phase 3 fields ====================

ALTER TABLE autonomy_incidents
    ADD COLUMN IF NOT EXISTS fingerprint TEXT;

ALTER TABLE autonomy_incidents
    ADD COLUMN IF NOT EXISTS surface TEXT;

ALTER TABLE autonomy_incidents
    ADD COLUMN IF NOT EXISTS tool_name TEXT;

ALTER TABLE autonomy_incidents
    ADD COLUMN IF NOT EXISTS occurrence_count INTEGER NOT NULL DEFAULT 1;

ALTER TABLE autonomy_incidents
    ADD COLUMN IF NOT EXISTS first_seen_at TIMESTAMPTZ;

ALTER TABLE autonomy_incidents
    ADD COLUMN IF NOT EXISTS last_seen_at TIMESTAMPTZ;

ALTER TABLE autonomy_incidents
    ADD COLUMN IF NOT EXISTS last_failure_class TEXT;

UPDATE autonomy_incidents
SET
    occurrence_count = COALESCE(NULLIF(occurrence_count, 0), 1),
    first_seen_at = COALESCE(first_seen_at, created_at),
    last_seen_at = COALESCE(last_seen_at, updated_at)
WHERE first_seen_at IS NULL
   OR last_seen_at IS NULL
   OR occurrence_count IS NULL
   OR occurrence_count = 0;
