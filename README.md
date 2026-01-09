# AB-Bot

A high-performance Polymarket trading platform built in Rust, featuring arbitrage detection, bot scanning, copy trading, and risk management.

## Features

- **Arbitrage Monitor** - Real-time detection of mispriced prediction markets
- **Bot Scanner** - Identify automated trading wallets through behavioral analysis
- **Copy Trading** - Mirror successful wallets with configurable allocation strategies
- **Risk Management** - Advanced stop-loss strategies and circuit breakers
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
- **Redis**: localhost:6379

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

# External APIs (for live trading)
POLYMARKET_API_URL=https://clob.polymarket.com
POLYGON_RPC_URL=https://polygon-rpc.com
```

## API Endpoints

### Health & Status
- `GET /health` - Health check
- `GET /ready` - Readiness check

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
- `GET /api/v1/wallets/:address/metrics` - Get wallet metrics

### Trading
- `POST /api/v1/orders` - Place order
- `POST /api/v1/orders/:id/cancel` - Cancel order

### Backtesting
- `POST /api/v1/backtest` - Run backtest
- `GET /api/v1/backtest/results` - List backtest results

### WebSocket Streams
- `WS /ws/orderbook` - Live orderbook updates
- `WS /ws/positions` - Position updates
- `WS /ws/signals` - Trading signals
- `WS /ws/all` - All streams combined

API documentation available at `/swagger-ui` when running.

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
cargo clippy --all-targets --all-features -- -D warnings
```

### Database Migrations

Migrations run automatically on first Docker start. For manual management:

```bash
# Run migrations
DATABASE_URL=postgres://abbot:abbot_secret@localhost:5432/ab_bot sqlx migrate run

# Generate SQLx offline cache (after modifying queries)
DATABASE_URL=postgres://abbot:abbot_secret@localhost:5432/ab_bot cargo sqlx prepare --workspace
```

## Dashboard Features

### Demo Mode
- Paper trading with simulated capital ($10,000 default)
- Full backtesting capabilities
- No wallet connection required

### Live Mode
- Real trading connected to Polymarket
- Actual order execution
- Real P&L tracking

### Pages
- **Dashboard** - Portfolio overview and metrics
- **Portfolio** - Position management
- **Discover** - Find top-performing wallets
- **Allocate** - Strategy allocation wizard
- **Backtest** - Historical simulations
- **Settings** - Configuration

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

See [CLAUDE.md](CLAUDE.md) for detailed development guidelines and changelog.
