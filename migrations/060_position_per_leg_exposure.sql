-- Per-leg exposure tracking: make position exposure explicit instead of inferring from zeroed prices.

ALTER TABLE positions
    ADD COLUMN IF NOT EXISTS held_yes_qty     NUMERIC(24, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS held_no_qty      NUMERIC(24, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS exited_yes_qty   NUMERIC(24, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS exited_no_qty    NUMERIC(24, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS resolution_winner VARCHAR(4)
        CHECK (resolution_winner IS NULL OR resolution_winner IN ('yes', 'no'));

ALTER TABLE positions
    ADD CONSTRAINT positions_held_yes_qty_nonneg    CHECK (held_yes_qty    >= 0),
    ADD CONSTRAINT positions_held_no_qty_nonneg     CHECK (held_no_qty     >= 0),
    ADD CONSTRAINT positions_exited_yes_qty_nonneg  CHECK (exited_yes_qty  >= 0),
    ADD CONSTRAINT positions_exited_no_qty_nonneg   CHECK (exited_no_qty   >= 0);

-- Backfill from existing price / state data (same inference the reconciler uses today).
-- state 4 = Closed, state 5 = EntryFailed (terminal — nothing held).
UPDATE positions SET
    held_yes_qty = CASE
        WHEN state IN (4, 5) THEN 0
        WHEN yes_entry_price > 0 AND yes_exit_price IS NULL THEN quantity
        ELSE 0
    END,
    held_no_qty = CASE
        WHEN state IN (4, 5) THEN 0
        WHEN no_entry_price > 0 AND no_exit_price IS NULL THEN quantity
        ELSE 0
    END,
    exited_yes_qty = CASE
        WHEN yes_entry_price > 0 AND (yes_exit_price IS NOT NULL OR state = 4) THEN quantity
        ELSE 0
    END,
    exited_no_qty = CASE
        WHEN no_entry_price > 0 AND (no_exit_price IS NOT NULL OR state = 4) THEN quantity
        ELSE 0
    END;

-- Partial index for fast reconciler / exposure queries on active positions.
CREATE INDEX IF NOT EXISTS idx_positions_exposure
    ON positions (state, held_yes_qty, held_no_qty)
    WHERE state NOT IN (4, 5);
