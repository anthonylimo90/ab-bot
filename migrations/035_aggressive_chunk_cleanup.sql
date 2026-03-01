-- Migration 035: Aggressive chunk cleanup — recover disk space after volume exhaustion.
--
-- Problem: 1-year retention on orderbook_snapshots and historical_trades filled
-- the 5GB TimescaleDB volume on Railway, crashing the database. Volume resized
-- to 10GB, but we must free space immediately and tighten policies to prevent
-- recurrence.
--
-- Strategy:
--   1. Drop hypertable chunks older than 30 days (immediate space recovery)
--   2. Tighten retention policies from 1 year → 90 days
--   3. Tighten compression from 7 days → 3 days (more space savings)
--   4. Run cleanup_stale_data() for regular tables
--   5. Drop stale continuous aggregate data

-- ===================
-- 1. Immediately drop old hypertable chunks (> 30 days)
-- ===================
-- This is the big win — drop_chunks is much faster than DELETE and reclaims
-- disk space at the filesystem level (no VACUUM needed).

SELECT drop_chunks('orderbook_snapshots', INTERVAL '30 days');
SELECT drop_chunks('historical_trades', INTERVAL '30 days');

-- ===================
-- 2. Tighten retention policies: 1 year → 90 days
-- ===================
-- Remove the old 1-year policies and add 90-day ones.

SELECT remove_retention_policy('orderbook_snapshots', if_exists => TRUE);
SELECT remove_retention_policy('historical_trades', if_exists => TRUE);

SELECT add_retention_policy('orderbook_snapshots', INTERVAL '90 days', if_not_exists => TRUE);
SELECT add_retention_policy('historical_trades', INTERVAL '90 days', if_not_exists => TRUE);

-- ===================
-- 3. Tighten compression: 7 days → 3 days
-- ===================
-- Compress sooner to reduce storage footprint. TimescaleDB compression
-- typically achieves 90%+ reduction on time-series data.

SELECT remove_compression_policy('orderbook_snapshots', if_exists => TRUE);
SELECT remove_compression_policy('historical_trades', if_exists => TRUE);

SELECT add_compression_policy('orderbook_snapshots', INTERVAL '3 days', if_not_exists => TRUE);
SELECT add_compression_policy('historical_trades', INTERVAL '3 days', if_not_exists => TRUE);

-- ===================
-- 4. Continuous aggregate refresh (skipped)
-- ===================
-- CALL refresh_continuous_aggregate() cannot run inside a transaction block,
-- and sqlx wraps each migration in a transaction. The continuous aggregates
-- will naturally handle stale materialized data through their background
-- refresh policies. No manual refresh needed here.

-- ===================
-- 5. Run regular table cleanup
-- ===================

SELECT cleanup_stale_data();

-- ===================
-- 6. Update cleanup function to also drop chunks on each cycle
-- ===================
-- Ensures that even if the automatic retention policy job is delayed,
-- the periodic cleanup catches it.

CREATE OR REPLACE FUNCTION cleanup_stale_data() RETURNS void AS $$
BEGIN
    -- Hypertable chunk cleanup (belt-and-suspenders with retention policy)
    PERFORM drop_chunks('orderbook_snapshots', INTERVAL '90 days');
    PERFORM drop_chunks('historical_trades', INTERVAL '90 days');

    -- wallet_trades: keep 30 days
    DELETE FROM wallet_trades WHERE timestamp < NOW() - INTERVAL '30 days';

    -- bot_scores: keep only latest per address
    DELETE FROM bot_scores
    WHERE id NOT IN (
        SELECT DISTINCT ON (address) id
        FROM bot_scores
        ORDER BY address, computed_at DESC
    );

    -- copy_trade_history: keep 30 days
    DELETE FROM copy_trade_history WHERE created_at < NOW() - INTERVAL '30 days';

    -- wallet_trade_signals: keep unprocessed + last 7 days of processed
    DELETE FROM wallet_trade_signals
    WHERE processed = TRUE AND created_at < NOW() - INTERVAL '7 days';

    -- arb_opportunities: keep 7 days
    DELETE FROM arb_opportunities WHERE timestamp < NOW() - INTERVAL '7 days';

    -- auto_rotation_history: keep 30 days
    DELETE FROM auto_rotation_history WHERE created_at < NOW() - INTERVAL '30 days';

    -- auth_challenges: expired challenges
    PERFORM cleanup_expired_auth_challenges();

    -- audit_log: keep 90 days
    DELETE FROM audit_log WHERE created_at < NOW() - INTERVAL '90 days';

    -- data_ingestion_jobs: keep 30 days
    DELETE FROM data_ingestion_jobs WHERE created_at < NOW() - INTERVAL '30 days';
END;
$$ LANGUAGE plpgsql;
