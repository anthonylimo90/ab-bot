# End-to-end correctness review — 2026-10-20

Reviewed commit: `05c1d014f06eb27b5ea7aeacbccded4257419a0a` on
`fix/wallet-key-ownership`.

The review found **17 actionable issues and one threshold-unit concern**. The most
serious problems cross component boundaries: execution can be correct locally
while position persistence, reconciliation, or mode selection subsequently
misrepresents the holdings. Passing the existing suite does not establish that
these lifecycle paths are safe.

Scope: application startup and service wiring; authentication and workspace
access; shared-wallet and browser-signed orders; submissions, reservations and
recovery; position entry/exit/persistence; market and inventory refresh; risk
controls; backtest data and accounting; dashboard state and API integration;
migrations and CI configuration.

This is a review of source and isolated fixtures, not an observation of production
holdings or an exchange certification. Live exchange orders, transfers, deployment
and production database changes were outside the validation performed.

## Prioritized repair list

P1 means a high-priority correctness, trading-safety or access-control problem.
P2 means a normal-priority reliability, reporting or integration problem. The
findings describe the reviewed commit; the repair status below reflects the
subsequent implementation in this working tree.

| ID | Priority | Finding | Status | Evidence |
| --- | --- | --- | --- | --- |
| R01 | P1 | Changing trading mode can close live positions using simulated fills | Fixed | Database + local HTTP reproduction |
| R02 | P1 | Position reconciliation can erase explicit recovery holdings | Fixed | Database reproduction |
| R03 | P1 | Partial exits are recorded as full exits, dropping unsold holdings | Fixed | Service + database reproduction |
| R04 | P1 | Actual entry price and quantity are not persisted | Fixed | Service + database reproduction |
| R05 | P1 | Two workers can claim and execute the same position exit | Fixed | Concurrent service reproduction |
| R06 | P2 | Quant exit thresholds use ambiguous percentage units | Fixed | Actual predicate exercised; intended units need confirmation |
| R07 | P1 | Filled manual buys disappear from exposure; manual reservations do not enforce the cap | Fixed | Database reproduction + request trace |
| R08 | P1 | Uncertain submissions have no complete reconciliation path and block subsequent exits | Fixed | Submission/caller/recovery trace |
| R09 | P1 | Stop-loss and take-profit orders are accepted without an execution path | Fixed | Request + worker trace |
| R10 | P1 | Private trading analytics bypass canonical workspace membership checks | Fixed | Route + handler trace |
| R11 | P2 | A legacy text failure reason aborts position reconciliation | Fixed | Database reproduction |
| R12 | P2 | Dashboard logout does not revoke the server session | Fixed | Actual store execution with API spy |
| R13 | P2 | Invite acceptance bypasses persistent session revocation | Fixed | Authentication + invite trace |
| R14 | P2 | Backtests timestamp later observations at the start of the bucket | Fixed | TimescaleDB reproduction |
| R15 | P2 | Backtest equity and risk metrics omit final liquidation costs | Fixed | Simulator reproduction |
| R16 | P2 | Dashboard CI fails under its configured Node 20 runtime | Fixed | Node 20.20.2 reproduction |
| R17 | P2 | Market search only searches the first 100 fetched markets | Fixed | Dashboard + API trace |
| R18 | P2 | CI Clippy does not select the whole workspace | Fixed | Workflow + Cargo metadata |

## Implemented repairs

- **R01–R05:** positions persist immutable mode/wallet provenance and a version.
  Every repository writer compares that version. Actual per-leg fills persist
  their weighted entry/exit prices and quantities; partial exits retain shares,
  and closure requires flat inventory. Execution binds to persisted provenance,
  including manual reservations, throughout mode changes. Two-worker coverage
  exercises the real exit handler and asserts one submitted sale.
- **R02/R11:** reconciliation updates marks and visibility from explicit holdings,
  preserving state and quantities without parsing failure reasons. Inventory
  refresh and orphan recovery also respect partial holdings and manual inventory.
- **R06:** thresholds are fractional returns: defaults are +15% and −10%.
  Tests cover both thresholds and values immediately inside them.
- **R07/R09:** manual reservations enforce the shared exposure cap. Database
  triggers atomically transfer fills into wallet-scoped inventory and reduce
  basis on sales; unknown fill prices use conservative exposure. Manual sells
  cannot consume automated holdings or another wallet's inventory. Unsupported
  stop-loss/take-profit orders are rejected before reservation.
