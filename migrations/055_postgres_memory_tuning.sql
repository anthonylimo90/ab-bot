-- Fix 12GB RAM usage caused by:
--   1. timescaledb-tune auto-setting shared_buffers to ~7.4GB (25% of host RAM)
--   2. Two unused continuous aggregates (orderbook_5min, orderbook_daily) running
--      background workers every 5min/1day for zero rows
--   3. orderbook_hourly CA with no retention/compression (grows forever)
--   4. trade_events table with no cleanup (unbounded growth)

-- ==========================================================================
-- 1. DROP UNUSED CONTINUOUS AGGREGATES
--    orderbook_5min  — 0 Rust references, inserts 0 rows per refresh cycle
--    orderbook_daily — 0 Rust references, never queried
-- ==========================================================================

-- Remove refresh policies first, then drop the views
SELECT remove_continuous_aggregate_policy('orderbook_5min', if_exists => TRUE);
DROP MATERIALIZED VIEW IF EXISTS orderbook_5min CASCADE;

SELECT remove_continuous_aggregate_policy('orderbook_daily', if_exists => TRUE);
DROP MATERIALIZED VIEW IF EXISTS orderbook_daily CASCADE;

-- ==========================================================================
-- 2. ADD RETENTION + COMPRESSION TO orderbook_hourly
--    Heavily used by: resolution_signal, cross_market_signal,
--    mean_reversion_signal, flow_signal, metrics_calculator,
--    advanced_predictor. 90 days matches orderbook_snapshots retention.
-- ==========================================================================

SELECT add_retention_policy(
    'orderbook_hourly',
    drop_after => INTERVAL '90 days',
    if_not_exists => TRUE
);

-- Compression on the CA's internal hypertable (compress after 7 days).
ALTER MATERIALIZED VIEW orderbook_hourly SET (
    timescaledb.compress = true
);

SELECT add_compression_policy(
    'orderbook_hourly',
    compress_after => INTERVAL '7 days',
    if_not_exists => TRUE
);

-- ==========================================================================
-- 3. ADD trade_events TO cleanup_stale_data()
--    Also add: dynamic_config_history, learning_shadow_predictions,
--    learning_rollout_observations, backtest stale cleanup
-- ==========================================================================

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

-- ==========================================================================
-- 4. POSTGRESQL MEMORY TUNING
--    ALTER SYSTEM cannot run inside a transaction (sqlx wraps migrations
--    in transactions), so memory tuning is applied separately via:
--      migrations/055_alter_system.sh  (run manually or via Railway deploy hook)
--    See that script for: shared_buffers=512MB, work_mem=4MB, etc.
-- ==========================================================================
