# Simplification and efficiency improvements

Completed locally on 2026-10-19: **9/9 improvements implemented**. This work builds
on the 17 fixes in [review-checklist.md](review-checklist.md). Final integration
verification passed; checks are recorded below.

| ID | Completed change | Verification |
| --- | --- | --- |
| E01 | Combine authorization database reads | Session and workspace revocation tests; one joined read per private event |
| E02 | Share the market catalog and refresh owner | Concurrent consumers make one catalog request; failed refresh retains data |
| E03 | Paginate a maintained markets projection | Transactional publication, inactive history, filtering and pagination tests |
| E04 | Coordinate inventory scans and preserve unknown balances | Cross-replica sharing, failed-read retention, atomic writes and unit conversion tests |
| E05 | Restrict exposure aggregation to contributing rows | All-state equivalence, competing reservations and synthetic query/write measurements |
| E06 | Coalesce orderbook refreshes and fetch outcomes concurrently | Browser burst/slow-read/disposal tests; HTTP concurrency barrier and one-side failure |
| E07 | Fetch closed positions by displayed page | 600 tied-timestamp records paginate without duplication; close-row decoding |
| E08 | Consolidate WebSocket transport | Real socket authentication, revocation, ping, lag/resume, public access and EOF tests |
| E09 | Prepare backtest input once | Same-timestamp context, fills, fees, equity, fresh strategy state and preparation benchmark |

## E01 — Authorization

[workspace_scope.rs](../crates/api-server/src/workspace_scope.rs) combines persistent
session state and canonical workspace membership into one database query for
private stream events. HTTP membership/flag lookups each use one query too.
JWT validation, strict expiry, current roles, account generations, persistent
logout revocation and authorization before every private event remain enforced.
No authorization TTL was added.

## E02 — Shared market catalog

[market_catalog.rs](../crates/api-server/src/market_catalog.rs) owns the shared
mapping used by arb, quant, exits and API reads. Arb owns the scheduled refresh at
the faster legacy arb/quant cadence, bounded to at least one second. Simultaneous
refreshes reuse one result. Targeted hydration runs during full downloads; newer
targeted observations survive publication. Refresh failures retain the last data.

## E03 — Markets pagination

[storage.rs](../crates/api-server/src/market_catalog/storage.rs) transactionally
publishes the existing markets table and a complete-publication timestamp. The
[markets handler](../crates/api-server/src/handlers/markets.rs) filters and paginates
that projection, with stable ID ordering and a maximum page size of 500. Requests
perform no full-catalog upstream download. Inactive historical rows remain
queryable. A cold catalog returns 503, and responses expose
`X-Catalog-Observed-At` and `X-Catalog-Stale` headers.

## E04 — Inventory reconciliation

[refresh.rs](../crates/api-server/src/wallet_inventory/refresh.rs) serializes scans
locally and across replicas. Overlapping requests share their completed result;
normal reads may reuse a successful scan for 30 seconds. Forced recovery refreshes
reuse only scans that complete after the request starts. Independent token balance
reads have concurrency bounded to four.

[wallet_inventory.rs](../crates/api-server/src/wallet_inventory.rs) requires every
balance read to succeed before publication. An unavailable, invalid or negative
balance cannot become a confirmed zero. The
[inventory repository](../crates/polymarket-core/src/db/inventory.rs) publishes all
batches in one transaction; a later batch failure preserves the old snapshot.
Only explicit observed balances determine zero holdings.

This also fixes an existing units error: CLOB collateral and conditional balances
are six-decimal base units. Shared conversion now supplies inventory, account
valuation and live preflight checks. An unavailable live balance preflight stops
submission.

Recovery previews read saved snapshots and show refresh/failure status. The
explicit refresh action and recovery runs perform coordinated fresh scans. Orphan
recovery requires a recent successful scan and uses a separate advisory lock to
serialize competing recovery callers.

## E05 — Exposure aggregation

[positions.rs](../crates/polymarket-core/src/db/positions.rs) retains the exact
exposure expression, order/intent reservations and entry locking. The positions
aggregate now selects `state = 0 OR held_yes_qty > 0 OR held_no_qty > 0`.
Migration 075 supplies a matching covering partial index. Pending reservations
and held inventory in closed, failed or recovery states remain included.

A seven-run local PostgreSQL 16 experiment used 250,000 exited rows and 128
mixed lifecycle/price/quantity cases, with order and unknown-intent reservations.
Totals matched exactly. The scan read 250,128 positions before and 100 after;
median aggregate time was 195.173 ms versus 0.307 ms. A separate seven-run experiment
of 1,000 active-row inserts measured median 0.728 ms without the index and 3.275 ms
with it. All experiment objects were temporary and rolled back.

