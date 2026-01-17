# Changelog

## 2026-01-16: Phase 12 - Rate Limiting & Audit Logging

**Admin Rate Limiting:**

- **`routes.rs`**: Rate limiting for admin endpoints
  - 10 requests per 60 seconds per IP for `/api/v1/users/*` routes
  - Uses `tower_governor` middleware
  - Returns HTTP 429 with `Retry-After` header when exceeded

**PostgreSQL Audit Storage:**

- **`audit_storage_pg.rs`**: New persistent audit log storage
  - `PostgresAuditStorage`: Implements `AuditStorage` trait
  - `store()`: INSERT audit events into `audit_log` table
  - `query()`: Dynamic filtering by user, action, resource, time range, success
  - `count()`: Count matching events with same filters
  - Bidirectional `AuditAction` <-> string conversion

**User Management Audit Actions:**

- **`audit.rs`**: New audit action types
  - `UserCreated`: User account created (self-registration or admin)
  - `UserUpdated`: User profile/role/password modified
  - `UserDeleted`: User account removed
  - `UserViewed`: User data accessed (single or list)
  - `log_user_action()`: Convenience method for user management events

**AppState Integration:**

- **`state.rs`**: Audit logger added to shared state
  - `audit_logger: Arc<AuditLogger>` field
  - Initialized with `PostgresAuditStorage` backend
  - Available to all handlers via `State(state)`

**Auth Handler Audit Logging:**

- **`handlers/auth.rs`**: Login and registration auditing
  - Successful login: `AuditAction::Login` with user ID
  - Failed login (user not found): `AuditAction::LoginFailed` with email
  - Failed login (wrong password): `AuditAction::LoginFailed` with user ID
  - Registration: `AuditAction::UserCreated` with source "self_registration"

**User Management Handler Audit Logging:**

- **`handlers/users.rs`**: Full CRUD auditing
  - `list_users()`: `UserViewed` with count of users returned
  - `create_user()`: `UserCreated` with email, role, admin who created
  - `get_user()`: `UserViewed` with target user email
  - `update_user()`: `UserUpdated` with list of fields changed
  - `delete_user()`: `UserDeleted` with target email and admin who deleted

**Admin User Management Dashboard:**

- **`/settings/users`**: Full admin user CRUD interface
  - List all users with email, role, created date, last login
  - Create new users with email, password, name, role
  - Edit user name, role, or reset password
  - Delete users with confirmation dialog
  - Role badges (Viewer, Trader, Admin)
  - Search and filter functionality

- **`/settings`**: User Management section for admins
  - Conditional display based on user role
  - Link to user management page

- **Self-signup removal**: Admin-only user creation
  - Removed `/signup` page and route
  - Login page directs to admin contact
  - Removed signup validation schema
  - Updated AuthGuard public routes

**Dashboard API Integration:**

- **`lib/api.ts`**: User management methods
  - `listUsers()`, `createUser()`, `getUser()`
  - `updateUser()`, `deleteUser()`

- **`types/api.ts`**: User management types
  - `UserListItem`, `CreateUserRequest`, `UpdateUserRequest`

**JWT Security & RBAC:**

- **`main.rs`**: JWT secret validation on startup
  - Minimum 32 characters required
  - Rejects default/empty values
  - Clear error messages with generation instructions

- **`middleware.rs`**: RBAC role syncing
  - Syncs JWT role claims to RBAC manager
  - Enables fine-grained permission checks
  - Idempotent role assignment

- **`docker-compose.yml`**: Enforced JWT_SECRET
  - Required environment variable (fails if not set)
  - Documentation for secure secret generation

- **`.env.example`**: Security documentation
  - JWT_SECRET requirements and generation command
  - Environment variable documentation

**Files Created:**
- `crates/auth/src/audit_storage_pg.rs`
- `dashboard/app/settings/users/page.tsx`

**Files Modified:**
- `crates/auth/src/audit.rs` - Added user management actions
- `crates/auth/src/lib.rs` - Export new types
- `crates/api-server/src/state.rs` - Added audit_logger
- `crates/api-server/src/routes.rs` - Added admin rate limiting
- `crates/api-server/src/handlers/mod.rs` - Export users module
- `crates/api-server/src/handlers/auth.rs` - Added audit logging
- `crates/api-server/src/handlers/users.rs` - Added audit logging with Claims extraction
- `crates/api-server/src/main.rs` - JWT secret validation
- `crates/api-server/src/middleware.rs` - RBAC role syncing
- `crates/api-server/Cargo.toml` - Added tower_governor
- `dashboard/app/(auth)/login/page.tsx` - Removed signup link
- `dashboard/app/settings/page.tsx` - Added user management section
- `dashboard/components/auth/AuthGuard.tsx` - Removed /signup route
- `dashboard/lib/api.ts` - Added user management methods
- `dashboard/lib/validations.ts` - Removed signup schema
- `dashboard/types/api.ts` - Added user management types
- `docker-compose.yml` - JWT_SECRET enforcement
- `.env.example` - Security documentation
- `docs/DEPLOY.md` - Updated deployment docs

