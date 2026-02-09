-- Phase 3: Backtesting Framework - TimescaleDB Tables

-- ===================
-- Enable TimescaleDB Extension
-- ===================

CREATE EXTENSION IF NOT EXISTS timescaledb CASCADE;

-- ===================
-- Orderbook Snapshots (Hypertable)
-- ===================

CREATE TABLE IF NOT EXISTS orderbook_snapshots (
    market_id VARCHAR(255) NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,

    -- Yes outcome prices
    yes_bid DECIMAL(18, 8) NOT NULL,
    yes_ask DECIMAL(18, 8) NOT NULL,
    yes_mid DECIMAL(18, 8) NOT NULL,
    yes_spread DECIMAL(18, 8) NOT NULL,

    -- No outcome prices
    no_bid DECIMAL(18, 8) NOT NULL,
    no_ask DECIMAL(18, 8) NOT NULL,
    no_mid DECIMAL(18, 8) NOT NULL,
    no_spread DECIMAL(18, 8) NOT NULL,

    -- Depth (quantity available at best price)
    yes_bid_depth DECIMAL(18, 8) NOT NULL DEFAULT 0,
    yes_ask_depth DECIMAL(18, 8) NOT NULL DEFAULT 0,
    no_bid_depth DECIMAL(18, 8) NOT NULL DEFAULT 0,
    no_ask_depth DECIMAL(18, 8) NOT NULL DEFAULT 0,

    -- Volume
    volume_24h DECIMAL(18, 8) NOT NULL DEFAULT 0,

    PRIMARY KEY (market_id, timestamp)
);

-- Convert to TimescaleDB hypertable
SELECT create_hypertable(
    'orderbook_snapshots',
    'timestamp',
    chunk_time_interval => INTERVAL '1 day',
    if_not_exists => TRUE
);

-- Compression policy (compress data older than 7 days)
ALTER TABLE orderbook_snapshots SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'market_id'
);

SELECT add_compression_policy('orderbook_snapshots', INTERVAL '7 days', if_not_exists => TRUE);

-- Retention policy (drop data older than 1 year)
SELECT add_retention_policy('orderbook_snapshots', INTERVAL '1 year', if_not_exists => TRUE);

-- Indexes for common queries
CREATE INDEX IF NOT EXISTS idx_orderbook_market_time ON orderbook_snapshots (market_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_orderbook_time ON orderbook_snapshots (timestamp DESC);

-- ===================
-- Historical Trades (Hypertable)
-- ===================

CREATE TABLE IF NOT EXISTS historical_trades (
    id UUID NOT NULL,
    market_id VARCHAR(255) NOT NULL,
    outcome_id VARCHAR(50) NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    price DECIMAL(18, 8) NOT NULL,
    quantity DECIMAL(18, 8) NOT NULL,
    side SMALLINT NOT NULL,  -- 0=buy, 1=sell
    fee DECIMAL(18, 8) NOT NULL DEFAULT 0,

    PRIMARY KEY (id, timestamp)
);

-- Convert to TimescaleDB hypertable
SELECT create_hypertable(
    'historical_trades',
    'timestamp',
    chunk_time_interval => INTERVAL '1 day',
    if_not_exists => TRUE
);

-- Compression policy
ALTER TABLE historical_trades SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'market_id'
);

SELECT add_compression_policy('historical_trades', INTERVAL '7 days', if_not_exists => TRUE);

