-- Market metadata from Gamma API.
-- Provides categories, tags, end dates, and resolution criteria
-- not available from the CLOB API.

CREATE TABLE IF NOT EXISTS market_metadata (
    condition_id TEXT PRIMARY KEY,
    question     TEXT NOT NULL,
    category     TEXT,
    tags         TEXT[] DEFAULT '{}',
    end_date     TIMESTAMPTZ,
    volume       DECIMAL(20, 10) DEFAULT 0,
    liquidity    DECIMAL(20, 10) DEFAULT 0,
    active       BOOLEAN NOT NULL DEFAULT TRUE,
    fetched_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Category lookup for signal generators (resolution proximity, cross-market)
CREATE INDEX IF NOT EXISTS idx_market_metadata_category
    ON market_metadata (category)
    WHERE category IS NOT NULL;

-- End date for resolution proximity signals
CREATE INDEX IF NOT EXISTS idx_market_metadata_end_date
    ON market_metadata (end_date)
    WHERE end_date IS NOT NULL AND active = TRUE;

-- Active markets filter
CREATE INDEX IF NOT EXISTS idx_market_metadata_active
    ON market_metadata (active);

-- Tag search (GIN index for array containment queries)
CREATE INDEX IF NOT EXISTS idx_market_metadata_tags
    ON market_metadata USING GIN (tags);
