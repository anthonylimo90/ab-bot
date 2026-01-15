# Polymarket Scanner System

Bot Detection & Arbitrage Monitoring Platform built in Rust.

## Project Overview

Dual-purpose Polymarket analysis system:
- **Arb Monitor**: Detects mispriced prediction markets in real-time with position lifecycle tracking
- **Bot Scanner**: Identifies automated trading wallets through behavioral pattern analysis

## Workspace Structure

```
ab-bot/
├── crates/
│   ├── api-server/      # REST/WebSocket API with OpenAPI docs (Axum)
│   ├── arb-monitor/     # Arbitrage detection and position tracking
│   ├── auth/            # JWT auth, API keys, key vault, audit logging
│   ├── backtester/      # Historical backtesting with TimescaleDB
│   ├── bot-scanner/     # Wallet behavior analysis and bot detection
│   ├── polymarket-core/ # Shared types, API clients, database models
│   ├── risk-manager/    # Stop-loss management, circuit breaker
│   ├── trading-engine/  # Order execution, copy trading, position management
│   └── wallet-tracker/  # Wallet discovery, profitability analysis, success prediction
├── migrations/          # SQL migrations for PostgreSQL/TimescaleDB
├── config/              # Environment-specific configuration
└── docs/                # Additional documentation
```

## Git Workflow

### Branch Naming Convention

```
main                     # Production-ready code, always deployable
develop                  # Integration branch for features
feature/<name>           # New features (e.g., feature/websocket-client)
fix/<name>               # Bug fixes (e.g., fix/spread-calculation)
refactor/<name>          # Code refactoring without behavior change
chore/<name>             # Maintenance tasks (deps, configs, CI)
```

### Commit Message Format

Use conventional commits for clear history:

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

**Types:**
- `feat`: New feature
- `fix`: Bug fix
- `refactor`: Code change that neither fixes a bug nor adds a feature
- `perf`: Performance improvement
- `test`: Adding or updating tests
- `docs`: Documentation changes
- `chore`: Maintenance (dependencies, build scripts, etc.)
- `ci`: CI/CD changes

**Scopes:**
- `arb`: Arbitrage monitor
- `bot`: Bot scanner
- `core`: Shared library
- `db`: Database/migrations
- `api`: API clients (CLOB, Polygon RPC)

**Examples:**
```
feat(arb): add websocket connection to CLOB API
fix(core): correct spread calculation with fees
refactor(bot): extract feature scoring into separate module
perf(arb): optimize position P&L updates
chore(deps): bump tokio to 1.35
```

### Development Workflow

1. **Start new work:**
   ```bash
   git checkout develop
   git pull origin develop
   git checkout -b feature/<name>
   ```

2. **Make commits:**
   - Commit early and often
   - Each commit should be atomic and pass all tests
   - Run `cargo fmt` and `cargo clippy` before committing

3. **Before pushing:**
   ```bash
   cargo fmt --all
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test --all
   ```

4. **Create PR:**
   - Push branch to origin
   - Create PR against `develop`
   - Ensure CI passes
   - Request review if applicable

5. **Merge to main:**
   - Only merge `develop` to `main` when ready for release
   - Tag releases with semver: `v0.1.0`, `v0.2.0`, etc.

### Protected Branches

- `main`: Requires passing CI, no direct pushes
- `develop`: Requires passing CI

## Build Commands

```bash
# Build all crates
cargo build --all

# Build release
cargo build --release --all

# Run tests
cargo test --all

# Run specific crate
cargo run -p arb-monitor
cargo run -p bot-scanner

# Format code
cargo fmt --all

# Lint
cargo clippy --all-targets --all-features -- -D warnings

# Check without building
cargo check --all
```

## Docker Deployment

### Quick Start

```bash
# Start all services (PostgreSQL, Redis, API Server)
docker compose up -d

# Check status
docker compose ps

# View logs
docker compose logs -f api-server

# Stop services
docker compose down
```

### Services

| Service | Port | Description |
|---------|------|-------------|
| PostgreSQL (TimescaleDB) | 5432 | Database with time-series extensions |
| Redis | 6379 | Caching and pub/sub |
| API Server | 3000 | REST/WebSocket API |
| Dashboard | 3002 | Next.js frontend (run separately) |

### Building Docker Images

```bash
# Build all images
docker compose build

# Build specific service
docker compose build api-server

# Force rebuild without cache
docker compose build --no-cache
```

### Running the Dashboard

The dashboard runs separately from Docker:

```bash
cd dashboard
npm install
npm run dev
# Dashboard starts on http://localhost:3002 (or 3001 if 3002 is busy)
```

Configure the dashboard to connect to the API by editing `dashboard/.env.local`:

```bash
NEXT_PUBLIC_API_URL=http://localhost:3000
NEXT_PUBLIC_WS_URL=ws://localhost:3000
```

### Optional Services

```bash
# Start with monitoring stack (Prometheus + Grafana)
docker compose --profile monitoring up -d

# Start with all services including bot-scanner
docker compose --profile full up -d
```

