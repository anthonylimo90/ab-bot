-- Operator changes to the circuit breaker (a manual trip, a reset) count up
-- operator_epoch. The trading process saves its state only while the epoch
-- is the one it last saw, so a trip or reset made through another API
-- process is adopted rather than overwritten.
ALTER TABLE circuit_breaker_state
    ADD COLUMN IF NOT EXISTS operator_epoch BIGINT NOT NULL DEFAULT 0;
