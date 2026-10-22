-- The login Grafana's PostgreSQL datasource uses: read-only, and only on the
-- trading and analytics tables an operator charts. Users, credentials,
-- sessions, signed orders and signed transactions stay out of reach.
-- Run by the monitoring profile's grafana-db-reader service on every start
-- (idempotent), as the database owner, with psql variable `pw` set.
SELECT 'CREATE ROLE grafana_reader LOGIN'
WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'grafana_reader')\gexec

ALTER ROLE grafana_reader LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE PASSWORD :'pw';
ALTER ROLE grafana_reader SET default_transaction_read_only = on;

DO $$
DECLARE
    t text;
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'account_snapshots', 'arb_opportunities', 'backtest_results',
        'bot_scores', 'cash_flow_events', 'circuit_breaker_state',
        'discovered_wallets', 'dynamic_config', 'dynamic_config_history',
        'dynamic_tuner_state', 'execution_reports', 'latency_arb_signals',
        'manual_inventory', 'market_flow_features', 'market_metadata',
        'markets', 'orders', 'position_order_links', 'positions',
        'quant_signals', 'strategy_health_snapshots', 'strategy_pnl_snapshots',
        'strategy_risk_state', 'trade_events', 'wallet_inventory',
        'wallet_success_metrics', 'wallet_trades'
    ] LOOP
        IF to_regclass(format('public.%I', t)) IS NOT NULL THEN
            EXECUTE format('GRANT SELECT ON public.%I TO grafana_reader', t);
        END IF;
    END LOOP;
END
$$;