### Environment Variables

Create a `.env` file in the project root:

```bash
# Database
POSTGRES_USER=abbot
POSTGRES_PASSWORD=abbot_secret
POSTGRES_DB=ab_bot

# API
JWT_SECRET=your-secret-key-here
CORS_PERMISSIVE=true

# External APIs (for live mode)
POLYMARKET_API_URL=https://clob.polymarket.com
POLYGON_RPC_URL=https://polygon-rpc.com

# Logging
RUST_LOG=api_server=info,tower_http=info
```

### Troubleshooting

**SQLx compile-time errors:**
The project uses SQLx offline mode for Docker builds. If you modify queries:

```bash
# Regenerate .sqlx cache (requires running database)
DATABASE_URL=postgres://abbot:abbot_secret@localhost:5432/ab_bot cargo sqlx prepare --workspace
```

**Migration conflicts:**
Migrations are run by PostgreSQL on first boot via init scripts. The API server has `SKIP_MIGRATIONS=true` to avoid conflicts.

**Port conflicts:**
If ports are in use, modify `docker-compose.yml` or use environment variables:

```bash
API_PORT=3001 POSTGRES_PORT=5433 docker compose up -d
```

## Environment Variables

Required environment variables (set in `.env` for local development):

```bash
# Polygon RPC
ALCHEMY_API_KEY=           # Alchemy API key for Polygon RPC
POLYGON_RPC_URL=           # Full RPC URL (or constructed from API key)

# Database
DATABASE_URL=              # PostgreSQL connection string
REDIS_URL=                 # Redis connection string

# Polymarket
POLYMARKET_CLOB_URL=       # CLOB API base URL

# Live Trading (optional)
WALLET_PRIVATE_KEY=        # 64-char hex private key for order signing (with or without 0x prefix)
LIVE_TRADING=              # Set to "true" to enable live order execution

# Alerts (optional)
TELEGRAM_BOT_TOKEN=        # For alert notifications
DISCORD_WEBHOOK_URL=       # For alert notifications
```

## Key Technical Decisions

- **Rust**: Selected for async performance (Tokio) and memory safety
- **PostgreSQL + TimescaleDB**: Time-series data for historical analysis
- **Redis**: Position state caching and pub/sub for real-time signals
- **Workspace structure**: Separate crates for modularity and compile times

## API Documentation

- Polymarket CLOB API: https://docs.polymarket.com
- Alchemy Polygon: https://docs.alchemy.com/reference/polygon-api-quickstart
- The Graph: https://thegraph.com/docs/

## Architecture Notes

### Arbitrage Monitor
- Entry signal: `yes_ask + no_ask < 0.98` (profitable after 2% fees)
- Exit strategies: Hold to resolution OR exit on spread correction
- Position states: PENDING → OPEN → EXIT_READY → CLOSING → CLOSED

### Bot Scanner
- Features: trade interval variance, win rate, latency, 24/7 activity
- Initial scoring: rule-based (50+ points = likely bot)
- Future: ML-based anomaly detection (Isolation Forest)

## Testing Strategy

- Unit tests for pure functions (spread calculation, scoring)
- Integration tests for API clients (mocked responses)
- End-to-end tests against testnet when available

---

## Changelog

### 2026-01-15: Phase 11 - Railway Deployment Pipeline

**Deployment Configuration:**

- **`railway.toml`**: Railway service configuration
  - Docker builder with existing multi-target Dockerfile
  - Health check on `/health` endpoint
  - Restart on failure with 5 max retries
  - Single replica configuration

- **`Dockerfile.dashboard`**: Next.js standalone containerization
  - Multi-stage build (deps → builder → runner)
  - Build-time environment variables via ARG
  - Non-root user (nextjs:nodejs) for security
  - Health check with wget
  - Optimized for Railway deployment

- **`.github/workflows/deploy.yml`**: CI/CD pipeline
  - Triggers on push to main or manual workflow_dispatch
  - Environment selection (production/staging)
  - Runs CI checks before deployment
  - Parallel deployment of api-server, arb-monitor, dashboard
  - Post-deployment health verification with retry logic
  - Deployment summary with commit info

**Services Deployed:**
- API Server (Rust/Axum) - Port 3000
- Arb Monitor (Rust background worker)
- Dashboard (Next.js) - Port 3000
- TimescaleDB (Railway template)
- Redis (Railway template)

**Environment Variables:**
```bash
DATABASE_URL=${{timescaledb.DATABASE_URL}}
REDIS_URL=${{redis.REDIS_URL}}
JWT_SECRET=<secret>
SKIP_MIGRATIONS=true
NEXT_PUBLIC_API_URL=https://api-server.railway.app
NEXT_PUBLIC_WS_URL=wss://api-server.railway.app
```

**Files Created:**
- `railway.toml`
- `Dockerfile.dashboard`
- `.github/workflows/deploy.yml`

