-- Canonical learning datasets and governance tables for model-based win-rate improvement.

CREATE TABLE IF NOT EXISTS learning_model_registry (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    model_key TEXT NOT NULL UNIQUE,
    strategy_scope TEXT NOT NULL,
    target TEXT NOT NULL,
    model_type TEXT NOT NULL,
    version TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'shadow',
    feature_view TEXT NOT NULL,
    training_window_start TIMESTAMPTZ,
    training_window_end TIMESTAMPTZ,
    validation_window_start TIMESTAMPTZ,
    validation_window_end TIMESTAMPTZ,
    metrics JSONB NOT NULL DEFAULT '{}'::JSONB,
    artifact_uri TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    activated_at TIMESTAMPTZ,
    CHECK (
        status IN ('draft', 'shadow', 'canary', 'active', 'retired', 'disabled')
    )
);

CREATE INDEX IF NOT EXISTS idx_learning_model_registry_scope_target_status
    ON learning_model_registry (strategy_scope, target, status, created_at DESC);

CREATE TRIGGER update_learning_model_registry_updated_at
    BEFORE UPDATE ON learning_model_registry
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TABLE IF NOT EXISTS learning_shadow_predictions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    model_id UUID NOT NULL REFERENCES learning_model_registry(id) ON DELETE CASCADE,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    strategy_scope TEXT NOT NULL,
    target TEXT NOT NULL,
    recommended_action TEXT,
    predicted_score DOUBLE PRECISION NOT NULL,
    threshold DOUBLE PRECISION,
    context JSONB NOT NULL DEFAULT '{}'::JSONB,
    predicted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    outcome_available_at TIMESTAMPTZ,
    CHECK (entity_type IN ('arb_attempt', 'quant_decision'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_learning_shadow_predictions_unique_entity
    ON learning_shadow_predictions (model_id, entity_type, entity_id, target);

CREATE INDEX IF NOT EXISTS idx_learning_shadow_predictions_predicted_at
    ON learning_shadow_predictions (predicted_at DESC);

CREATE INDEX IF NOT EXISTS idx_learning_shadow_predictions_entity
    ON learning_shadow_predictions (entity_type, entity_id, predicted_at DESC);

CREATE TABLE IF NOT EXISTS learning_offline_evaluations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    model_id UUID NOT NULL REFERENCES learning_model_registry(id) ON DELETE CASCADE,
    dataset_name TEXT NOT NULL,
    evaluation_scope TEXT NOT NULL,
    window_start TIMESTAMPTZ,
    window_end TIMESTAMPTZ,
    metrics JSONB NOT NULL DEFAULT '{}'::JSONB,
    decision_policy JSONB NOT NULL DEFAULT '{}'::JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (evaluation_scope IN ('arb', 'quant', 'portfolio'))
);

CREATE INDEX IF NOT EXISTS idx_learning_offline_evaluations_model_created
    ON learning_offline_evaluations (model_id, created_at DESC);

CREATE TABLE IF NOT EXISTS learning_model_rollouts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    model_id UUID NOT NULL REFERENCES learning_model_registry(id) ON DELETE CASCADE,
    strategy_scope TEXT NOT NULL,
    rollout_mode TEXT NOT NULL,
    authority_level TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    bounds JSONB NOT NULL DEFAULT '{}'::JSONB,
    guardrails JSONB NOT NULL DEFAULT '{}'::JSONB,
    baseline_window_hours INTEGER NOT NULL DEFAULT 24,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at TIMESTAMPTZ,
    rollback_reason TEXT,
    CHECK (rollout_mode IN ('shadow', 'canary', 'bounded', 'full')),
    CHECK (
        authority_level IN ('observe', 'tail_reject', 'size_adjust', 'priority_only', 'full')
    ),
    CHECK (status IN ('active', 'rolled_back', 'completed', 'paused'))
);

CREATE INDEX IF NOT EXISTS idx_learning_model_rollouts_status_started
    ON learning_model_rollouts (status, started_at DESC);

