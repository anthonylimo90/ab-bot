-- Token-to-Condition ID mapping cache.
-- Populated during market cache refresh to resolve token_id → condition_id
-- for copy trades where condition_id is absent from the Data API response.

CREATE TABLE IF NOT EXISTS token_condition_cache (
    token_id TEXT PRIMARY KEY,
    condition_id TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for reverse lookups (condition_id → token_ids)
CREATE INDEX IF NOT EXISTS idx_token_condition_cache_condition
    ON token_condition_cache (condition_id);
