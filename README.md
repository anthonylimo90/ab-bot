# AB-Bot

A Rust-based Polymarket trading platform focused on arbitrage execution, quantitative signal trading, manual order placement, risk controls, and research tooling.

## What The System Does Today

### Live trading paths
- **Arbitrage engine**: `arb-monitor` detects mispriced binary markets, `api-server` executes two-leg entries, and the exit handler closes or resolves positions.
- **Quant signal engine**: flow, mean reversion, resolution proximity, and cross-market generators emit `QuantSignal`s that the quant executor evaluates and trades.
- **Manual trading**: authenticated users can place orders directly and prepare Wallet / MetaMask signed orders through the API and dashboard.

### Supporting systems
- **Risk management**: circuit breaker, recovery logic, and manual risk controls per workspace.
- **Dynamic tuning**: runtime config updates over Redis, mainly for arbitrage thresholds, with quant base size support.
- **Wallet research**: wallet harvesting, profitability metrics, scoring, discovery, and recommendation endpoints for operator research.
- **Backtesting**: historical simulation for arbitrage, momentum, mean reversion, and grid strategies.
- **Dashboard**: Next.js UI for markets, positions, risk, signals, history, backtests, settings, and admin/workspace flows.

### What was removed
- Copy trading, tracked-wallet roster automation, auto-rotation, allocation management, and related copy-trading tables/configuration were removed from the live product.
- Recommendation endpoints still exist, but they are **advisory only** and do not drive a live wallet-following engine.

## Architecture

```text
ab-bot/
├── crates/
│   ├── api-server/       # REST/WebSocket API, executors, handlers, runtime wiring
│   ├── arb-monitor/      # Arbitrage opportunity detection and market monitoring
│   ├── auth/             # JWT, RBAC, wallet auth, key vault
│   ├── backtester/       # Historical simulation framework
│   ├── bot-scanner/      # Wallet behavior analysis
│   ├── polymarket-core/  # Shared types, DB repos, API clients
│   ├── risk-manager/     # Circuit breaker and stop logic
│   ├── trading-engine/   # Order execution helpers and position manager utilities
│   └── wallet-tracker/   # Wallet discovery, metrics, scoring, prediction
├── dashboard/            # Next.js frontend
├── docs/                 # Deployment and runtime state-machine docs
├── migrations/           # PostgreSQL / TimescaleDB migrations
└── docker/               # Local infrastructure config
```

## Runtime Flow

### Arbitrage
1. `arb-monitor` scans markets and emits `ArbOpportunity` events.
2. `api-server` validates freshness, depth, and guard rails in `arb_executor`.
3. If accepted, it buys both YES and NO legs and persists the position.
4. `exit_handler` owns exit execution and resolution handling.
5. Realized exit P&L is fed into the circuit breaker.

### Quant
1. Signal generators poll feature tables and emit `QuantSignal`s.
2. `quant_signal_executor` applies confidence, staleness, depth, dedup, and max-position checks.
3. It opens single-leg YES or NO positions and links them back to `quant_signals`.
4. `exit_handler` evaluates open quant positions for generic take-profit / stop-loss / max-hold exits.
5. Closed quant positions feed strategy performance snapshots and per-strategy risk state.

## Quick Start

### Prerequisites
- Docker and Docker Compose
- Rust 1.85+
- Node.js 18+ for the dashboard

### Local stack

```bash
git clone https://github.com/anthonylimo90/ab-bot.git
cd ab-bot
docker compose up -d
```

Services:
- API server: `http://localhost:3000`
- Dashboard: `http://localhost:3002`
- PostgreSQL: `localhost:5432`

### Dashboard

```bash
cd dashboard
npm install
npm run dev
```

## Environment

Create `.env` in the repo root:

```bash
# Database
POSTGRES_USER=abbot
POSTGRES_PASSWORD=abbot_secret
POSTGRES_DB=ab_bot

# API
JWT_SECRET=replace-me

# Redis
REDIS_URL=redis://app:<app_password>@redis:6379
DYNAMIC_TUNER_REDIS_URL=redis://dynamic_tuner:<tuner_password>@redis:6379
DYNAMIC_CONFIG_REDIS_URL=redis://dynamic_subscriber:<subscriber_password>@redis:6379

# External APIs
POLYMARKET_API_URL=https://clob.polymarket.com
POLYGON_RPC_URL=https://polygon-rpc.com

# Email
RESEND_API_KEY=re_...

# Live trading
WALLET_PRIVATE_KEY=0x...
LIVE_TRADING=true

# Wallet harvester
HARVESTER_ENABLED=true
HARVESTER_INTERVAL_SECS=300
HARVESTER_TRADES_PER_FETCH=200
HARVESTER_MAX_NEW_PER_CYCLE=20

# Circuit breaker
CB_MAX_DAILY_LOSS=2500
CB_MAX_DRAWDOWN_PCT=0.20
CB_MAX_CONSECUTIVE_LOSSES=8
CB_COOLDOWN_MINUTES=30
```

