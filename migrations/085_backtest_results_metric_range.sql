-- Widen backtest_results metric columns and allow NULL for undefined metrics.
--
-- The result UPDATE failed with "numeric field overflow" whenever a metric
-- exceeded its narrow type (e.g. annualized_return DECIMAL(10, 6) holds < 10^4,
-- but compounding a short-window return annualizes to far more). Because the
-- failure was only logged, the row stayed `running` forever. Separately, the
-- API layer stored +inf profit_factor / sortino_ratio / calmar_ratio as 0.
--
-- NUMERIC(24, 8) holds |value| < 10^16, far beyond any real metric. The writer
-- now stores NULL for non-finite or out of range values, so the columns that
-- can be undefined (sharpe, sortino, profit factor, annualized return) no
-- longer require NOT NULL; the other metric columns were already nullable or
-- are finite by construction (max_drawdown stays NOT NULL). Existing rows keep
-- their values; their zeros for previously non-finite metrics cannot be
-- recovered.
--
-- win_rate (5, 4) is a share in [0, 1], total_return/final_value/fees are
-- money columns and total_trades-style counters are integers; they are
-- unchanged.

ALTER TABLE backtest_results
    ALTER COLUMN return_pct TYPE NUMERIC(24, 8),
    ALTER COLUMN annualized_return TYPE NUMERIC(24, 8),
    ALTER COLUMN max_drawdown TYPE NUMERIC(24, 8),
    ALTER COLUMN sharpe_ratio TYPE NUMERIC(24, 8),
    ALTER COLUMN sortino_ratio TYPE NUMERIC(24, 8),
    ALTER COLUMN profit_factor TYPE NUMERIC(24, 8),
    ALTER COLUMN total_return_pct TYPE NUMERIC(24, 8),
    ALTER COLUMN max_drawdown_pct TYPE NUMERIC(24, 8),
    ALTER COLUMN expectancy TYPE NUMERIC(24, 8),
    ALTER COLUMN calmar_ratio TYPE NUMERIC(24, 8),
    ALTER COLUMN var_95 TYPE NUMERIC(24, 8),
    ALTER COLUMN cvar_95 TYPE NUMERIC(24, 8),
    ALTER COLUMN recovery_factor TYPE NUMERIC(24, 8),
    ALTER COLUMN best_trade_return TYPE NUMERIC(24, 8),
    ALTER COLUMN worst_trade_return TYPE NUMERIC(24, 8);

ALTER TABLE backtest_results
    ALTER COLUMN annualized_return DROP NOT NULL,
    ALTER COLUMN sharpe_ratio DROP NOT NULL,
    ALTER COLUMN sortino_ratio DROP NOT NULL,
    ALTER COLUMN profit_factor DROP NOT NULL;

-- Finds stale `running` rows for the reaper without scanning the whole table.
CREATE INDEX IF NOT EXISTS idx_backtest_running_created
    ON backtest_results (created_at)
    WHERE status = 'running';
