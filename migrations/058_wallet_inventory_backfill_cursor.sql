ALTER TABLE wallet_inventory_sync_state
    ADD COLUMN IF NOT EXISTS backfill_cursor_block BIGINT;

ALTER TABLE wallet_inventory_sync_state
    ADD COLUMN IF NOT EXISTS backfill_completed_at TIMESTAMPTZ;
