CREATE TABLE IF NOT EXISTS wallet_inventory (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    wallet_address VARCHAR NOT NULL,
    token_id VARCHAR NOT NULL,
    condition_id VARCHAR,
    outcome VARCHAR(16),
    linked_position_id UUID REFERENCES positions(id) ON DELETE SET NULL,
    quantity DECIMAL(24, 8) NOT NULL DEFAULT 0,
    cost_basis DECIMAL(24, 8),
    current_price DECIMAL(24, 8),
    marked_value DECIMAL(24, 8),
    is_orphan BOOLEAN NOT NULL DEFAULT FALSE,
    discovery_source VARCHAR(32) NOT NULL DEFAULT 'unknown',
    recovery_status VARCHAR(32) NOT NULL DEFAULT 'linked',
    last_exit_error TEXT,
    last_exit_attempted_at TIMESTAMPTZ,
    first_observed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_observed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT wallet_inventory_wallet_token_unique UNIQUE (wallet_address, token_id),
    CONSTRAINT wallet_inventory_valid_outcome
        CHECK (outcome IS NULL OR LOWER(outcome) IN ('yes', 'no')),
    CONSTRAINT wallet_inventory_valid_recovery_status
        CHECK (recovery_status IN ('linked', 'observed', 'sell_failed', 'recovered')),
    CONSTRAINT wallet_inventory_non_negative_quantity CHECK (quantity >= 0),
    CONSTRAINT wallet_inventory_non_negative_cost_basis
        CHECK (cost_basis IS NULL OR cost_basis >= 0),
    CONSTRAINT wallet_inventory_non_negative_marked_value
        CHECK (marked_value IS NULL OR marked_value >= 0)
);

CREATE INDEX IF NOT EXISTS idx_wallet_inventory_wallet_quantity
    ON wallet_inventory (wallet_address, quantity DESC);

CREATE INDEX IF NOT EXISTS idx_wallet_inventory_condition
    ON wallet_inventory (condition_id)
    WHERE condition_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_wallet_inventory_orphan_status
    ON wallet_inventory (wallet_address, is_orphan, recovery_status, last_exit_attempted_at DESC);

CREATE INDEX IF NOT EXISTS idx_wallet_inventory_linked_position
    ON wallet_inventory (linked_position_id)
    WHERE linked_position_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS wallet_inventory_sync_state (
    wallet_address VARCHAR PRIMARY KEY,
    last_scanned_block BIGINT NOT NULL DEFAULT 0,
    backfill_cursor_block BIGINT,
    backfill_completed_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TRIGGER update_wallet_inventory_updated_at
    BEFORE UPDATE ON wallet_inventory
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_wallet_inventory_sync_state_updated_at
    BEFORE UPDATE ON wallet_inventory_sync_state
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();
