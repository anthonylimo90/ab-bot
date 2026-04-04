-- Cleanup stale positions that are stuck in terminal failure states.
--
-- Position states: Pending=0, Open=1, ExitReady=2, Closing=3, Closed=4,
--                  EntryFailed=5, ExitFailed=6, Stalled=7
--
-- This migration closes out positions in EntryFailed(5), ExitFailed(6),
-- and Stalled(7) states that are older than 24 hours, setting them to
-- Closed(4) so they no longer appear in startup reconciliation.

UPDATE positions
SET state = 4,  -- Closed
    exit_timestamp = COALESCE(exit_timestamp, NOW()),
    failure_reason = CASE
        WHEN state = 5 THEN COALESCE(failure_reason, 8)  -- EntryFailed -> mark as cleaned
        WHEN state = 6 THEN COALESCE(failure_reason, 8)  -- ExitFailed -> mark as cleaned
        WHEN state = 7 THEN COALESCE(failure_reason, 8)  -- Stalled -> mark as cleaned
        ELSE failure_reason
    END,
    updated_at = NOW()
WHERE state IN (5, 6, 7)  -- EntryFailed, ExitFailed, Stalled
  AND entry_timestamp < NOW() - INTERVAL '24 hours';
