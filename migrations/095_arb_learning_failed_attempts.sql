-- Arbitrage entries end in 'entry_failed' (the first leg failed) or
-- 'one_legged_exit_ready' (the second leg failed after the first filled), not
-- 'position_failed': count both as failed attempts, the second as one-legged.
CREATE OR REPLACE VIEW canonical_arb_learning_attempts AS
SELECT
    COALESCE(
        te.metadata ->> 'attempt_id',
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
    te.metadata ->> 'failure_stage' AS failure_stage,
    te.metadata ->> 'token_source' AS token_source,
    (te.event_type = 'one_legged_exit_ready'
        OR COALESCE((te.metadata ->> 'one_legged')::BOOLEAN, FALSE)) AS one_legged,
    (te.metadata ->> 'signal_age_ms')::DOUBLE PRECISION AS signal_age_ms,
    (te.metadata ->> 'token_lookup_ms')::DOUBLE PRECISION AS token_lookup_ms,
    (te.metadata ->> 'depth_check_ms')::DOUBLE PRECISION AS depth_check_ms,
    (te.metadata ->> 'preflight_ms')::DOUBLE PRECISION AS preflight_ms,
    (te.metadata ->> 'yes_order_ms')::DOUBLE PRECISION AS yes_order_ms,
    (te.metadata ->> 'no_order_ms')::DOUBLE PRECISION AS no_order_ms,
    (te.metadata ->> 'inter_leg_gap_ms')::DOUBLE PRECISION AS inter_leg_gap_ms,
    (te.metadata ->> 'request_to_fill_ms')::DOUBLE PRECISION AS request_to_fill_ms,
    (te.metadata ->> 'request_to_open_ms')::DOUBLE PRECISION AS request_to_open_ms,
    (te.metadata ->> 'total_time_ms')::DOUBLE PRECISION AS total_time_ms,
    (te.metadata ->> 'execution_slippage_bps')::DOUBLE PRECISION AS execution_slippage_bps,
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
WHERE te.strategy = 'arb'
  AND te.event_type IN (
      'signal_skipped', 'position_open', 'position_failed', 'entry_failed',
      'one_legged_exit_ready'
  );
