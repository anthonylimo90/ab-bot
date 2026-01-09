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
│   ├── arb-monitor/     # Arbitrage detection and position tracking
│   ├── auth/            # JWT auth, API keys, key vault, audit logging
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
