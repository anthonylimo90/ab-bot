-- Add composite indices for frequently queried column combinations

-- workspace_wallet_allocations: most-queried table in auto-optimizer and allocation handlers
CREATE INDEX IF NOT EXISTS idx_wwa_workspace_wallet
    ON workspace_wallet_allocations(workspace_id, wallet_address);
CREATE INDEX IF NOT EXISTS idx_wwa_workspace_tier
    ON workspace_wallet_allocations(workspace_id, tier);

-- workspace_members: permission checks on every authenticated request
CREATE INDEX IF NOT EXISTS idx_wm_workspace_user
    ON workspace_members(workspace_id, user_id);

-- positions: lifecycle queries filter by strategy + state
CREATE INDEX IF NOT EXISTS idx_positions_strategy_state
    ON positions(exit_strategy, state);
CREATE INDEX IF NOT EXISTS idx_positions_state_retry
    ON positions(state, retry_count) WHERE state IN (6, 7);

-- user_wallets: vault operations filter by user + address
CREATE INDEX IF NOT EXISTS idx_user_wallets_user_address
    ON user_wallets(user_id, address);

-- workspace_invites: listing and validation queries
CREATE INDEX IF NOT EXISTS idx_invites_workspace_pending
    ON workspace_invites(workspace_id, expires_at) WHERE accepted_at IS NULL;
