-- Phase 2: Wallet Tracking & Success Prediction Tables

-- ===================
-- Wallet Success Metrics
-- ===================

CREATE TABLE IF NOT EXISTS wallet_success_metrics (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    address VARCHAR(42) NOT NULL UNIQUE,

    -- Return metrics
    roi_30d DECIMAL(10, 6) NOT NULL DEFAULT 0,
    roi_90d DECIMAL(10, 6) NOT NULL DEFAULT 0,
    roi_all_time DECIMAL(10, 6) NOT NULL DEFAULT 0,
    annualized_return DECIMAL(10, 6) NOT NULL DEFAULT 0,

    -- Risk metrics
    sharpe_30d DECIMAL(10, 6) NOT NULL DEFAULT 0,
    sortino_30d DECIMAL(10, 6) NOT NULL DEFAULT 0,
    max_drawdown_30d DECIMAL(10, 6) NOT NULL DEFAULT 0,
    volatility_30d DECIMAL(10, 6) NOT NULL DEFAULT 0,

    -- Consistency metrics
    consistency_score DECIMAL(5, 4) NOT NULL DEFAULT 0,
    win_rate_30d DECIMAL(5, 4) NOT NULL DEFAULT 0,
    profit_factor DECIMAL(10, 4) NOT NULL DEFAULT 0,

    -- Trade counts
    trades_30d INTEGER NOT NULL DEFAULT 0,
    winning_trades_30d INTEGER NOT NULL DEFAULT 0,
    losing_trades_30d INTEGER NOT NULL DEFAULT 0,

    -- Prediction
    predicted_success_prob DECIMAL(5, 4) NOT NULL DEFAULT 0,
    prediction_confidence DECIMAL(5, 4) NOT NULL DEFAULT 0,
    prediction_category SMALLINT NOT NULL DEFAULT 3,  -- 0=high, 1=moderate, 2=low, 3=uncertain

    -- Metadata
    last_computed TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_wsm_address ON wallet_success_metrics(address);
CREATE INDEX IF NOT EXISTS idx_wsm_roi_30d ON wallet_success_metrics(roi_30d DESC);
CREATE INDEX IF NOT EXISTS idx_wsm_sharpe ON wallet_success_metrics(sharpe_30d DESC);
CREATE INDEX IF NOT EXISTS idx_wsm_prediction ON wallet_success_metrics(predicted_success_prob DESC);
CREATE INDEX IF NOT EXISTS idx_wsm_consistency ON wallet_success_metrics(consistency_score DESC);
CREATE INDEX IF NOT EXISTS idx_wsm_category ON wallet_success_metrics(prediction_category);

-- ===================
-- Discovered Wallets History
-- ===================

CREATE TABLE IF NOT EXISTS discovered_wallets (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    address VARCHAR(42) NOT NULL,

    -- Discovery snapshot
    total_trades BIGINT NOT NULL DEFAULT 0,
    win_count BIGINT NOT NULL DEFAULT 0,
    loss_count BIGINT NOT NULL DEFAULT 0,
    win_rate DECIMAL(5, 4) NOT NULL DEFAULT 0,
    total_volume DECIMAL(18, 8) NOT NULL DEFAULT 0,
    total_pnl DECIMAL(18, 8) NOT NULL DEFAULT 0,
    roi DECIMAL(10, 6) NOT NULL DEFAULT 0,

    -- Activity window
    first_trade TIMESTAMPTZ NOT NULL,
    last_trade TIMESTAMPTZ NOT NULL,

    -- Bot detection
    is_bot BOOLEAN NOT NULL DEFAULT FALSE,
    bot_score INTEGER,

    -- Discovery metadata
    discovery_criteria JSONB,
    discovered_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_dw_address ON discovered_wallets(address);
CREATE INDEX IF NOT EXISTS idx_dw_discovered ON discovered_wallets(discovered_at);
CREATE INDEX IF NOT EXISTS idx_dw_roi ON discovered_wallets(roi DESC);
CREATE INDEX IF NOT EXISTS idx_dw_win_rate ON discovered_wallets(win_rate DESC);
CREATE INDEX IF NOT EXISTS idx_dw_is_bot ON discovered_wallets(is_bot);

-- ===================
-- Copy Trade History
-- ===================

CREATE TABLE IF NOT EXISTS copy_trade_history (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    source_wallet VARCHAR(42) NOT NULL,
    source_tx_hash VARCHAR(66) NOT NULL,

    -- Source trade details
    source_market_id VARCHAR(255) NOT NULL,
    source_token_id VARCHAR(255) NOT NULL,
    source_direction SMALLINT NOT NULL,  -- 0=buy, 1=sell
    source_price DECIMAL(18, 8) NOT NULL,
    source_quantity DECIMAL(18, 8) NOT NULL,
    source_timestamp TIMESTAMPTZ NOT NULL,

    -- Copy trade details
    copy_order_id UUID,
    copy_execution_id UUID REFERENCES execution_reports(id),
    copy_price DECIMAL(18, 8),
    copy_quantity DECIMAL(18, 8),
    copy_timestamp TIMESTAMPTZ,

    -- Performance
    allocation_pct DECIMAL(5, 2) NOT NULL,
    slippage DECIMAL(8, 6),
    pnl DECIMAL(18, 8),

    -- Status
    status SMALLINT NOT NULL DEFAULT 0,  -- 0=pending, 1=executed, 2=partial, 3=skipped, 4=failed
    skip_reason VARCHAR(255),
    error_message TEXT,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_cth_source_wallet ON copy_trade_history(source_wallet);
CREATE INDEX IF NOT EXISTS idx_cth_source_tx ON copy_trade_history(source_tx_hash);
CREATE INDEX IF NOT EXISTS idx_cth_market ON copy_trade_history(source_market_id);
CREATE INDEX IF NOT EXISTS idx_cth_status ON copy_trade_history(status);
CREATE INDEX IF NOT EXISTS idx_cth_created ON copy_trade_history(created_at);
CREATE INDEX IF NOT EXISTS idx_cth_execution ON copy_trade_history(copy_execution_id) WHERE copy_execution_id IS NOT NULL;

-- ===================
-- Wallet Trade Signals (real-time monitoring)
-- ===================

CREATE TABLE IF NOT EXISTS wallet_trade_signals (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    wallet_address VARCHAR(42) NOT NULL,
    tx_hash VARCHAR(66) NOT NULL,
    block_number BIGINT NOT NULL,

    -- Trade details
    market_id VARCHAR(255) NOT NULL,
    token_id VARCHAR(255) NOT NULL,
    direction SMALLINT NOT NULL,  -- 0=buy, 1=sell
    price DECIMAL(18, 8) NOT NULL,
    quantity DECIMAL(18, 8) NOT NULL,
    value DECIMAL(18, 8) NOT NULL,

    -- Processing
    processed BOOLEAN NOT NULL DEFAULT FALSE,
    processed_at TIMESTAMPTZ,
    copy_trade_id UUID REFERENCES copy_trade_history(id),

    -- Timestamps
    trade_timestamp TIMESTAMPTZ NOT NULL,
    detected_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_wts_wallet ON wallet_trade_signals(wallet_address);
CREATE INDEX IF NOT EXISTS idx_wts_tx ON wallet_trade_signals(tx_hash);
CREATE INDEX IF NOT EXISTS idx_wts_block ON wallet_trade_signals(block_number);
CREATE INDEX IF NOT EXISTS idx_wts_market ON wallet_trade_signals(market_id);
CREATE INDEX IF NOT EXISTS idx_wts_processed ON wallet_trade_signals(processed) WHERE processed = FALSE;
CREATE INDEX IF NOT EXISTS idx_wts_trade_ts ON wallet_trade_signals(trade_timestamp);
CREATE UNIQUE INDEX IF NOT EXISTS idx_wts_unique_tx ON wallet_trade_signals(tx_hash, wallet_address);

-- ===================
-- Tracked Wallet Performance (extend tracked_wallets)
-- ===================

ALTER TABLE tracked_wallets
ADD COLUMN IF NOT EXISTS win_rate DECIMAL(5, 4),
ADD COLUMN IF NOT EXISTS sharpe_ratio DECIMAL(10, 6),
ADD COLUMN IF NOT EXISTS roi_30d DECIMAL(10, 6),
ADD COLUMN IF NOT EXISTS consistency_score DECIMAL(5, 4),
ADD COLUMN IF NOT EXISTS trades_copied INTEGER NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS trades_skipped INTEGER NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS avg_slippage DECIMAL(8, 6),
ADD COLUMN IF NOT EXISTS last_analyzed TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_tw_win_rate ON tracked_wallets(win_rate DESC) WHERE enabled = TRUE;
CREATE INDEX IF NOT EXISTS idx_tw_roi ON tracked_wallets(roi_30d DESC) WHERE enabled = TRUE;

-- ===================
-- Update Triggers
-- ===================

CREATE TRIGGER update_wsm_updated_at
    BEFORE UPDATE ON wallet_success_metrics
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_cth_updated_at
    BEFORE UPDATE ON copy_trade_history
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();
