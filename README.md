# AB-Bot

A high-performance Polymarket trading platform built in Rust, featuring automated wallet selection, copy trading, arbitrage detection, and risk management.

## Features

### Core Trading
- **Copy Trading** - Mirror successful wallets with configurable allocation strategies
- **Arbitrage Monitor** - Real-time detection of mispriced prediction markets
- **Risk Management** - Advanced stop-loss strategies, circuit breakers, and adaptive risk appetite (Conservative/Balanced/Aggressive presets)
- **Live Wallet Integration** - EIP-712 signing for real order execution on Polymarket
- **MetaMask / WalletConnect** - Connect browser wallets for authentication and trade signing
- **Demo Mode** - Paper trading with simulated capital ($10,000 default)

### Automated Wallet Management
- **Auto-Select** - Automatically fills Active roster with best-performing wallets
- **Auto-Drop** - Demotes wallets that fail performance thresholds (immediate or grace period)
- **Auto-Swap** - Replaces underperformers with better candidates from the pool
- **Inactivity Demotion** - Wallets with 0% copy fill rate (≥10 attempts, 0 fills in 24h) are immediately demoted
- **Event-Driven Rotation** - Position closes and circuit breaker trips trigger immediate optimizer runs (15-min cycle + events)
- **Confidence-Weighted Allocation** - Higher allocation to wallets with higher prediction confidence
- **Probation System** - New wallets start at 50% allocation for 7 days
- **Pin/Ban Support** - User overrides for automation behavior

### Wallet Discovery & Analysis
- **CLOB Trade Harvester** - Background service that discovers wallets from live Polymarket trades, aggregates per-wallet statistics, and accumulates data across cycles
- **Bot Scanner** - Identify automated trading wallets through behavioral analysis
- **Wallet Discovery** - Find and track top-performing wallets with success predictions (works without Polygon RPC using CLOB data)
- **Success Metrics** - ROI, Sharpe ratio, win rate, drawdown tracking
- **Demo P&L Simulator** - See potential returns from copy trading strategies

### Wallet & Key Management
- **Self-Serve Wallet UI** - Connect and manage multiple wallets from the Settings page
- **Encrypted Key Vault** - Secure private key storage with AES encryption
- **Hot Wallet Reload** - Swap the active trading wallet without restarting the server
- **Primary Wallet Selection** - Designate which vault wallet is used for live trading

### Multi-Tenant Workspaces
- **Workspaces** - Isolated trading environments with separate budgets and rosters
- **Role-Based Access** - Owner, Manager, Trader, Viewer roles
- **Email Invites** - Invite team members to collaborate
- **Setup Wizard** - Guided onboarding for new workspaces

### Platform
- **Backtesting** - Historical simulation framework with realistic execution models
- **REST/WebSocket API** - Full-featured API with OpenAPI documentation
- **Dashboard** - Next.js frontend with Demo and Live trading modes

## Architecture

```
ab-bot/
├── crates/
│   ├── api-server/       # REST/WebSocket API (Axum)
│   ├── arb-monitor/      # Arbitrage detection engine
│   ├── auth/             # JWT, API keys, RBAC, audit logging
│   ├── backtester/       # Historical simulation framework
│   ├── bot-scanner/      # Wallet behavior analysis
│   ├── polymarket-core/  # Shared types and API clients
│   ├── risk-manager/     # Stop-loss and circuit breakers
│   ├── trading-engine/   # Order execution and copy trading
│   └── wallet-tracker/   # Wallet discovery and success prediction
├── dashboard/            # Next.js frontend
├── migrations/           # PostgreSQL/TimescaleDB migrations
└── docker/               # Docker configuration files
```

## Quick Start

### Prerequisites

- Docker & Docker Compose
- Node.js 18+ (for dashboard)
- Rust 1.85+ (for local development)

### Using Docker (Recommended)

```bash
# Clone the repository
git clone https://github.com/anthonylimo90/ab-bot.git
cd ab-bot

# Start all services
docker compose up -d

# Check status
docker compose ps
```

Services will be available at:
- **API Server**: http://localhost:3000
- **PostgreSQL**: localhost:5432
- **Redis**: internal Docker network only (not host-exposed by default)

### Running the Dashboard

```bash
cd dashboard
npm install
npm run dev
```

Dashboard will be available at http://localhost:3002

### Environment Configuration

Create a `.env` file in the project root:

