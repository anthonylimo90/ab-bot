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
- `trading-engine` - Order execution, copy trading
- `wallet-tracker` - Wallet discovery, profitability analysis

## Dashboard

Next.js 15 + React 19 app with TanStack Query v5, Zustand v5, Tailwind, Recharts, and RainbowKit (wallet connect).

## Commands

```bash
cargo build --all                # Build
cargo test --all                 # Test
cargo fmt --all                  # Format
cargo clippy --all-targets --all-features -- -D warnings  # Lint
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

- DO run `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings` before committing
- DO use conventional commits
- DO branch from `develop` for new work (`feature/`, `fix/`, `refactor/`)
- DO regenerate `.sqlx/` cache after modifying queries
- DON'T push directly to `main`
- DON'T commit `.env` files or secrets
- DON'T use interactive git flags (`-i`)

## Key Files

- `crates/api-server/src/main.rs` - Server entry point (Axum, Clap CLI with `serve`/`seed-admin`)
- `crates/api-server/src/state.rs` - App state, circuit breaker config
- `migrations/` - SQLx migrations (20 files)
- `docker-compose.yml` - Postgres (TimescaleDB), Redis, API, arb-monitor (profiles: `full`, `monitoring`)
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
LIVE_TRADING=true          # Enable live orders (optional)
CB_MAX_DAILY_LOSS=2500     # Circuit breaker daily loss limit (optional)
CB_MAX_DRAWDOWN_PCT=0.20   # Circuit breaker max drawdown (optional)
CB_MAX_CONSECUTIVE_LOSSES=8 # Circuit breaker consecutive losses (optional)
CB_COOLDOWN_MINUTES=30     # Circuit breaker cooldown minutes (optional)
SKIP_MIGRATIONS=           # Skip DB migrations on startup (optional)
```

## CI

GitHub Actions (`.github/workflows/ci.yml`) runs on push/PR to `main` and `develop`:
format, clippy, test, audit, dashboard lint/build, docker build. Uses `SQLX_OFFLINE=true`.

## SQLx Offline Mode

After modifying SQL queries, regenerate the cache:

```bash
DATABASE_URL=postgres://abbot:abbot_secret@localhost:5432/ab_bot cargo sqlx prepare --workspace
```

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

## External Docs

- Polymarket CLOB: https://docs.polymarket.com
- Alchemy Polygon: https://docs.alchemy.com/reference/polygon-api-quickstart
