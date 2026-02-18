-- Add strategy classification to wallet features for diversity-aware selection.
ALTER TABLE wallet_features ADD COLUMN IF NOT EXISTS strategy_type TEXT;
CREATE INDEX IF NOT EXISTS idx_wallet_features_strategy_type ON wallet_features (strategy_type) WHERE strategy_type IS NOT NULL;
