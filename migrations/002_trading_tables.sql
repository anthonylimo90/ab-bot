-- Phase 1: Trading & Risk Management Tables

-- ===================
-- Tracked Wallets (Copy Trading)
-- ===================

CREATE TABLE IF NOT EXISTS tracked_wallets (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    address VARCHAR(42) NOT NULL UNIQUE,
    alias VARCHAR(100),
    allocation_pct DECIMAL(5, 2) NOT NULL DEFAULT 20.00,
    copy_delay_ms INTEGER NOT NULL DEFAULT 0,
    max_position_size DECIMAL(18, 8),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    added_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_copied_trade TIMESTAMPTZ,
    total_copied_value DECIMAL(18, 8) NOT NULL DEFAULT 0,
    total_pnl DECIMAL(18, 8) NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_tracked_wallets_address ON tracked_wallets(address);
CREATE INDEX IF NOT EXISTS idx_tracked_wallets_enabled ON tracked_wallets(enabled);
CREATE INDEX IF NOT EXISTS idx_tracked_wallets_pnl ON tracked_wallets(total_pnl DESC);

-- ===================
-- Stop-Loss Rules
-- ===================

CREATE TABLE IF NOT EXISTS stop_loss_rules (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    position_id UUID NOT NULL REFERENCES positions(id) ON DELETE CASCADE,
    market_id VARCHAR(255) NOT NULL,
    outcome_id VARCHAR(255) NOT NULL,
    entry_price DECIMAL(18, 8) NOT NULL,
    quantity DECIMAL(18, 8) NOT NULL,
    stop_type SMALLINT NOT NULL,  -- 0=fixed, 1=percentage, 2=trailing, 3=time_based
    trigger_price DECIMAL(18, 8),
    loss_percentage DECIMAL(5, 4),
    trailing_offset_pct DECIMAL(5, 4),
    peak_price DECIMAL(18, 8),
    deadline TIMESTAMPTZ,
    activated BOOLEAN NOT NULL DEFAULT FALSE,
    activated_at TIMESTAMPTZ,
    executed BOOLEAN NOT NULL DEFAULT FALSE,
    executed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_stop_loss_position ON stop_loss_rules(position_id);
CREATE INDEX IF NOT EXISTS idx_stop_loss_market ON stop_loss_rules(market_id);
CREATE INDEX IF NOT EXISTS idx_stop_loss_activated ON stop_loss_rules(activated) WHERE activated = TRUE AND executed = FALSE;

-- ===================
-- Execution Reports
-- ===================

CREATE TABLE IF NOT EXISTS execution_reports (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    order_id UUID NOT NULL,
    exchange_order_id VARCHAR(255),
    market_id VARCHAR(255) NOT NULL,
    outcome_id VARCHAR(255) NOT NULL,
    side SMALLINT NOT NULL,  -- 0=buy, 1=sell
    status SMALLINT NOT NULL,  -- 0=created, 1=pending, 2=partial, 3=filled, 4=cancelled, 5=rejected, 6=expired
    requested_quantity DECIMAL(18, 8) NOT NULL,
    filled_quantity DECIMAL(18, 8) NOT NULL,
    average_price DECIMAL(18, 8) NOT NULL,
    fees_paid DECIMAL(18, 8) NOT NULL,
    executed_at TIMESTAMPTZ NOT NULL,
    transaction_hash VARCHAR(66),
    error_message TEXT,
    source SMALLINT NOT NULL DEFAULT 0,  -- 0=manual, 1=arbitrage, 2=copy_trade, 3=stop_loss
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_exec_reports_order ON execution_reports(order_id);
CREATE INDEX IF NOT EXISTS idx_exec_reports_market ON execution_reports(market_id);
CREATE INDEX IF NOT EXISTS idx_exec_reports_executed ON execution_reports(executed_at);
CREATE INDEX IF NOT EXISTS idx_exec_reports_source ON execution_reports(source);

-- ===================
-- Users (Auth)
-- ===================

CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    email VARCHAR(255) NOT NULL UNIQUE,
    password_hash VARCHAR(255) NOT NULL,
    role SMALLINT NOT NULL DEFAULT 0,  -- 0=viewer, 1=trader, 2=admin
    api_key_hash VARCHAR(255),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_login TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);
CREATE INDEX IF NOT EXISTS idx_users_role ON users(role);

-- ===================
-- API Keys
-- ===================

CREATE TABLE IF NOT EXISTS api_keys (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    key_hash VARCHAR(255) NOT NULL UNIQUE,
    key_prefix VARCHAR(8) NOT NULL,
    role SMALLINT NOT NULL DEFAULT 0,
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_api_keys_user ON api_keys(user_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_api_keys_active ON api_keys(active) WHERE active = TRUE;

-- ===================
-- Audit Log
-- ===================

CREATE TABLE IF NOT EXISTS audit_log (
    id BIGSERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    user_id VARCHAR(100),
    action VARCHAR(50) NOT NULL,
    resource VARCHAR(255) NOT NULL,
    details JSONB,
    ip_address INET,
    user_agent TEXT,
    success BOOLEAN NOT NULL DEFAULT TRUE,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_user ON audit_log(user_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_action ON audit_log(action);
CREATE INDEX IF NOT EXISTS idx_audit_resource ON audit_log(resource);

-- ===================
-- Circuit Breaker State (persistent)
-- ===================

CREATE TABLE IF NOT EXISTS circuit_breaker_state (
    id INTEGER PRIMARY KEY DEFAULT 1,
    tripped BOOLEAN NOT NULL DEFAULT FALSE,
    trip_reason VARCHAR(50),
    tripped_at TIMESTAMPTZ,
    resume_at TIMESTAMPTZ,
    daily_pnl DECIMAL(18, 8) NOT NULL DEFAULT 0,
    peak_value DECIMAL(18, 8) NOT NULL DEFAULT 0,
    current_value DECIMAL(18, 8) NOT NULL DEFAULT 0,
    consecutive_losses INTEGER NOT NULL DEFAULT 0,
    trips_today INTEGER NOT NULL DEFAULT 0,
    last_reset_date DATE NOT NULL DEFAULT CURRENT_DATE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT single_row CHECK (id = 1)
);

-- Insert initial row
INSERT INTO circuit_breaker_state (id) VALUES (1) ON CONFLICT (id) DO NOTHING;

-- ===================
-- Position Source Tracking (extend positions)
-- ===================

ALTER TABLE positions
ADD COLUMN IF NOT EXISTS source SMALLINT NOT NULL DEFAULT 0,  -- 0=manual, 1=arbitrage, 2=copy_trade, 3=recommendation
ADD COLUMN IF NOT EXISTS source_wallet VARCHAR(42),
ADD COLUMN IF NOT EXISTS source_signal_id UUID,
ADD COLUMN IF NOT EXISTS stop_loss_id UUID,
ADD COLUMN IF NOT EXISTS tags TEXT[],
ADD COLUMN IF NOT EXISTS notes TEXT;

CREATE INDEX IF NOT EXISTS idx_positions_source ON positions(source);
CREATE INDEX IF NOT EXISTS idx_positions_source_wallet ON positions(source_wallet) WHERE source_wallet IS NOT NULL;

-- ===================
-- Update Triggers
-- ===================

CREATE TRIGGER update_tracked_wallets_updated_at
    BEFORE UPDATE ON tracked_wallets
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_stop_loss_rules_updated_at
    BEFORE UPDATE ON stop_loss_rules
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_users_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_circuit_breaker_updated_at
    BEFORE UPDATE ON circuit_breaker_state
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();
