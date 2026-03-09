# Win Rate Roadmap

## Goal

Improve win rate by separating two problems that currently look like one:

1. `arb` win rate is primarily an execution attribution problem.
2. `quant` win rate is primarily a decision-quality and exit-quality problem.

The system should evolve in phases so that each later optimization is built on trustworthy telemetry rather than guesswork.

## Phase 0: Measurement First

Status: in progress

Deliverables:

- canonical arb attempt telemetry on terminal lifecycle events
- arb telemetry API under trade-flow
- dashboard page for arb timing, failures, and recent attempts

Questions this phase answers:

- how old are winning vs failing arb signals when they reach execution?
- how much time is spent in token lookup, depth checks, and preflight?
- how long does each leg take in live conditions?
- how often do one-legged failures happen, and where?

Exit criteria:

- operators can explain whether arb misses come from stale input, slow preflight, first-leg latency, second-leg latency, or book deterioration

## Phase 1: Arb Execution Hardening

Primary objective:

- improve arb conversion without a full platform rewrite

Work items:

- carry forward signal-time quotes and compare them to execution-time quotes
- reduce duplicate order-book fetches in the entry path
- add explicit `request -> yes sent -> yes filled -> no sent -> no filled -> open` timestamps
- classify failures into structural buckets: stale, depth collapse, quote drift, first-leg reject, second-leg reject, connectivity, token lookup
- add alerting thresholds for one-legged failures and total-attempt latency drift

Potential architecture changes:

- specialized arb entry path with a tighter hot loop
- pre-hydrated token and book snapshots for selected markets
- paired-order execution path if exchange semantics allow it safely

Do not do yet:

- full service decomposition only for theoretical latency gains

## Phase 2: Quant Research/Live Parity

Primary objective:

- improve quant win rate by improving the quality of decisions, not by shaving milliseconds

Work items:

- shorten strategy-health cadence from batch snapshots to near-real-time scorecards
- promote expected-vs-realized attribution to first-class strategy inputs
- add thesis-specific exits by strategy instead of shared fallback exits
- measure regime-conditioned performance for flow, mean reversion, cross-market, and resolution proximity
- feed skip reasons and exit outcomes back into strategy ranking

Exit criteria:

- each quant sleeve has a current realized scorecard with sample size, edge capture, failure rate, and regime split

## Phase 3: Model-Ready Dataset

Primary objective:

- build the dataset required for a self-improving model without contaminating it

Required data grain:

- one row per arb attempt
- one row per quant signal decision

Required features:

- signal-time market state
- execution-time market state
- regime
- wallet-quality or participant-quality context
- strategy config at decision time
- latency and failure telemetry
- realized outcome after close

Required labels:

- executed and opened
- skipped and skip reason
- failed and failure stage
- realized pnl
- edge capture ratio
- time-to-close

Guardrails:

- immutable event log
- reproducible offline training set
- strict train/validation/test time splits
- no hidden data from post-decision states in pre-decision features

## Phase 4: Self-Improving Model

Primary objective:

- let the system adapt win rate using learned policy updates while staying capital-safe

What “self-improving” actually requires:

- a policy target:
  should we trade?
  how much size?
  which markets first?
  when should we back off?

- an evaluator:
  offline replay for decision quality
  shadow-mode online evaluation before capital changes

- a deployment contract:
  bounded parameter changes
  automatic rollback on degradation
  canary rollout by strategy or workspace

Initial model candidates:

1. arb conversion model
   predict `open_success_probability`, `one_legged_risk`, and `expected_edge_capture`

2. arb ranking model
   rank opportunities by expected realized value instead of raw modeled edge

3. quant execution gate model
   predict whether a generated signal should actually be executed under current regime and recent telemetry

4. quant exit model
   strategy-specific close policy learned from realized outcomes, not just static thresholds

## Phase 5: Controlled Online Learning

Primary objective:

- adapt continuously without allowing feedback loops to destroy the edge

Rules:

- no direct unconstrained online learning on live capital
- updates must be batched, reviewed by evaluator, then applied with bounds
- keep exploration budget explicit and small
- monitor drift by strategy and market regime
- freeze learning when drawdown or failure-rate thresholds trigger

## Recommended Build Order

1. finish arb telemetry and operator dashboard
2. add arb attempt funnel timestamps and alerting
3. improve arb quote reuse and preflight cost
4. move strategy health from periodic snapshots toward continuous feedback
5. assemble model-ready datasets
6. ship shadow-mode models before live policy changes
