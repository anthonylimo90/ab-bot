-- Canonical trade lifecycle events for analytics and realtime trade-flow.

CREATE TABLE IF NOT EXISTS trade_events (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    strategy TEXT NOT NULL,
    execution_mode TEXT NOT NULL,
    source TEXT NOT NULL,
    market_id TEXT NOT NULL,
    position_id UUID,
    signal_id UUID,
    event_type TEXT NOT NULL,
    state_from TEXT,
    state_to TEXT,
    reason TEXT,
    direction TEXT,
    confidence DOUBLE PRECISION,
    expected_edge DECIMAL(20, 10),
    observed_edge DECIMAL(20, 10),
    requested_size_usd DECIMAL(20, 10),
    filled_size_usd DECIMAL(20, 10),
    fill_price DECIMAL(20, 10),
    realized_pnl DECIMAL(20, 10),
    unrealized_pnl DECIMAL(20, 10),
    metadata JSONB NOT NULL DEFAULT '{}'::JSONB
);

CREATE INDEX IF NOT EXISTS idx_trade_events_occurred_at
    ON trade_events (occurred_at DESC);

CREATE INDEX IF NOT EXISTS idx_trade_events_strategy_occurred_at
    ON trade_events (strategy, occurred_at DESC);

CREATE INDEX IF NOT EXISTS idx_trade_events_market_occurred_at
    ON trade_events (market_id, occurred_at DESC);

CREATE INDEX IF NOT EXISTS idx_trade_events_position_occurred_at
    ON trade_events (position_id, occurred_at ASC)
    WHERE position_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_trade_events_signal_occurred_at
    ON trade_events (signal_id, occurred_at ASC)
    WHERE signal_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_trade_events_type_occurred_at
    ON trade_events (event_type, occurred_at DESC);
