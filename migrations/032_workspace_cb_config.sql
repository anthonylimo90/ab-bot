-- Circuit breaker config overrides per workspace.
-- When present, these take priority over env-var defaults.
ALTER TABLE workspaces
  ADD COLUMN IF NOT EXISTS cb_max_daily_loss NUMERIC(20,2),
  ADD COLUMN IF NOT EXISTS cb_max_drawdown_pct NUMERIC(10,6),
  ADD COLUMN IF NOT EXISTS cb_max_consecutive_losses INTEGER,
  ADD COLUMN IF NOT EXISTS cb_cooldown_minutes INTEGER,
  ADD COLUMN IF NOT EXISTS cb_enabled BOOLEAN;
