-- Add functional indexes on LOWER(wallet) columns to support case-insensitive lookups.
-- Queries using LOWER(source_wallet) = LOWER($1) cannot use the plain btree index,
-- causing full table scans on copy_trade_history (1.8s+ for 1389 rows).

-- copy_trade_history: used by metrics calculator fetch_trades
CREATE INDEX IF NOT EXISTS idx_cth_source_wallet_lower
    ON copy_trade_history (LOWER(source_wallet));

CREATE INDEX IF NOT EXISTS idx_cth_source_wallet_lower_ts
    ON copy_trade_history (LOWER(source_wallet), source_timestamp);

-- wallet_trades: used by metrics calculator fallback fetch_trades
CREATE INDEX IF NOT EXISTS idx_wt_wallet_address_lower
    ON wallet_trades (LOWER(wallet_address));

CREATE INDEX IF NOT EXISTS idx_wt_wallet_address_lower_ts
    ON wallet_trades (LOWER(wallet_address), timestamp);

-- workspace_wallet_allocations: used by risk_scorer and handlers
CREATE INDEX IF NOT EXISTS idx_wwa_wallet_address_lower
    ON workspace_wallet_allocations (LOWER(wallet_address));
