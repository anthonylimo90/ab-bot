# Polymarket Scanner System

Rust workspace for bot detection and arbitrage monitoring on Polymarket.

## Crates

- `api-server` - REST/WebSocket API (Axum, port 3000)
- `arb-monitor` - Arbitrage detection and position tracking
- `auth` - JWT, RBAC, API keys, audit logging
- `backtester` - Historical backtesting with TimescaleDB
- `bot-scanner` - Wallet behavior analysis and bot detection
- `polymarket-core` - Shared types, API clients, database models
- `risk-manager` - Stop-loss, circuit breaker
- `trading-engine` - Order execution, position management
- `wallet-tracker` - Wallet discovery, profitability analysis

## Dashboard

Next.js 15 + React 19 app with TanStack Query v5, Zustand v5, Tailwind, Recharts, and RainbowKit (wallet connect).

## Commands

```bash
cargo build --all                # Build
cargo test --all                 # Test
cargo fmt --all                  # Format
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings  # Lint
cargo run -p arb-monitor         # Run specific crate
docker compose up -d             # Start services
cd dashboard && npm run dev      # Run dashboard (Next.js, port 3000)
```

## Commit Format

```
<type>(<scope>): <description>
```

**Types:** feat, fix, refactor, perf, test, docs, chore, ci
**Scopes:** arb, bot, core, db, api, dashboard

## Subagent Preferences

- Always use `sonnet` model for subagents (never haiku)

## Dos and Don'ts

- DO run `cargo fmt --all && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` before committing
- DO use conventional commits
- DO branch from `develop` for new work (`feature/`, `fix/`, `refactor/`)
- DO add a new numbered migration for schema changes; never edit an applied one
- DON'T push directly to `main`
- DON'T commit `.env` files or secrets
- DON'T use interactive git flags (`-i`)

## Key Files

- `crates/api-server/src/main.rs` - Server entry point (Axum, Clap CLI with `serve`/`seed-admin`)
- `crates/api-server/src/state.rs` - App state, circuit breaker config
- `crates/api-server/src/dynamic_tuner.rs` - Dynamic config tuning with watchdog (adaptive relaxation, Redis pub/sub)
- `crates/api-server/src/arb_executor.rs` - Arbitrage signal execution pipeline
- `crates/api-server/src/handlers/workspaces.rs` - Workspace settings API (dynamic config bounds, circuit breaker overrides)
- `crates/api-server/src/signals/` - Quant signal system (generators, resolution)
- `migrations/` - SQLx migrations, applied in order by the API server at startup (numbering has gaps)
- `crates/api-server/src/intent_reconciler.rs` - Resolves journaled live orders (`execution_intents`)
- `crates/api-server/src/withdrawals.rs` - Withdrawal capacity, journal and chain reconciler
- `docker-compose.yml` - Postgres (TimescaleDB), Redis, API, arb-monitor (profiles: `full`, `monitoring`); the API applies migrations
- `.env.example` - All environment variables with defaults

## Key Patterns

**Arbitrage entry signal:** `yes_ask + no_ask < 0.98` (profitable after 2% fees)

**Position states:** PENDING -> OPEN -> EXIT_READY -> CLOSING -> CLOSED

**Bot detection:** 50+ points = likely bot (trade interval, win rate, latency, 24/7 activity)

## Environment Variables

```bash
ENVIRONMENT=development    # development | staging | production
DATABASE_URL=              # PostgreSQL connection
REDIS_URL=                 # Redis connection
JWT_SECRET=                # Min 32 chars, required
ALCHEMY_API_KEY=           # Required for bot-scanner
POLYGON_RPC_URL=           # Polygon RPC endpoint
WALLET_PRIVATE_KEY=        # For live trading (optional)
LIVE_TRADING=false         # Enable live orders (optional)
CB_MAX_DAILY_LOSS=2500     # Circuit breaker daily loss limit (optional)
CB_MAX_DRAWDOWN_PCT=0.20   # Circuit breaker max drawdown (optional)
CB_MAX_CONSECUTIVE_LOSSES=8 # Circuit breaker consecutive losses (optional)
CB_COOLDOWN_MINUTES=30     # Circuit breaker cooldown minutes (optional)
SKIP_MIGRATIONS=           # Skip DB migrations on startup (optional)

# Arb Monitor
ARB_MAX_SIGNAL_AGE_SECS=30             # Max arb signal age
ARB_CACHE_REFRESH_SECS=300             # Market cache refresh interval
CLOB_MARKET_LIMIT=200000               # Market cache pagination limit
```

## CI

GitHub Actions (`.github/workflows/ci.yml`) runs on push/PR to `main` and `develop`:
format, clippy, test (Postgres), full migration chain (TimescaleDB), audit,
dashboard lint/test/build, and a build of every deployed Dockerfile.

## SQL

Queries are checked at runtime (`sqlx::query`, not the `query!` macros), so
there is no `.sqlx` cache to regenerate. Database-backed tests use
`#[sqlx::test(migrations = false)]` with the tables they need; run them with
`DATABASE_URL` pointing at a disposable Postgres. The full-schema test
(`migration_tests::complete_schema`, ignored by default) needs TimescaleDB.

## Troubleshooting

**Railway log rate limiting (500 logs/sec):**
- Set `RUST_LOG=api_server=info,tower_http=error,polymarket_core=warn,sqlx=warn`
- Avoid DEBUG level logging in production
- WebSocket errors should be WARN not ERROR level

**500 errors with 0ms latency:**
- Usually indicates middleware failure before handler runs
- `tower_governor` rate limiter needs `SmartIpKeyExtractor` behind proxies (Railway, etc.)
- Default `PeerIpKeyExtractor` fails when proxy IP != client IP

**Debugging API errors:**
- Request logging: `INFO api_server: Incoming request method=X uri=Y`
- JSON parse errors: `WARN api_server::handlers::auth: ... JSON parsing failed`
- 500 errors: `ERROR api_server::error: Internal server error`

**CLOB API 429 rate limits:**
- Retry logic with exponential backoff: 2s → 4s → 8s
- Logs: `CLOB API returned 429, retrying in Xs`
- If persistent, reduce `CLOB_MARKET_LIMIT` or increase `ARB_CACHE_REFRESH_SECS`

## External Docs

- Polymarket CLOB: https://docs.polymarket.com
- Alchemy Polygon: https://docs.alchemy.com/reference/polygon-api-quickstart