Wallet smoke test:

```bash
WALLET_PRIVATE_KEY=0x... cargo run --example test_wallet
```

## API Surface

### Public / demo
- `GET /health`, `GET /ready`
- `GET /api/v1/discover/trades`
- `GET /api/v1/discover/wallets`
- `GET /api/v1/discover/wallets/:address`
- `GET /api/v1/regime/current`
- `GET /api/v1/recommendations/rotation`
- `POST /api/v1/recommendations/:id/dismiss`
- `POST /api/v1/recommendations/:id/accept`
- invite acceptance endpoints
- WebSocket: `/ws/orderbook`, `/ws/positions`, `/ws/signals`, `/ws/all`

### Auth
- register, login, refresh, logout, current-user
- forgot/reset password
- wallet challenge / verify / link

### Markets / positions / trading
- markets list, detail, orderbook
- positions list, detail, manual close
- order placement, cancel, status
- order prepare / submit for wallet signing

### Wallets / vault
- wallet metrics and wallet trades
- vault wallet CRUD, primary wallet selection, balance lookup

### Workspaces / risk / activity
- workspace list, current workspace, detail, switch
- members, invites, member role updates
- service status, dynamic tuner status/history
- risk status, circuit-breaker trip/reset/config
- activity feed

### Signals / backtests / discovery
- recent quant signals
- flow features
- market metadata
- strategy performance snapshots
- backtest create, list, detail
- wallet discovery leaderboard and regime endpoint

### Admin
- admin user management
- admin workspace management

Swagger UI is available at `/swagger-ui`.

## Dashboard Pages

Current dashboard routes under `dashboard/app`:
- `/` overview
- `/activity`
- `/markets`
- `/positions`
- `/history`
- `/signals`
- `/backtest`
- `/risk`
- `/tuner`
- `/settings`
- `/settings/users`
- `/admin`, `/admin/users`, `/admin/workspaces`
- auth and invite flows

## Dynamic Configuration

Runtime config is distributed over Redis pub/sub.

Currently supported live knobs:
- `ARB_POSITION_SIZE`
- `ARB_MIN_NET_PROFIT`
- `ARB_MIN_BOOK_DEPTH`
- `ARB_MAX_SIGNAL_AGE_SECS`
- `QUANT_BASE_POSITION_SIZE`

Workspace config endpoints:
- `PUT /api/v1/workspaces/:workspace_id/dynamic-tuning/opportunity-selection`
- `PUT /api/v1/workspaces/:workspace_id/dynamic-tuning/arb-executor`
- `PUT /api/v1/workspaces/:workspace_id/risk/circuit-breaker/config`

## Development

```bash
cargo build --all
cargo test --all
cargo run -p api-server
cargo run -p arb-monitor
cargo run -p bot-scanner
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

### Migrations

```bash
DATABASE_URL=postgres://abbot:abbot_secret@localhost:5432/ab_bot sqlx migrate run
DATABASE_URL=postgres://abbot:abbot_secret@localhost:5432/ab_bot cargo sqlx prepare --workspace
```

## Deployment

Railway is the primary target:
- `Dockerfile` for `api-server`
- `Dockerfile.arb-monitor` for `arb-monitor`
- `Dockerfile.dashboard` for `dashboard`

See [docs/DEPLOY.md](docs/DEPLOY.md) for deployment steps.

## Known Limitations

- Quant exits now use a generic risk-managed close policy; strategy-specific exit logic is still future work.
- Recommendation / rotation endpoints are advisory only and do not operate a live wallet-following engine.
- Arbitrage backtests were moved closer to live behavior, but they are still not a full live-parity replay of the production execution stack.
- Some legacy handlers and comments still reference removed copy-trading schema and should be cleaned up in a later pass.

## Tech Stack

### Backend
- Rust
- Tokio
- Axum
- SQLx
- PostgreSQL + TimescaleDB
- Redis

### Frontend
- Next.js 15
- TypeScript
- Tailwind CSS
- Zustand
- TanStack Query
- Recharts

## License

MIT
