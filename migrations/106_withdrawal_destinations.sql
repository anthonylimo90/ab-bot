-- Withdrawals go only to destinations an owner or admin added beforehand,
-- each usable once a delay has passed since it was added (time to notice an
-- address added by someone who took over a session). Removing one is
-- immediate. The rows are kept, with who added and removed them.

CREATE TABLE withdrawal_destinations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    -- Lowercase 0x address.
    address TEXT NOT NULL CHECK (address ~ '^0x[0-9a-f]{40}$'),
    label TEXT,
    added_by UUID REFERENCES users(id) ON DELETE SET NULL,
    added_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    usable_from TIMESTAMPTZ NOT NULL,
    removed_by UUID REFERENCES users(id) ON DELETE SET NULL,
    removed_at TIMESTAMPTZ
);

-- One live entry per address and workspace.
CREATE UNIQUE INDEX idx_withdrawal_destinations_active
    ON withdrawal_destinations (workspace_id, address)
    WHERE removed_at IS NULL;

-- The daily cap sums a workspace's recent withdrawals.
CREATE INDEX IF NOT EXISTS idx_wallet_withdrawals_workspace_requested_at
    ON wallet_withdrawals (workspace_id, requested_at DESC);
