CREATE TABLE wallet_inventory_refresh_status (
    wallet_address TEXT PRIMARY KEY,
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    last_error TEXT,
    result JSONB
);