### 2026-01-15: Phase 10 - Resilience & Recovery

**Position Failure States & Recovery:**

- **`PositionState`**: New failure states added
  - `EntryFailed`: Position entry order failed
  - `ExitFailed`: Position exit order failed
  - `FailureReason` enum: `OrderRejected`, `InsufficientFunds`, `OrderTimeout`, `MarketClosed`, `ConnectivityError`, `Unknown`
  - `retry_count`, `max_retries`, `failure_reason` fields on Position
  - `can_retry()`, `mark_entry_failed()`, `mark_exit_failed()` methods

- **`PositionTracker`**: Reconciliation on startup
  - `reconcile_on_startup()`: Detect and recover interrupted positions
  - `ReconciliationResult`: Tracks healthy, stale pending, interrupted closing positions
  - `PositionSummary`: Overview of position states
  - `age_secs()` helper on Position for staleness detection

**Circuit Breaker Gradual Recovery Mode:**

- **`CircuitBreakerConfig`**: New recovery options
  - `gradual_recovery_enabled`: Enable staged recovery (default: false)
  - `recovery_stages`: Number of stages (default: 4 = 25%, 50%, 75%, 100%)
  - `recovery_stage_minutes`: Time between stages (default: 15)
  - `require_profit_to_advance`: Require profitable trade before advancing (default: true)

- **`RecoveryState`**: Track recovery progression
  - `current_stage`, `total_stages`, `capacity_pct()`
  - `had_profit_this_stage`, `trades_this_stage`, `recovery_pnl`
  - `is_fully_recovered()` check

- **`CircuitBreaker`**: Recovery methods
  - `trading_capacity()`: Returns current capacity (0.25 to 1.0)
  - `is_in_recovery()`, `recovery_state()`: Query recovery status
  - `try_advance_recovery()`: Advance to next stage when conditions met
  - `exit_recovery()`: Force exit from recovery mode
  - Re-tripping with scaled thresholds during recovery

- **`CircuitBreakerRepository`**: Database persistence
  - `load()`, `save()`: Persist state across restarts
  - `get_last_reset_date()`, `update_last_reset_date()`: Daily reset tracking

**Stop-Loss Improvements:**

- **`StopLossRepository`**: Database persistence
  - `insert()`, `update()`, `delete()`, `delete_by_position()`
  - `get_active()`: Load active rules on startup

- **`StopLossManager`**: Enhanced monitoring
  - `with_persistence()`: Constructor with database connection
  - `load_active_rules()`: Recover rules on startup
  - `check_triggers_detailed()`: Returns `(Vec<TriggeredStop>, CheckTriggersSummary)`
  - `rules_missing_market_data()`: Identify rules that couldn't be checked

- **`CheckTriggersSummary`**: Visibility into check results
  - Counts: `total_rules`, `triggered`, `not_triggered`
  - Skip reasons: `skipped_market_missing`, `skipped_no_bids`, `skipped_not_active`, `skipped_already_executed`

- **`CheckSkipReason`**: Why a rule was skipped
  - `MarketDataMissing`, `NoBidsAvailable`, `RuleNotActive`, `RuleAlreadyExecuted`, `TrailingNoPeak`

**AES-256-GCM Encryption in KeyVault:**

- Replaced XOR encryption with authenticated encryption
- 12-byte random nonce per encryption operation
- Tamper detection via GCM authentication tag
- Ciphertext randomness (same plaintext produces different ciphertext)

**Order Executor Resilience:**

- **`ExecutorConfig`**: Retry configuration
  - `timeout_ms`: Order timeout (default: 30000ms)
  - `max_retries`: Maximum retry attempts (default: 3)
  - `retry_base_delay_ms`: Initial backoff delay (default: 100ms)
  - `retry_max_delay_ms`: Maximum backoff delay (default: 5000ms)

- **`OrderExecutor`**: Retry logic
  - `execute_with_retry()`: Exponential backoff with jitter
  - `is_retryable_error()`: Detect transient errors (timeout, connection, rate limit, 502/503/504)

**Integration Tests:**

- 17 tests in `tests/integration_tests.rs`
- Coverage: ExecutorConfig, CircuitBreaker, StopLossRule, trailing stops
- Position state transitions, failure states, P&L calculations
- RBAC permissions, KeyVault encryption, JWT auth
- Compound stops, volatility stops, break-even stops

**Database Migrations:**

- `20260110_009_position_failure_states.sql`: Add failure columns to positions

**Files Created:**
- `crates/risk-manager/src/circuit_breaker_repo.rs`
- `crates/risk-manager/src/stop_loss_repo.rs`
- `migrations/20260110_009_position_failure_states.sql`
- `tests/integration_tests.rs`

