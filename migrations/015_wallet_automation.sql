-- Fully Automated Wallet Selection System
-- This migration adds support for automated wallet promotion/demotion,
-- pin/ban functionality, probation periods, and confidence-weighted allocation

-- ===================
-- Workspace Wallet Bans Table
-- ===================
-- Wallets that users explicitly don't want auto-promoted

CREATE TABLE IF NOT EXISTS workspace_wallet_bans (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    wallet_address VARCHAR(42) NOT NULL,

    -- Ban details
    reason TEXT,

    -- Audit
    banned_by UUID REFERENCES users(id) ON DELETE SET NULL,
    banned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Optional expiry (NULL = permanent)
    expires_at TIMESTAMPTZ,

    UNIQUE(workspace_id, wallet_address)
);

CREATE INDEX idx_wallet_bans_workspace ON workspace_wallet_bans(workspace_id);
CREATE INDEX idx_wallet_bans_wallet ON workspace_wallet_bans(wallet_address);
CREATE INDEX idx_wallet_bans_expires ON workspace_wallet_bans(expires_at) WHERE expires_at IS NOT NULL;

-- ===================
-- Extend Workspace Wallet Allocations
-- ===================

-- Pin functionality (prevents auto-demotion)
ALTER TABLE workspace_wallet_allocations
ADD COLUMN IF NOT EXISTS pinned BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE workspace_wallet_allocations
ADD COLUMN IF NOT EXISTS pinned_at TIMESTAMPTZ;

ALTER TABLE workspace_wallet_allocations
ADD COLUMN IF NOT EXISTS pinned_by UUID REFERENCES users(id) ON DELETE SET NULL;

-- Probation tracking (new wallets start at 50% allocation for 7 days)
ALTER TABLE workspace_wallet_allocations
ADD COLUMN IF NOT EXISTS probation_until TIMESTAMPTZ;

ALTER TABLE workspace_wallet_allocations
ADD COLUMN IF NOT EXISTS probation_allocation_pct DECIMAL(5, 2) NOT NULL DEFAULT 50.0;

-- Loss tracking for immediate demotion triggers
ALTER TABLE workspace_wallet_allocations
ADD COLUMN IF NOT EXISTS consecutive_losses INTEGER NOT NULL DEFAULT 0;

ALTER TABLE workspace_wallet_allocations
ADD COLUMN IF NOT EXISTS last_loss_at TIMESTAMPTZ;

-- Confidence score from AdvancedPredictor (for allocation weighting)
ALTER TABLE workspace_wallet_allocations
ADD COLUMN IF NOT EXISTS confidence_score DECIMAL(5, 4);

ALTER TABLE workspace_wallet_allocations
ADD COLUMN IF NOT EXISTS confidence_updated_at TIMESTAMPTZ;

-- Demotion grace period tracking
ALTER TABLE workspace_wallet_allocations
ADD COLUMN IF NOT EXISTS grace_period_started_at TIMESTAMPTZ;

ALTER TABLE workspace_wallet_allocations
ADD COLUMN IF NOT EXISTS grace_period_reason TEXT;

-- ===================
-- Extend Workspaces Table
-- ===================

-- Auto-selection (fills empty Active slots with best candidates)
-- Default TRUE for hands-off experience
ALTER TABLE workspaces
ADD COLUMN IF NOT EXISTS auto_select_enabled BOOLEAN NOT NULL DEFAULT TRUE;

-- Auto-demotion (removes underperformers)
-- Default TRUE for hands-off experience
ALTER TABLE workspaces
ADD COLUMN IF NOT EXISTS auto_demote_enabled BOOLEAN NOT NULL DEFAULT TRUE;

-- Probation configuration
ALTER TABLE workspaces
ADD COLUMN IF NOT EXISTS probation_days INTEGER NOT NULL DEFAULT 7;

-- Maximum pinned wallets (ensure automation always has slots to optimize)
ALTER TABLE workspaces
ADD COLUMN IF NOT EXISTS max_pinned_wallets INTEGER NOT NULL DEFAULT 3;

-- Allocation strategy: 'equal', 'confidence_weighted', 'performance'
ALTER TABLE workspaces
ADD COLUMN IF NOT EXISTS allocation_strategy VARCHAR(20) NOT NULL DEFAULT 'confidence_weighted';

-- Max drawdown threshold for demotion
ALTER TABLE workspaces
ADD COLUMN IF NOT EXISTS max_drawdown_pct DECIMAL(5, 2) NOT NULL DEFAULT 30.0;

-- Inactivity threshold (days without trades)
ALTER TABLE workspaces
ADD COLUMN IF NOT EXISTS inactivity_days INTEGER NOT NULL DEFAULT 14;

-- Constraints
ALTER TABLE workspaces
ADD CONSTRAINT valid_allocation_strategy
CHECK (allocation_strategy IN ('equal', 'confidence_weighted', 'performance'));

