-- Taker fees charged to a position's fills. An arbitrage position records the
-- fee terms its market was priced with (a fill of C shares at price p costs
-- C × taker_fee_rate × (p(1 − p))^taker_fee_exponent USDC, charged on top of
-- the order's amount) and accumulates the fees of its fills in fees_paid,
-- which its P&L deducts. Positions without terms (quant signals, which keep
-- their flat fee model, and arbitrage opened before this migration) are
-- unchanged.

ALTER TABLE positions
    ADD COLUMN IF NOT EXISTS taker_fee_rate NUMERIC,
    ADD COLUMN IF NOT EXISTS taker_fee_exponent NUMERIC,
    ADD COLUMN IF NOT EXISTS fees_paid NUMERIC NOT NULL DEFAULT 0;
