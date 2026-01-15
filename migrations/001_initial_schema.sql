-- Initial database schema for Polymarket Scanner

-- Enable required extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- ===================
-- Positions (Arb Monitor)
-- ===================

CREATE TABLE IF NOT EXISTS positions (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    market_id VARCHAR(255) NOT NULL,
    yes_entry_price DECIMAL(18, 8) NOT NULL,
    no_entry_price DECIMAL(18, 8) NOT NULL,
    quantity DECIMAL(18, 8) NOT NULL,
    entry_timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    exit_strategy SMALLINT NOT NULL DEFAULT 1,  -- 0 = hold_to_resolution, 1 = exit_on_correction
    state SMALLINT NOT NULL DEFAULT 0,          -- 0 = pending, 1 = open, 2 = exit_ready, 3 = closing, 4 = closed
    unrealized_pnl DECIMAL(18, 8) NOT NULL DEFAULT 0,
    realized_pnl DECIMAL(18, 8),
    exit_timestamp TIMESTAMPTZ,
    yes_exit_price DECIMAL(18, 8),
    no_exit_price DECIMAL(18, 8),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for positions
CREATE INDEX idx_positions_market_id ON positions(market_id);
CREATE INDEX idx_positions_state ON positions(state);
CREATE INDEX idx_positions_entry_timestamp ON positions(entry_timestamp);

-- ===================
-- Wallet Features (Bot Scanner)
-- ===================

CREATE TABLE IF NOT EXISTS wallet_features (
    address VARCHAR(42) PRIMARY KEY,
    total_trades BIGINT NOT NULL DEFAULT 0,
    interval_cv DOUBLE PRECISION,
    win_rate DOUBLE PRECISION,
    avg_latency_ms DOUBLE PRECISION,
    markets_traded BIGINT NOT NULL DEFAULT 0,
    has_opposing_positions BOOLEAN NOT NULL DEFAULT FALSE,
    opposing_position_count BIGINT NOT NULL DEFAULT 0,
    hourly_distribution BIGINT[24] NOT NULL DEFAULT ARRAY[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]::BIGINT[],
    activity_spread DOUBLE PRECISION NOT NULL DEFAULT 0,
    total_volume DECIMAL(18, 8) NOT NULL DEFAULT 0,
    first_trade TIMESTAMPTZ,
    last_trade TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ===================
-- Bot Scores (Bot Scanner)
-- ===================

CREATE TABLE IF NOT EXISTS bot_scores (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    address VARCHAR(42) NOT NULL REFERENCES wallet_features(address),
    total_score INTEGER NOT NULL,
    signals JSONB NOT NULL DEFAULT '[]'::JSONB,
    classification SMALLINT NOT NULL DEFAULT 0,  -- 0 = likely_human, 1 = suspicious, 2 = likely_bot
    computed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for bot_scores
CREATE INDEX idx_bot_scores_address ON bot_scores(address);
CREATE INDEX idx_bot_scores_classification ON bot_scores(classification);
CREATE INDEX idx_bot_scores_total_score ON bot_scores(total_score DESC);
CREATE INDEX idx_bot_scores_computed_at ON bot_scores(computed_at);

-- ===================
-- Arbitrage Opportunities (Historical)
-- ===================

CREATE TABLE IF NOT EXISTS arb_opportunities (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    market_id VARCHAR(255) NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    yes_ask DECIMAL(18, 8) NOT NULL,
    no_ask DECIMAL(18, 8) NOT NULL,
    total_cost DECIMAL(18, 8) NOT NULL,
    gross_profit DECIMAL(18, 8) NOT NULL,
    net_profit DECIMAL(18, 8) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for arb_opportunities
CREATE INDEX idx_arb_opportunities_market_id ON arb_opportunities(market_id);
CREATE INDEX idx_arb_opportunities_timestamp ON arb_opportunities(timestamp);
CREATE INDEX idx_arb_opportunities_net_profit ON arb_opportunities(net_profit DESC);

-- ===================
-- Update timestamp trigger
-- ===================

CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ language 'plpgsql';

CREATE TRIGGER update_positions_updated_at
    BEFORE UPDATE ON positions
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_wallet_features_updated_at
    BEFORE UPDATE ON wallet_features
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();
