-- Demo positions table for workspace-scoped demo trading
-- These positions are visible to all workspace members

CREATE TABLE IF NOT EXISTS demo_positions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    created_by UUID NOT NULL REFERENCES users(id),
    wallet_address VARCHAR(255) NOT NULL,
    wallet_label VARCHAR(255),
    market_id VARCHAR(255) NOT NULL,
    market_question TEXT,
    outcome VARCHAR(10) NOT NULL CHECK (outcome IN ('yes', 'no')),
    quantity NUMERIC(20, 8) NOT NULL,
    entry_price NUMERIC(10, 4) NOT NULL,
    current_price NUMERIC(10, 4),
    opened_at TIMESTAMPTZ NOT NULL,
    closed_at TIMESTAMPTZ,
    exit_price NUMERIC(10, 4),
    realized_pnl NUMERIC(20, 8),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for fetching positions by workspace
CREATE INDEX IF NOT EXISTS idx_demo_positions_workspace ON demo_positions(workspace_id);

-- Index for fetching open positions by workspace (common query)
CREATE INDEX IF NOT EXISTS idx_demo_positions_open ON demo_positions(workspace_id) WHERE closed_at IS NULL;

-- Index for fetching closed positions by workspace
CREATE INDEX IF NOT EXISTS idx_demo_positions_closed ON demo_positions(workspace_id) WHERE closed_at IS NOT NULL;

-- Demo balance table for workspace-scoped demo balances
CREATE TABLE IF NOT EXISTS demo_balances (
    workspace_id UUID PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    balance NUMERIC(20, 8) NOT NULL DEFAULT 10000,
    initial_balance NUMERIC(20, 8) NOT NULL DEFAULT 10000,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

COMMENT ON TABLE demo_positions IS 'Demo trading positions shared across workspace members';
COMMENT ON TABLE demo_balances IS 'Demo trading balance per workspace';