```bash
# Database
POSTGRES_USER=abbot
POSTGRES_PASSWORD=abbot_secret
POSTGRES_DB=ab_bot

# API
JWT_SECRET=your-secret-key-here

# Redis ACL users (dynamic_tuner is sole writer for dynamic:config:update)
REDIS_URL=redis://app:<app_password>@redis:6379
DYNAMIC_TUNER_REDIS_URL=redis://dynamic_tuner:<tuner_password>@redis:6379
DYNAMIC_CONFIG_REDIS_URL=redis://dynamic_subscriber:<subscriber_password>@redis:6379

# External APIs
POLYMARKET_API_URL=https://clob.polymarket.com
POLYGON_RPC_URL=https://polygon-rpc.com

# Email (Resend)
RESEND_API_KEY=re_...

# Live Trading (optional)
WALLET_PRIVATE_KEY=0x...  # 64-char hex private key for order signing
LIVE_TRADING=true         # Enable live order execution

# Wallet Harvester (optional)
HARVESTER_ENABLED=true            # Enable background wallet discovery
HARVESTER_INTERVAL_SECS=300       # Harvest cycle interval (default: 5 min)
HARVESTER_TRADES_PER_FETCH=200    # CLOB trades per cycle
HARVESTER_MAX_NEW_PER_CYCLE=20    # Max wallets to store per cycle

# Circuit Breaker (optional)
CB_MAX_DAILY_LOSS=2500
CB_MAX_DRAWDOWN_PCT=0.20
CB_MAX_CONSECUTIVE_LOSSES=8
CB_COOLDOWN_MINUTES=30
```

### Testing Wallet Connection

```bash
# Test that your wallet can connect to Polymarket
WALLET_PRIVATE_KEY=0x... cargo run --example test_wallet
```

## Automation System

The platform includes a fully automated wallet selection system that runs hands-off from day one.

### How It Works

1. **Auto-Select (Promotion)**
   - System automatically fills empty Active slots (max 5) with best candidates
   - Candidates ranked by composite score: ROI (30%) + Sharpe (25%) + Win Rate (25%) + Consistency (20%)
   - New wallets start in 7-day probation with 50% of target allocation

2. **Auto-Drop (Demotion)**
   - Wallets demoted to Bench when they fail thresholds
   - **Immediate triggers**: 5+ consecutive losses, drawdown > 25%, circuit breaker trip, 0% copy fill rate (24h)
   - **Grace period triggers**: ROI < -3% for 48h, Sharpe < 0.5 for 24h

3. **Confidence-Weighted Allocation**
   - Uses ensemble of 4 rule-based models (Statistical, Momentum, Risk-Adjusted, Behavioral)
   - Higher confidence = higher allocation (range: 10% to 35% per wallet)
   - Market regime adjustments (Bull/Bear, Calm/Volatile)

4. **User Overrides**
   - **Pin wallet**: Prevents auto-demotion (max 3 pins)
   - **Ban wallet**: Prevents auto-promotion
   - **Manual drop**: Instant, respected immediately

### Thresholds

**Promotion Criteria (must meet ALL):**
| Metric | Minimum |
|--------|---------|
| ROI 30d | ≥ 5% |
| Sharpe Ratio | ≥ 1.0 |
| Win Rate | ≥ 50% |
| Trade Count | ≥ 10 |
| Max Drawdown | ≤ 20% |

**Demotion Triggers:**
| Condition | Action |
|-----------|--------|
| 5+ consecutive losses | Immediate demote |
| Drawdown > 25% | Immediate demote |
| Circuit breaker trip | Immediate demote |
| 0% copy fill rate (24h, ≥10 attempts) | Immediate demote |
| ROI < -3% for 48h | Demote after grace period |
| Sharpe < 0.5 for 24h | Demote after grace period |

## Copy Trading Pipeline

The copy trading system evaluates each incoming wallet trade through a multi-stage filter pipeline:

1. **Minimum value check** — skip trades below `COPY_MIN_TRADE_VALUE` ($0.50 default)
2. **Latency check** — skip trades older than `COPY_MAX_LATENCY_SECS` (120s default, watchdog-driven)
3. **Active market filter** — skip trades for resolved/delisted markets via shared `active_clob_markets` set (refreshed every 5 min from CLOB API); blocks all trades when cache is not yet populated
4. **Circuit breaker** — respect trading halts
5. **Near-resolution filter** — skip trades where price < 0.03 or > 0.97 (3% hard floor, catches stub quotes from resolved markets)
6. **Policy checks** — daily capital limit, max positions, slippage tolerance
7. **Order execution** — place order via CLOB API

