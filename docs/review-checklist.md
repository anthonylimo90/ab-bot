# Repository review checklist

Baseline: `42d72d93d86bbd6699a96e1260f4ea63043d2c41`.

Issues retain the IDs from the initial review. Work proceeds in the order below, with related changes reviewed together where necessary. An item is complete only after its fix and relevant verification are recorded. All 17 findings now have local fixes and verification. Changes are on `fix/wallet-key-ownership`; no push or deployment has been made.

| Order | Issue | Priority | Problem | Status |
| --- | --- | --- | --- | --- |
| 1 | F02 | P1 | Wallet registration can overwrite another user's vault key | Fixed; 8 regression tests passed |
| 2 | F03 | P1 | Personal wallet changes replace the shared trading signer | Fixed; original 10 plus 2 rotation/restart regressions passed |
| 3 | F04 | P1 | Registered nonmembers can read and queue exits of positions | Fixed; 7 authorization regressions passed |
| 4 | F16 | P2 | Private WebSocket streams lack authentication | Fixed; API and dashboard checks passed |
| 5 | F05 | P1 | Undrained execution reports block after channel capacity | Fixed; 23 engine tests passed |
| 6 | F06 | P1 | Order retries can submit distinct trades | Fixed; concurrency/restart/timeout regressions passed |
| 7 | F07 | P1 | Resting limit orders are reported as fully filled | Fixed; fill and snapshot regressions passed |
| 8 | F08 | P1 | Order cancellation only changes local state | Fixed; signed HTTP cancellation and fill-race tests passed |
| 9 | F09 | P1 | Withdrawal retries can submit duplicate transfers | Fixed; server replay and browser intent tests passed |
| 10 | F01 | P1 | Migration 068 mixes TEXT and INTEGER in COALESCE | Fixed with F12; migration regressions passed |
| 11 | F12 | P1 | Migration 068 closes unresolved exposure by age alone | Fixed with F01; migration regressions passed |
| 12 | F10 | P1 | Manual order size displays shares but sends USDC | Fixed; amount and dashboard checks passed |
| 13 | F11 | P1 | Wallet-signed submissions omit exchange request authentication | Fixed; signature and authenticated HTTP regressions passed |
| 14 | F13 | P1 | Manual cash movements are recorded as realized P&L | Fixed; risk-manager suite passed |
| 15 | F14 | P1 | Exposure limits omit holdings in recovery states | Fixed; holdings/reservations/concurrency regressions passed |
| 16 | F15 | P2 | Deleted users retain access through existing JWTs | Fixed; persistent session regressions passed |
| 17 | F17 | P2 | Repeated backtest buys replace existing inventory | Fixed; all 33 backtester tests passed |

## Verification record

Final checks on 2026-10-19:

- `cargo test --workspace --locked`: **456 passed, 0 failed, 12 ignored** across all workspace packages, integration tests and documentation examples. One ignored test requires TimescaleDB and was run separately below; the remaining 11 are pre-existing environment-dependent tests/examples.
- Full migration chain **001–072** passed on an isolated TimescaleDB **2.17.2 / PostgreSQL 16** service, both through SQL execution and through SQLx's actual migration transactions. The dedicated `complete_schema_migrates_with_sqlx_transactions` regression passed separately.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`: **passed** on Rust 1.99.1. Existing source lint failures were repaired. Cargo still reports future-compatibility notices for third-party dependencies; these do not fail the check.
- Dashboard: TypeScript, ESLint, the browser retry/reload regression and the production build **passed**. The build retains existing third-party dependency warnings.
- `cargo fmt --all -- --check` and `git diff --check`: **passed**.
- PostgreSQL-backed regressions use isolated test databases and focused fixtures. The separate TimescaleDB regression additionally validates the full production migration chain. Changed application queries are runtime SQLx queries, so no query-macro offline cache is present or required.
- Baseline source snapshot: all 391 tracked files matched their GitHub blob hashes; Cargo manifests and dashboard dependency/lockfile checks passed before changes.
- Initial broad checks exhausted the environment's disk. Generated Cargo artifacts were cleaned with Cargo, test/debug symbols were disabled in the external verification runner, and all checks were rerun. Repository build profiles were not changed.
- No live orders, transfers, production migrations, pushes or deployments were performed. Exchange protocol regressions use local HTTP fixtures and public test keys.