**Files Modified:**
- `crates/arb-monitor/src/position_tracker.rs` - Reconciliation logic
- `crates/auth/Cargo.toml` - Add aes-gcm dependency
- `crates/auth/src/key_vault.rs` - AES-GCM encryption
- `crates/polymarket-core/src/db/positions.rs` - Failure state queries
- `crates/polymarket-core/src/types/position.rs` - Failure states and retry logic
- `crates/risk-manager/src/circuit_breaker.rs` - Gradual recovery mode
- `crates/risk-manager/src/lib.rs` - Export new types
- `crates/risk-manager/src/stop_loss.rs` - Detailed check reporting
- `crates/trading-engine/src/executor.rs` - Timeout and retry logic

### 2026-01-10: Phase 9 - Live API Integration & Demo Mode

**Demo Mode Implementation:**

- **`demo-portfolio-store.ts`**: Zustand store for tracking simulated positions
  - `DemoPosition`: Track wallet copies with entry/current price, quantity
  - `balance`: Demo balance (default $10,000)
  - `addPosition()`, `closePosition()`, `updatePrices()`, `reset()`
  - Automatic P&L calculation on position changes

- **`wallet-store.ts`**: Connected wallet management for live mode
  - `ConnectedWallet`: User's trading wallets from KeyVault
  - `fetchWallets()`, `connectWallet()`, `disconnectWallet()`
  - `setPrimary()`: Set primary wallet for trading
  - Selector pattern to avoid HMR issues

- **`ModeToggle` Component**: Functional demo/live mode switching
  - Shows demo balance when in demo mode
  - Shows connected wallet or "Connect" button in live mode
  - Persisted mode preference

**API Server - New Endpoints:**

- **`/api/v1/recommendations/rotation`**: Rotation recommendations
  - Analyzes tracked wallets for alpha decay, consistent losses, high risk
  - Identifies bench wallets outperforming Active roster
  - Returns prioritized recommendations with evidence
  - Query params: `urgency`, `limit`

- **`/api/v1/recommendations/:id/dismiss`**: Dismiss recommendation
- **`/api/v1/recommendations/:id/accept`**: Accept recommendation

- **`/api/v1/vault/wallets`**: Secure wallet key management
  - Store encrypted wallet credentials in KeyVault
  - List connected wallets, set primary wallet
  - Database-backed user-wallet mapping

**Dashboard - Pages Connected to Real API:**

- **Bench Page (Discover Tab)**:
  - Fetches from `/api/v1/discover/wallets`
  - Loading skeletons, error states, refresh button
  - Real wallet metrics (ROI, Sharpe, win rate, drawdown)

- **Rotation Page**:
  - Fetches from `/api/v1/recommendations/rotation`
  - Accept/dismiss mutations with optimistic updates
  - Auto-refresh every minute

- **Roster Page (Active 5)**:
  - Fetches from `/api/v1/wallets` with `copy_enabled: true`
  - Transforms API data to RosterWallet format
  - Loading skeletons, error handling

- **Allocate Page**:
  - Functional "Activate" button
  - Creates demo positions from selected strategies
  - Deducts from demo balance
  - Shows activation success state

- **Portfolio Page**:
  - Shows demo positions when in demo mode
  - Real-time balance from demo-portfolio-store

**New Query Hooks:**

- `useRotationRecommendationsQuery()`: Fetch rotation recommendations
- `useDismissRecommendation()`: Mutation for dismissing
- `useAcceptRecommendation()`: Mutation for accepting

**API Client Improvements:**

- Added generic `get<T>()`, `post<T>()`, `put<T>()`, `delete<T>()` methods
- Enables flexible API calls without dedicated methods

**Database Migrations:**

- `20260110_007_add_user_name.sql`: Add name column to users
- `20260110_008_user_wallets.sql`: User-wallet mapping for vault

**Files Created:**
- `crates/api-server/src/handlers/recommendations.rs`
- `crates/api-server/src/handlers/vault.rs`
- `crates/api-server/src/handlers/auth.rs`
- `dashboard/stores/demo-portfolio-store.ts`
- `dashboard/stores/wallet-store.ts`
- `dashboard/stores/auth-store.ts`
- `dashboard/hooks/queries/useRecommendationsQuery.ts`
- `dashboard/components/wallet/ConnectWalletModal.tsx`
- `dashboard/app/(auth)/login/page.tsx`
- `dashboard/app/(auth)/signup/page.tsx`

**Files Modified:**
- `crates/api-server/src/routes.rs` - Add recommendation routes
- `crates/api-server/src/handlers/mod.rs` - Export new modules
- `dashboard/app/bench/page.tsx` - Connect to discover API
- `dashboard/app/rotation/page.tsx` - Connect to recommendations API
- `dashboard/app/roster/page.tsx` - Connect to wallets API
- `dashboard/app/allocate/page.tsx` - Implement activation flow
- `dashboard/app/portfolio/page.tsx` - Show demo positions
- `dashboard/components/layout/ModeToggle.tsx` - Functional toggle
- `dashboard/lib/api.ts` - Add generic HTTP methods

### 2026-01-10: Phase 8 - Discovery & Demo Dashboard

**New API Endpoints:**

