-- Fee bumps: a transfer left waiting below the network's gas price is signed
-- again at the same nonce for more gas. Only one transaction per nonce can be
-- mined, but any of them may be, so the hash of every transaction broadcast
-- for a withdrawal is kept: tx_hash holds the latest (or, once settled, the
-- one mined) and replaced_tx_hashes the others. signed_at is when tx_hash's
-- transaction was signed (NULL for rows journaled before this column).

ALTER TABLE wallet_withdrawals
    ADD COLUMN IF NOT EXISTS replaced_tx_hashes TEXT[] NOT NULL DEFAULT '{}',
    ADD COLUMN IF NOT EXISTS signed_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_wallet_withdrawals_replaced_tx_hashes
    ON wallet_withdrawals USING GIN (replaced_tx_hashes);
