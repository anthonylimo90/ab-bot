-- Speed up latest-price lookups used by flow signal generation.
CREATE INDEX IF NOT EXISTS idx_orderbook_hourly_market_bucket
    ON orderbook_hourly (market_id, bucket DESC);
