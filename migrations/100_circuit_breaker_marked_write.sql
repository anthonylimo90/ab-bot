-- Tell a takeover whether the stored loss counters were last written by a
-- version that marks the closes it counts (positions.breaker_recorded_at).
--
-- marked_write_at: set to the write's time (equal to updated_at) by every
--   write of this version that leaves the stored counters holding exactly
--   the marked closes: the leader's full save, and the claim of the lead
--   that marks what an earlier version counted. Writes that leave the
--   counters alone (a trip stored on its own, an operator change) keep it
--   equal or apart as they find it. A write by an earlier version (which
--   counts closes without marking them) moves updated_at alone, so the next
--   claim finds the two apart and marks the closes left unmarked instead of
--   counting them again: after the first upgrade, and after a rollback and
--   a return to this version. NULL until this version first writes.
ALTER TABLE circuit_breaker_state
    ADD COLUMN IF NOT EXISTS marked_write_at TIMESTAMPTZ;
