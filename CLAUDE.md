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
│   ├── bot-scanner/     # Wallet behavior analysis and bot detection
│   └── polymarket-core/ # Shared types, API clients, database models
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