- **`/api/v1/discover/trades`**: Get live trades from monitored wallets
  - Query params: `wallet`, `limit`, `min_value`
  - Returns: `LiveTrade[]` with wallet, market, price, quantity, direction

- **`/api/v1/discover/wallets`**: Discover top-performing wallets
  - Query params: `sort_by` (roi/sharpe/winRate/trades), `period` (7d/30d/90d), `min_trades`, `min_win_rate`, `limit`
  - Returns: `DiscoveredWallet[]` with ROI, Sharpe ratio, win rate, prediction category

- **`/api/v1/discover/simulate`**: Run demo P&L simulation
  - Query params: `amount`, `period`, `wallets`
  - Returns: `DemoPnlSimulation` with equity curve and wallet breakdown

**New Dashboard Components:**

- **`LiveActivityFeed`**: Real-time feed of wallet trades
  - Auto-refresh every 10 seconds
  - Shows wallet, market, direction, value, price
  - Color-coded buy/sell indicators

- **`WalletLeaderboard`**: Top wallets discovery
  - Sort by ROI, Sharpe, win rate, or activity
  - Filter by time period (7d/30d/90d)
  - Track wallet functionality
  - Prediction badges (High/Moderate/Low potential)

- **`DemoPnlCalculator`**: What-if P&L simulator
  - Preset amounts ($100, $500, $1000, $5000)
  - Period selection (7d/30d/90d)
  - Visual equity curve
  - Per-wallet breakdown

**Files Created:**
- `crates/api-server/src/handlers/discover.rs` - Discovery API handlers
- `dashboard/components/discover/LiveActivityFeed.tsx`
- `dashboard/components/discover/WalletLeaderboard.tsx`
- `dashboard/components/discover/DemoPnlCalculator.tsx`
- `dashboard/components/discover/index.ts`

**Files Modified:**
- `crates/api-server/src/handlers/mod.rs` - Export discover module
- `crates/api-server/src/routes.rs` - Add discover routes and OpenAPI docs
- `crates/api-server/Cargo.toml` - Add rand dependency
- `dashboard/lib/api.ts` - Add discovery API methods
- `dashboard/types/api.ts` - Add discovery types

### 2026-01-10: Phase 7 - Live Wallet Integration

**New Modules:**

- **`auth/wallet.rs`**: Trading wallet management
  - `TradingWallet::from_env()` - Load from `WALLET_PRIVATE_KEY` env var
  - `TradingWallet::from_private_key()` - Parse hex key with/without 0x prefix
  - `address()`, `address_string()` - Get wallet address
  - `sign_message()`, `sign_message_hex()` - EIP-191 signing

- **`polymarket-core/signing/`**: EIP-712 order signing
  - `domain.rs`: EIP-712 domain separators for CTF Exchange
  - `order_types.rs`: `OrderData`, `SignedOrder`, `OrderBuilder`
  - `signer.rs`: `OrderSigner` with `sign_order()`, `sign_auth_message()`

- **`polymarket-core/api/clob.rs`**: Authenticated CLOB client
  - `ApiCredentials`: API key, secret, passphrase storage
  - `AuthenticatedClobClient`: Wrapper with wallet signing
  - `derive_api_key()`: L1 auth to get API credentials
  - `create_order()`, `post_order()`: Order creation and submission
  - `cancel_order()`, `get_open_orders()`: Order management
  - `sign_l2_request()`: HMAC-SHA256 for authenticated requests

- **`trading-engine/executor.rs`**: Live trading integration
  - `OrderExecutor::new_with_wallet()` - Constructor with wallet
  - `initialize_live_trading()` - Derive API credentials
  - `is_live_ready()`, `wallet_address()` - Status checks
  - Live order execution methods

**Test Wallet Example:**

```bash
# Test wallet connection
WALLET_PRIVATE_KEY=0x... cargo run --example test_wallet
```

Tests: wallet loading, signer creation, message signing, CLOB API connection

**Dependencies Added:**
- `alloy-primitives`, `alloy-sol-types`, `alloy-signer`, `alloy-signer-local` - Ethereum signing
- `hmac` - HMAC-SHA256 for L2 auth

**Files Created:**
- `crates/auth/src/wallet.rs`
- `crates/polymarket-core/src/signing/mod.rs`
- `crates/polymarket-core/src/signing/domain.rs`
- `crates/polymarket-core/src/signing/order_types.rs`
- `crates/polymarket-core/src/signing/signer.rs`
- `examples/test_wallet.rs`

**Files Modified:**
- `Cargo.toml` - Workspace dependencies for alloy/hmac
- `crates/auth/Cargo.toml` - alloy-signer dependencies
- `crates/auth/src/lib.rs` - Export wallet module
- `crates/polymarket-core/Cargo.toml` - Signing dependencies
- `crates/polymarket-core/src/lib.rs` - Export signing module
- `crates/polymarket-core/src/api/clob.rs` - AuthenticatedClobClient
- `crates/polymarket-core/src/error.rs` - Signing errors
- `crates/trading-engine/Cargo.toml` - Add auth dependency
- `crates/trading-engine/src/executor.rs` - Live trading methods