CREATE TABLE IF NOT EXISTS learning_rollout_observations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    rollout_id UUID NOT NULL REFERENCES learning_model_rollouts(id) ON DELETE CASCADE,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    failure_rate DOUBLE PRECISION,
    one_legged_rate DOUBLE PRECISION,
    drawdown_pct DOUBLE PRECISION,
    latency_p90_ms DOUBLE PRECISION,
    edge_capture_ratio DECIMAL(20, 10),
    guardrail_state TEXT NOT NULL DEFAULT 'ok',
    notes JSONB NOT NULL DEFAULT '{}'::JSONB,
    CHECK (guardrail_state IN ('ok', 'warn', 'rollback'))
);

CREATE INDEX IF NOT EXISTS idx_learning_rollout_observations_rollout_observed
    ON learning_rollout_observations (rollout_id, observed_at DESC);

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
        WHEN 'position_failed' THEN 'failed'
        ELSE 'skipped'
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
    COALESCE((te.metadata ->> 'one_legged')::BOOLEAN, FALSE) AS one_legged,
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
  AND te.event_type IN ('signal_skipped', 'position_open', 'position_failed');

COMMENT ON VIEW canonical_arb_learning_attempts IS
    'One row per terminal arbitrage attempt, enriched with execution telemetry and realized outcomes.';