## E06 — Orderbook refreshes

[coalesced-refresh.ts](../dashboard/lib/coalesced-refresh.ts) bounds notifications
to a one-second cadence, prevents overlapping reads and retains a trailing refresh.
[MarketDetailSheet.tsx](../dashboard/components/markets/MarketDetailSheet.tsx) uses
it for matching market events and reconnect catch-up, with slower live polling and
faster disconnected polling. Market changes cancel queued notifications. Full
orderbook depth is retained. The API uses cached token metadata and fetches YES
and NO books concurrently, preserving a healthy side when its peer fails.

## E07 — Position history

The [positions page](../dashboard/app/positions/page.tsx) requests 25 closed rows
using limit/offset, only while the Closed tab is active. It retains previous-page
data during loading, uses the summary count, resets on new closes and clamps pages
when counts shrink. SQL timestamp ordering has an ID tie-breaker and bounded inputs.

The database regression exposed an existing PostgreSQL type-widening bug:
`COALESCE(smallint_column, 0)` returns an integer. Explicit smallint casts fix
exit-strategy decoding in list/detail requests and source decoding in manual closes.

## E08 — WebSocket transport

[websocket.rs](../crates/api-server/src/websocket.rs) uses one driver for all five
routes. Public/private selection is explicit; private receivers are created only
after first-frame authentication. The driver handles ping, lag, clean EOF and idle
revocation uniformly. Revocation closes with code 1008. Actual
[socket protocol tests](../crates/api-server/src/websocket/socket_tests.rs) cover all
private channels, public access without auth tables, lag/resume and disconnection.
Serialization remains per recipient; further fanout infrastructure needs CPU evidence.

## E09 — Backtest preparation

[simulator.rs](../crates/backtester/src/simulator.rs) owns one chronologically
ordered snapshot vector and indexes timestamps with lightweight ranges. Preparation
requires no snapshot clone, hash grouping or timestamp sort. Strategy comparisons
load/prepare once and give each strategy fresh simulation state. Position marking
uses market lookups, and database rows are consumed during decoding.

The deterministic regression preserves complete same-timestamp market context,
fill callbacks, fees, equity and independent strategy results. A five-run debug
benchmark of 100,000 snapshots measured median preparation at 76.931 ms before and
9.880 ms after. It removes 24.8 MB of cloned snapshot structs plus market ID storage.
Out-of-order and empty input fail explicitly.

All runtime measurements above are synthetic local experiments, not production
latency, memory or throughput estimates.

## Integration verification

- Full workspace: **471 passed, 0 failed, 13 ignored**, across unit, integration
  and documentation suites. Existing environment-dependent/example skips remain;
  the full migration check and preparation benchmark were run separately.
- Strict Clippy: `--workspace --all-targets --all-features --locked -- -D warnings`
  passed after six mechanical lint repairs in the new catalog/inventory code.
- Complete SQLx migration run: **001–075 passed** against the isolated local
  TimescaleDB service, including SQLx transaction behavior and latest-version check.
- Dashboard: production build (24 pages), ESLint, TypeScript and **three unit tests**
  passed. Existing optional wallet SDK/dependency notices remain nonblocking.
- Rust formatting, Markdown source links and `git diff --check` passed.

The backtest benchmark can be rerun with
`cargo test -p backtester --lib benchmark_sorted_preparation -- --ignored --nocapture`.
The migration test requires an isolated TimescaleDB database URL and can be run with
`cargo test --workspace complete_schema_migrates_with_sqlx_transactions -- --ignored`.
No live orders, transfers, production migrations or publishing were performed.

## Deployment

Apply migrations 073–075 before running the updated API: catalog publication
metadata, inventory refresh status, and the exposure-candidate index. Existing
migration-history and client compatibility instructions remain in
[DEPLOY.md](DEPLOY.md). Changes are local and unpublished.

Further candidates from the review, outside the nine items above:

- Trade-flow journeys: the history lookup joined on a text expression and so
  read all of `trade_events` on every poll; it now looks journeys up by their
  signal and position uuid indexes (same rows, checked against the previous
  query on randomized data), and the summary reads position history once.
- Wallet bundle: wagmi, RainbowKit and WalletConnect load only on `/login`,
  the one page with wallet sign-in; the root layout's scripts fell from 371 to
  182 KiB gzipped for every other route. The chart library is already loaded
  only by `/signals`, the one page that draws charts with it.
- The unused snapshot batch insertion helper (and the other backtester data
  writers, which nothing calls) are removed.
- Worker reads: the only concurrent fan-out of reads, inventory balance
  reads, is bounded to four; none is unbounded.
