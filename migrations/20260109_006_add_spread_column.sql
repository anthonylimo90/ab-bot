-- Phase 5: Add spread column to arb_opportunities

ALTER TABLE arb_opportunities
ADD COLUMN IF NOT EXISTS spread DECIMAL(10, 8);

-- Compute spread from existing data
UPDATE arb_opportunities
SET spread = (yes_ask + no_ask - 1)
WHERE spread IS NULL;
