-- Durable journal for on-chain withdrawals.
--
-- A transfer is signed before it is sent, so its hash and nonce are recorded
-- first (status 'submitting'); the broadcast can then fail or time out without
-- losing track of a transfer that may still be mined. A reconciler resolves
-- every 'submitting'/'submitted' row from the chain: confirmed, reverted, or
-- never mined because its nonce was used by another transaction.
--
-- raw_tx is the signed transaction (public data once broadcast) so an
-- unresolved transfer can be re-sent instead of guessed about.
-- journaled marks rows written by this flow: a journaled 'pending' row without
-- a hash was never signed, so it can be failed safely. Older rows may have been
-- sent without a recorded hash and are left for an operator.

ALTER TABLE wallet_withdrawals
    ADD COLUMN IF NOT EXISTS nonce BIGINT,
    ADD COLUMN IF NOT EXISTS raw_tx TEXT,
    ADD COLUMN IF NOT EXISTS journaled BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX IF NOT EXISTS idx_wallet_withdrawals_unresolved
    ON wallet_withdrawals (wallet_address, updated_at)
    WHERE status IN ('pending', 'submitting', 'submitted');
