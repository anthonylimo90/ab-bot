-- Seed dynamic_config keys for quant signal system.
-- These knobs are tuned at runtime by the DynamicTuner and
-- can be adjusted via the workspace settings API.

INSERT INTO dynamic_config (
    key, current_value, default_value, min_value, max_value,
    max_step_pct, enabled, last_good_value, updated_by, last_reason
) VALUES
    -- Minimum confidence threshold for quant signal execution (0.0–1.0)
    ('QUANT_MIN_CONFIDENCE', 0.65, 0.65, 0.40, 0.95, 0.10, TRUE, 0.65, 'bootstrap', 'initial seed'),
    -- Base position size in USD before confidence weighting
    ('QUANT_BASE_POSITION_SIZE', 30, 30, 5, 200, 0.15, TRUE, 30, 'bootstrap', 'initial seed'),
    -- Minimum |imbalance_ratio| for flow signal trigger
    ('FLOW_MIN_IMBALANCE', 0.25, 0.25, 0.10, 0.60, 0.12, TRUE, 0.25, 'bootstrap', 'initial seed'),
    -- Minimum absolute price change (fraction) for mean reversion trigger
    ('MEAN_REV_MIN_MOVE_PCT', 0.10, 0.10, 0.05, 0.30, 0.12, TRUE, 0.10, 'bootstrap', 'initial seed')
ON CONFLICT (key) DO UPDATE SET
    min_value = EXCLUDED.min_value,
    max_value = EXCLUDED.max_value,
    max_step_pct = EXCLUDED.max_step_pct,
    current_value = GREATEST(EXCLUDED.min_value, LEAST(EXCLUDED.max_value, dynamic_config.current_value)),
    last_good_value = GREATEST(EXCLUDED.min_value, LEAST(EXCLUDED.max_value, dynamic_config.last_good_value));
