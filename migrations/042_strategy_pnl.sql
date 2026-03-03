-- Per-strategy P&L snapshots.
-- Computed periodically by the strategy P&L calculator.
-- Each row captures a strategy's performance over a rolling window.
-- Used for strategy performance dashboards and dynamic tuner feedback.

CREATE TABLE IF NOT EXISTS strategy_pnl_snapshots (
    strategy    TEXT NOT NULL,           -- flow, cross_market, mean_reversion, resolution_proximity, arb, copy_trade
    period_end  TIMESTAMPTZ NOT NULL,
    period_days INT NOT NULL,            -- 7 or 30
    total_signals INT DEFAULT 0,
    executed    INT DEFAULT 0,
    wins        INT DEFAULT 0,
    losses      INT DEFAULT 0,
    net_pnl     DECIMAL(20, 10) DEFAULT 0,
    avg_pnl     DECIMAL(20, 10) DEFAULT 0,
    win_rate    DOUBLE PRECISION,
    sharpe      DOUBLE PRECISION,
    max_drawdown_pct DOUBLE PRECISION DEFAULT 0,
    avg_hold_hours   DOUBLE PRECISION DEFAULT 0,
    PRIMARY KEY (strategy, period_end, period_days)
);

-- Convert to hypertable for time-series queries
SELECT create_hypertable(
    'strategy_pnl_snapshots',
    'period_end',
    chunk_time_interval => INTERVAL '7 days',
    if_not_exists => TRUE
);

-- Compression for older snapshots
ALTER TABLE strategy_pnl_snapshots SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'strategy,period_days'
);

SELECT add_compression_policy('strategy_pnl_snapshots', INTERVAL '30 days', if_not_exists => TRUE);

-- 1 year retention
SELECT add_retention_policy('strategy_pnl_snapshots', INTERVAL '365 days', if_not_exists => TRUE);

-- Dashboard query: latest snapshot per strategy
CREATE INDEX IF NOT EXISTS idx_strategy_pnl_latest
    ON strategy_pnl_snapshots (strategy, period_days, period_end DESC);
