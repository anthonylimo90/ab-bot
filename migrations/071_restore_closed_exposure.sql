-- Repair deployments where an earlier cleanup hid positive explicit holdings.
-- Preserve quantities, cost basis, P&L and failure details; resume reconciliation.
UPDATE positions
SET state = 7, -- Stalled, awaiting evidence-based recovery
    is_open = TRUE,
    exit_timestamp = NULL,
    last_updated = NOW(),
    updated_at = NOW()
WHERE state = 4
  AND (held_yes_qty > 0 OR held_no_qty > 0);
