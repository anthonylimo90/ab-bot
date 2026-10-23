-- Which closes the circuit breaker has taken in. A process that stops between
-- booking a close and recording it with the breaker leaves the close unmarked,
-- for the next trading leader to record.
ALTER TABLE positions ADD COLUMN IF NOT EXISTS breaker_recorded_at TIMESTAMPTZ;

-- Closes booked before this column existed were recorded at the time (or
-- missed for good): none is fed to the breaker again.
UPDATE positions
SET breaker_recorded_at = COALESCE(exit_timestamp, NOW())
WHERE state = 4 AND breaker_recorded_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_positions_breaker_unrecorded
    ON positions (exit_timestamp)
    WHERE state = 4 AND breaker_recorded_at IS NULL;
