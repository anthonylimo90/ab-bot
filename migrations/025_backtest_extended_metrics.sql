-- Extended backtest metrics surfaced from the backtester engine
ALTER TABLE backtest_results
ADD COLUMN IF NOT EXISTS expectancy DECIMAL(10, 6),
ADD COLUMN IF NOT EXISTS calmar_ratio DECIMAL(10, 4),
ADD COLUMN IF NOT EXISTS var_95 DECIMAL(10, 6),
ADD COLUMN IF NOT EXISTS cvar_95 DECIMAL(10, 6),
ADD COLUMN IF NOT EXISTS recovery_factor DECIMAL(10, 4),
ADD COLUMN IF NOT EXISTS best_trade_return DECIMAL(10, 6),
ADD COLUMN IF NOT EXISTS worst_trade_return DECIMAL(10, 6),
ADD COLUMN IF NOT EXISTS max_consecutive_wins INTEGER,
ADD COLUMN IF NOT EXISTS max_consecutive_losses INTEGER;
