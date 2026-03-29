-- Migration 064: Query performance indexes
--
-- Five missing indexes identified from hot query paths:
--
--   1. wallet_success_metrics uses LOWER(COALESCE(wallet_address, address)) in reads
--      but no functional index exists — every lookup evaluates COALESCE + LOWER against all rows.
--
--   2. positions.exit_timestamp range scans for closed position history queries have
--      no partial index — full table scan for every closed positions request.
--
--   3. Dashboard closed-positions view filters on (workspace_id, state=4, exit_timestamp DESC)
--      with no covering index.
--
--   4. Flow calibration queries filter quant_signals on kind='flow' + execution_status + time
--      but no partial index exists for this exact access pattern.
--
--   5. wallet_success_metrics.last_computed ordering (stale-entry refresh queries) has
--      no index — sorts the full table on every refresh cycle.

-- ==========================================================================
-- 1. FUNCTIONAL INDEX FOR wallet_success_metrics LOWER(COALESCE(...)) LOOKUPS
-- ==========================================================================

CREATE INDEX IF NOT EXISTS idx_wsm_lower_coalesce_address
    ON wallet_success_metrics (LOWER(COALESCE(wallet_address, address)));

-- ==========================================================================
-- 2. PARTIAL INDEX FOR positions.exit_timestamp RANGE SCANS
-- ==========================================================================

CREATE INDEX IF NOT EXISTS idx_positions_exit_timestamp
    ON positions (exit_timestamp)
    WHERE exit_timestamp IS NOT NULL;

-- ==========================================================================
-- 3. COVERING INDEX FOR CLOSED POSITIONS DASHBOARD VIEW
--    Supports: WHERE state = 4 ORDER BY exit_timestamp DESC
-- ==========================================================================

CREATE INDEX IF NOT EXISTS idx_positions_closed
    ON positions (exit_timestamp DESC)
    WHERE state = 4;

-- ==========================================================================
-- 4. PARTIAL INDEX FOR FLOW CALIBRATION QUANT SIGNAL QUERIES
--    Supports: WHERE kind = 'flow' AND execution_status = ? ORDER BY generated_at DESC
-- ==========================================================================

CREATE INDEX IF NOT EXISTS idx_quant_signals_flow_calibration
    ON quant_signals (kind, execution_status, generated_at DESC)
    WHERE kind = 'flow';

-- ==========================================================================
-- 5. INDEX FOR wallet_success_metrics STALE-ENTRY REFRESH ORDERING
-- ==========================================================================

CREATE INDEX IF NOT EXISTS idx_wsm_last_computed
    ON wallet_success_metrics (last_computed DESC)
    WHERE last_computed IS NOT NULL;
