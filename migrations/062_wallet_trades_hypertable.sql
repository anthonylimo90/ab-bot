-- Migration 062: Convert wallet_trades to a hypertable
--
-- Problem: wallet_trades is a plain table growing to ~21.6M rows (30 days × 30K
-- trades/hour × 24h). This causes:
--   1. Slow inserts — every batch maintains indexes on the full 21.6M-row table
--   2. Heavy autovacuum — bulk DELETE every 4 hours churns through millions of rows
--   3. Slow flow-feature queries — no chunk pruning, index scans entire table
--   4. 10GB+ OS page cache from background workers constantly scanning the table
--
-- Fix: convert to a 1-day-chunk hypertable with 14-day retention + 7-day
-- compression. Inserts only touch the current chunk (~750K rows). Time-range
-- queries prune to the relevant chunks. TimescaleDB retention policy replaces the
-- expensive periodic DELETE with instant chunk drops.
--
-- Pre-condition: UNIQUE (transaction_hash) → UNIQUE (transaction_hash, timestamp)
-- TimescaleDB requires all unique constraints to include the partitioning column.
-- Polymarket transaction hashes are blockchain tx hashes — each has one immutable
-- timestamp — so this composite key is equivalent to the original in practice.
--
-- Rust change required: wallet_harvester.rs ON CONFLICT (transaction_hash)
-- → ON CONFLICT (transaction_hash, timestamp)

-- ==========================================================================
-- 1. PRE-CLEANUP: delete all but the last 3 days
--    21.6M rows → ~2.25M rows — makes create_hypertable(migrate_data=TRUE)
--    complete in ~30s instead of 5+ minutes.
-- ==========================================================================

DELETE FROM wallet_trades WHERE timestamp < NOW() - INTERVAL '3 days';

-- ==========================================================================
-- 2. DROP INCOMPATIBLE CONSTRAINTS
--    Hypertables require unique constraints to include the partition column.
-- ==========================================================================

ALTER TABLE wallet_trades DROP CONSTRAINT IF EXISTS wallet_trades_pkey;
ALTER TABLE wallet_trades DROP CONSTRAINT IF EXISTS unique_transaction;

-- ==========================================================================
-- 3. ADD COMPOSITE UNIQUE CONSTRAINT
--    (transaction_hash, timestamp) is effectively unique because each
--    Polymarket blockchain tx has exactly one timestamp.
-- ==========================================================================

ALTER TABLE wallet_trades
    ADD CONSTRAINT unique_transaction_timestamp
    UNIQUE (transaction_hash, timestamp);

-- ==========================================================================
-- 4. CONVERT TO HYPERTABLE
--    migrate_data=TRUE copies the remaining ~2.25M rows into 3 daily chunks.
--    Existing indexes are preserved as chunk-level indexes.
-- ==========================================================================

SELECT create_hypertable(
    'wallet_trades',
    'timestamp',
    chunk_time_interval => INTERVAL '1 day',
    migrate_data        => TRUE,
    if_not_exists       => TRUE
);

-- ==========================================================================
-- 5. COMPRESSION: compress chunks older than 7 days (~90% size reduction)
--    segment by wallet_address so per-wallet queries decompress fewer segments
-- ==========================================================================

ALTER TABLE wallet_trades SET (
    timescaledb.compress           = true,
    timescaledb.compress_orderby   = 'timestamp DESC',
    timescaledb.compress_segmentby = 'wallet_address'
);

SELECT add_compression_policy(
    'wallet_trades',
    compress_after => INTERVAL '7 days',
    if_not_exists  => TRUE
);

-- ==========================================================================
-- 6. RETENTION: drop chunks older than 14 days automatically
--    Replaces the expensive periodic DELETE in cleanup_stale_data().
-- ==========================================================================

SELECT add_retention_policy(
    'wallet_trades',
    drop_after    => INTERVAL '14 days',
    if_not_exists => TRUE
);

-- ==========================================================================
-- 7. UPDATE cleanup_stale_data() TO USE drop_chunks FOR wallet_trades
--    drop_chunks is instant (drops the whole chunk file) vs DELETE which
--    scans and marks millions of rows then requires VACUUM.
-- ==========================================================================

CREATE OR REPLACE FUNCTION cleanup_stale_data() RETURNS void AS $$
BEGIN
    -- Hypertable chunk cleanup (belt-and-suspenders with retention policy)
    PERFORM drop_chunks('orderbook_snapshots', INTERVAL '90 days');
    PERFORM drop_chunks('historical_trades', INTERVAL '90 days');
    PERFORM drop_chunks('wallet_trades', INTERVAL '14 days');

    -- bot_scores: keep only latest per address
    DELETE FROM bot_scores bs
    WHERE EXISTS (
        SELECT 1 FROM bot_scores newer
        WHERE newer.address = bs.address
          AND newer.computed_at > bs.computed_at
    );

    -- arb_opportunities: keep 7 days
    DELETE FROM arb_opportunities WHERE timestamp < NOW() - INTERVAL '7 days';

    -- auth_challenges: expired challenges
    PERFORM cleanup_expired_auth_challenges();

    -- audit_log: keep 90 days
    DELETE FROM audit_log WHERE created_at < NOW() - INTERVAL '90 days';

    -- data_ingestion_jobs: keep 30 days
    DELETE FROM data_ingestion_jobs WHERE created_at < NOW() - INTERVAL '30 days';

    -- trade_events: keep 90 days (high-volume lifecycle events)
    DELETE FROM trade_events WHERE occurred_at < NOW() - INTERVAL '90 days';

    -- dynamic_config_history: keep 30 days
    DELETE FROM dynamic_config_history WHERE created_at < NOW() - INTERVAL '30 days';

    -- learning tables: keep 30 days
    DELETE FROM learning_shadow_predictions WHERE predicted_at < NOW() - INTERVAL '30 days';
    DELETE FROM learning_rollout_observations WHERE observed_at < NOW() - INTERVAL '30 days';
END;
$$ LANGUAGE plpgsql;