- **R08:** live position orders have durable position/leg links and atomic fill
  receipts. Pending reports retain their lifecycle reservation. A 30-second
  worker reconciles terminal exchange orders using authenticated confirmed trade
  records, applies inventory once, and then releases the fence. Missing, active,
  inconsistent or unconfirmed exchange evidence retains the fence. Attempts
  proven never to have crossed the durable submission boundary can retire after
  five minutes; a late submitter cannot POST a retired attempt. Interrupted local
  bookkeeping resumes only after all receipts settle.
- **R10/R12/R13:** private analytics require canonical workspace membership;
  learning mutations require trading access as well as their existing admin
  route guard. Logout revokes the server session before clearing browser state,
  reports failures, and permits retry. Invite acceptance uses persistent session
  authorization. Actual HTTP tests cover nonmembership, membership revocation,
  logout replay across instances and password-reset tokens on the invite route.
- **R14/R15:** historical bars become available at bucket close; incomplete bars
  are excluded. Equity includes initial capital and terminal liquidation costs,
  and the reported end time matches the final observation.
- **R16–R18:** Node 24 is consistent across dashboard CI, package metadata and
  Docker stages; CI Clippy selects the workspace. Market search filters the full
  catalog before pagination, with Previous/Next controls and an explicit page
  count. A database test finds a match beyond the original first 100 rows.

