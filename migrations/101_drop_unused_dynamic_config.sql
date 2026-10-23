-- Remove dynamic config rows nothing reads. QUANT_MIN_CONFIDENCE,
-- FLOW_MIN_IMBALANCE and MEAN_REV_MIN_MOVE_PCT (seeded by 041) are read from
-- the environment only: the tuner has no target for them and the runtime
-- snapshot does not apply them, yet they showed in the tuner table as if
-- they were tuned. Their environment settings are unchanged.
DELETE FROM dynamic_config
WHERE key IN ('QUANT_MIN_CONFIDENCE', 'FLOW_MIN_IMBALANCE', 'MEAN_REV_MIN_MOVE_PCT');
