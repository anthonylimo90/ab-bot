-- Persist entry-time fee modeling on positions so P&L stays consistent across restarts.
ALTER TABLE positions
ADD COLUMN IF NOT EXISTS fee_model SMALLINT NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS resolution_payout_per_share DECIMAL(18, 8),
ADD COLUMN IF NOT EXISTS yes_entry_fee_shares DECIMAL(18, 8) NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS no_entry_fee_shares DECIMAL(18, 8) NOT NULL DEFAULT 0;

-- Backfill legacy positions to preserve the previous flat-fee resolution model.
UPDATE positions
SET resolution_payout_per_share = GREATEST(
        0::DECIMAL(18, 8),
        1::DECIMAL(18, 8) - ((yes_entry_price + no_entry_price) * 0.02::DECIMAL(18, 8))
    )
WHERE resolution_payout_per_share IS NULL;

ALTER TABLE positions
ALTER COLUMN resolution_payout_per_share SET NOT NULL;

COMMENT ON COLUMN positions.fee_model IS
    'Position fee model: 0=legacy flat notional fee, 1=share-based entry fee model';
COMMENT ON COLUMN positions.resolution_payout_per_share IS
    'Worst-case resolution payout per share after entry-time fees';
COMMENT ON COLUMN positions.yes_entry_fee_shares IS
    'Buy-side fee charged in YES shares at entry';
COMMENT ON COLUMN positions.no_entry_fee_shares IS
    'Buy-side fee charged in NO shares at entry';
