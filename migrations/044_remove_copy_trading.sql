-- Remove copy-trading system tables, columns, and config rows.
-- Quant signal system is now the sole execution path.

-- Drop copy-trading-only tables
DROP TABLE IF EXISTS copy_trade_history CASCADE;
DROP TABLE IF EXISTS wallet_trade_signals CASCADE;
DROP TABLE IF EXISTS tracked_wallets CASCADE;
DROP TABLE IF EXISTS ensemble_predictions CASCADE;
DROP TABLE IF EXISTS demo_positions CASCADE;
DROP TABLE IF EXISTS pending_wallet_orders CASCADE;
DROP TABLE IF EXISTS workspace_wallet_bans CASCADE;
DROP TABLE IF EXISTS auto_rotation_history CASCADE;
DROP TABLE IF EXISTS workspace_wallet_allocations CASCADE;
DROP TABLE IF EXISTS workspace_invites CASCADE;
DROP TABLE IF EXISTS stop_loss_rules CASCADE;

-- Remove copy-trading columns from positions table
ALTER TABLE positions DROP COLUMN IF EXISTS is_copy_trade;
ALTER TABLE positions DROP COLUMN IF EXISTS source_wallet;
ALTER TABLE positions DROP COLUMN IF EXISTS source_token_id;

-- Remove copy-trading columns from workspaces table
ALTER TABLE workspaces DROP COLUMN IF EXISTS copy_trading_enabled;
ALTER TABLE workspaces DROP COLUMN IF EXISTS auto_select_enabled;
ALTER TABLE workspaces DROP COLUMN IF EXISTS auto_demote_enabled;
ALTER TABLE workspaces DROP COLUMN IF EXISTS last_optimization_at;

-- Delete copy-trading dynamic config rows
DELETE FROM dynamic_config WHERE key LIKE 'COPY_%';
DELETE FROM dynamic_config_history WHERE key LIKE 'COPY_%';
