# Changelog

## 2026-01-18: Phase 13 - Automated Wallet Management & Multi-Tenant Workspaces

**Multi-Tenant Workspace System:**

- **`workspaces` table**: Isolated trading environments
  - Per-workspace budgets, rosters, and settings
  - `setup_mode`: 'automatic' or 'manual' wallet selection
  - `auto_optimize_enabled`, `auto_select_enabled`, `auto_demote_enabled` flags
  - Configurable thresholds: `min_roi_30d`, `min_sharpe`, `min_win_rate`, `min_trades_30d`, `max_drawdown_pct`

- **`workspace_members` table**: Role-based team access
  - Roles: `owner`, `manager`, `trader`, `viewer`
  - Owners can manage members and settings
  - Managers can manage roster and trading
  - Traders can view and trade
  - Viewers have read-only access

- **`workspace_invites` table**: Email-based invitations
  - Token-based invite links with expiration
  - Resend email integration for delivery
  - Accept/decline flow with workspace joining

**Automated Wallet Selection System:**

- **`auto_optimizer.rs`**: Event-driven automation service
  - `AutomationEvent`: PositionClosed, CircuitBreakerTripped, MetricsUpdated, WorkspaceCreated
  - Hourly scheduled optimization cycle
  - Redis pub/sub for real-time event handling

- **Auto-Select (Promotion)**:
  - Fills empty Active slots (max 5) with best candidates
  - Composite scoring: ROI (30%) + Sharpe (25%) + Win Rate (25%) + Consistency (20%)
  - `rank_candidates()`: Scores and sorts by total_score
  - `fill_empty_slots()`: Queries `wallet_success_metrics` for qualifying wallets

- **Auto-Drop (Demotion)**:
  - Immediate triggers: 5+ consecutive losses, drawdown > 30%, circuit breaker trip
  - Grace period triggers: ROI < 0% for 48h, Sharpe < 0.5 for 24h, no trades in 14 days
  - `DemotionTrigger` enum with `is_immediate()` and `grace_period_hours()`
  - `check_demotion_triggers()`: Evaluates wallet against thresholds

- **Probation System**:
  - New wallets start with 7-day probation at 50% allocation
  - `probation_until`, `probation_allocation_pct` columns
  - `process_probation_graduations()`: Graduate or demote after probation
  - Must maintain criteria throughout probation period

- **Confidence-Weighted Allocation**:
  - `AllocationStrategy` enum: Equal, ConfidenceWeighted, Performance
  - `calculate_confidence_weighted_allocations()`: 0.5x to 1.5x multiplier based on confidence
  - `calculate_data_confidence()`: Based on trade count, sharpe, win rate
  - Allocation caps: min 10%, max 35% per wallet

- **Pin/Ban Support**:
  - `pin_wallet()`, `unpin_wallet()`: Prevent auto-demotion (max 3 pins)
  - `ban_wallet()`, `unban_wallet()`: Prevent auto-promotion
  - `workspace_wallet_bans` table with optional expiration

- **Rotation History & Undo**:
  - `auto_rotation_history` table: Full audit trail of all actions
  - `undo_rotation()`: Revert actions within 1-hour window
  - `get_automation_preview()`: Preview next automation actions

**Wallet Roster System:**

- **`workspace_wallet_allocations` table**: Active/Bench tiered management
  - `tier`: 'active' (max 5) or 'bench' (unlimited)
  - `allocation_pct`: Percentage of portfolio
  - `pinned`, `pinned_at`, `pinned_by`: User override protection
  - `consecutive_losses`, `last_loss_at`: Loss streak tracking
  - `grace_period_started_at`, `grace_period_reason`: Grace period tracking
  - `auto_assigned`, `auto_assigned_reason`: Track automated vs manual additions

- **`handlers/allocations.rs`**: Roster management API
  - `list_allocations()`: Get workspace roster
  - `add_allocation()`: Add wallet to roster
  - `update_allocation()`: Modify allocation percentage
  - `remove_allocation()`: Remove from roster
  - `promote_allocation()`: Move from Bench to Active
  - `demote_allocation()`: Move from Active to Bench

**Onboarding System:**

- **`handlers/onboarding.rs`**: Guided workspace setup
  - `get_status()`: Check onboarding progress
  - `set_mode()`: Choose automatic or manual setup
  - `set_budget()`: Configure portfolio budget
  - `auto_setup()`: Automatically select top wallets
  - `complete_onboarding()`: Finalize setup

- **Setup Modes**:
  - `automatic`: System selects and manages wallets
  - `manual`: User has full control over roster

**Demo Trading System:**

- **`handlers/demo.rs`**: Paper trading API
  - `list_positions()`: Get demo positions
  - `create_position()`: Open simulated position
  - `update_position()`: Modify position
  - `close_position()`: Close with P&L calculation
  - `get_balance()`: Check demo balance
  - `update_balance()`: Adjust demo capital
  - `reset_portfolio()`: Reset to initial state

