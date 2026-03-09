# Self-Improving Model Design

## Goal

Increase win rate without handing uncontrolled authority to a live model.

The system should improve through a gated loop:

1. capture canonical attempt-level and decision-level data
2. train and replay models offline
3. run them in live shadow mode
4. grant bounded authority only after forward evidence
5. auto-rollback on failure-rate, drawdown, or latency drift

## Canonical datasets

Two canonical SQL views now define the learning surface:

- `canonical_arb_learning_attempts`
  - one row per terminal arb attempt
  - keyed by `attempt_id`
  - includes execution telemetry, failure stage, one-legged state, and realized outcomes when available
- `canonical_quant_learning_decisions`
  - one row per quant signal/decision
  - keyed by `decision_id`
  - includes execution lifecycle, hold time, and realized outcomes when available

This keeps research and live attribution anchored to the same source of truth instead of rebuilding rows from ad hoc logs.

## Model registry and governance

The learning control plane uses four tables:

- `learning_model_registry`
  - versioned model definitions
  - target, scope, feature view, metrics, artifact location, status
- `learning_shadow_predictions`
  - live predictions that do not control execution yet
  - records `entity_type`, `entity_id`, target, score, threshold, and recommended action
- `learning_offline_evaluations`
  - replay outputs against historical windows
  - stores metrics and the decision policy used in replay
- `learning_model_rollouts`
  - bounded production rollouts
  - stores authority level, bounds, and guardrail thresholds
- `learning_rollout_observations`
  - live monitoring points for rollouts
  - stores failure, one-legged rate, drawdown, latency, and edge capture

## Initial model targets

Arb:

- `open_success_probability`
  - probability both legs complete and position opens
- `one_legged_risk`
  - probability first leg fills but second leg does not
- `realized_edge_capture`
  - ratio of realized edge to expected edge after latency and slippage

Quant:

- `execute_success_probability`
  - probability a signal executes cleanly
- `realized_pnl_sign`
  - classification target for positive vs negative realized outcome
- `realized_edge_capture`
  - ratio of realized outcome to modeled edge

## Shadow mode contract

Shadow mode must write one prediction row per live decision:

- arb entity: `attempt_id`
- quant entity: `decision_id`
- target and score
- threshold used
- recommended action
- optional context snapshot

The executor ignores the recommendation in shadow mode. The only purpose is to measure forward accuracy against the canonical views.

## Offline evaluator contract

An evaluator run should:

1. select a canonical feature view and date window
2. load the matching model version
3. replay historical rows with the candidate decision policy
4. compare against baseline rules
5. persist metrics to `learning_offline_evaluations`

Minimum arb metrics:

- ROC-AUC or calibration for success/risk targets
- open-rate delta
- one-legged-rate delta
- realized edge capture delta
- simulated P&L delta after fees

Minimum quant metrics:

- execution-rate delta
- win-rate delta
- realized P&L delta
- drawdown delta
- edge capture delta

## Rollout policy

Production rollouts should stay bounded at first:

- `observe`
  - model only logs
- `tail_reject`
  - model can reject only worst-risk slice
- `size_adjust`
  - model can reduce size within a configured band
- `priority_only`
  - model can reprioritize but not block
- `full`
  - reserved for mature models only

Bounds belong in `learning_model_rollouts.bounds`.

Example:

```json
{
  "max_reject_pct": 0.05,
  "max_size_reduction_pct": 0.2,
  "scope": ["arb"],
  "markets": ["pilot"]
}
```

Guardrails belong in `learning_model_rollouts.guardrails`.

Example:

```json
{
  "max_failure_rate": 0.08,
  "max_one_legged_rate": 0.02,
  "max_drawdown_pct": 0.03,
  "max_latency_p90_ms": 900
}
```

When a rollout breaches a guardrail, the control plane should:

1. write a `learning_rollout_observations` row with `guardrail_state = "rollback"`
2. mark the rollout `rolled_back`
3. remove model authority from the executor path

## Immediate next steps

1. Add admin/workspace APIs for creating and editing model rollouts instead of relying on direct DB writes.
2. Replace heuristic shadow baselines with trained models once replay quality is strong enough.
3. Tighten rollout metrics from simple loss/drawdown proxies to portfolio-aware attribution.
4. Compare baseline vs model decisions on the Learning Loop dashboard before granting broader live authority.
