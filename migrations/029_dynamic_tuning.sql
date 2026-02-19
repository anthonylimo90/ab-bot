-- Dynamic tuning configuration and audit history

CREATE TABLE IF NOT EXISTS dynamic_config (
    key VARCHAR(64) PRIMARY KEY,
    current_value DECIMAL(20, 10) NOT NULL,
    default_value DECIMAL(20, 10) NOT NULL,
    min_value DECIMAL(20, 10) NOT NULL,
    max_value DECIMAL(20, 10) NOT NULL,
    max_step_pct DECIMAL(10, 6) NOT NULL DEFAULT 0.12,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    last_good_value DECIMAL(20, 10) NOT NULL,
    pending_eval BOOLEAN NOT NULL DEFAULT FALSE,
    pending_baseline JSONB,
    last_applied_at TIMESTAMPTZ,
    last_reason TEXT,
    updated_by VARCHAR(64) NOT NULL DEFAULT 'bootstrap',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT dynamic_config_min_max CHECK (min_value <= max_value),
    CONSTRAINT dynamic_config_current_bounds CHECK (current_value >= min_value AND current_value <= max_value),
    CONSTRAINT dynamic_config_last_good_bounds CHECK (last_good_value >= min_value AND last_good_value <= max_value)
);

CREATE INDEX IF NOT EXISTS idx_dynamic_config_enabled ON dynamic_config(enabled);

CREATE TABLE IF NOT EXISTS dynamic_config_history (
    id BIGSERIAL PRIMARY KEY,
    config_key VARCHAR(64),
    old_value DECIMAL(20, 10),
    new_value DECIMAL(20, 10),
    action VARCHAR(32) NOT NULL,
    reason TEXT NOT NULL,
    metrics_snapshot JSONB,
    outcome_metrics JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT dynamic_config_history_action_valid CHECK (
        action IN ('observed', 'recommended', 'applied', 'rollback', 'frozen', 'skipped', 'evaluation')
    )
);

CREATE INDEX IF NOT EXISTS idx_dynamic_config_history_key_time
    ON dynamic_config_history(config_key, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_dynamic_config_history_action_time
    ON dynamic_config_history(action, created_at DESC);

DROP TRIGGER IF EXISTS update_dynamic_config_updated_at ON dynamic_config;
CREATE TRIGGER update_dynamic_config_updated_at
    BEFORE UPDATE ON dynamic_config
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();
