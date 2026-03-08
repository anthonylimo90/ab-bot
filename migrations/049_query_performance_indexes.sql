-- Performance indexes for high-traffic positions and flow-feature queries.

-- Ensure newly inserted positions have a sortable opened_at and backfill any gaps.
UPDATE positions
SET opened_at = entry_timestamp
WHERE opened_at IS NULL;

-- Positions dashboard queries filter on open/closed and sort by opened time.
CREATE INDEX IF NOT EXISTS idx_positions_is_open_opened_at
    ON positions (is_open, (COALESCE(opened_at, entry_timestamp)) DESC);

-- Flow feature aggregation scans recent wallet trades and only needs a few columns.
CREATE INDEX IF NOT EXISTS idx_wallet_trades_flow_window
    ON wallet_trades (timestamp DESC, condition_id)
    INCLUDE (wallet_address, side, value)
    WHERE condition_id IS NOT NULL;

-- Flow aggregation only needs the latest bot score per wallet.
CREATE INDEX IF NOT EXISTS idx_bot_scores_latest_lookup
    ON bot_scores (address, computed_at DESC)
    INCLUDE (total_score);
