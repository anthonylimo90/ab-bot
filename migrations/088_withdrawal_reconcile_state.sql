-- Withdrawal reconciler state.
--
-- trading_wallet records, when a withdrawal is sent, whether its source was
-- the live trading wallet, so its cash flow is recorded (or not) by what was
-- true then rather than by the workspace selection when it settles. NULL on
-- older rows, which fall back to the current selection.
--
-- superseded_seen_at is when the reconciler first found the transfer's nonce
-- used by another final transaction while the transfer itself was unknown. A
-- transfer is failed only when a second look minutes later finds the same,
-- so one lagging RPC backend cannot fail a transfer that was mined.

ALTER TABLE wallet_withdrawals
    ADD COLUMN IF NOT EXISTS trading_wallet BOOLEAN,
    ADD COLUMN IF NOT EXISTS superseded_seen_at TIMESTAMPTZ;
