-- Persist the complete circuit-breaker state so restarts and day rollovers
-- never clear an active trip, a hard-kill latch or gradual recovery.
--
-- drawdown_peak: reference equity for the soft max-drawdown trip; it is
--   rebased when trading resumes after a drawdown cooldown, while peak_value
--   remains the high-water mark for the hard kill switch.
-- portfolio_value_seeded: whether peak/current values come from real equity
--   snapshots. Existing rows start unseeded, so the first equity snapshot
--   replaces any peak accumulated from realized P&L deltas.
-- cash_flows_applied_through: cursor over cash_flow_events already applied to
--   the drawdown references (withdrawals are not trading losses).
ALTER TABLE circuit_breaker_state
    ADD COLUMN IF NOT EXISTS drawdown_peak DECIMAL(18, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS portfolio_value_seeded BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS recovery_state JSONB,
    ADD COLUMN IF NOT EXISTS cash_flows_applied_through TIMESTAMPTZ;
