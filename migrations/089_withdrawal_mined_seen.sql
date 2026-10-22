-- When the withdrawal reconciler first saw a receipt for a transfer whose
-- block was not yet final. The wallet balance already lacks the amount then,
-- so the circuit breaker waits for the transfer's cash flow instead of
-- reading the drop as a trading loss, however long the transfer took to mine.

ALTER TABLE wallet_withdrawals
    ADD COLUMN IF NOT EXISTS mined_seen_at TIMESTAMPTZ;