-- Retention policy
SELECT add_retention_policy('historical_trades', INTERVAL '1 year', if_not_exists => TRUE);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_hist_trades_market_time ON historical_trades (market_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_hist_trades_time ON historical_trades (timestamp DESC);

-- ===================
-- Backtest Results
-- ===================

CREATE TABLE IF NOT EXISTS backtest_results (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),

    -- Strategy info
    strategy_name VARCHAR(100) NOT NULL,
    strategy_params JSONB NOT NULL DEFAULT '{}',

    -- Time range
    start_time TIMESTAMPTZ NOT NULL,
    end_time TIMESTAMPTZ NOT NULL,
    data_points INTEGER NOT NULL,

    -- Capital
    initial_capital DECIMAL(18, 8) NOT NULL,
    final_value DECIMAL(18, 8) NOT NULL,

    -- Returns
    total_return DECIMAL(18, 8) NOT NULL,
    return_pct DECIMAL(10, 6) NOT NULL,
    annualized_return DECIMAL(10, 6) NOT NULL,

    -- Risk metrics
    max_drawdown DECIMAL(10, 6) NOT NULL,
    sharpe_ratio DECIMAL(10, 6) NOT NULL,
    sortino_ratio DECIMAL(10, 6) NOT NULL,

    -- Trade metrics
    win_rate DECIMAL(5, 4) NOT NULL,
    profit_factor DECIMAL(10, 4) NOT NULL,
    total_trades INTEGER NOT NULL,
    winning_trades INTEGER NOT NULL,
    losing_trades INTEGER NOT NULL,

    -- Costs
    total_fees DECIMAL(18, 8) NOT NULL,
    total_slippage DECIMAL(18, 8) NOT NULL,
    avg_trade_duration_hours DECIMAL(10, 2) NOT NULL,

    -- Full data (stored as JSONB for flexibility)
    equity_curve JSONB,
    trade_log JSONB,

    -- Metadata
    computed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    notes TEXT
);

CREATE INDEX IF NOT EXISTS idx_backtest_strategy ON backtest_results(strategy_name);
CREATE INDEX IF NOT EXISTS idx_backtest_computed ON backtest_results(computed_at);
CREATE INDEX IF NOT EXISTS idx_backtest_sharpe ON backtest_results(sharpe_ratio DESC);
CREATE INDEX IF NOT EXISTS idx_backtest_return ON backtest_results(return_pct DESC);

-- ===================
-- Strategy Configurations
-- ===================

CREATE TABLE IF NOT EXISTS strategy_configs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name VARCHAR(100) NOT NULL UNIQUE,
    strategy_type VARCHAR(50) NOT NULL,
    parameters JSONB NOT NULL DEFAULT '{}',
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    description TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_strat_config_type ON strategy_configs(strategy_type);
CREATE INDEX IF NOT EXISTS idx_strat_config_enabled ON strategy_configs(enabled) WHERE enabled = TRUE;

CREATE TRIGGER update_strategy_configs_updated_at
    BEFORE UPDATE ON strategy_configs
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- ===================
-- Data Ingestion Jobs
-- ===================

CREATE TABLE IF NOT EXISTS data_ingestion_jobs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    job_type VARCHAR(50) NOT NULL,  -- 'orderbook', 'trades', 'full_sync'
    market_id VARCHAR(255),
    status VARCHAR(20) NOT NULL DEFAULT 'pending',  -- pending, running, completed, failed
    start_time TIMESTAMPTZ,
    end_time TIMESTAMPTZ,
    records_processed BIGINT NOT NULL DEFAULT 0,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_ingestion_status ON data_ingestion_jobs(status);
CREATE INDEX IF NOT EXISTS idx_ingestion_type ON data_ingestion_jobs(job_type);
CREATE INDEX IF NOT EXISTS idx_ingestion_market ON data_ingestion_jobs(market_id) WHERE market_id IS NOT NULL;

CREATE TRIGGER update_ingestion_jobs_updated_at
    BEFORE UPDATE ON data_ingestion_jobs
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- ===================
-- Continuous Aggregates for Common Queries
-- ===================

-- 5-minute OHLCV aggregation for yes outcome
CREATE MATERIALIZED VIEW IF NOT EXISTS orderbook_5min
WITH (timescaledb.continuous) AS
SELECT
    market_id,
    time_bucket('5 minutes', timestamp) AS bucket,
    first(yes_mid, timestamp) AS open,
    max(yes_mid) AS high,
    min(yes_mid) AS low,
    last(yes_mid, timestamp) AS close,
    avg(volume_24h) AS avg_volume,
    count(*) AS tick_count
