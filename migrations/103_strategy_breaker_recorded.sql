-- Each closed quant position is counted by its strategy's breaker once,
-- marked here in the same transaction that saves the breaker's state. The
-- executor used to replay the day's closes on every start, over the saved
-- state (re-halting a strategy whose streak had already reset, restarting a
-- halt's cooldown).
ALTER TABLE positions ADD COLUMN IF NOT EXISTS strategy_breaker_recorded_at TIMESTAMPTZ;

-- Closes before this migration are already in the saved breaker state.
UPDATE positions
SET strategy_breaker_recorded_at = COALESCE(exit_timestamp, NOW())
WHERE source = 3 AND state = 4 AND strategy_breaker_recorded_at IS NULL;