CREATE OR REPLACE VIEW canonical_quant_learning_decisions AS
WITH trade_event_rollup AS (
    SELECT
        te.signal_id,
        MIN(te.occurred_at) FILTER (WHERE te.event_type = 'signal_generated') AS signal_generated_at,
        MIN(te.occurred_at) FILTER (WHERE te.event_type = 'entry_requested') AS entry_requested_at,
        MIN(te.occurred_at) FILTER (WHERE te.event_type = 'entry_filled') AS entry_filled_at,
        MIN(te.occurred_at) FILTER (WHERE te.event_type = 'position_open') AS position_opened_at,
        MIN(te.occurred_at) FILTER (WHERE te.event_type = 'exit_requested') AS exit_requested_at,
        MAX(te.occurred_at)
            FILTER (WHERE te.event_type IN ('closed_via_exit', 'closed_via_resolution'))
            AS position_closed_at,
        MAX(te.occurred_at) FILTER (WHERE te.event_type = 'position_failed') AS position_failed_at,
        (ARRAY_AGG(te.position_id ORDER BY te.occurred_at DESC)
            FILTER (WHERE te.position_id IS NOT NULL))[1] AS position_id,
        (ARRAY_AGG(te.event_type ORDER BY te.occurred_at DESC))[1] AS latest_event_type,
        (ARRAY_AGG(te.reason ORDER BY te.occurred_at DESC)
            FILTER (WHERE te.reason IS NOT NULL))[1] AS latest_reason,
        (ARRAY_AGG(te.state_to ORDER BY te.occurred_at DESC)
            FILTER (WHERE te.state_to IS NOT NULL))[1] AS latest_state_to,
        (ARRAY_AGG(te.expected_edge ORDER BY te.occurred_at DESC)
            FILTER (WHERE te.expected_edge IS NOT NULL))[1] AS expected_edge,
        (ARRAY_AGG(te.observed_edge ORDER BY te.occurred_at DESC)
            FILTER (WHERE te.observed_edge IS NOT NULL))[1] AS observed_edge,
        (ARRAY_AGG(te.requested_size_usd ORDER BY te.occurred_at DESC)
            FILTER (WHERE te.requested_size_usd IS NOT NULL))[1] AS requested_size_usd,
        (ARRAY_AGG(te.filled_size_usd ORDER BY te.occurred_at DESC)
            FILTER (WHERE te.filled_size_usd IS NOT NULL))[1] AS filled_size_usd,
        (ARRAY_AGG(te.fill_price ORDER BY te.occurred_at DESC)
            FILTER (WHERE te.fill_price IS NOT NULL))[1] AS fill_price,
        (ARRAY_AGG(te.realized_pnl ORDER BY te.occurred_at DESC)
            FILTER (WHERE te.realized_pnl IS NOT NULL))[1] AS event_realized_pnl
    FROM trade_events te
    WHERE te.source = 'quant'
      AND te.signal_id IS NOT NULL
    GROUP BY te.signal_id
)
SELECT
    qs.id AS decision_id,
    qs.generated_at AS decision_time,
    qs.kind AS strategy,
    qs.condition_id AS market_id,
    qs.direction,
    qs.confidence,
    qs.size_usd,
    qs.execution_status,
    COALESCE(qs.skip_reason, ter.latest_reason) AS skip_reason,
    COALESCE(qs.position_id, ter.position_id) AS position_id,
    ter.signal_generated_at,
    ter.entry_requested_at,
    ter.entry_filled_at,
    ter.position_opened_at,
    ter.exit_requested_at,
    COALESCE(p.exit_timestamp, ter.position_closed_at) AS closed_at,
    ter.position_failed_at,
    ter.expected_edge,
    ter.observed_edge,
    ter.requested_size_usd,
    ter.filled_size_usd,
    ter.fill_price,
    COALESCE(p.realized_pnl, ter.event_realized_pnl) AS realized_pnl,
    p.unrealized_pnl,
    CASE p.state
        WHEN 0 THEN 'pending'
        WHEN 1 THEN 'open'
        WHEN 2 THEN 'exit_ready'
        WHEN 3 THEN 'closing'
        WHEN 4 THEN 'closed'
        WHEN 5 THEN 'entry_failed'
        WHEN 6 THEN 'exit_failed'
        WHEN 7 THEN 'stalled'
        ELSE ter.latest_state_to
    END AS position_state,
    CASE
        WHEN qs.execution_status = 'skipped' THEN 'skipped'
        WHEN qs.execution_status = 'failed' OR ter.latest_event_type = 'position_failed' THEN 'failed'
        WHEN COALESCE(p.exit_timestamp, ter.position_closed_at) IS NOT NULL
         AND COALESCE(p.realized_pnl, ter.event_realized_pnl) > 0::DECIMAL(20, 10)
        THEN 'win'
        WHEN COALESCE(p.exit_timestamp, ter.position_closed_at) IS NOT NULL
         AND COALESCE(p.realized_pnl, ter.event_realized_pnl) < 0::DECIMAL(20, 10)
        THEN 'loss'
        WHEN ter.position_opened_at IS NOT NULL OR p.state IN (1, 2, 3, 6, 7) THEN 'open'
        WHEN ter.entry_requested_at IS NOT NULL THEN 'executing'
        ELSE 'pending'
    END AS decision_outcome,
    CASE
        WHEN p.entry_timestamp IS NOT NULL AND p.exit_timestamp IS NOT NULL THEN
            EXTRACT(EPOCH FROM (p.exit_timestamp - p.entry_timestamp)) / 3600.0
        ELSE NULL
    END AS hold_hours,
    CASE
        WHEN ter.expected_edge IS NOT NULL
         AND ter.expected_edge <> 0::DECIMAL(20, 10)
         AND ter.observed_edge IS NOT NULL
        THEN ter.observed_edge / ter.expected_edge
        ELSE NULL
    END AS observed_edge_capture_ratio,
    CASE
        WHEN ter.expected_edge IS NOT NULL
         AND ter.expected_edge <> 0::DECIMAL(20, 10)
         AND COALESCE(p.realized_pnl, ter.event_realized_pnl) IS NOT NULL
        THEN COALESCE(p.realized_pnl, ter.event_realized_pnl) / ter.expected_edge
        ELSE NULL
    END AS realized_edge_capture_ratio,
    qs.metadata
FROM quant_signals qs
LEFT JOIN trade_event_rollup ter
    ON ter.signal_id = qs.id
LEFT JOIN positions p
    ON p.id = COALESCE(qs.position_id, ter.position_id);

COMMENT ON VIEW canonical_quant_learning_decisions IS
    'One row per quantitative signal/decision, enriched with execution lifecycle and realized outcomes.';
