-- Automated backtest scheduling and strategy health snapshots.

ALTER TABLE backtest_results
ADD COLUMN IF NOT EXISTS schedule_id UUID,
ADD COLUMN IF NOT EXISTS trigger_mode VARCHAR(20) NOT NULL DEFAULT 'manual',
ADD COLUMN IF NOT EXISTS trigger_label TEXT,
ADD COLUMN IF NOT EXISTS markets TEXT[];

CREATE INDEX IF NOT EXISTS idx_backtest_schedule_created
    ON backtest_results (schedule_id, created_at DESC)
    WHERE schedule_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_backtest_trigger_mode_created
    ON backtest_results (trigger_mode, created_at DESC);

CREATE TABLE IF NOT EXISTS backtest_schedules (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name TEXT NOT NULL UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    strategy JSONB NOT NULL,
    lookback_days INTEGER NOT NULL,
    initial_capital DECIMAL(18, 8) NOT NULL,
    markets TEXT[],
    slippage_model JSONB NOT NULL DEFAULT '{"type":"none"}'::jsonb,
    fee_pct DECIMAL(8, 6) NOT NULL DEFAULT 0.020000,
    interval_hours INTEGER NOT NULL,
    next_run_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_run_at TIMESTAMPTZ,
    last_result_id UUID REFERENCES backtest_results(id) ON DELETE SET NULL,
    last_status VARCHAR(20),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (lookback_days > 0),
    CHECK (interval_hours > 0)
);

CREATE INDEX IF NOT EXISTS idx_backtest_schedules_due
    ON backtest_schedules (enabled, next_run_at ASC)
    WHERE enabled = TRUE;

CREATE TRIGGER update_backtest_schedules_updated_at
    BEFORE UPDATE ON backtest_schedules
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TABLE IF NOT EXISTS strategy_health_snapshots (
    strategy TEXT NOT NULL,
    period_end TIMESTAMPTZ NOT NULL,
    period_days INT NOT NULL,
    generated_signals INT DEFAULT 0,
    executed_signals INT DEFAULT 0,
    skipped_signals INT DEFAULT 0,
    expired_signals INT DEFAULT 0,
    open_positions INT DEFAULT 0,
    exit_ready_positions INT DEFAULT 0,
    closed_positions INT DEFAULT 0,
    entry_failed_positions INT DEFAULT 0,
    exit_failed_positions INT DEFAULT 0,
    total_expected_edge DECIMAL(20, 10) DEFAULT 0,
    total_observed_edge DECIMAL(20, 10) DEFAULT 0,
    total_realized_pnl DECIMAL(20, 10) DEFAULT 0,
    avg_hold_hours DOUBLE PRECISION,
    skip_rate DOUBLE PRECISION,
    failure_rate DOUBLE PRECISION,
    edge_capture_ratio DECIMAL(20, 10),
    recommendation TEXT NOT NULL,
    rationale TEXT NOT NULL,
    latest_backtest_id UUID,
    latest_backtest_return_pct DECIMAL(20, 10),
    latest_backtest_created_at TIMESTAMPTZ,
    PRIMARY KEY (strategy, period_end, period_days)
);

CREATE INDEX IF NOT EXISTS idx_strategy_health_latest
    ON strategy_health_snapshots (strategy, period_days, period_end DESC);