ALTER TABLE workspaces
ADD CONSTRAINT valid_probation_days
CHECK (probation_days BETWEEN 1 AND 30);

ALTER TABLE workspaces
ADD CONSTRAINT valid_max_pinned
CHECK (max_pinned_wallets BETWEEN 0 AND 5);

-- ===================
-- Extend Auto Rotation History
-- ===================

-- Add more action types for the automation system
ALTER TABLE auto_rotation_history
DROP CONSTRAINT IF EXISTS valid_action;

ALTER TABLE auto_rotation_history
ADD CONSTRAINT valid_action CHECK (
    action IN (
        'promote', 'demote', 'replace', 'add', 'remove',
        'pin', 'unpin', 'ban', 'unban',
        'probation_start', 'probation_graduate', 'probation_fail',
        'emergency_demote', 'grace_period_start', 'grace_period_demote',
        'auto_swap', 'undo'
    )
);

-- Add undo tracking
ALTER TABLE auto_rotation_history
ADD COLUMN IF NOT EXISTS undone BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE auto_rotation_history
ADD COLUMN IF NOT EXISTS undone_at TIMESTAMPTZ;

ALTER TABLE auto_rotation_history
ADD COLUMN IF NOT EXISTS undone_by UUID REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE auto_rotation_history
ADD COLUMN IF NOT EXISTS undo_expires_at TIMESTAMPTZ;

-- Metrics snapshot at time of action
ALTER TABLE auto_rotation_history
ADD COLUMN IF NOT EXISTS metrics_snapshot JSONB;

-- ===================
-- Helper Functions
-- ===================

-- Function to count pinned wallets in a workspace
CREATE OR REPLACE FUNCTION count_pinned_wallets(ws_id UUID)
RETURNS INTEGER AS $$
    SELECT COUNT(*)::INTEGER
    FROM workspace_wallet_allocations
    WHERE workspace_id = ws_id AND pinned = TRUE;
$$ LANGUAGE SQL STABLE;

-- Function to validate max pinned wallets before pinning
CREATE OR REPLACE FUNCTION check_pinned_wallet_limit()
RETURNS TRIGGER AS $$
DECLARE
    max_pins INTEGER;
BEGIN
    IF NEW.pinned = TRUE AND (OLD.pinned IS NULL OR OLD.pinned = FALSE) THEN
        SELECT max_pinned_wallets INTO max_pins
        FROM workspaces
        WHERE id = NEW.workspace_id;

        IF count_pinned_wallets(NEW.workspace_id) >= max_pins THEN
            RAISE EXCEPTION 'Cannot have more than % pinned wallets per workspace', max_pins;
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER enforce_pinned_wallet_limit
    BEFORE INSERT OR UPDATE ON workspace_wallet_allocations
    FOR EACH ROW
    EXECUTE FUNCTION check_pinned_wallet_limit();

-- Function to check if a wallet is banned
CREATE OR REPLACE FUNCTION is_wallet_banned(ws_id UUID, wallet VARCHAR(42))
RETURNS BOOLEAN AS $$
    SELECT EXISTS (
        SELECT 1
        FROM workspace_wallet_bans
        WHERE workspace_id = ws_id
        AND wallet_address = wallet
        AND (expires_at IS NULL OR expires_at > NOW())
    );
$$ LANGUAGE SQL STABLE;

-- Function to calculate effective allocation considering probation
CREATE OR REPLACE FUNCTION effective_allocation_pct(
    base_allocation DECIMAL(5, 2),
    probation_until TIMESTAMPTZ,
    probation_pct DECIMAL(5, 2)
)
RETURNS DECIMAL(5, 2) AS $$
BEGIN
    IF probation_until IS NOT NULL AND probation_until > NOW() THEN
        RETURN base_allocation * (probation_pct / 100.0);
    END IF;
    RETURN base_allocation;
END;
$$ LANGUAGE plpgsql STABLE;

-- ===================
-- Indexes for Performance
-- ===================

CREATE INDEX IF NOT EXISTS idx_allocations_pinned
ON workspace_wallet_allocations(workspace_id)
WHERE pinned = TRUE;

CREATE INDEX IF NOT EXISTS idx_allocations_probation
ON workspace_wallet_allocations(probation_until)
WHERE probation_until IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_allocations_grace_period
ON workspace_wallet_allocations(grace_period_started_at)
WHERE grace_period_started_at IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_rotation_history_undo
ON auto_rotation_history(undo_expires_at)
WHERE NOT undone AND undo_expires_at IS NOT NULL;

-- ===================
-- Update existing workspaces to have automation enabled by default
-- but only for new workspaces going forward (existing ones keep their settings)
-- ===================
-- Note: The DEFAULT TRUE on columns will only apply to new rows,
-- existing workspaces will get NULL/FALSE which preserves their manual behavior
