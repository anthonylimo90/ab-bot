-- The login Grafana's PostgreSQL datasource uses: SELECT only, and only on the
-- trading and analytics tables an operator charts. Users, credentials,
-- sessions, signed orders and signed transactions stay out of reach.
-- Run by the monitoring profile's grafana-db-reader service on every start
-- (idempotent), as the database owner, once the API has applied migrations.
-- The password comes from the environment, never the command line, and the
-- statement that sets it is kept out of the server log.
\getenv pw GRAFANA_DB_PASSWORD
SET log_statement = 'none';
SET log_min_error_statement = 'panic';

SELECT 'CREATE ROLE grafana_reader LOGIN'
WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'grafana_reader')\gexec

ALTER ROLE grafana_reader LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE CONNECTION LIMIT 5 PASSWORD :'pw';
ALTER ROLE grafana_reader SET default_transaction_read_only = on;
ALTER ROLE grafana_reader SET statement_timeout = '30s';

-- Exactly the tables below: one taken off the list loses its grant. In one
-- transaction, so a live Grafana never sees the tables without grants.
BEGIN;
REVOKE ALL ON ALL TABLES IN SCHEMA public FROM grafana_reader;

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
COMMIT;
