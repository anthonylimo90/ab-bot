-- Migration: Create pending_wallet_orders table for MetaMask order signing flow
-- This table stores orders that are prepared but not yet signed/submitted

CREATE TABLE IF NOT EXISTS pending_wallet_orders (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    maker_address VARCHAR(42) NOT NULL,
    token_id VARCHAR(78) NOT NULL,
    side SMALLINT NOT NULL CHECK (side IN (0, 1)), -- 0 = BUY, 1 = SELL
    maker_amount VARCHAR(78) NOT NULL,
    taker_amount VARCHAR(78) NOT NULL,
    salt VARCHAR(78) NOT NULL,
    expiration BIGINT NOT NULL,
    nonce VARCHAR(78) NOT NULL DEFAULT '0',
    fee_rate_bps INTEGER NOT NULL DEFAULT 0,
    signature_type SMALLINT NOT NULL DEFAULT 0, -- 0 = EOA, 1 = POLY, 2 = POLY_PROXY
    neg_risk BOOLEAN NOT NULL DEFAULT FALSE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for user lookup
CREATE INDEX idx_pending_wallet_orders_user ON pending_wallet_orders(user_id);

-- Index for cleanup of expired orders
CREATE INDEX idx_pending_wallet_orders_expires ON pending_wallet_orders(expires_at);

-- Comment
COMMENT ON TABLE pending_wallet_orders IS 'Pending orders awaiting wallet signature for MetaMask trade signing flow';
