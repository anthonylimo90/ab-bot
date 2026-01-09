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