### 2026-01-09: Phase 6.1 - Docker Deployment Fixes

**Issues Fixed:**

- **Chrono API compatibility**: Fixed `num_hours()` method calls that don't work on `DateTime` subtraction results
  - Changed `(dt1 - dt2).num_hours()` to `dt1.signed_duration_since(dt2).num_hours()`
  - Affected files: `trading-engine/src/recommendation.rs`, `backtester/src/simulator.rs`, `risk-manager/src/advanced_stops.rs`

- **`home` crate version**: Pinned to v0.5.9 (v0.5.12 requires unreleased Rust 1.88)
  - Added `cargo update home --precise 0.5.9` to Dockerfile

- **SQLx offline mode**: Enabled for Docker builds without database connection
  - Set `SQLX_OFFLINE=true` environment variable
  - Generated `.sqlx/` cache directory with query metadata

- **Migration conflicts**: Resolved race condition between PostgreSQL init and API server
  - Added `SKIP_MIGRATIONS=true` to API server environment
  - PostgreSQL init runs migrations via `/docker-entrypoint-initdb.d/`

- **Migration ordering**: Made migration 006 idempotent
  - Added `IF EXISTS` checks for out-of-order execution safety
  - Fixed reserved word `timestamp` → `snapshot_time` in TimescaleDB function

- **Dockerfile simplification**: Removed cargo-chef (incompatible with edition2024)
  - Updated from Rust 1.75 to Rust 1.85
  - Single-stage build with workspace compilation

**Files Modified:**
- `Dockerfile` - Simplified build, added SQLX_OFFLINE, home crate pinning
- `docker-compose.yml` - Added SKIP_MIGRATIONS, migrations volume mount
- `Cargo.toml` - Added home crate version pin
- `.dockerignore` - Removed benches/ exclusion
- `crates/api-server/src/main.rs` - Added SKIP_MIGRATIONS support
- `crates/trading-engine/src/recommendation.rs` - Fixed chrono API
- `crates/backtester/src/simulator.rs` - Fixed chrono API
- `crates/risk-manager/src/advanced_stops.rs` - Fixed chrono API
- `migrations/20260109_004_timescale.sql` - Fixed reserved word
- `migrations/20260109_005_api_server_tables.sql` - Changed to ALTER TABLE
- `migrations/20260109_006_add_spread_column.sql` - Made idempotent

**New Files:**
- `.sqlx/` - SQLx offline cache (4 JSON files)
- `dashboard/` - Next.js frontend application

### 2026-01-09: Phase 5 - Advanced Features

**Enhanced Modules:**

- **`risk-manager`**: Advanced stop-loss strategies
  - `advanced_stops.rs`: New sophisticated stop mechanisms
    - `CompoundStop`: Combine multiple conditions (AND/OR/AtLeast logic)
    - `StopCondition`: 9 condition types (price, percentage, loss, time, volatility, volume, market hours, consecutive candles, support levels)
    - `VolatilityStop`: ATR-based dynamic stops with configurable period/multiplier
    - `StepTrailingStop`: Stepped trailing with configurable step size
    - `BreakEvenStop`: Auto-move to break-even after target profit
    - `TimeDecayStop`: Tighten stops as deadline approaches
    - `SessionStop`: Market hours awareness with prime-time adjustments

- **`wallet-tracker`**: Ensemble prediction models
  - `advanced_predictor.rs`: Multi-model prediction system
    - `EnsemblePrediction`: Combines 4 models with configurable weights
    - `PredictionFeatures`: 25+ features including risk metrics, time-series, behavioral
    - `MarketConditionAnalyzer`: Detect market regime (Bull/Bear, Volatile/Calm)
    - Models: Statistical, Momentum, Risk-Adjusted, Behavioral
    - Market regime adjustments for context-aware predictions

- **`auth`**: Role-Based Access Control (RBAC)
  - `rbac.rs`: Comprehensive permission system
    - `Permission`: Resource + Action with optional conditions
    - `Resource`: 14 resource types (Position, Order, Market, Wallet, StopLoss, etc.)
    - `Action`: 10 action types (Create, Read, Update, Delete, Execute, etc.)
    - `Role`: Named permission sets with inheritance
    - `DefaultRoles`: Viewer, Trader, Admin, CopyTrader, RiskManager
    - `RbacManager`: Async permission checking with role inheritance
    - `TimeWindow`: Time-based permission restrictions
    - `PermissionConditions`: IP whitelist, max amount, MFA requirements

- **`trading-engine`**: Recommendation engine
  - `recommendation.rs`: Personalized trading recommendations
    - `RecommendationEngine`: Generate context-aware suggestions
    - `RecommendationType`: 6 types (CopyWallet, EnterPosition, AdjustPosition, Arbitrage, RiskAction, StrategyChange)
    - `RecommendedAction`: Detailed action specifications
    - `RiskProfile`: User preferences (tolerance, limits, holding period)
    - `Evidence`: Supporting factors for each recommendation
    - Recommendations: Wallet copying, arbitrage, position management, risk alerts