New migrations: **076–078**. Existing provenance remains unknown until verified;
no migration guesses whether historical holdings were paper/live or which wallet
owned them. Historical unlinked uncertain intents likewise require verified
association before they can be automatically reconciled. See the deployment
procedure in [DEPLOY.md](DEPLOY.md#correctness-repair-upgrade-076078).

The exchange evidence parser follows the authenticated order/trade schemas in the
[official CLOB client types](https://github.com/Polymarket/clob-client/blob/main/src/types.ts)
and [Python client](https://github.com/Polymarket/py-clob-client/blob/main/py_clob_client/client.py).
Local HTTP fixtures exercise these contracts; no production orders were sent.

## Repair validation

- Workspace Rust suite: **490 passed, 0 failed, 14 ignored**. The ignored
  migration-chain and Timescale aggregation tests were also run explicitly and
  **both passed**; the other 12 ignored cases were not relabeled as passing.
- Strict workspace Clippy (`--workspace --all-targets --all-features --locked
  -- -D warnings`), Rust formatting and whitespace checks pass.
- Dashboard: **5 unit tests passed**, TypeScript check passed, ESLint passed,
  production build generated all **24 pages** on Node **24.19.0**.
- Focused final checks cover the receipt/recovery path, partial/manual inventory
  ownership, shared-fee resolution and duplicate-close rejection. Recovery tests
  use local authenticated HTTP fixtures, including 150 unrelated wallet intents
  and unconfirmed trades that must remain fenced.
- Full migration chain **001–078** passed using SQLx transactions in isolated
  TimescaleDB. Production upgrades and live exchange submissions were not run.

Logs are retained in `/workspace/ab-bot-review-work/`: `repair-verified-workspace.log`,
`repair-final-clippy.log`, `repair-final-inventory.log`,
`repair-final-resolution.log`, `repair-final-recovery.log`,
`repair-migrations.log`, `repair-timescale-backtest.log`, and the
`repair-dashboard-{tests,tsc,lint,build}.log` files.

The fixes are recorded on the existing working branch. Deploying them requires
the migration and historical-provenance verification described above.

## Original findings and repair boundaries

### R01 — Live positions can be closed with paper fills

[ExitHandler::execute_exit](../crates/api-server/src/exit_handler.rs#L341) chooses
the execution mode from the current executor. Positions do not persist and enforce
their entry mode or trading-wallet identity. Disabling live mode makes
[the executor](../crates/trading-engine/src/executor.rs#L1104) simulate market
fills, and the exit handler persists those fills against the existing position.

Reproduction: create a position with live entry events and 10 held YES shares,
mark it exit-ready, disable live mode, then run the actual exit handler against a
local orderbook fixture. It reaches `Closed` and clears its holdings without an
exchange sale. Enabling live mode with paper positions creates the reverse risk.

Repair: persist immutable execution provenance on positions and intents, enforce
it throughout exits and recovery, and prevent runtime mode changes from converting
existing holdings. Treat legacy provenance as unknown until reconciled.

### R02 — The periodic reconciler destroys known holdings

[The reconciliation query](../crates/api-server/src/position_reconciler.rs#L193)
classifies exit failures using keywords in `failure_reason` and legacy exit-price
fields. When those heuristics do not recognize exposure, it changes state to
`EntryFailed`; the subsequent update sets both held quantities to zero.

Reproduction: an `ExitFailed` position with 10 explicit YES shares and the valid
JSON reason `{"connectivity_error":{"message":"connection reset"}}` becomes
`EntryFailed`, `held_yes_qty=0`, `is_open=false`.

Repair: explicit quantities must take precedence over inferred state and error
text. Reconciliation must not reduce holdings without authoritative fill,
redemption or inventory evidence. Coordinate this change with R03 and R05.

### R03 — A partial exit clears the unsold remainder

[PositionService::close_position](../crates/api-server/src/position_service.rs#L446)
moves every remaining held share into `exited_*_qty` on `MarketExit`.
`ExecutionReport::is_success` includes partial fills, while the exit handler uses
the shared `position.quantity` rather than the remaining quantity of each leg.
Legacy `held_outcomes()` also treats an exit-price field as proof that the whole
leg has exited.

Reproduction: record a confirmed sale of 4 out of 10 YES shares, then close through
the service. The call succeeds and records 10 exited shares and zero held shares.

Repair: close only after confirmed quantities exhaust the holdings; use each
leg's remaining quantity for exits, accumulate fill proceeds, and allow further
fills on a partially exited leg. Quantity rounding must not silently dispose of
the remainder.

### R04 — Entry fills are only partly persisted

[record_entry_fill](../crates/api-server/src/position_service.rs#L178) changes the
entry price and quantity in memory, but
[PositionRepository::update](../crates/polymarket-core/src/db/positions.rs#L171)
does not update `yes_entry_price`, `no_entry_price` or `quantity`.

Reproduction: a planned 10-share entry at 0.40 fills 8 shares at 0.45. The stored
row remains `(entry_price=0.40, quantity=10, held_yes_qty=8)`. A later worker uses
the wrong basis and can attempt to sell more than was acquired.

Repair: persist actual per-leg fill quantities and cost basis atomically with the
fill event. A single shared quantity is insufficient when legs differ; make
remaining inventory and realized proceeds explicit.

### R05 — Position transitions are not exclusive claims

[mark_closing](../crates/api-server/src/position_service.rs#L331) validates only
the caller's in-memory position.
[The update](../crates/polymarket-core/src/db/positions.rs#L192) predicates on ID
alone, with no expected state or version. Each API instance starts its own exit
worker, and the arb monitor also persists cached position structs.

Reproduction: two workers start with the same `ExitReady` snapshot and
concurrently call `mark_closing`; both calls succeed. Each can then generate a
different order ID. The wallet submission fence prevents overlapping unresolved
submissions, but permits the second after the first is acknowledged.

Repair: use an atomic state/version transition or durable claim before exchange
submission. Associate the exit attempt with a stable intent ID and reject stale
position writes. Add a two-worker test through submission and reconciliation.

### R06 — Quant threshold units need an explicit contract

[Defaults](../crates/api-server/src/exit_handler.rs#L63) are `0.15` and `0.10`,
while [generic_quant_exit](../crates/api-server/src/exit_handler.rs#L964)
multiplies the P&L ratio by 100 before comparing them. The effective defaults are
therefore **+0.15% and −0.10%**, not +15% and −10%.

The actual predicate exits a newly opened position at a 1% gain. A diagnostic
assertion assuming a 15% target fails. The repository does not explicitly document
the intended units, so that assertion alone is not proof of the intended target.
This is a potential 100× configuration error requiring a unit decision.

Repair: choose ratios or percentage points, document the convention, and test
boundary values. Review the thresholds against the configured fee model.

### R07 — Manual fills fall out of risk exposure

[reserve_order](../crates/api-server/src/handlers/trading.rs#L198) takes the
exposure lock but never evaluates a prospective cap. After a manual shared-wallet
buy fills, the request path updates `orders` without creating tracked position
inventory. [The exposure aggregate](../crates/polymarket-core/src/db/positions.rs#L28)
counts only outstanding order quantities and unresolved intents alongside
positions; it does not count filled manual holdings or wallet inventory.

Reproduction: a filled 100-share buy at 0.50, with no corresponding position,
contributes **0** instead of **50** to exposure. Inventory refresh does not fix
this aggregate.

Repair: enforce a shared prospective limit for manual orders and atomically
transfer filled reservations into wallet-scoped inventory/cost-basis exposure.
Avoid double-counting holdings represented in multiple tables.

### R08 — Uncertain submissions cannot complete the recovery lifecycle

[The submission fence](../crates/trading-engine/src/executor/submission.rs#L55)
blocks all later orders for a wallet while an intent is `submitting` or `unknown`,
including sells needed to unwind other positions. The repository has no worker or
operator endpoint that reconciles those intents to a terminal outcome.
[Order polling](../crates/api-server/src/handlers/trading.rs#L656) can reconcile an
order with a known exchange ID, but updates only `orders`, not the intent fence.

The automated callers compound this:
[quant](../crates/api-server/src/quant_signal_executor.rs#L1140) and
[arb](../crates/api-server/src/arb_executor.rs#L1091) treat every non-success report,
including `Pending`, as a rejected entry. Their generated order IDs are not
durably linked to the corresponding position/leg in the intent schema.

Repair: retain uncertain lifecycle state, link intents to positions and legs, and
implement reconciliation from authoritative exchange evidence. Release fences
only after recording the corresponding fills or confirmed non-execution; do not
remove the protective fence as a shortcut.

### R09 — Accepted protective orders never trigger

[The stop-order branch](../crates/api-server/src/handlers/trading.rs#L379)
returns a pending order for `StopLoss` and `TakeProfit`. No running worker reads
these rows and evaluates their trigger prices. The risk endpoint explicitly notes
that the former stop-loss-rules system was removed; generic quant exits do not
implement these individual API orders.

Repair: reject these order types as unsupported until there is a durable trigger,
submission and reconciliation path, or implement that path with restart and
duplicate-trigger tests.

### R10 — Trading analytics leak across workspace boundaries

[The routes](../crates/api-server/src/routes.rs#L514) put activity, signal and
trade-flow reads behind authentication, but the handlers do not enforce canonical
workspace access. For example,
[get_trade_flow_journeys](../crates/api-server/src/handlers/trade_flow.rs#L2718)
and [list_activity](../crates/api-server/src/handlers/activity.rs#L212) query shared
trading records without accepting claims or a workspace-access extractor.

A valid account that is not a member of the canonical workspace is denied the
positions endpoint but can retrieve overlapping position/trade details through
these analytics endpoints. This finding is established by route and query tracing;
an actual nonmember HTTP request was not run in this review.

Repair: apply the same canonical read authorization to every private projection,
including activity, execution telemetry and signal performance. Add endpoint
tests for nonmembers and membership revocation, not only tests of the guard.

### R11 — Plain-text failure reasons stop every reconciliation cycle

[The query casts TEXT to JSONB directly](../crates/api-server/src/position_reconciler.rs#L86)
before its text fallbacks. Existing migrations deliberately preserve legacy text.

Reproduction: one `ExitFailed` row containing `legacy exit failure` causes
PostgreSQL error `22P02: invalid input syntax for type json`. The entire cycle
fails, including updates for unrelated valid rows.

Repair: parse legacy reasons safely or normalize them through a reviewed
migration, without losing text or holdings. Test a mixed batch of JSON, plain
text and null reasons.

### R12 — Browser logout leaves its token valid on the server

[The auth store](../dashboard/stores/auth-store.ts#L48) clears the API token and
local state without calling the implemented
[server logout endpoint](../crates/api-server/src/handlers/auth.rs#L426).
Header, admin-header and workspace-gate logout actions all use that store.

Reproduction: execute the actual store with an API spy. The local token becomes
null and the number of server revocation requests is zero. A previously copied
token remains usable until expiry or separate revocation.

Repair: submit authenticated logout before discarding the token, handle failure
explicitly, and test browser logout followed by an attempted token replay.

### R13 — Invite acceptance bypasses persistent revocation

[extract_optional_claims](../crates/api-server/src/handlers/workspaces.rs#L285)
calls only `JwtAuth::validate_token`. The public invite-acceptance route then
checks the user's ID and email, but not `auth_version`, current role or
`revoked_sessions`. A password-reset-invalidated token, or one revoked on another
instance, can still accept an otherwise valid invite for its account.

Repair: use the shared database-backed session authorization for the optional
Bearer token. Add cross-instance revocation and password-reset tests on the
actual invite route.

### R14 — Backtest aggregation exposes later data at earlier timestamps

[query_snapshots](../crates/backtester/src/data_store.rs#L349) pairs
`last(price, timestamp)` with the start of `time_bucket`, then publishes that
bucket as the snapshot timestamp. Strategies therefore see values that were not
available at their simulated clock time.

Reproduction on TimescaleDB: observations at 12:00 (0.10) and 12:04 (0.90), queried
at five-minute resolution, produce a **12:00 snapshot priced at 0.90**.

Repair: define when an aggregated bar becomes available and timestamp it at that
availability time. Handle incomplete final bars explicitly. Apply the same
point-in-time convention to prices, depth, volume and strategy features.

### R15 — Terminal liquidation is missing from backtest equity metrics

[run_prepared](../crates/backtester/src/simulator.rs#L523) records equity before
forced final closes, then
[calculates drawdown and return-risk metrics](../crates/backtester/src/simulator.rs#L972)
from that unchanged curve. Final value includes the exit fees and slippage.

Reproduction: start at 1,000, buy 100 notional with 2% fees and flat prices, then
force-close at the end. `final_value=996`, but the final equity point is `998`.

Repair: finalize liquidation accounting and update terminal equity before
computing metrics. Ensure the reported period, final cash and equity curve agree.

### R16 — CI's Node version cannot execute the new test script

[The workflow](../.github/workflows/ci.yml#L149) installs Node 20.
[The test script](../dashboard/package.json#L10) invokes
`node --experimental-strip-types --test`.

Reproduction with Node **20.20.2** exits with code 9 and
`node: bad option: --experimental-strip-types`. The same three tests pass on the
local Node 24 runtime. This mismatch was introduced in commit `05c1d01`.

Repair: align CI and local supported runtimes, or transpile TypeScript for the
existing runtime. Verify the exact CI command under the selected version.

### R17 — Market discovery silently stops at 100 results

[MarketsPage](../dashboard/app/markets/page.tsx#L40) fetches `limit: 100`, does not
request subsequent offsets, and performs search only over those returned rows.
The backend now supplies a bounded, stable ID-ordered page. A matching market
outside the first page is invisible to search, and the displayed count looks like
the complete market count.

Repair: implement server-side search and paginated browsing, with a total or
clear loaded-count indication. Test a matching market beyond the first page.

### R18 — CI Clippy does not cover all workspace packages

[CI runs Clippy without `--workspace`](../.github/workflows/ci.yml#L87). This
repository has a root package as well as workspace members. Cargo metadata reports
`workspace_default_members = ["ab-bot"]`; the API server, arb monitor and other
members are not all selected for their own lint targets.

Repair: use the same explicit workspace scope as local validation:
`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`.
The test job's `cargo test --all` already selects the workspace.

## Original review verification and evidence

- Re-ran the existing workspace suite: **471 passed, 0 failed, 13 ignored**.
- Re-ran the existing dashboard unit suite on local Node 24: **3 passed**.
- Temporary position/exit diagnostic module: **1 control passed, 8 assertions
  failed**. Seven failures establish defects; the eighth exercises R06's
  threshold-unit assumption, which remains explicitly qualified.
- Temporary backtest diagnostic module: **2 assertions failed**, reproducing R14
  and R15 against the simulator and isolated TimescaleDB.
- Actual dashboard auth-store execution showed **zero server logout requests**.
- The exact type-stripping flag failed under **Node 20.20.2**.
- Cargo metadata confirmed the default package-selection gap.
- Temporary diagnostic module declarations were removed, and production source
  was restored byte-for-byte. Existing ignored tests were not relabeled as passed.

The isolated harnesses and full output remain in
`/workspace/ab-bot-review-work/`:

- `correctness_review_tests.rs`
- `backtest_correctness_review_tests.rs`
- `second-review-baseline-tests.log`
- `second-review-reproductions.log`
- `second-review-backtest-reproductions.log`
- `second-review-dashboard-tests.log`
- `second-review-dashboard-logout.log`
- `second-review-cargo-metadata.json`

The harnesses assert intended behavior and intentionally fail on the reviewed
commit. They were temporarily included under their owning Rust module to exercise
private lifecycle methods. Promote the relevant cases into permanent regression
tests as each repair is implemented.

## Original recommended sequence

1. Repair R01–R05 together at the design level, then implement and validate each
   separately: execution provenance, explicit holdings, partial fills, durable
   cost basis, and exclusive transitions.
2. Complete reservation-to-fill and uncertain-submission recovery (R07–R08).
   Reject unsupported protective orders immediately (R09).
3. Close private-data and session gaps (R10, R12, R13), and make reconciliation
   tolerate legacy data (R11).
4. Set the threshold-unit contract (R06), fix point-in-time backtest data and
   terminal accounting (R14–R15), then complete dashboard pagination (R17).
5. Repair CI runtime and workspace coverage (R16, R18); these are small changes
   suitable for an early verification-focused commit.

Deployment still needs the migration-history procedure already documented in
[DEPLOY.md](DEPLOY.md), especially for databases that applied the original
migration 068. This review did not validate an upgrade against a production
database snapshot or measure production traffic.
