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
-- 4. TUNE POSTGRESQL MEMORY VIA ALTER SYSTEM
--    These write to postgresql.auto.conf which takes precedence over
--    timescaledb-tune's modifications to postgresql.conf.
--    shared_buffers change requires a server restart to take effect.
-- ==========================================================================

-- shared_buffers: from ~7.4GB (auto-tuned) to 512MB
-- Plenty for this workload (~30 connections, small trading bot)
ALTER SYSTEM SET shared_buffers = '512MB';

-- work_mem: per-sort/hash memory; 4MB default is fine for 30 connections
ALTER SYSTEM SET work_mem = '4MB';

-- maintenance_work_mem: for VACUUM, CREATE INDEX, etc.
ALTER SYSTEM SET maintenance_work_mem = '128MB';

-- effective_cache_size: hint to planner about OS page cache
ALTER SYSTEM SET effective_cache_size = '1536MB';

-- WAL configuration: reduce checkpoint pressure
ALTER SYSTEM SET wal_buffers = '16MB';
ALTER SYSTEM SET checkpoint_completion_target = '0.9';
ALTER SYSTEM SET max_wal_size = '1GB';
ALTER SYSTEM SET min_wal_size = '256MB';

-- Connection limits
ALTER SYSTEM SET max_connections = '50';

-- TimescaleDB background workers: reduce from default
-- 1 for orderbook_hourly refresh + compression/retention jobs
ALTER SYSTEM SET timescaledb.max_background_workers = '4';

-- Reload settings that can be changed without restart
SELECT pg_reload_conf();