**Key Features:**
- Compound stop conditions with flexible logic
- Volatility-aware position protection
- Ensemble ML-style predictions without ML frameworks
- Market regime detection and adjustment
- Fine-grained RBAC with time/IP/amount conditions
- Role inheritance for flexible permission management
- Context-aware trading recommendations
- Risk profile-based personalization

### 2026-01-09: Phase 4 - API Server

**New Crates:**

- **`api-server`**: REST and WebSocket API for the trading platform (Axum)
  - `lib.rs`: ApiServer with ServerConfig, middleware setup (CORS, tracing)
  - `error.rs`: Comprehensive error handling with ApiError enum and HTTP status mapping
  - `state.rs`: Shared AppState with database pool and broadcast channels
  - `routes.rs`: Complete route definitions with OpenAPI documentation
  - `websocket.rs`: Real-time WebSocket handlers
    - `OrderbookUpdate`: Live orderbook changes with arbitrage spread
    - `PositionUpdate`: Position open/close/price change events
    - `SignalUpdate`: Trading signals (arbitrage, copy trade, stop-loss)
    - Four endpoints: `/ws/orderbook`, `/ws/positions`, `/ws/signals`, `/ws/all`
  - `handlers/`: RESTful API handlers
    - `health.rs`: `/health` and `/ready` endpoints with database check
    - `markets.rs`: Market data (`/api/v1/markets`, orderbook)
    - `positions.rs`: Position CRUD (`/api/v1/positions`, close)
    - `wallets.rs`: Wallet tracking (`/api/v1/wallets`, metrics)
    - `trading.rs`: Order execution (`/api/v1/orders`, cancel)
    - `backtest.rs`: Backtesting (`/api/v1/backtest`, results)

**API Endpoints:**
- Health: `GET /health`, `GET /ready`
- Markets: `GET /api/v1/markets`, `GET /api/v1/markets/:id`, `GET /api/v1/markets/:id/orderbook`
- Positions: `GET /api/v1/positions`, `GET /api/v1/positions/:id`, `POST /api/v1/positions/:id/close`
- Wallets: `GET/POST /api/v1/wallets`, `GET/PUT/DELETE /api/v1/wallets/:address`, `GET /api/v1/wallets/:address/metrics`
- Trading: `POST /api/v1/orders`, `GET /api/v1/orders/:id`, `POST /api/v1/orders/:id/cancel`
- Backtest: `POST /api/v1/backtest`, `GET /api/v1/backtest/results`, `GET /api/v1/backtest/results/:id`
- Docs: Swagger UI at `/swagger-ui`

**Database Schema (Migration 005):**
- `markets`: Market data with pricing, volume, liquidity
- `orders`: Order management with status tracking
- `backtest_results`: Backtest outputs with equity curve
- Extended `positions` with outcome, side, stop-loss, take-profit
- Extended `tracked_wallets` with copy trading settings
- Extended `wallet_success_metrics` with additional analytics

**Key Features:**
- OpenAPI 3.0 documentation with utoipa
- Swagger UI for interactive API exploration
- WebSocket support with tokio broadcast channels
- Type-safe request/response handling
- Comprehensive error responses with codes

### 2026-01-09: Phase 3 - Backtesting Framework

**New Crates:**

- **`backtester`**: Historical simulation framework for strategy testing
  - `strategy.rs`: Pluggable Strategy trait with built-in implementations
    - `Strategy` trait: Async interface with initialize/on_data/on_fill/finalize lifecycle
    - `Signal`: Trade signal with entry price, stop loss, take profit, confidence
    - `StrategyContext`: Portfolio state, positions, market data for decision making
    - `ArbitrageStrategy`: Trades mispriced yes/no outcomes
    - `MomentumStrategy`: Follows price trends with configurable lookback
    - `MeanReversionStrategy`: Trades z-score deviations from moving average
  - `data_store.rs`: TimescaleDB-backed historical data storage
    - `MarketSnapshot`: Point-in-time orderbook state (bid/ask/depth/spread)
    - `HistoricalTrade`: Trade records with price/quantity/side/fee
    - `DataQuery`: Flexible time-bucketed queries with aggregation
    - `TimeResolution`: Second to daily aggregation levels
  - `simulator.rs`: Full backtest engine with realistic execution
    - `BacktestSimulator`: Run strategies against historical data
    - `SlippageModel`: None, Fixed, VolumeBased, SpreadBased models
    - `SimulatorConfig`: Fees, margin, position limits, reinvestment
    - `BacktestResult`: Comprehensive metrics (Sharpe, Sortino, drawdown, win rate)