- **`demo_positions` table**: Simulated position tracking
  - Links to workspace and user
  - Full position lifecycle (entry, current price, P&L)
  - Closed position history with realized P&L

- **`demo_balances` table**: Per-workspace demo capital
  - Default $10,000 starting balance
  - Tracks available and invested amounts

**Dashboard Updates:**

- **Trading Page** (`/trading`): Unified copy trading interface
  - Active tab: Active roster wallets with positions
  - Bench tab: Bench wallets ready for promotion
  - Positions tab: Manual position management
  - History tab: Closed positions log
  - Automation tab: Auto-optimizer controls

- **`AutomationPanel` component**: Automation controls
  - ON/OFF toggles for auto-select and auto-demote
  - Threshold sliders (ROI, Sharpe, Win Rate, Trades, Drawdown)
  - Live rotation history feed
  - Manual trigger button for immediate optimization
  - Status indicator (last run, next run)

- **`WalletCard` component**: Enhanced wallet display
  - Pin/unpin button with visual indicator
  - Probation badge with days remaining
  - Confidence score display
  - Quick actions: Promote, Demote, Remove, Ban

- **`PortfolioSummary` component**: Portfolio overview
  - Total value, P&L, allocation breakdown
  - Active vs Bench wallet counts

- **Stores**:
  - `roster-store.ts`: Workspace roster state management
  - `demo-portfolio-store.ts`: Demo positions and balance

**New API Endpoints:**

- `GET /api/v1/workspaces` - List user's workspaces
- `GET /api/v1/workspaces/current` - Get current workspace
- `GET /api/v1/workspaces/:id` - Get workspace details
- `PUT /api/v1/workspaces/:id` - Update workspace settings
- `POST /api/v1/workspaces/switch/:id` - Switch to workspace
- `GET /api/v1/workspaces/:id/members` - List workspace members
- `GET /api/v1/workspaces/:id/optimizer-status` - Get automation status
- `GET /api/v1/invites` - List pending invites
- `POST /api/v1/invites` - Create invite
- `DELETE /api/v1/invites/:id` - Revoke invite
- `GET /api/v1/invites/:token/info` - Get invite info (public)
- `POST /api/v1/invites/:token/accept` - Accept invite
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
- `GET /api/v1/auto-rotation/history` - Get rotation history
- `POST /api/v1/auto-rotation/history/:id/acknowledge` - Acknowledge entry
- `POST /api/v1/auto-rotation/trigger` - Trigger optimization manually
- `GET /api/v1/onboarding/status` - Get onboarding status
- `POST /api/v1/onboarding/mode` - Set workspace mode
- `POST /api/v1/onboarding/budget` - Set budget
- `POST /api/v1/onboarding/auto-setup` - Run auto-setup
- `POST /api/v1/onboarding/complete` - Complete onboarding
- `GET /api/v1/demo/positions` - List demo positions
- `POST /api/v1/demo/positions` - Create demo position
- `PUT /api/v1/demo/positions/:id` - Update demo position
- `DELETE /api/v1/demo/positions/:id` - Close demo position
- `GET /api/v1/demo/balance` - Get demo balance
- `PUT /api/v1/demo/balance` - Update demo balance
- `POST /api/v1/demo/reset` - Reset demo portfolio

**Database Migrations:**

- `20260117_010_workspaces.sql`: Workspaces, members, invites tables
- `20260117_011_workspace_allocations.sql`: Wallet allocations with tiers
- `20260117_012_automation.sql`: Auto-rotation history, bans, automation columns
- `20260117_013_demo_trading.sql`: Demo positions and balances
- `20260118_014_allocation_enhancements.sql`: Pin, probation, grace period columns

**Files Created:**

- `crates/api-server/src/auto_optimizer.rs`
- `crates/api-server/src/handlers/workspaces.rs`
- `crates/api-server/src/handlers/admin_workspaces.rs`
- `crates/api-server/src/handlers/invites.rs`
- `crates/api-server/src/handlers/allocations.rs`
- `crates/api-server/src/handlers/auto_rotation.rs`
- `crates/api-server/src/handlers/onboarding.rs`
- `crates/api-server/src/handlers/demo.rs`
- `dashboard/app/trading/page.tsx`
- `dashboard/components/trading/AutomationPanel.tsx`
- `dashboard/components/trading/WalletCard.tsx`
- `dashboard/components/trading/PortfolioSummary.tsx`
- `dashboard/components/trading/ManualPositions.tsx`
- `dashboard/stores/roster-store.ts`

**Files Modified:**

- `crates/api-server/src/routes.rs` - Added new route groups
- `crates/api-server/src/handlers/mod.rs` - Export new handlers
- `crates/api-server/src/state.rs` - Added AutoOptimizer to state
- `crates/api-server/src/main.rs` - Initialize automation service
- `dashboard/lib/api.ts` - Added workspace, roster, demo API methods
- `dashboard/types/api.ts` - Added new type definitions
- `dashboard/stores/mode-store.ts` - Demo/Live mode state
- `dashboard/components/layout/Sidebar.tsx` - Updated navigation

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