All skip reasons are tracked and visible in the dashboard:

| Skip Reason | Description |
|-------------|-------------|
| `market_not_active` | Market resolved or delisted |
| `too_stale` | Trade exceeded max latency |
| `below_minimum` | Below minimum trade value |
| `near_resolution` | Price too close to 0 or 1 |
| `SlippageTooHigh` | Slippage exceeds tolerance |
| `market_cache_empty` | Market cache not yet loaded |
| `circuit_breaker` | Circuit breaker tripped |
| `insufficient_capital` | Not enough capital |
| `max_positions_reached` | Position limit reached |

**Resilience features:**
- **429 retry** — CLOB API rate limits retried with 2s/4s/8s exponential backoff
- **Watchdog** — when 0 fills detected over the tuner window, adaptively relaxes latency, slippage, and min value thresholds

## Dynamic Configuration

All key trading parameters are dynamically tunable at runtime via the API or the auto-tuner. Changes propagate in real-time via Redis pub/sub.

| Parameter | Default | Range | Description |
|-----------|---------|-------|-------------|
| `COPY_MAX_LATENCY_SECS` | 120 | watchdog-driven | Max trade staleness |
| `COPY_NEAR_RESOLUTION_MARGIN` | 0.03 | 0.03–0.25 | Near-resolution price filter |
| `COPY_MIN_TRADE_VALUE` | $0.50 | $0.50–$50 | Minimum copy trade value |
| `COPY_MAX_SLIPPAGE_PCT` | 5% | 1%–15% | Max slippage tolerance |
| `COPY_TOTAL_CAPITAL` | $10,000 | $100–$500K | Total capital for allocation |
| `COPY_DAILY_CAPITAL_LIMIT` | $5,000 | — | Daily capital limit |
| `COPY_MAX_OPEN_POSITIONS` | 15 | — | Max simultaneous positions |
| `COPY_STOP_LOSS_PCT` | 15% | 5%–100% | Stop-loss threshold |
| `COPY_TAKE_PROFIT_PCT` | 25% | 5%–100% | Take-profit threshold |
| `COPY_MAX_HOLD_HOURS` | 72h | 1–720 | Max position hold time |
| `ARB_POSITION_SIZE` | $50 | $10–$500 | Arb position size |
| `ARB_MIN_NET_PROFIT` | 0.001 | 0.0005–0.05 | Min arb profit threshold |
| `ARB_MIN_BOOK_DEPTH` | $100 | $25–$1,000 | Min orderbook depth |
| `ARB_MAX_SIGNAL_AGE_SECS` | 30s | 5–300 | Max arb signal age |

**Dynamic Tuning API:**
- `PUT /api/v1/workspaces/:id/dynamic-tuning/copy-trading` — Update copy trading parameters
- `PUT /api/v1/workspaces/:id/dynamic-tuning/arb-executor` — Update arb executor parameters

## API Endpoints

### Health & Status
- `GET /health` - Health check
- `GET /ready` - Readiness check

### Authentication
- `POST /api/v1/auth/register` - Register new user
- `POST /api/v1/auth/login` - Login
- `POST /api/v1/auth/refresh` - Refresh token
- `GET /api/v1/auth/me` - Get current user
- `POST /api/v1/auth/forgot-password` - Request password reset
- `POST /api/v1/auth/reset-password` - Reset password

### Workspaces
- `GET /api/v1/workspaces` - List user's workspaces
- `GET /api/v1/workspaces/current` - Get current workspace
- `GET /api/v1/workspaces/:id` - Get workspace details
- `PUT /api/v1/workspaces/:id` - Update workspace settings
- `POST /api/v1/workspaces/switch/:id` - Switch to workspace
- `GET /api/v1/workspaces/:id/members` - List workspace members
- `GET /api/v1/workspaces/:id/optimizer-status` - Get automation status

### Invites
- `GET /api/v1/invites` - List pending invites
- `POST /api/v1/invites` - Create invite
- `DELETE /api/v1/invites/:id` - Revoke invite
- `GET /api/v1/invites/:token/info` - Get invite info (public)
- `POST /api/v1/invites/:token/accept` - Accept invite

