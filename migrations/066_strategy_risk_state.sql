-- Per-strategy risk state persistence.
-- Tracks daily P&L and consecutive losses per strategy (flow, mean_reversion, etc.)
-- so the quant executor can maintain independent circuit breakers that survive restarts.

CREATE TABLE IF NOT EXISTS strategy_risk_state (
    strategy TEXT PRIMARY KEY,
    daily_pnl DECIMAL(20,10) NOT NULL DEFAULT 0,
    daily_pnl_date DATE NOT NULL DEFAULT CURRENT_DATE,
    consecutive_losses INTEGER NOT NULL DEFAULT 0,
    halted BOOLEAN NOT NULL DEFAULT false,
    halt_reason TEXT,
    halted_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
