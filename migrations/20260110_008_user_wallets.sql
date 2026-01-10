-- User wallets table for storing connected wallet metadata
-- Private keys are stored in KeyVault, not in the database
-- Migration: 008 - Add user wallets table

CREATE TABLE IF NOT EXISTS user_wallets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    address VARCHAR(42) NOT NULL,
    label VARCHAR(255),
    is_primary BOOLEAN DEFAULT false,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(user_id, address)
);

-- Index for faster lookups by user
CREATE INDEX IF NOT EXISTS idx_user_wallets_user_id ON user_wallets(user_id);

-- Index for address lookups
CREATE INDEX IF NOT EXISTS idx_user_wallets_address ON user_wallets(address);

-- Ensure only one primary wallet per user
CREATE UNIQUE INDEX IF NOT EXISTS idx_user_wallets_primary
ON user_wallets(user_id)
WHERE is_primary = true;