## F02 Wallet key ownership

Acceptance criteria:

- A private key must derive the requested wallet address before it is stored.
- Adding a wallet must never overwrite another account's vault material.
- Concurrent registrations of the same address must have a single authorized outcome.
- Deleting wallet metadata must not remove a key referenced by another account.
- Legacy duplicate registrations must be handled without deleting their keys or balances.
- Regression checks cover mismatched keys and cross-user registration/deletion.

Implementation: registration proves control of the address, checks ownership globally, preserves existing signer material, and serializes metadata/vault mutations with database advisory locks. Deletion keeps a key while any other metadata row references it, including legacy mixed-case duplicates. CI now provides PostgreSQL for the new database regression tests.

Verification: eight focused regressions passed, then passed again in the 134-test API suite. They cover key mismatches, cross-account registration, concurrent registration, primary selection races, legacy shared deletion, mixed-case rows, existing vault bindings, and failed metadata insertion.

## F03 Shared trading signer isolation

Acceptance criteria:

- Adding, removing, and choosing a personal primary wallet never reload the shared trading signer.
- Startup never selects a trading signer from the most recently updated personal wallet.
- The selected vault or environment key must derive the configured trading address.
- An unrelated environment key cannot replace a missing or mismatched selected signer.
- Signer rotation is blocked while positions remain pending, open, recovering, or retain holdings.
- Exchange approvals use the same selected signer as order execution.
- Dashboard primary-wallet controls describe personal preferences and display the workspace trading wallet separately.

Implementation: personal wallet handlers only mutate account wallet metadata and vault references. The handler-facing signer activation method was removed. Startup selects `TRADING_WALLET_ADDRESS` or the canonical workspace's saved address; environment-only `WALLET_PRIVATE_KEY` configuration remains supported when no address is pinned. Missing/mismatched signer material leaves live execution unavailable. Operator configuration changes require a restart, with an exposure check before replacing a saved wallet. Settings and recovery displays now distinguish personal primary wallets from the bot trading wallet. See [deployment guidance](DEPLOY.md#trading-wallet-selection).

Verification: ten new regressions passed as part of the 134-test API suite. They cover personal preference isolation, configured startup selection, absent configuration, invalid addresses, permitted/blocked operator rotation, key identity, corrupt vault bindings, and environment-only compatibility. Dashboard typechecking and lint passed.

## F04 Shared position and order authorization

Acceptance criteria:

- Position list, summary, detail, and order status REST requests require current membership in the canonical trading workspace.
- Manual close, order placement, and cancellation require both platform trading permission and a workspace owner, admin, or member role; workspace viewers cannot trade.
- Membership in another workspace and platform administration alone grant no access to the shared trading book.
- Authorization runs before object lookups, request body processing, exchange calls, or state changes.
- Membership removal and role changes take effect on the next request, including requests using existing JWT claims.
- Missing or malformed identities and unknown workspace roles fail closed.

Implementation: all seven handlers require typed workspace access extractors. The extractors resolve membership from PostgreSQL on each request and enforce read or trading permissions before the handler executes. OpenAPI responses now document authorization failures. Personal wallet-signed orders remain a separate user-owned flow. Private WebSocket access is tracked separately as F16.

Verification: seven PostgreSQL-backed HTTP guard regressions passed in the **141-test API library suite**. They cover workspace roles, platform roles, nonmembers, other-workspace membership, permission revocation with existing claims, invalid identities/roles, and absence of a workspace. Denied requests never reach the test handler's database mutation. No exchange submission was used.

## F16 Private WebSocket authentication

Private positions, signals, trade-flow and multiplexed sockets require a valid JWT in an `authenticate` first frame within five seconds, followed by canonical workspace read authorization, before subscribing or sending any private data. Tokens stay out of URLs. Each private event rechecks token and membership; idle connections recheck every 15 seconds. Orderbook-only market data stays public. Dashboard clients send authentication automatically and close stale connections on token changes.