**Files Deleted:**
- `dashboard/app/(auth)/signup/page.tsx`

## 2026-01-15: Phase 11 - Railway Deployment Pipeline

**Deployment Configuration:**

- **`railway.toml`**: Railway service configuration
  - Docker builder with existing multi-target Dockerfile
  - Health check on `/health` endpoint
  - Restart on failure with 5 max retries
  - Single replica configuration

- **`Dockerfile.dashboard`**: Next.js standalone containerization
  - Multi-stage build (deps -> builder -> runner)
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

**Files Created:**
- `railway.toml`
- `Dockerfile.dashboard`
- `.github/workflows/deploy.yml`

## 2026-01-15: Phase 10 - Resilience & Recovery

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

## 2026-01-10: Phase 9 - Live API Integration & Demo Mode

**Demo Mode Implementation:**

- **`demo-portfolio-store.ts`**: Zustand store for tracking simulated positions
- **`wallet-store.ts`**: Connected wallet management for live mode
- **`ModeToggle` Component**: Functional demo/live mode switching

**API Server - New Endpoints:**

- **`/api/v1/recommendations/rotation`**: Rotation recommendations
- **`/api/v1/recommendations/:id/dismiss`**: Dismiss recommendation
- **`/api/v1/recommendations/:id/accept`**: Accept recommendation
- **`/api/v1/vault/wallets`**: Secure wallet key management

**Dashboard - Pages Connected to Real API:**

- Bench, Rotation, Roster, Allocate, Portfolio pages

**Database Migrations:**

- `20260110_007_add_user_name.sql`: Add name column to users
- `20260110_008_user_wallets.sql`: User-wallet mapping for vault

## 2026-01-10: Phase 8 - Discovery & Demo Dashboard

**New API Endpoints:**

- **`/api/v1/discover/trades`**: Get live trades from monitored wallets
- **`/api/v1/discover/wallets`**: Discover top-performing wallets
- **`/api/v1/discover/simulate`**: Run demo P&L simulation

**New Dashboard Components:**

- **`LiveActivityFeed`**: Real-time feed of wallet trades
- **`WalletLeaderboard`**: Top wallets discovery
- **`DemoPnlCalculator`**: What-if P&L simulator

## 2026-01-10: Phase 7 - Live Wallet Integration

**New Modules:**

- **`auth/wallet.rs`**: Trading wallet management with EIP-191 signing
- **`polymarket-core/signing/`**: EIP-712 order signing
- **`polymarket-core/api/clob.rs`**: Authenticated CLOB client
- **`trading-engine/executor.rs`**: Live trading integration

## 2026-01-09: Phase 6.1 - Docker Deployment Fixes

- Fixed chrono API compatibility (`signed_duration_since`)
- Pinned `home` crate to v0.5.9
- Enabled SQLx offline mode for Docker builds
- Resolved migration conflicts with `SKIP_MIGRATIONS`
- Made migration 006 idempotent
- Simplified Dockerfile (Rust 1.85, removed cargo-chef)

## 2026-01-09: Phase 5 - Advanced Features

- **`risk-manager`**: Advanced stop-loss strategies (compound, volatility, trailing, break-even, time-decay)
- **`wallet-tracker`**: Ensemble prediction models with market regime detection
- **`auth`**: Role-Based Access Control (RBAC) with 14 resource types, 10 actions
- **`trading-engine`**: Recommendation engine with 6 recommendation types

## 2026-01-09: Phase 4 - API Server

- REST and WebSocket API (Axum)
- OpenAPI 3.0 documentation with Swagger UI
- Endpoints: health, markets, positions, wallets, trading, backtest
- WebSocket: orderbook, positions, signals

## 2026-01-09: Phase 3 - Backtesting Framework

- **`backtester`**: Strategy trait, TimescaleDB data store, simulator
- Built-in strategies: Arbitrage, Momentum, MeanReversion
- Slippage models: Fixed, VolumeBased, SpreadBased

## 2026-01-09: Phase 2 - Wallet Tracking & Copy Trading

- **`wallet-tracker`**: Discovery, profitability analysis, success prediction, trade monitoring
- Database: wallet_success_metrics, discovered_wallets, copy_trade_history

## 2026-01-09: Phase 1 - Trading & Risk Foundation

- **`trading-engine`**: OrderExecutor, CopyTrader, PositionManager
- **`risk-manager`**: StopLossManager, CircuitBreaker
- **`auth`**: JwtAuth, ApiKeyAuth, KeyVault, AuditLogger

## 2026-01-09: Initial Setup

- Created Rust workspace with polymarket-core, arb-monitor, bot-scanner
- CLOB API client with WebSocket streaming
- Polygon RPC client for on-chain data
- Position lifecycle management
- Bot detection scoring system
- PostgreSQL + Redis setup
- Telegram/Discord alerting
