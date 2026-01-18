-- User Onboarding & Workspace Management Tables
-- This migration adds support for workspaces, invites, allocations, and onboarding

-- ===================
-- Workspaces Table
-- ===================
-- Container for shared rosters and trading configuration

CREATE TABLE IF NOT EXISTS workspaces (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(100) NOT NULL,
    description TEXT,

    -- Setup mode
    setup_mode VARCHAR(20) NOT NULL DEFAULT 'manual',

    -- Budget configuration
    total_budget DECIMAL(18, 8) NOT NULL DEFAULT 0,
    reserved_cash_pct DECIMAL(5, 2) NOT NULL DEFAULT 10.0,

    -- Auto-optimization settings
    auto_optimize_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    optimization_interval_hours INTEGER NOT NULL DEFAULT 24,

    -- Auto-selection criteria thresholds
    min_roi_30d DECIMAL(10, 4) DEFAULT 5.0,
    min_sharpe DECIMAL(10, 4) DEFAULT 1.0,
    min_win_rate DECIMAL(5, 2) DEFAULT 50.0,
    min_trades_30d INTEGER DEFAULT 10,

    -- Trading wallet (connected wallet address for live execution)
    trading_wallet_address VARCHAR(42),

    -- Timestamps
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT valid_setup_mode CHECK (setup_mode IN ('manual', 'automatic'))
);

CREATE INDEX IF NOT EXISTS idx_workspaces_created_by ON workspaces(created_by);
CREATE INDEX IF NOT EXISTS idx_workspaces_name ON workspaces(name);

-- ===================
-- Workspace Members Table
-- ===================
-- Users belonging to workspaces with role-based access

CREATE TABLE IF NOT EXISTS workspace_members (
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,

    -- Role within workspace (not platform role)
    role VARCHAR(20) NOT NULL DEFAULT 'member',

    joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (workspace_id, user_id),
    CONSTRAINT valid_workspace_role CHECK (role IN ('owner', 'admin', 'member', 'viewer'))
);

CREATE INDEX IF NOT EXISTS idx_workspace_members_user ON workspace_members(user_id);
CREATE INDEX IF NOT EXISTS idx_workspace_members_workspace ON workspace_members(workspace_id);

-- ===================
-- User Settings Table
-- ===================
-- User preferences and onboarding state

CREATE TABLE IF NOT EXISTS user_settings (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,

    -- Onboarding state
    onboarding_completed BOOLEAN NOT NULL DEFAULT FALSE,
    onboarding_step INTEGER NOT NULL DEFAULT 0,

    -- Current workspace context
    default_workspace_id UUID REFERENCES workspaces(id) ON DELETE SET NULL,

    -- Preferences (can be extended)
    preferences JSONB DEFAULT '{}',

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_user_settings_workspace ON user_settings(default_workspace_id);

-- ===================
-- Workspace Wallet Allocations Table
-- ===================
-- Workspace's wallet roster with tier (active/bench) and allocations

CREATE TABLE IF NOT EXISTS workspace_wallet_allocations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    wallet_address VARCHAR(42) NOT NULL,

    -- Allocation configuration
    allocation_pct DECIMAL(5, 2) NOT NULL DEFAULT 20.0,
    max_position_size DECIMAL(18, 8),

    -- Tier: active (Active 5) or bench (watchlist)
    tier VARCHAR(20) NOT NULL DEFAULT 'bench',

    -- Auto-assignment tracking
    auto_assigned BOOLEAN NOT NULL DEFAULT FALSE,
    auto_assigned_reason TEXT,

    -- Backtest results (from auto-selection or manual backtest)
    backtest_roi DECIMAL(10, 4),
    backtest_sharpe DECIMAL(10, 4),
    backtest_win_rate DECIMAL(5, 2),

    -- Copy behavior settings
    copy_behavior VARCHAR(20) DEFAULT 'copy_all',
    arb_threshold_pct DECIMAL(5, 2),

    -- Audit
    added_by UUID REFERENCES users(id) ON DELETE SET NULL,
    added_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(workspace_id, wallet_address),
    CONSTRAINT valid_tier CHECK (tier IN ('active', 'bench')),
    CONSTRAINT valid_copy_behavior CHECK (copy_behavior IN ('copy_all', 'events_only', 'arb_threshold'))
);

CREATE INDEX IF NOT EXISTS idx_workspace_allocations_workspace ON workspace_wallet_allocations(workspace_id);
CREATE INDEX IF NOT EXISTS idx_workspace_allocations_tier ON workspace_wallet_allocations(tier);
CREATE INDEX IF NOT EXISTS idx_workspace_allocations_wallet ON workspace_wallet_allocations(wallet_address);

