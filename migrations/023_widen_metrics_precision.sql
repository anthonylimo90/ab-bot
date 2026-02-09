-- Widen DECIMAL precision on wallet_success_metrics to prevent numeric field overflow.
-- DECIMAL(10,6) only allows 4 digits before the decimal point, which overflows
-- for annualized returns, large ROIs, and extreme volatility values.
-- DECIMAL(20,8) allows 12 digits before the decimal, handling any realistic value.

ALTER TABLE wallet_success_metrics
ALTER COLUMN roi_30d TYPE DECIMAL(20, 8),
ALTER COLUMN roi_90d TYPE DECIMAL(20, 8),
ALTER COLUMN roi_all_time TYPE DECIMAL(20, 8),
ALTER COLUMN annualized_return TYPE DECIMAL(20, 8),
ALTER COLUMN sharpe_30d TYPE DECIMAL(20, 8),
ALTER COLUMN sortino_30d TYPE DECIMAL(20, 8),
ALTER COLUMN max_drawdown_30d TYPE DECIMAL(20, 8),
ALTER COLUMN volatility_30d TYPE DECIMAL(20, 8),
ALTER COLUMN roi TYPE DECIMAL(20, 8);
