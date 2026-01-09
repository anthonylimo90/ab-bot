-- Phase 5: Add spread column to arb_opportunities (if table exists)
-- This migration runs after 001_initial_schema.sql creates arb_opportunities

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'arb_opportunities') THEN
        ALTER TABLE arb_opportunities
        ADD COLUMN IF NOT EXISTS spread DECIMAL(10, 8);

        -- Compute spread from existing data
        UPDATE arb_opportunities
        SET spread = (yes_ask + no_ask - 1)
        WHERE spread IS NULL;
    END IF;
END $$;
