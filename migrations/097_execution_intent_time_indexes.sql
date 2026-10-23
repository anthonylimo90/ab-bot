-- The equity feed looks for live orders placed while it valued the account
-- (by creation time), and withdrawals hold back the cash of recently booked
-- buys (reconciled intents by update time): both read only recent rows of a
-- table that grows with every order.
CREATE INDEX IF NOT EXISTS idx_execution_intents_created_at
    ON execution_intents (created_at);
CREATE INDEX IF NOT EXISTS idx_execution_intents_reconciled_updated_at
    ON execution_intents (updated_at)
    WHERE status = 'reconciled';
