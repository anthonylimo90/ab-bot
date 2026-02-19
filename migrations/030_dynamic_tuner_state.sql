-- Runtime heartbeat/state for dynamic tuner observability

CREATE TABLE IF NOT EXISTS dynamic_tuner_state (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE,
    last_run_at TIMESTAMPTZ,
    last_run_status VARCHAR(32),
    last_run_reason TEXT,
    last_metrics JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT dynamic_tuner_state_singleton CHECK (singleton = TRUE)
);

INSERT INTO dynamic_tuner_state (singleton, last_run_status, last_run_reason)
VALUES (TRUE, 'bootstrap', 'dynamic tuner state initialized')
ON CONFLICT (singleton) DO NOTHING;

DROP TRIGGER IF EXISTS update_dynamic_tuner_state_updated_at ON dynamic_tuner_state;
CREATE TRIGGER update_dynamic_tuner_state_updated_at
    BEFORE UPDATE ON dynamic_tuner_state
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();
