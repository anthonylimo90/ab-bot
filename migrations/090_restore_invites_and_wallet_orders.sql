-- Migration 044 (copy-trading removal) also dropped two tables that live
-- features still use: workspace invites, which the dashboard's invite flow
-- and admin-created workspaces depend on, and pending wallet orders, behind
-- POST /api/v1/orders/prepare and /orders/submit. Every one of those requests
-- has failed on a database built from the migrations. Recreate both as they
-- were defined (011, 012, 018, 020); IF NOT EXISTS leaves a database that
-- still has them untouched.

CREATE TABLE IF NOT EXISTS workspace_invites (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    email VARCHAR(255) NOT NULL,
    role VARCHAR(20) NOT NULL DEFAULT 'member',
    token_hash VARCHAR(255) NOT NULL UNIQUE,
    invited_by UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    accepted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT valid_invite_role CHECK (role IN ('owner', 'admin', 'member', 'viewer'))
);
CREATE INDEX IF NOT EXISTS idx_workspace_invites_workspace ON workspace_invites(workspace_id);
CREATE INDEX IF NOT EXISTS idx_workspace_invites_email ON workspace_invites(email);
CREATE INDEX IF NOT EXISTS idx_workspace_invites_expires ON workspace_invites(expires_at);
CREATE INDEX IF NOT EXISTS idx_invites_workspace_pending
    ON workspace_invites(workspace_id, expires_at) WHERE accepted_at IS NULL;

CREATE TABLE IF NOT EXISTS pending_wallet_orders (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    maker_address VARCHAR(42) NOT NULL,
    token_id VARCHAR(78) NOT NULL,
    side SMALLINT NOT NULL CHECK (side IN (0, 1)),
    maker_amount VARCHAR(78) NOT NULL,
    taker_amount VARCHAR(78) NOT NULL,
    salt VARCHAR(78) NOT NULL,
    expiration BIGINT NOT NULL,
    nonce VARCHAR(78) NOT NULL DEFAULT '0',
    fee_rate_bps INTEGER NOT NULL DEFAULT 0,
    signature_type SMALLINT NOT NULL DEFAULT 0,
    neg_risk BOOLEAN NOT NULL DEFAULT FALSE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_pending_wallet_orders_user ON pending_wallet_orders(user_id);
CREATE INDEX IF NOT EXISTS idx_pending_wallet_orders_expires ON pending_wallet_orders(expires_at);
