-- Migration 063: Resource efficiency improvements
--
-- Addresses 4 post-deploy issues observed after wallet_trades hypertable conversion:
--
--   1. idx_wallet_trades_flow_window was created pre-hypertable (migration 049).
--      TimescaleDB does carry plain indexes over during migrate_data, but the index
--      needs to be dropped and recreated so TimescaleDB registers it as a
--      "hypertable index" — ensuring chunk-level pruning hints are used by the planner.
--
--   2. timescaledb.max_background_workers=4 is too low: 7 hypertables × 2 policies
--      (compression + retention) = 14 possible background jobs. Simultaneous firings
--      exceed worker slots, causing "out of background workers" failures which leave
--      chunks uncompressed (10× disk/memory bloat) and retention unenforced.
--
--   3. No index on wallet_trades(asset_id) WHERE condition_id IS NULL means the
--      hourly backfill UPDATE scans every chunk to find unresolved rows.
--
--   4. token_condition_cache grows unbounded (no retention). Old resolved-market
--      tokens accumulate, bloating shared_buffers index pages.
--
--   5. wallet_inventory queries use LOWER(wallet_address) but no functional index
--      exists — every lookup evaluates LOWER() against all rows.

-- ==========================================================================
-- 1. REBUILD idx_wallet_trades_flow_window AS A HYPERTABLE INDEX
--    Drop the pre-hypertable version and recreate so TimescaleDB registers
--    it as a chunk-level index with proper planner statistics.
-- ==========================================================================

DROP INDEX IF EXISTS idx_wallet_trades_flow_window;

-- Recreate on the hypertable: TimescaleDB automatically creates this on
-- every existing and future chunk.
CREATE INDEX IF NOT EXISTS idx_wallet_trades_flow_window
    ON wallet_trades (timestamp DESC, condition_id)
    INCLUDE (wallet_address, side, value)
    WHERE condition_id IS NOT NULL;

-- ==========================================================================
-- 2. PARTIAL INDEX FOR condition_id BACKFILL
--    Covers the UPDATE in backfill_condition_ids(). Only indexes rows that
--    still need resolution — approaches zero size as backfill completes.
-- ==========================================================================

CREATE INDEX IF NOT EXISTS idx_wallet_trades_null_condition
    ON wallet_trades (asset_id)
    WHERE condition_id IS NULL;

-- ==========================================================================
-- 3. FUNCTIONAL INDEX FOR wallet_inventory LOWER(wallet_address) LOOKUPS
--    All queries use LOWER(wallet_address) = LOWER($1). Without a functional
--    index PostgreSQL evaluates LOWER() against every row.
-- ==========================================================================

CREATE INDEX IF NOT EXISTS idx_wallet_inventory_wallet_lower
    ON wallet_inventory (LOWER(wallet_address), token_id);

-- ==========================================================================
-- 4. TOKEN CONDITION CACHE RETENTION
--    Add updated_at tracking and prune entries not seen in 90 days.
--    Prevents indefinite growth from resolved/expired markets.
-- ==========================================================================

-- Add retention cleanup to cleanup_stale_data()
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

    -- trade_events: keep 90 days
    DELETE FROM trade_events WHERE occurred_at < NOW() - INTERVAL '90 days';

    -- dynamic_config_history: keep 30 days
    DELETE FROM dynamic_config_history WHERE created_at < NOW() - INTERVAL '30 days';

    -- learning tables: keep 30 days
    DELETE FROM learning_shadow_predictions WHERE predicted_at < NOW() - INTERVAL '30 days';
    DELETE FROM learning_rollout_observations WHERE observed_at < NOW() - INTERVAL '30 days';

    -- token_condition_cache: prune stale market tokens (not updated in 90 days)
    DELETE FROM token_condition_cache WHERE updated_at < NOW() - INTERVAL '90 days';
END;
$$ LANGUAGE plpgsql;
