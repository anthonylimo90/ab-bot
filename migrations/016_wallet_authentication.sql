-- Migration: 016_wallet_authentication.sql
-- Adds wallet authentication (SIWE) support

-- Add wallet_address column to users table
ALTER TABLE users
ADD COLUMN wallet_address VARCHAR(42) UNIQUE,
ADD COLUMN wallet_linked_at TIMESTAMPTZ;

-- Index for wallet address lookups
CREATE INDEX idx_users_wallet_address ON users(wallet_address) WHERE wallet_address IS NOT NULL;

-- Auth challenges table for SIWE nonces
CREATE TABLE auth_challenges (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    wallet_address VARCHAR(42) NOT NULL,
    nonce VARCHAR(64) NOT NULL UNIQUE,
    issued_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ
);

-- Index for nonce lookups
CREATE INDEX idx_auth_challenges_nonce ON auth_challenges(nonce);
-- Index for cleanup of expired challenges
CREATE INDEX idx_auth_challenges_expires_at ON auth_challenges(expires_at);

-- Pending order signatures for MetaMask trade signing
CREATE TABLE pending_order_signatures (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    wallet_address VARCHAR(42) NOT NULL,
    order_data JSONB NOT NULL,
    order_hash VARCHAR(66) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    signed_at TIMESTAMPTZ,
    signature VARCHAR(132)
);

-- Index for user's pending orders
CREATE INDEX idx_pending_order_signatures_user_id ON pending_order_signatures(user_id);
-- Index for cleanup of expired orders
CREATE INDEX idx_pending_order_signatures_expires_at ON pending_order_signatures(expires_at);
