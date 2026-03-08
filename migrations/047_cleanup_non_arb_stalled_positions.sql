-- Normalize legacy/non-arb stalled rows after the stale watchdog fix.
-- Recommendation/quant positions should resume normal exit evaluation.
-- Legacy copy-trade rows with zero exposure should be retired from the active set.

UPDATE positions
SET state = 1,
    failure_reason = NULL,
    last_updated = NOW(),
    is_open = TRUE
WHERE state = 7
  AND source = 3;

UPDATE positions
SET state = 5,
    last_updated = NOW(),
    is_open = FALSE
WHERE state = 7
  AND source = 2
  AND COALESCE(quantity, 0) = 0
  AND COALESCE(unrealized_pnl, 0) = 0
  AND realized_pnl IS NULL;