### Allocations (Roster)
- `GET /api/v1/allocations` - List wallet allocations
- `POST /api/v1/allocations` - Add wallet to roster
- `PUT /api/v1/allocations/:address` - Update allocation
- `DELETE /api/v1/allocations/:address` - Remove from roster
- `POST /api/v1/allocations/:address/promote` - Promote to Active
- `POST /api/v1/allocations/:address/demote` - Demote to Bench
- `PUT /api/v1/allocations/:address/pin` - Pin wallet
- `DELETE /api/v1/allocations/:address/pin` - Unpin wallet
- `POST /api/v1/allocations/bans` - Ban wallet
- `DELETE /api/v1/allocations/bans/:address` - Unban wallet
- `GET /api/v1/allocations/bans` - List banned wallets

### Auto-Rotation
- `GET /api/v1/auto-rotation/history` - Get rotation history
- `POST /api/v1/auto-rotation/history/:id/acknowledge` - Acknowledge entry
- `POST /api/v1/auto-rotation/trigger` - Trigger optimization manually

### Dynamic Tuning & Risk
- `PUT /api/v1/workspaces/:id/dynamic-tuning/copy-trading` - Update copy trading config
- `PUT /api/v1/workspaces/:id/dynamic-tuning/arb-executor` - Update arb executor config
- `PUT /api/v1/workspaces/:id/risk/circuit-breaker/config` - Per-workspace circuit breaker overrides

### Onboarding
- `GET /api/v1/onboarding/status` - Get onboarding status
- `POST /api/v1/onboarding/mode` - Set workspace mode
- `POST /api/v1/onboarding/budget` - Set budget
- `POST /api/v1/onboarding/auto-setup` - Run auto-setup
- `POST /api/v1/onboarding/complete` - Complete onboarding

### Demo Positions
- `GET /api/v1/demo/positions` - List demo positions
- `POST /api/v1/demo/positions` - Create demo position
- `PUT /api/v1/demo/positions/:id` - Update demo position
- `DELETE /api/v1/demo/positions/:id` - Close demo position
- `GET /api/v1/demo/balance` - Get demo balance
- `PUT /api/v1/demo/balance` - Update demo balance
- `POST /api/v1/demo/reset` - Reset demo portfolio

### Markets
- `GET /api/v1/markets` - List markets
- `GET /api/v1/markets/:id` - Get market details
- `GET /api/v1/markets/:id/orderbook` - Get orderbook

### Positions
- `GET /api/v1/positions` - List positions
- `GET /api/v1/positions/:id` - Get position details
- `POST /api/v1/positions/:id/close` - Close position

### Wallets
- `GET /api/v1/wallets` - List tracked wallets
- `POST /api/v1/wallets` - Add wallet to track
- `GET /api/v1/wallets/:address` - Get wallet details
- `GET /api/v1/wallets/:address/metrics` - Get wallet metrics

### Trading
- `POST /api/v1/orders` - Place order
- `POST /api/v1/orders/:id/cancel` - Cancel order

### Discovery
- `GET /api/v1/discover/trades` - Live trades from CLOB API
- `GET /api/v1/discover/wallets` - Top-performing wallets leaderboard (real data from harvester)
- `GET /api/v1/discover/simulate` - Demo P&L simulation

### Vault & Wallet Management
- `POST /api/v1/vault/wallets` - Store wallet with encrypted private key
- `GET /api/v1/vault/wallets` - List connected wallets
- `GET /api/v1/vault/wallets/:id` - Get wallet details
- `DELETE /api/v1/vault/wallets/:id` - Remove wallet from vault
- `POST /api/v1/vault/wallets/:id/primary` - Set as primary trading wallet
- `POST /api/v1/wallet-auth/challenge` - Request wallet auth challenge
- `POST /api/v1/wallet-auth/verify` - Verify wallet signature
- `POST /api/v1/wallet-auth/link` - Link wallet to account
- `POST /api/v1/order-signing/prepare` - Prepare order for client-side signing

### Backtesting
- `POST /api/v1/backtest` - Run backtest
- `GET /api/v1/backtest/results` - List backtest results

### WebSocket Streams
- `WS /ws/orderbook` - Live orderbook updates
- `WS /ws/positions` - Position updates
- `WS /ws/signals` - Trading signals
- `WS /ws/all` - All streams combined

API documentation available at `/swagger-ui` when running.

## Dashboard Features

### Pages

