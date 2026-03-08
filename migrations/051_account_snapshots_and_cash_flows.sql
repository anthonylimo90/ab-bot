CREATE TABLE IF NOT EXISTS account_snapshots (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    snapshot_time TIMESTAMPTZ NOT NULL,
    wallet_address TEXT,
    cash_balance DECIMAL(18, 8) NOT NULL DEFAULT 0,
    position_value DECIMAL(18, 8) NOT NULL DEFAULT 0,
    total_equity DECIMAL(18, 8) NOT NULL DEFAULT 0,
    unrealized_pnl DECIMAL(18, 8) NOT NULL DEFAULT 0,
    realized_pnl_24h DECIMAL(18, 8) NOT NULL DEFAULT 0,
    net_cash_flows_24h DECIMAL(18, 8) NOT NULL DEFAULT 0,
    open_positions INTEGER NOT NULL DEFAULT 0,
    open_markets INTEGER NOT NULL DEFAULT 0,
    unpriced_open_positions INTEGER NOT NULL DEFAULT 0,
    unpriced_position_cost_basis DECIMAL(18, 8) NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (workspace_id, snapshot_time)
);

CREATE INDEX IF NOT EXISTS idx_account_snapshots_workspace_time
    ON account_snapshots (workspace_id, snapshot_time DESC);

CREATE TABLE IF NOT EXISTS cash_flow_events (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    amount DECIMAL(18, 8) NOT NULL,
    currency TEXT NOT NULL DEFAULT 'USDC',
    note TEXT,
    occurred_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_by UUID
);

CREATE INDEX IF NOT EXISTS idx_cash_flow_events_workspace_time
    ON cash_flow_events (workspace_id, occurred_at DESC);
