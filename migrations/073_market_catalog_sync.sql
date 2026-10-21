-- Marks a complete catalog publication, including an intentionally empty page.
CREATE TABLE market_catalog_sync (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    refreshed_at TIMESTAMPTZ NOT NULL
);
