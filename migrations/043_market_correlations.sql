-- Market correlation pairs.
-- Stores Pearson correlation coefficients between same-category markets
-- computed weekly from orderbook_hourly price series.
-- Used by the cross-market signal generator to detect divergence opportunities.

CREATE TABLE IF NOT EXISTS market_correlations (
    condition_id_a  TEXT NOT NULL,
    condition_id_b  TEXT NOT NULL,
    correlation     DOUBLE PRECISION NOT NULL,   -- Pearson r, range [-1, 1]
    category        TEXT,                        -- shared category from market_metadata
    sample_size     INT NOT NULL,                -- number of overlapping hourly observations
    computed_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (condition_id_a, condition_id_b),
    -- Canonical ordering avoids duplicate pairs
    CONSTRAINT market_correlations_ordered CHECK (condition_id_a < condition_id_b)
);

-- Lookup correlations for a specific market
CREATE INDEX IF NOT EXISTS idx_market_correlations_a
    ON market_correlations (condition_id_a, correlation DESC);

CREATE INDEX IF NOT EXISTS idx_market_correlations_b
    ON market_correlations (condition_id_b, correlation DESC);

-- Filter by category for batch recomputation
CREATE INDEX IF NOT EXISTS idx_market_correlations_category
    ON market_correlations (category);

-- Only keep strongly correlated pairs (|r| > 0.5)
-- Weakly correlated pairs are noise and waste storage.
CREATE INDEX IF NOT EXISTS idx_market_correlations_strong
    ON market_correlations (ABS(correlation) DESC)
    WHERE ABS(correlation) > 0.50;

-- For stale pair pruning (weekly recomputation deletes pairs > 14 days old)
CREATE INDEX IF NOT EXISTS idx_market_correlations_computed_at
    ON market_correlations (computed_at DESC);
