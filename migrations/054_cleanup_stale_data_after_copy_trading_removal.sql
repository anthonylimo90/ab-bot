-- Repair cleanup_stale_data() after copy-trading tables were removed in migration 044.
-- The old function still referenced dropped relations, causing periodic hygiene runs
-- to fail noisily in production.

CREATE OR REPLACE FUNCTION cleanup_stale_data() RETURNS void AS $$
BEGIN
    -- Hypertable chunk cleanup (belt-and-suspenders with retention policy)
    PERFORM drop_chunks('orderbook_snapshots', INTERVAL '90 days');
    PERFORM drop_chunks('historical_trades', INTERVAL '90 days');

    -- wallet_trades: keep 30 days
    DELETE FROM wallet_trades WHERE timestamp < NOW() - INTERVAL '30 days';

    -- bot_scores: keep only latest per address
    DELETE FROM bot_scores
    WHERE id NOT IN (
        SELECT DISTINCT ON (address) id
        FROM bot_scores
        ORDER BY address, computed_at DESC
    );

    -- arb_opportunities: keep 7 days
    DELETE FROM arb_opportunities WHERE timestamp < NOW() - INTERVAL '7 days';

    -- auth_challenges: expired challenges
    PERFORM cleanup_expired_auth_challenges();

    -- audit_log: keep 90 days
    DELETE FROM audit_log WHERE created_at < NOW() - INTERVAL '90 days';

    -- data_ingestion_jobs: keep 30 days
    DELETE FROM data_ingestion_jobs WHERE created_at < NOW() - INTERVAL '30 days';
END;
$$ LANGUAGE plpgsql;
