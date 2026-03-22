-- Persist on-chain wallet withdrawals initiated from the dashboard/vault API.

CREATE TABLE IF NOT EXISTS wallet_withdrawals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id UUID REFERENCES workspaces(id) ON DELETE SET NULL,
    wallet_address VARCHAR(42) NOT NULL,
    destination_address VARCHAR(42) NOT NULL,
    asset VARCHAR(16) NOT NULL DEFAULT 'USDC',
    amount NUMERIC(20, 6) NOT NULL,
    status VARCHAR(32) NOT NULL DEFAULT 'pending',
    tx_hash VARCHAR(80),
    error TEXT,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    confirmed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_wallet_withdrawals_user_requested_at
    ON wallet_withdrawals(user_id, requested_at DESC);

CREATE INDEX IF NOT EXISTS idx_wallet_withdrawals_wallet_requested_at
    ON wallet_withdrawals(wallet_address, requested_at DESC);

CREATE UNIQUE INDEX IF NOT EXISTS idx_wallet_withdrawals_tx_hash
    ON wallet_withdrawals(tx_hash)
    WHERE tx_hash IS NOT NULL;