**Database Schema (Migration 004 - TimescaleDB):**
- `orderbook_snapshots` hypertable with automatic compression (7 days)
- `historical_trades` hypertable for trade history
- `backtest_results` for storing simulation outputs
- `strategy_configs` for saved strategy parameters
- Continuous aggregates: 5-minute, hourly, daily OHLCV
- Retention policy: 1 year automatic data cleanup

**Test Coverage:** 85 tests passing (+15 new)
- backtester: 15 tests (strategy, data_store, simulator)

### 2026-01-09: Phase 2 - Wallet Tracking & Copy Trading

**New Crates:**

- **`wallet-tracker`**: Wallet discovery and success prediction system
  - `discovery.rs`: Find profitable wallets via configurable criteria (min trades, win rate, volume, ROI)
    - `DiscoveryCriteria`: Builder pattern for filter configuration
    - `DiscoveredWallet`: Wallet data with computed metrics
    - `WalletDiscovery`: Service for discovering and caching profitable wallets
    - `RankingMetric`: Sort by ROI, win rate, volume, PnL, trade count, or consistency
  - `profitability.rs`: Comprehensive financial metrics calculation
    - `WalletMetrics`: 15+ metrics including Sharpe ratio, Sortino ratio, max drawdown, volatility
    - `ProfitabilityAnalyzer`: Calculates metrics from trade history using statrs
    - `TimePeriod`: Day, week, month, quarter, year, all-time analysis windows
  - `success_predictor.rs`: Rule-based success prediction models
    - `SuccessPredictor`: Predict future performance from historical metrics
    - `PredictionModel`: RuleBased, Linear, WeightedAverage models
    - `SuccessPrediction`: Probability, confidence, and factor breakdown
    - `PredictionCategory`: HighPotential, Moderate, LowPotential, Uncertain
  - `trade_monitor.rs`: Real-time wallet trade monitoring
    - `TradeMonitor`: Monitor wallets for new trades with broadcast channels
    - `WalletTrade`: Trade detection with market, price, quantity, value
    - `MonitorConfig`: Poll interval, min trade value, max trade age

**Database Schema (Migration 003):**
- `wallet_success_metrics`: Computed profitability and prediction scores
- `discovered_wallets`: Historical wallet discovery snapshots
- `copy_trade_history`: Full copy trading audit trail
- `wallet_trade_signals`: Real-time trade detection queue
- Extended `tracked_wallets` with performance metrics

**Test Coverage:** 70 tests passing (+15 new)
- wallet-tracker: 15 tests (discovery, profitability, prediction, monitoring)

### 2026-01-09: Phase 1 - Trading & Risk Foundation

**New Crates:**

- **`trading-engine`**: Order execution and copy trading system
  - `OrderExecutor`: Low-latency order placement with paper/live trading modes
  - `CopyTrader`: Track and mirror wallets with allocation strategies (equal weight, configured, performance-weighted)
  - `PositionManager`: Position sizing and limit enforcement across strategies
  - Order types: `MarketOrder`, `LimitOrder`, `ExecutionReport`, `ArbOrder`

- **`risk-manager`**: Stop-loss and circuit breaker protection
  - `StopLossManager`: Fixed, percentage, trailing, and time-based stop-losses
  - `CircuitBreaker`: Daily loss limits, max drawdown, consecutive loss protection with cooldown

- **`auth`**: Authentication and security layer
  - `JwtAuth`: JWT token-based authentication with roles (Viewer, Trader, Admin)
  - `ApiKeyAuth`: Programmatic API key authentication with expiry
  - `KeyVault`: Secure wallet key storage with encryption (memory, file, AWS-ready)
  - `AuditLogger`: Security audit trail for compliance

**Enhanced `polymarket-core`:**
- Added `types/order.rs` with order types for trading execution

**Database Schema (Migration 002):**
- `tracked_wallets`: Copy trading wallet configuration
- `stop_loss_rules`: Stop-loss rule storage
- `execution_reports`: Trade execution history
- `users`: Authentication users with roles
- `api_keys`: API key management
- `audit_log`: Security audit trail
- `circuit_breaker_state`: Persistent circuit breaker state
- Extended `positions` table with source tracking (manual, arbitrage, copy_trade)

**Test Coverage:** 55 tests passing
- auth: 21 tests (JWT, API keys, key vault, audit)
- risk-manager: 13 tests (stop-loss triggers, circuit breaker)
- trading-engine: 9 tests (executor, copy trader, position manager)
- polymarket-core: 9 tests (orders, markets, positions, wallets)
- bot-scanner: 3 tests (feature extraction, scoring)

### 2026-01-09: Initial Setup

- Created Rust workspace with 3 crates: polymarket-core, arb-monitor, bot-scanner
- Implemented CLOB API client with pagination and WebSocket streaming
- Implemented Polygon RPC client for on-chain wallet data
- Created position lifecycle management (PENDING → OPEN → EXIT_READY → CLOSING → CLOSED)
- Created bot detection scoring system with 5 behavioral signals
- Set up PostgreSQL database with initial schema
- Set up Redis for pub/sub signals
- Integrated Telegram/Discord alerting
