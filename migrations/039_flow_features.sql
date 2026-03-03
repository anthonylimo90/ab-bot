-- Market flow features for quantitative signal generation.
-- Aggregated from wallet_trades at multiple time windows (15min, 60min, 240min).
-- Used by flow signal generator to detect smart money order flow imbalances.

CREATE TABLE IF NOT EXISTS market_flow_features (
    condition_id     TEXT NOT NULL,
    window_end       TIMESTAMPTZ NOT NULL,
    window_minutes   INT NOT NULL,
    buy_volume       DECIMAL(20, 10) DEFAULT 0,
    sell_volume      DECIMAL(20, 10) DEFAULT 0,
    net_flow         DECIMAL(20, 10) DEFAULT 0,
    imbalance_ratio  DECIMAL(10, 6) DEFAULT 0,  -- net_flow / total, range [-1, 1]
    unique_buyers    INT DEFAULT 0,
    unique_sellers   INT DEFAULT 0,
    smart_money_flow DECIMAL(20, 10) DEFAULT 0, -- flow from wallets with bot_score < 30
    trade_count      INT DEFAULT 0,
    PRIMARY KEY (condition_id, window_end, window_minutes)
);

-- Convert to TimescaleDB hypertable for time-series performance
SELECT create_hypertable(
    'market_flow_features',
    'window_end',
    chunk_time_interval => INTERVAL '1 day',
    if_not_exists => TRUE
);

-- Enable compression for older data
ALTER TABLE market_flow_features SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'condition_id,window_minutes'
);

SELECT add_compression_policy('market_flow_features', INTERVAL '7 days', if_not_exists => TRUE);

-- Retain 90 days of flow data
SELECT add_retention_policy('market_flow_features', INTERVAL '90 days', if_not_exists => TRUE);

-- Primary query: latest features for a market at a given window
CREATE INDEX IF NOT EXISTS idx_flow_features_market_window
    ON market_flow_features (condition_id, window_minutes, window_end DESC);

-- Signal generator scan: high-imbalance markets
CREATE INDEX IF NOT EXISTS idx_flow_features_imbalance
    ON market_flow_features (window_minutes, window_end DESC, imbalance_ratio)
    WHERE trade_count >= 5;