FROM orderbook_snapshots
GROUP BY market_id, time_bucket('5 minutes', timestamp)
WITH NO DATA;

-- Refresh policy for continuous aggregate
SELECT add_continuous_aggregate_policy('orderbook_5min',
    start_offset => INTERVAL '1 hour',
    end_offset => INTERVAL '5 minutes',
    schedule_interval => INTERVAL '5 minutes',
    if_not_exists => TRUE
);

-- Hourly aggregation
CREATE MATERIALIZED VIEW IF NOT EXISTS orderbook_hourly
WITH (timescaledb.continuous) AS
SELECT
    market_id,
    time_bucket('1 hour', timestamp) AS bucket,
    first(yes_mid, timestamp) AS open,
    max(yes_mid) AS high,
    min(yes_mid) AS low,
    last(yes_mid, timestamp) AS close,
    avg(yes_spread) AS avg_spread,
    avg(volume_24h) AS avg_volume,
    count(*) AS tick_count
FROM orderbook_snapshots
GROUP BY market_id, time_bucket('1 hour', timestamp)
WITH NO DATA;

SELECT add_continuous_aggregate_policy('orderbook_hourly',
    start_offset => INTERVAL '4 hours',
    end_offset => INTERVAL '1 hour',
    schedule_interval => INTERVAL '1 hour',
    if_not_exists => TRUE
);

-- Daily aggregation
CREATE MATERIALIZED VIEW IF NOT EXISTS orderbook_daily
WITH (timescaledb.continuous) AS
SELECT
    market_id,
    time_bucket('1 day', timestamp) AS bucket,
    first(yes_mid, timestamp) AS open,
    max(yes_mid) AS high,
    min(yes_mid) AS low,
    last(yes_mid, timestamp) AS close,
    avg(yes_spread) AS avg_spread,
    max(volume_24h) AS max_volume,
    count(*) AS tick_count
FROM orderbook_snapshots
GROUP BY market_id, time_bucket('1 day', timestamp)
WITH NO DATA;

SELECT add_continuous_aggregate_policy('orderbook_daily',
    start_offset => INTERVAL '3 days',
    end_offset => INTERVAL '1 day',
    schedule_interval => INTERVAL '1 day',
    if_not_exists => TRUE
);

-- ===================
-- Helper Functions
-- ===================

-- Function to get data range for a market
CREATE OR REPLACE FUNCTION get_market_data_range(p_market_id VARCHAR(255))
RETURNS TABLE (min_time TIMESTAMPTZ, max_time TIMESTAMPTZ, snapshot_count BIGINT)
LANGUAGE SQL
AS $$
    SELECT
        MIN(timestamp),
        MAX(timestamp),
        COUNT(*)
    FROM orderbook_snapshots
    WHERE market_id = p_market_id;
$$;

-- Function to calculate arbitrage opportunities
CREATE OR REPLACE FUNCTION find_arbitrage_opportunities(
    p_min_spread DECIMAL DEFAULT 0.02,
    p_start_time TIMESTAMPTZ DEFAULT NOW() - INTERVAL '1 day',
    p_end_time TIMESTAMPTZ DEFAULT NOW()
)
RETURNS TABLE (
    market_id VARCHAR(255),
    snapshot_time TIMESTAMPTZ,
    yes_ask DECIMAL,
    no_ask DECIMAL,
    spread DECIMAL
)
LANGUAGE SQL
AS $$
    SELECT
        market_id,
        timestamp AS snapshot_time,
        yes_ask,
        no_ask,
        (1 - yes_ask - no_ask) AS spread
    FROM orderbook_snapshots
    WHERE timestamp >= p_start_time
      AND timestamp <= p_end_time
      AND (1 - yes_ask - no_ask) >= p_min_spread
    ORDER BY spread DESC, timestamp DESC;
$$;
