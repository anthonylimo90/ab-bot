-- Migration 034: Data hygiene — purge stale data and add scheduled cleanup functions.
--
-- Problem: wallet_trades, bot_scores, copy_trade_history, wallet_trade_signals,
-- and arb_opportunities grow unboundedly. No retention policies exist.
-- The system accumulates months of data that slows queries and wastes storage.

-- ===================
-- 1. Purge stale wallet_trades (keep last 30 days)
-- ===================
DELETE FROM wallet_trades WHERE timestamp < NOW() - INTERVAL '30 days';

-- ===================
-- 2. Purge stale bot_scores (keep only latest per wallet)
-- ===================
DELETE FROM bot_scores
WHERE id NOT IN (
    SELECT DISTINCT ON (address) id
    FROM bot_scores
    ORDER BY address, computed_at DESC
);

-- ===================
-- 3. Purge old copy_trade_history (keep last 30 days)
-- ===================
DELETE FROM copy_trade_history WHERE created_at < NOW() - INTERVAL '30 days';

-- ===================
-- 4. Purge old processed signals (keep last 7 days)
-- ===================
DELETE FROM wallet_trade_signals
WHERE processed = TRUE AND created_at < NOW() - INTERVAL '7 days';

-- ===================
-- 5. Purge old arb_opportunities (keep last 7 days)
-- ===================
DELETE FROM arb_opportunities WHERE timestamp < NOW() - INTERVAL '7 days';

-- ===================
-- 6. Purge old auto_rotation_history (keep last 30 days)
-- ===================
DELETE FROM auto_rotation_history WHERE created_at < NOW() - INTERVAL '30 days';

-- ===================
-- 7. Create reusable cleanup function for periodic invocation
-- ===================
CREATE OR REPLACE FUNCTION cleanup_stale_data() RETURNS void AS $$
BEGIN
    -- wallet_trades: keep 30 days
    DELETE FROM wallet_trades WHERE timestamp < NOW() - INTERVAL '30 days';

    -- bot_scores: keep only latest per address
    DELETE FROM bot_scores
    WHERE id NOT IN (
        SELECT DISTINCT ON (address) id
        FROM bot_scores
        ORDER BY address, computed_at DESC
    );

    -- copy_trade_history: keep 30 days
    DELETE FROM copy_trade_history WHERE created_at < NOW() - INTERVAL '30 days';

    -- wallet_trade_signals: keep unprocessed + last 7 days of processed
    DELETE FROM wallet_trade_signals
    WHERE processed = TRUE AND created_at < NOW() - INTERVAL '7 days';

    -- arb_opportunities: keep 7 days
    DELETE FROM arb_opportunities WHERE timestamp < NOW() - INTERVAL '7 days';

    -- auto_rotation_history: keep 30 days
    DELETE FROM auto_rotation_history WHERE created_at < NOW() - INTERVAL '30 days';

    -- auth_challenges: expired challenges
    PERFORM cleanup_expired_auth_challenges();

    -- audit_log: keep 90 days
    DELETE FROM audit_log WHERE created_at < NOW() - INTERVAL '90 days';
END;
$$ LANGUAGE plpgsql;

-- ===================
-- 8. VACUUM ANALYZE the cleaned tables to reclaim space
-- ===================
-- Note: VACUUM cannot run inside a transaction block in SQLx migrations.
-- The application should run VACUUM ANALYZE periodically or rely on autovacuum.

-- ===================
-- 9. Add index to speed up cleanup queries
-- ===================
CREATE INDEX IF NOT EXISTS idx_wallet_trades_timestamp ON wallet_trades(timestamp);
CREATE INDEX IF NOT EXISTS idx_copy_trade_history_created_at ON copy_trade_history(created_at);
CREATE INDEX IF NOT EXISTS idx_auto_rotation_history_created_at ON auto_rotation_history(created_at);
CREATE INDEX IF NOT EXISTS idx_audit_log_created_at ON audit_log(created_at);
