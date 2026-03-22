UPDATE positions
SET source = 1,
    updated_at = NOW()
WHERE source = 0
  AND exit_strategy = 0
  AND source_signal_id IS NULL
  AND (
      (yes_entry_price > 0 AND no_entry_price > 0)
      OR failure_reason ILIKE '%one-legged%'
      OR failure_reason ILIKE '%one_legged%'
  );
