-- Pipeline indexes for wallet discovery and copy trading pipeline performance.
-- These improve the auto-optimizer candidate query, metrics calculator priority,
-- and dormancy demotion lookups.

-- Speed up dormancy checks and staleness ordering in candidate queries
CREATE INDEX IF NOT EXISTS idx_wallet_features_last_trade
    ON wallet_features (last_trade DESC);

-- Speed up case-insensitive wallet address lookups in candidate and metrics queries
CREATE INDEX IF NOT EXISTS idx_wallet_features_address_lower
    ON wallet_features (LOWER(address));

-- Speed up active wallet priority in metrics calculator and backtest refresh
CREATE INDEX IF NOT EXISTS idx_wwa_wallet_address_lower
    ON workspace_wallet_allocations (LOWER(wallet_address));
