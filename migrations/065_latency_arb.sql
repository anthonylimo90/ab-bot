-- CEX latency arbitrage signal tracking table.
-- Records every signal evaluated (executed or skipped) for paper-mode
-- validation and performance analysis.

CREATE TABLE IF NOT EXISTS latency_arb_signals (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    cex_symbol TEXT NOT NULL,
    direction TEXT NOT NULL,
    magnitude_pct DOUBLE PRECISION NOT NULL,
    condition_id TEXT NOT NULL,
    polymarket_side TEXT NOT NULL,
    yes_price_at_signal DECIMAL(10,6),
    kelly_size_usd DECIMAL(20,10),
    executed BOOLEAN NOT NULL DEFAULT false,
    skip_reason TEXT,
    signal_age_ms INTEGER,
    generated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_latency_arb_signals_generated_at
    ON latency_arb_signals (generated_at DESC);

CREATE INDEX IF NOT EXISTS idx_latency_arb_signals_condition
    ON latency_arb_signals (condition_id, generated_at DESC);