Verification: 142 API library tests passed, including token/membership/expiry/revocation coverage. Dashboard TypeScript passed. The isolated PostgreSQL container was restarted after the environment resumed; tests were rerun successfully.

## F05 Execution report backpressure

Replaced the undrained MPSC queue with optional broadcast telemetry. Execution returns the authoritative report directly; absent or lagging observers cannot delay fills or position bookkeeping.

Verification: all 23 trading-engine tests passed, including 5,000 limit executions with no observer and an undrained observer, each beyond the former 1,000-report capacity.

## F06 Durable submission and retry safety

API orders require and reserve a client intent key before execution. Replays return the same stored order; different payloads with the same key conflict. Live execution does not automatically rerun operations. Migration 069 adds a submission journal, including the exact signed payload and EIP-712 exchange identity, committed before posting. A submitting or unknown intent prevents further live submissions across replicas and restarts until exchange evidence has been reconciled. A transport timeout remains pending rather than being asserted rejected. Paper execution retains safe retries.

Verification: 144 API tests and 25 engine tests passed, including concurrent API reservations, payload conflicts, persisted replays, live timeout behavior, and an ambiguous submission fenced after executor recreation. No live orders were sent.

## F07 Confirmed fills and exchange identity

Accepted/live/delayed limit orders retain their exchange ID and zero confirmed fills. Matched responses require actual making/taking amounts before recording quantities, average prices or fees. Partial fills stay partial. Migration 070 stores exchange identity and execution provenance; historical provenance remains unknown. Order status reads reconcile cumulative exchange fills without assuming the limit price was the average fill price. Reconciliation rejects mismatched identity, quantities or regressions.

Verification: 145 API tests and 26 engine tests passed, including resting/partial/full responses and monotonic exchange snapshots. FOK responses lacking fill evidence retain a reconciliation fence.

## F08 Exchange-confirmed cancellation

Live cancellation uses the authenticated `DELETE /order` API with the exact order-ID JSON body included in its HMAC. A successful HTTP status without acknowledgement of that ID is not a confirmed cancellation. The API reconciles the exchange snapshot after every attempt and preserves fills that win the cancellation race. Paper orders can be cancelled locally; unknown historical provenance and unresolved identities require reconciliation. The executor no longer claims local removal cancelled a live order.

Verification: API, engine and core suites passed. A local HTTP fixture verifies the method, path, exact body and L2 signature, and rejects missing/refused acknowledgements. Snapshot regression covers a fully filled order concurrent with cancellation.

## F09 Withdrawal retry identity

Withdrawals now require a client UUID, and retries verify the original owner, amount and destination. The dashboard persists the intent key through errors/reloads and clears it only after confirmation. Pending/submitted results are described as pending. Ambiguous broadcast and receipt timeouts retain unresolved status and the same idempotency fence. Reserve queries fail closed on database errors.

Verification: the 145-test API suite passed, followed by the added withdrawal replay regression. Dashboard TypeScript and the Node retry/reload intent regression passed.

## F01/F12 Preserve unresolved positions

Migration 068 no longer closes positions based on age or writes an integer into TEXT failure details. It deliberately leaves reconciliation to exchange/on-chain evidence. Migration 071 restores any closed position with positive explicit holdings to Stalled and visible reconciliation, preserving quantities, cost basis, P&L and failure details.

Verification: PostgreSQL regressions apply 068 to TEXT failure reasons across stale failure/recovery states and prove none disappear. They apply/rerun 071 and prove exposed closed rows reopen while flat closed rows remain closed. Separate migration checks cover durable intents and historical order provenance.

## F10 Manual order share units

Manual order size consistently means outcome shares for BUY and SELL. Signed amounts use shares × price for USDC, summaries use the same notional, and UI labels distinguish sale proceeds from potential profit. Share quantity precision and maximum size are validated.

Verification: BUY/SELL/fractional-share amount regressions and dashboard TypeScript passed.

## F11 Wallet-specific exchange authentication

