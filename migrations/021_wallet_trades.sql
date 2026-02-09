-- Store individual trade records from Data API for profitability calculation
--
-- This table captures raw trade data from Polymarket's Data API, allowing us to
-- calculate accurate wallet performance metrics (ROI, PnL, Sharpe ratio) without
-- requiring users to have copy-traded the wallet.
--
-- Data flows:
--   1. Wallet harvester fetches trades from Data API every 5 minutes
--   2. Individual trades stored here (deduplicated by transaction_hash)
--   3. Metrics calculator queries this table to compute profitability
--   4. Discovery endpoint uses computed metrics to find profitable wallets

CREATE TABLE IF NOT EXISTS wallet_trades (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Unique transaction identifier
    transaction_hash VARCHAR NOT NULL,

    -- Wallet that executed the trade
    wallet_address VARCHAR NOT NULL,

    -- Market/asset identifiers
    asset_id VARCHAR NOT NULL,
    condition_id VARCHAR,

    -- Trade details
    side VARCHAR NOT NULL CHECK (side IN ('BUY', 'SELL')),
    price DECIMAL(20, 10) NOT NULL,
    quantity DECIMAL(20, 10) NOT NULL,
    value DECIMAL(20, 10) NOT NULL,  -- price * quantity

    -- Timing
    timestamp TIMESTAMPTZ NOT NULL,

    -- Optional metadata for display/filtering
    title TEXT,
    slug TEXT,
    outcome TEXT,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Deduplication: same transaction should only be stored once
    CONSTRAINT unique_transaction UNIQUE (transaction_hash)
);

-- Index for profitability queries: get all trades for a wallet in time window
CREATE INDEX IF NOT EXISTS idx_wallet_trades_wallet_timestamp
    ON wallet_trades (wallet_address, timestamp DESC);

-- Index for transaction hash lookups (deduplication check)
CREATE INDEX IF NOT EXISTS idx_wallet_trades_tx_hash
    ON wallet_trades (transaction_hash);

-- Index for market analysis queries
CREATE INDEX IF NOT EXISTS idx_wallet_trades_asset
    ON wallet_trades (asset_id, timestamp DESC);

-- Composite index for condition-based queries
CREATE INDEX IF NOT EXISTS idx_wallet_trades_condition
    ON wallet_trades (condition_id, timestamp DESC)
    WHERE condition_id IS NOT NULL;

COMMENT ON TABLE wallet_trades IS 'Individual trade records from Polymarket Data API for wallet profitability analysis';
COMMENT ON COLUMN wallet_trades.transaction_hash IS 'Unique blockchain transaction hash';
COMMENT ON COLUMN wallet_trades.wallet_address IS 'Proxy wallet address that executed the trade';
COMMENT ON COLUMN wallet_trades.value IS 'Trade value in dollars (price * quantity)';
COMMENT ON COLUMN wallet_trades.side IS 'BUY = entering position, SELL = exiting position';
