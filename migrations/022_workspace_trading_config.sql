-- Add trading configuration columns to workspaces table.
-- These allow workspace owners to configure trading services from the dashboard.
ALTER TABLE workspaces
ADD COLUMN IF NOT EXISTS polygon_rpc_url VARCHAR(512),
ADD COLUMN IF NOT EXISTS alchemy_api_key VARCHAR(512),
ADD COLUMN IF NOT EXISTS arb_auto_execute BOOLEAN DEFAULT FALSE,
ADD COLUMN IF NOT EXISTS copy_trading_enabled BOOLEAN DEFAULT TRUE,
ADD COLUMN IF NOT EXISTS live_trading_enabled BOOLEAN DEFAULT FALSE;