Browser wallets sign a fresh ClobAuth proof. The backend verifies its signer/freshness, derives credentials for that wallet, and submits the signed order with the exact L2 HMAC and API-key owner. No shared bot credentials or server private key are substituted. Submission uses the durable journal, scoped to the wallet, and acknowledged results can be replayed. The client retains signed intent through errors/reloads. Related blockers were fixed: safe 53-bit salts, canonical EIP-712 address padding/domains, exchange-derived contract/fee metadata, bounded expiry, and GTD submission matching the signed expiration.

Verification: 150 API, 95 core and 26 engine tests passed. Known public test keys verify standard/neg-risk order recovery, auth proof identity/freshness and tamper rejection. A local HTTP fixture verifies wallet identity, owner, exact body and L2 signature. Dashboard TypeScript passed. The linker ran out of disk during an extra standalone test; generated crate artifacts were cleaned with Cargo and the combined suite reran successfully.

## F13 Realized P&L accounting

Manual fill cash movement no longer feeds circuit-breaker profit/loss or win/loss counters. Buying inventory is not a realized loss; selling gross proceeds is not a realized profit. The cost-basis-aware position exit paths remain the source of realized results.

Verification: all 32 risk-manager tests passed. Subsequent API suites also passed after removal of the invalid booking path.

## F14 Exposure and atomic entry reservations

Exposure counts explicit holdings in every state (including failed, stalled and incorrectly closed rows), pending position cost, remaining buy orders, and unresolved signed buy submissions. Pending positions retain the full prospective reservation while legs fill. Missing price information is conservatively valued. Query errors propagate and arbitrage skips trading. Arbitrage and quant create their pending positions under a shared PostgreSQL advisory lock, checking current plus prospective exposure atomically; other entry/order insertions participate in the same lock.

Verification: 150 API and 97 core tests passed. Regressions cover recovery/closed holdings, partial/pending reservations, query failures, and concurrent arbitrage/quant entries where only one fits the cap.

## F15 Persistent account/session revocation

Every protected HTTP request and private stream event validates the account's existence, current role, credential generation, expiry and persistent logout revocation. Migration 072 adds account generations and a shared revocation table. Credential resets, admin password changes and role changes invalidate earlier sessions. New password/wallet login and refresh tokens carry the current generation. Password reset token reuse is rejected atomically. Admin role updates now bind their database type correctly.

Verification: 152 API tests and 42 auth tests passed (two existing environment-dependent auth tests remain ignored). Regressions use independently recreated JWT validators to prove deleted accounts, changed credentials/roles and logged-out sessions cannot keep access. Private stream regression also passed with persistent session validation.

## F17 Repeated backtest inventory

Repeated buys accumulate shares and use a quantity-weighted entry price while preserving the first entry time. Existing inventory counts against the position cap. Closing settles all entry lots, includes entry and exit fees in realized P&L, records one completed round trip, and avoids double-counting entry rows in trade statistics. Strategy fill callbacks receive the actual exit price on closes.

Verification: all 33 backtester tests passed. The new YES/NO repeated-entry regression checks shares, weighted basis, cash, valuation, fees, every closed lot and one round-trip win.

## Integration repairs

- A failed API preflight marks its reserved order rejected only if no durable exchange submission exists; an ambiguous submitted intent remains pending.
- Signer rotation checks ambiguous and resting live orders as well as positions. A permitted startup selection is pinned under the shared database locks, so later restarts recognize that signer after new exposure is opened.
- REST share precision and supported time-in-force values match execution; limit FOK requests reach the FOK exchange path.
- Private stream clients report connected only after subscription acknowledgement and stop automatic retries after authentication rejection.
- CI supplies PostgreSQL and runs the browser intent regression.
- Baseline compiler/lint repairs include descending sort keys, duplicate chart logic, token-cache visibility, boxed large WebSocket/error payloads, and localized allowances for generated async-trait attributes and existing dependency-wiring argument lists.

All 17 reviewed findings have local fixes. Integrated checks passed. See [deployment compatibility and reconciliation notes](DEPLOY.md#review-fix-migrations-and-client-compatibility) before rollout.

## Follow-on efficiency work

All nine improvements in [efficiency-review.md](efficiency-review.md) are implemented
locally. The latest integrated verification passed 471 workspace tests, strict
Clippy, SQLx migrations through 075 and the dashboard production build.
