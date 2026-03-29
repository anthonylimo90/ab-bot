-- Add client_request_id to wallet_withdrawals for idempotent retry safety.

ALTER TABLE wallet_withdrawals
    ADD COLUMN IF NOT EXISTS client_request_id UUID;

-- Unique constraint scoped to wallet_address so two different wallets can't
-- collide on the same UUID.  NULL values are excluded (existing rows without
-- a client_request_id, and older client versions that omit it).
CREATE UNIQUE INDEX IF NOT EXISTS idx_wallet_withdrawals_client_request_id
    ON wallet_withdrawals (wallet_address, client_request_id)
    WHERE client_request_id IS NOT NULL;
