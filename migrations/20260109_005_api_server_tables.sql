-- Phase 4: API Server Tables

-- ===================
-- Markets Table
-- ===================

CREATE TABLE IF NOT EXISTS markets (
    id VARCHAR(255) PRIMARY KEY,
    question TEXT NOT NULL,
    description TEXT,
    category VARCHAR(100) NOT NULL,
    end_date TIMESTAMPTZ NOT NULL,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    yes_price DECIMAL(18, 8) NOT NULL DEFAULT 0.5,
    no_price DECIMAL(18, 8) NOT NULL DEFAULT 0.5,
    volume_24h DECIMAL(18, 8) NOT NULL DEFAULT 0,
    liquidity DECIMAL(18, 8) NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_markets_category ON markets(category);
CREATE INDEX idx_markets_active ON markets(active);
CREATE INDEX idx_markets_end_date ON markets(end_date);
CREATE INDEX idx_markets_volume ON markets(volume_24h DESC);
CREATE INDEX idx_markets_liquidity ON markets(liquidity DESC);

-- ===================
-- Orders Table
-- ===================

CREATE TABLE IF NOT EXISTS orders (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    client_order_id VARCHAR(255),
    market_id VARCHAR(255) NOT NULL REFERENCES markets(id),
    outcome VARCHAR(10) NOT NULL,  -- 'yes' or 'no'
    side VARCHAR(10) NOT NULL,  -- 'buy' or 'sell'
    order_type VARCHAR(20) NOT NULL,  -- 'market', 'limit', 'stop_loss', 'take_profit'
    status VARCHAR(20) NOT NULL DEFAULT 'pending',  -- 'pending', 'open', 'partially_filled', 'filled', 'cancelled', 'rejected', 'expired'
    quantity DECIMAL(18, 8) NOT NULL,
    filled_quantity DECIMAL(18, 8) NOT NULL DEFAULT 0,
    price DECIMAL(18, 8),  -- limit price
    avg_fill_price DECIMAL(18, 8),
    stop_price DECIMAL(18, 8),
    time_in_force VARCHAR(10) NOT NULL DEFAULT 'GTC',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    filled_at TIMESTAMPTZ,

    CONSTRAINT valid_outcome CHECK (outcome IN ('yes', 'no')),
    CONSTRAINT valid_side CHECK (side IN ('buy', 'sell')),
    CONSTRAINT valid_order_type CHECK (order_type IN ('market', 'limit', 'stop_loss', 'take_profit')),
    CONSTRAINT valid_status CHECK (status IN ('pending', 'open', 'partially_filled', 'filled', 'cancelled', 'rejected', 'expired'))
);

CREATE INDEX idx_orders_market ON orders(market_id);
CREATE INDEX idx_orders_status ON orders(status);
CREATE INDEX idx_orders_client_id ON orders(client_order_id) WHERE client_order_id IS NOT NULL;
CREATE INDEX idx_orders_created ON orders(created_at);
CREATE UNIQUE INDEX idx_orders_client_id_unique ON orders(client_order_id) WHERE client_order_id IS NOT NULL;

-- ===================
-- Backtest Results Table
-- ===================

CREATE TABLE IF NOT EXISTS backtest_results (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    strategy JSONB NOT NULL,
    start_date TIMESTAMPTZ NOT NULL,
    end_date TIMESTAMPTZ NOT NULL,
    initial_capital DECIMAL(18, 8) NOT NULL,
    slippage_model JSONB,
    fee_pct DECIMAL(8, 6) NOT NULL DEFAULT 0.001,

    -- Results (populated after completion)
    final_value DECIMAL(18, 8),
    total_return DECIMAL(18, 8),
    total_return_pct DECIMAL(10, 4),
    annualized_return DECIMAL(10, 4),
    sharpe_ratio DECIMAL(10, 4),
    sortino_ratio DECIMAL(10, 4),
    max_drawdown DECIMAL(18, 8),
    max_drawdown_pct DECIMAL(10, 4),
    total_trades BIGINT,
    winning_trades BIGINT,
    losing_trades BIGINT,
    win_rate DECIMAL(6, 4),
    avg_win DECIMAL(18, 8),
    avg_loss DECIMAL(18, 8),
    profit_factor DECIMAL(10, 4),
    total_fees DECIMAL(18, 8),

    -- Equity curve
    equity_curve JSONB,

    -- Status
    status VARCHAR(20) NOT NULL DEFAULT 'pending',
    error TEXT,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,

    CONSTRAINT valid_backtest_status CHECK (status IN ('pending', 'running', 'completed', 'failed'))
);

CREATE INDEX idx_backtest_status ON backtest_results(status);
CREATE INDEX idx_backtest_created ON backtest_results(created_at DESC);

-- ===================
-- Extend Positions for API
-- ===================

ALTER TABLE positions
ADD COLUMN IF NOT EXISTS outcome VARCHAR(10),
ADD COLUMN IF NOT EXISTS side VARCHAR(10) DEFAULT 'long',
ADD COLUMN IF NOT EXISTS entry_price DECIMAL(18, 8),
ADD COLUMN IF NOT EXISTS current_price DECIMAL(18, 8),
ADD COLUMN IF NOT EXISTS stop_loss DECIMAL(18, 8),
ADD COLUMN IF NOT EXISTS take_profit DECIMAL(18, 8),
ADD COLUMN IF NOT EXISTS is_copy_trade BOOLEAN NOT NULL DEFAULT FALSE,
ADD COLUMN IF NOT EXISTS is_open BOOLEAN NOT NULL DEFAULT TRUE,
ADD COLUMN IF NOT EXISTS opened_at TIMESTAMPTZ;

-- Update existing positions to have opened_at from entry_timestamp
UPDATE positions SET opened_at = entry_timestamp WHERE opened_at IS NULL;

CREATE INDEX idx_positions_is_open ON positions(is_open);
CREATE INDEX idx_positions_is_copy ON positions(is_copy_trade);

-- ===================
-- Extend Tracked Wallets for API
-- ===================

ALTER TABLE tracked_wallets
ADD COLUMN IF NOT EXISTS label VARCHAR(100),
ADD COLUMN IF NOT EXISTS copy_enabled BOOLEAN NOT NULL DEFAULT FALSE,
ADD COLUMN IF NOT EXISTS success_score DECIMAL(10, 4) NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS total_trades BIGINT NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS last_activity TIMESTAMPTZ;

-- Create alias for label if needed
UPDATE tracked_wallets SET label = alias WHERE label IS NULL AND alias IS NOT NULL;

CREATE INDEX idx_tracked_wallets_success ON tracked_wallets(success_score DESC);
CREATE INDEX idx_tracked_wallets_copy ON tracked_wallets(copy_enabled);

-- ===================
-- Extend Wallet Success Metrics for API
-- ===================

ALTER TABLE wallet_success_metrics
ADD COLUMN IF NOT EXISTS wallet_address VARCHAR(42),
ADD COLUMN IF NOT EXISTS roi DECIMAL(10, 6),
ADD COLUMN IF NOT EXISTS avg_trade_size DECIMAL(18, 8),
ADD COLUMN IF NOT EXISTS avg_hold_time_hours DOUBLE PRECISION DEFAULT 0,
ADD COLUMN IF NOT EXISTS recent_pnl_30d DECIMAL(18, 8) DEFAULT 0,
ADD COLUMN IF NOT EXISTS category_win_rates JSONB DEFAULT '{}',
ADD COLUMN IF NOT EXISTS calculated_at TIMESTAMPTZ;

-- Update wallet_address from address
UPDATE wallet_success_metrics SET wallet_address = address WHERE wallet_address IS NULL;

-- Update calculated_at from last_computed
UPDATE wallet_success_metrics SET calculated_at = last_computed WHERE calculated_at IS NULL;

-- Update roi from roi_all_time
UPDATE wallet_success_metrics SET roi = roi_all_time WHERE roi IS NULL;

CREATE INDEX idx_wsm_wallet ON wallet_success_metrics(wallet_address);

-- ===================
-- Update Triggers
-- ===================

CREATE TRIGGER update_markets_updated_at
    BEFORE UPDATE ON markets
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_orders_updated_at
    BEFORE UPDATE ON orders
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();
