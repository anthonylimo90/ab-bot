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
- `crates/api-server/src/copy_trading.rs` - Copy trade signal evaluation pipeline (market filter, staleness, skip reasons)
- `crates/api-server/src/auto_optimizer.rs` - Wallet rotation and demotion engine (fill rate, drawdown, losses)
- `crates/api-server/src/dynamic_tuner.rs` - Dynamic config tuning with watchdog (adaptive relaxation, Redis pub/sub)
- `crates/api-server/src/handlers/workspaces.rs` - Workspace settings API (dynamic config bounds, circuit breaker overrides)
- `crates/trading-engine/src/copy_trader.rs` - Copy trade execution (near-resolution floor, slippage, order placement)
- `migrations/` - SQLx migrations (33 files)
- `docker-compose.yml` - Postgres (TimescaleDB), Redis, API, arb-monitor (profiles: `full`, `monitoring`)
- `.env.example` - All environment variables with defaults

## Key Patterns

**Arbitrage entry signal:** `yes_ask + no_ask < 0.98` (profitable after 2% fees)

**Position states:** PENDING -> OPEN -> EXIT_READY -> CLOSING -> CLOSED

**Bot detection:** 50+ points = likely bot (trade interval, win rate, latency, 24/7 activity)

**Demotion triggers (immediate):** 5+ consecutive losses, drawdown > 25%, circuit breaker trip, 0% copy fill rate (≥10 attempts, 0 fills in 24h)

**Demotion triggers (grace period):** ROI < -3% for 48h, Sharpe < 0.5 for 24h

**Copy trade skip reasons:** `market_not_active`, `too_stale`, `below_minimum`, `near_resolution`, `SlippageTooHigh`, `market_cache_empty`, `circuit_breaker`, `insufficient_capital`, `max_positions_reached`, `duplicate`

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

# Copy Trading
COPY_TRADING_ENABLED=false             # Opt-in (default false)
COPY_TOTAL_CAPITAL=10000               # Total capital for copy trading
COPY_MIN_TRADE_VALUE=0.50              # Minimum copy trade value ($0.50)
COPY_MAX_SLIPPAGE_PCT=0.05             # Max slippage tolerance (5%)
COPY_MAX_LATENCY_SECS=120              # Max trade staleness (seconds)
COPY_NEAR_RESOLUTION_MARGIN=0.03       # Near-resolution price filter (3% floor enforced)
COPY_DAILY_CAPITAL_LIMIT=5000          # Daily capital limit
COPY_MAX_OPEN_POSITIONS=15             # Max simultaneous positions
COPY_STOP_LOSS_PCT=0.15               # Stop-loss threshold (15%)
COPY_TAKE_PROFIT_PCT=0.25             # Take-profit threshold (25%)
COPY_MAX_HOLD_HOURS=72                # Max position hold time (hours)
AUTO_ROTATION_INTERVAL_SECS=900        # Auto-optimizer cycle interval (seconds)
TRADE_MONITOR_MAX_AGE_SECS=120         # Max trade signal age (seconds)

# Arb Monitor
ARB_MAX_SIGNAL_AGE_SECS=30             # Max arb signal age
ARB_CACHE_REFRESH_SECS=300             # Market cache refresh interval
CLOB_MARKET_LIMIT=200000               # Market cache pagination limit
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

**Zero copy trade fills (market_not_active):**
- Check `active_set_size` in skip logs — should be 500K+ entries
- Watchdog logs every 5 min with `attempts`/`fills`/`top_skip_reason`
- Market cache refreshes every 5 min from CLOB API (`/markets?active=true`)
- If all skips are `market_not_active`, monitored wallets may be trading resolved markets
- Watchdog applies adaptive relaxation when 0 fills detected (relaxes latency, slippage, min value)

**Near-resolution margin DB value = 0:**
- Backend enforces 3% floor regardless (`copy_trader.rs` `MIN_MARGIN_RAW=300`)
- Dynamic tuner clamps DB value 0→0.03 at startup via `clamp_dynamic_value`
- Workspace API now rejects values < 0.03 (bounds: 0.03–0.25)

**CLOB API 429 rate limits:**
- Retry logic with exponential backoff: 2s → 4s → 8s
- Logs: `CLOB API returned 429, retrying in Xs`
- If persistent, reduce `CLOB_MARKET_LIMIT` or increase `ARB_CACHE_REFRESH_SECS`

## External Docs

- Polymarket CLOB: https://docs.polymarket.com
- Alchemy Polygon: https://docs.alchemy.com/reference/polygon-api-quickstart
