-- CLOB V2 (live since 2026-04-28) signs an order's creation time in
-- milliseconds and no longer signs a nonce or fee rate. Orders prepared for
-- the V1 exchange can no longer be submitted, so they are dropped.
DELETE FROM pending_wallet_orders;
ALTER TABLE pending_wallet_orders
    ADD COLUMN order_timestamp BIGINT NOT NULL,
    DROP COLUMN nonce,
    DROP COLUMN fee_rate_bps;
