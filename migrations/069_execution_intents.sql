-- Durable submission boundary: a crash/timeout must never create a second trade.
CREATE TABLE IF NOT EXISTS execution_intents (
    id UUID PRIMARY KEY,
    market_id TEXT NOT NULL,
    exchange_order_id TEXT,
    signed_order JSONB NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('submitting', 'unknown', 'acknowledged', 'rejected', 'reconciled')),
    response JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_execution_intents_unresolved
    ON execution_intents(status) WHERE status IN ('submitting', 'unknown');
