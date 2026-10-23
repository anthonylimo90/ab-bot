-- Opened and failed arbitrage attempts end in events the position service
-- records, which carry neither the attempt id nor the executor's telemetry:
-- their rows took the position id as attempt id and joined no shadow
-- prediction, so rollout guardrails saw no opened or failed arb attempt.
-- Each row now reads the attempt's metadata from its position's entry
-- request (which carries the attempt id), overlaid with its own. A failed
-- entry later found holding a leg (recovery moves it to one-legged) is one
-- attempt: only its one-legged row remains.
CREATE OR REPLACE VIEW canonical_arb_learning_attempts AS
SELECT
    COALESCE(
        attempt.m ->> 'attempt_id',
        te.signal_id::TEXT,
        te.position_id::TEXT,
        te.id::TEXT
    ) AS attempt_id,
    te.id AS terminal_event_id,
    te.occurred_at AS attempt_time,
    te.execution_mode,
    te.source,
    te.market_id,
    te.position_id,
    te.signal_id,
    te.event_type AS terminal_event_type,
    CASE te.event_type
        WHEN 'position_open' THEN 'opened'
        WHEN 'signal_skipped' THEN 'skipped'
        ELSE 'failed'
    END AS outcome,
    te.reason,
    te.state_from,
    te.state_to,
    te.direction,
    te.confidence,
    te.expected_edge,
    te.observed_edge,
    te.requested_size_usd,
    te.filled_size_usd,
    te.fill_price,
    attempt.m ->> 'failure_stage' AS failure_stage,
    attempt.m ->> 'token_source' AS token_source,
    (te.event_type = 'one_legged_exit_ready'
        OR COALESCE((attempt.m ->> 'one_legged')::BOOLEAN, FALSE)) AS one_legged,
    (attempt.m ->> 'signal_age_ms')::DOUBLE PRECISION AS signal_age_ms,
    (attempt.m ->> 'token_lookup_ms')::DOUBLE PRECISION AS token_lookup_ms,
    (attempt.m ->> 'depth_check_ms')::DOUBLE PRECISION AS depth_check_ms,
    (attempt.m ->> 'preflight_ms')::DOUBLE PRECISION AS preflight_ms,
    (attempt.m ->> 'yes_order_ms')::DOUBLE PRECISION AS yes_order_ms,
    (attempt.m ->> 'no_order_ms')::DOUBLE PRECISION AS no_order_ms,
    (attempt.m ->> 'inter_leg_gap_ms')::DOUBLE PRECISION AS inter_leg_gap_ms,
    (attempt.m ->> 'request_to_fill_ms')::DOUBLE PRECISION AS request_to_fill_ms,
    (attempt.m ->> 'request_to_open_ms')::DOUBLE PRECISION AS request_to_open_ms,
    (attempt.m ->> 'total_time_ms')::DOUBLE PRECISION AS total_time_ms,
    (attempt.m ->> 'execution_slippage_bps')::DOUBLE PRECISION AS execution_slippage_bps,
    COALESCE(te.realized_pnl, p.realized_pnl) AS realized_pnl,
    p.unrealized_pnl,
    p.exit_timestamp AS realized_at,
    CASE p.state
        WHEN 0 THEN 'pending'
        WHEN 1 THEN 'open'
        WHEN 2 THEN 'exit_ready'
        WHEN 3 THEN 'closing'
        WHEN 4 THEN 'closed'
        WHEN 5 THEN 'entry_failed'
        WHEN 6 THEN 'exit_failed'
        WHEN 7 THEN 'stalled'
        ELSE NULL
    END AS position_state,
    CASE
        WHEN p.entry_timestamp IS NOT NULL AND p.exit_timestamp IS NOT NULL THEN
            EXTRACT(EPOCH FROM (p.exit_timestamp - p.entry_timestamp)) / 3600.0
        ELSE NULL
    END AS hold_hours,
    CASE
        WHEN te.expected_edge IS NOT NULL
         AND te.expected_edge <> 0::DECIMAL(20, 10)
         AND te.observed_edge IS NOT NULL
        THEN te.observed_edge / te.expected_edge
        ELSE NULL
    END AS observed_edge_capture_ratio,
    CASE
        WHEN te.expected_edge IS NOT NULL
         AND te.expected_edge <> 0::DECIMAL(20, 10)
         AND COALESCE(te.realized_pnl, p.realized_pnl) IS NOT NULL
        THEN COALESCE(te.realized_pnl, p.realized_pnl) / te.expected_edge
        ELSE NULL
    END AS realized_edge_capture_ratio,
    te.metadata
FROM trade_events te
LEFT JOIN positions p
    ON p.id = te.position_id
-- The attempt's telemetry: what its entry request recorded (the attempt id
-- among it), overlaid with the terminal event's own.
CROSS JOIN LATERAL (
    SELECT COALESCE((
        SELECT r.metadata
        FROM trade_events r
        WHERE r.position_id = te.position_id
          AND r.event_type = 'entry_requested'
        ORDER BY r.occurred_at ASC
        LIMIT 1
    ), '{}'::JSONB) || COALESCE(te.metadata, '{}'::JSONB) AS m
) attempt
WHERE te.strategy = 'arb'
  AND te.event_type IN (
      'signal_skipped', 'position_open', 'position_failed', 'entry_failed',
      'one_legged_exit_ready'
  )
  -- A failed entry later found holding a leg ends one-legged: one row.
  AND NOT (
      te.event_type = 'entry_failed'
      AND EXISTS (
          SELECT 1
          FROM trade_events l
          WHERE l.position_id = te.position_id
            AND l.event_type = 'one_legged_exit_ready'
      )
  );