- **Dashboard** - Portfolio overview, watchdog alert banner (zero-fill detection), and live activity feed
- **Trading** - Unified copy trading and portfolio management
  - Active tab: Active roster wallets with fill rate metrics (color-coded), grace period badges
  - Bench tab: Bench wallets ready for promotion
  - Positions tab: Manual position management with order form
  - History tab: Closed positions
  - Automation tab: Auto-optimizer controls, rotation history with pagination, inactivity demotion icons
- **Markets** - Browse active markets with live orderbook via WebSocket, search/filter
- **Signals** - Real-time trading signals with skip reason labels and breakdown analytics
- **History** - Trade copy history with human-readable skip reasons and breakdown pills
- **Discover** - Find top-performing wallets
- **Backtest** - Historical simulations
- **Settings** - Configuration, wallet management, and WalletConnect setup
- **Wallet Detail** (`/wallet/[address]`) - Per-wallet metrics with fill rate stat card, grace period badge, equity curve

### Demo Mode
- Paper trading with simulated capital ($10,000 default)
- Full backtesting capabilities
- No wallet connection required

### Live Mode
- Real trading connected to Polymarket
- Actual order execution
- Real P&L tracking

### Automation Panel
- ON/OFF toggles for auto-select and auto-demote
- Risk appetite presets: Conservative, Balanced, Aggressive
- Adjustable thresholds (ROI, Sharpe, Win Rate, Trades, Drawdown)
- Allocation strategies: Equal, Confidence-Weighted, Performance-Weighted
- Dynamic config sliders: latency, slippage, min trade value, near-resolution margin
- Live rotation history with pagination ("Load more"), inactivity demotion icons
- Manual trigger button for immediate optimization

### Allocation API Response
The `/api/v1/allocations` endpoint returns expanded wallet data including:
- `fill_rate_24h`, `fill_attempts_24h`, `fill_count_24h` — 24h copy trade fill rate
- `grace_period_started_at`, `grace_period_reason` — grace period status
- `pinned`, `pinned_at` — pin status
- `probation_until`, `probation_allocation_pct` — probation details
- `consecutive_losses`, `last_loss_at` — loss tracking
- `confidence_score` — prediction confidence

## Development

### Building from Source

```bash
# Build all crates
cargo build --all

# Run tests
cargo test --all

# Run specific service
cargo run -p api-server
cargo run -p arb-monitor
cargo run -p bot-scanner

# Format and lint
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

### Database Migrations

Migrations run automatically on first Docker start. For manual management:

```bash
# Run migrations
DATABASE_URL=postgres://abbot:abbot_secret@localhost:5432/ab_bot sqlx migrate run

# Generate SQLx offline cache (after modifying queries)
DATABASE_URL=postgres://abbot:abbot_secret@localhost:5432/ab_bot cargo sqlx prepare --workspace
```

## Deployment

### Railway (Recommended)

Deploy to [Railway](https://railway.app) using GitHub integration.

**Services:**

| Service | Dockerfile | Description |
|---------|------------|-------------|
| api-server | `Dockerfile` | REST/WebSocket API |
| arb-monitor | `Dockerfile.arb-monitor` | Market monitoring worker |
| dashboard | `Dockerfile.dashboard` | Next.js frontend |

**Quick Setup:**
1. Create Railway project with TimescaleDB and Redis
2. Enable uuid-ossp extension: `CREATE EXTENSION IF NOT EXISTS "uuid-ossp";`
3. Connect GitHub repo to create each service
4. Configure environment variables per service
5. Deploy

See [docs/DEPLOY.md](docs/DEPLOY.md) for complete step-by-step instructions.

**Configuration Files:**
- `railway.toml` - Service-specific build and deploy settings
- `Dockerfile.*` - Service-specific container builds
- `.dockerignore` - Build context optimization

## Tech Stack

### Backend
- **Rust** - Async runtime with Tokio
- **Axum** - Web framework
- **SQLx** - Database driver with compile-time checking
- **PostgreSQL + TimescaleDB** - Time-series database
- **Redis** - Caching and pub/sub

### Frontend
- **Next.js 15** - React framework
- **TypeScript** - Type safety
- **Tailwind CSS** - Styling
- **Zustand** - State management
- **TanStack Query** - Data fetching
- **Recharts** - Charting

## License

MIT

## Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit changes (`git commit -m 'feat: add amazing feature'`)
4. Push to branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

See [CLAUDE.md](CLAUDE.md) for detailed development guidelines.
