-- The browser-wallet order endpoints (/api/v1/orders/prepare and /submit)
-- are removed: nothing prepares or submits orders signed by a user's own
-- wallet any more. Orders prepared and never submitted are dropped with the
-- table. Orders already submitted were journaled as execution intents, which
-- the intent reconciler still resolves (or releases once expired).
DROP TABLE IF EXISTS pending_wallet_orders;