-- ===================
-- Workspace Invites Table
-- ===================
-- Pending invitations to join a workspace

CREATE TABLE IF NOT EXISTS workspace_invites (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,

    -- Invitee email (may not be a user yet)
    email VARCHAR(255) NOT NULL,

    -- Role to assign upon acceptance
    role VARCHAR(20) NOT NULL DEFAULT 'member',

    -- Secure token hash (token is sent via email)
    token_hash VARCHAR(255) NOT NULL UNIQUE,

    -- Audit and expiry
    invited_by UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    accepted_at TIMESTAMPTZ,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT valid_invite_role CHECK (role IN ('admin', 'member', 'viewer'))
);

CREATE INDEX IF NOT EXISTS idx_workspace_invites_workspace ON workspace_invites(workspace_id);
CREATE INDEX IF NOT EXISTS idx_workspace_invites_email ON workspace_invites(email);
CREATE INDEX IF NOT EXISTS idx_workspace_invites_token ON workspace_invites(token_hash);
CREATE INDEX IF NOT EXISTS idx_workspace_invites_expires ON workspace_invites(expires_at);

-- ===================
-- Auto Rotation History Table
-- ===================
-- Audit trail for automatic and manual roster changes

CREATE TABLE IF NOT EXISTS auto_rotation_history (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,

    -- Action type
    action VARCHAR(20) NOT NULL,

    -- Wallets involved
    wallet_in VARCHAR(42),  -- Wallet being promoted/added
    wallet_out VARCHAR(42), -- Wallet being demoted/removed

    -- Explanation
    reason TEXT NOT NULL,
    evidence JSONB DEFAULT '{}',

    -- Who triggered (NULL = automatic)
    triggered_by UUID REFERENCES users(id) ON DELETE SET NULL,

    -- Notification tracking
    notification_sent BOOLEAN NOT NULL DEFAULT FALSE,
    acknowledged BOOLEAN NOT NULL DEFAULT FALSE,
    acknowledged_at TIMESTAMPTZ,
    acknowledged_by UUID REFERENCES users(id) ON DELETE SET NULL,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT valid_action CHECK (action IN ('promote', 'demote', 'replace', 'add', 'remove'))
);

CREATE INDEX IF NOT EXISTS idx_rotation_history_workspace ON auto_rotation_history(workspace_id);
CREATE INDEX IF NOT EXISTS idx_rotation_history_created ON auto_rotation_history(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_rotation_history_acknowledged ON auto_rotation_history(acknowledged) WHERE NOT acknowledged;

-- ===================
-- Update Triggers
-- ===================

DROP TRIGGER IF EXISTS update_workspaces_updated_at ON workspaces;
CREATE TRIGGER update_workspaces_updated_at
    BEFORE UPDATE ON workspaces
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

DROP TRIGGER IF EXISTS update_user_settings_updated_at ON user_settings;
CREATE TRIGGER update_user_settings_updated_at
    BEFORE UPDATE ON user_settings
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

DROP TRIGGER IF EXISTS update_workspace_allocations_updated_at ON workspace_wallet_allocations;
CREATE TRIGGER update_workspace_allocations_updated_at
    BEFORE UPDATE ON workspace_wallet_allocations
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- ===================
-- Helper Functions
-- ===================

-- Function to count active wallets in a workspace (max 5)
CREATE OR REPLACE FUNCTION count_active_wallets(ws_id UUID)
RETURNS INTEGER AS $$
    SELECT COUNT(*)::INTEGER
    FROM workspace_wallet_allocations
    WHERE workspace_id = ws_id AND tier = 'active';
$$ LANGUAGE SQL STABLE;

-- Function to validate Active 5 limit before promotion
CREATE OR REPLACE FUNCTION check_active_wallet_limit()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.tier = 'active' AND (OLD.tier IS NULL OR OLD.tier = 'bench') THEN
        IF count_active_wallets(NEW.workspace_id) >= 5 THEN
            RAISE EXCEPTION 'Cannot have more than 5 active wallets per workspace';
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS enforce_active_wallet_limit ON workspace_wallet_allocations;
CREATE TRIGGER enforce_active_wallet_limit
    BEFORE INSERT OR UPDATE ON workspace_wallet_allocations
    FOR EACH ROW
    EXECUTE FUNCTION check_active_wallet_limit();
