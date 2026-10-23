-- Fence the trading leader's saves of the circuit breaker, and share its
-- holds on new entries.
--
-- leader_epoch: counted up by each process that takes over trading. A save
--   carries the epoch its process claimed and is refused once another
--   process has claimed a newer one, so a save stalled across a change of
--   leader cannot overwrite the new leader's counters. 0 until the first
--   claim (the version before this one never claims).
-- entry_holds: why the leader holds new entries (settings it cannot read,
--   closes not yet counted), so every API process refuses entries and a
--   reset while they stand.
ALTER TABLE circuit_breaker_state
    ADD COLUMN IF NOT EXISTS leader_epoch BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS entry_holds JSONB NOT NULL DEFAULT '[]'::jsonb;
