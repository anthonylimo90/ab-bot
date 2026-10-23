# Railway Deployment Guide

Complete guide for deploying AB-Bot to [Railway](https://railway.app) with GitHub integration.

## Prerequisites

- GitHub repository with the AB-Bot codebase
- Railway account (https://railway.app)
- PostgreSQL client (`psql`) for database setup
- Railway CLI (optional, for local management)

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Railway Project                          │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌──────────────┐    ┌──────────────┐                      │
│  │  TimescaleDB │    │    Redis     │                      │
│  │  (Database)  │    │   (Cache)    │                      │
│  └──────┬───────┘    └──────┬───────┘                      │
│         │                   │                              │
│         └─────────┬─────────┘                              │
│                   │                                        │
│         ┌─────────▼─────────┐            ┌──────────────┐  │
│         │    API Server     │◄───────────│  Dashboard   │  │
│         │   (Rust/Axum)     │            │  (Next.js)   │  │
│         └─────────┬─────────┘            └──────────────┘  │
│                   │                                        │
│         ┌─────────▼─────────┐                              │
│         │   Arb Monitor     │                              │
│         │ (Background Job)  │                              │
│         └───────────────────┘                              │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## Step 1: Create Railway Project

1. Go to https://railway.app/dashboard
2. Click **"+ New Project"**
3. Select **"Empty Project"**
4. Name it `ab-bot` or similar

## Step 2: Add Database Services

### Add TimescaleDB

1. In your project, click **"+ New"**
2. Select **"Database"** → **"TimescaleDB"**
3. Click **"Deploy"**
4. Wait for provisioning (1-2 minutes)

### Add Redis

1. Click **"+ New"**
2. Select **"Database"** → **"Redis"**
3. Click **"Deploy"**

## Step 3: Initialize Database

**Critical:** Enable the uuid-ossp extension before running migrations.

1. Get your TimescaleDB connection string from Railway:
   - Click on TimescaleDB service
   - Go to **Variables** tab
   - Copy `DATABASE_URL`

2. Connect and enable extension:
   ```bash
   psql "your-database-url"
   ```

3. Run:
   ```sql
   CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
   \q
   ```

## Step 4: Create Application Services

### 4.1 API Server

1. Click **"+ New"** → **"GitHub Repo"**
2. Select your ab-bot repository
3. Rename the service to `api-server`
4. Configure in **Settings** tab:

   **Config-as-code:** set **Railway Config File** to `/railway/api-server.toml`
   (an absolute path). It sets the Dockerfile, health check, restart policy,
   a single replica and the 50s drain the graceful shutdown below needs.
   Without it, set them by hand: Builder `Dockerfile`, Dockerfile Path
   `Dockerfile`, healthcheck `/health`.

   **Watch Paths:** (optional)
   ```
   crates/**
   Cargo.*
   Dockerfile
   migrations/**
   ```

5. Add **Variables**:
   ```
   DATABASE_URL=${{TimescaleDB.DATABASE_URL}}
   REDIS_URL=${{Redis.REDIS_URL}}
   DYNAMIC_TUNER_REDIS_URL=${{Redis.REDIS_URL}}
   DYNAMIC_CONFIG_REDIS_URL=${{Redis.REDIS_URL}}
   JWT_SECRET=<generate-32-char-secret>
   RUST_LOG=warn,api_server=info,auth=info,risk_manager=info,trading_engine=info,tower_http=error
   ENVIRONMENT=production
   CORS_ALLOWED_ORIGINS=https://<your-dashboard-domain>
   APP_DOMAIN=<your-dashboard-domain>
   APP_URI=https://<your-dashboard-domain>
   TRUSTED_PROXY_HOPS=1
   SKIP_MIGRATIONS=false
   RAILWAY_DEPLOYMENT_DRAINING_SECONDS=50
   ```

   On SIGTERM the API server stops accepting requests and new trading
   operations, then lets open requests (up to 15s) and, at the same time,
   started operations such as an arb between its two legs (up to 40s, an
   order's whole preparation deadline) finish. A second SIGTERM stops the
   wait. Railway's default drain is only a few seconds, so give it 50.

   `CORS_ALLOWED_ORIGINS` is what restricts cross-origin access; without it all
   origins are allowed whatever `CORS_PERMISSIVE` says. Wallet sign-in stays
   disabled outside development until `APP_DOMAIN` and `APP_URI` are set.
   Railway puts one proxy in front of the service, hence `TRUSTED_PROXY_HOPS=1`.

6. In **Networking**, click **"Generate Domain"** for public URL

### 4.2 Arb Monitor

1. Click **"+ New"** → **"GitHub Repo"**
2. Select your ab-bot repository
3. Rename to `arb-monitor`
4. Configure in **Settings** tab:

   **Config-as-code:** set **Railway Config File** to
   `/railway/arb-monitor.toml` (or by hand: Builder `Dockerfile`, Dockerfile
   Path `Dockerfile.arb-monitor`).

5. Add **Variables**:
   ```
   DATABASE_URL=${{TimescaleDB.DATABASE_URL}}
   REDIS_URL=${{Redis.REDIS_URL}}
   DYNAMIC_CONFIG_REDIS_URL=${{Redis.REDIS_URL}}
   RUST_LOG=warn,arb_monitor=info
   POLYMARKET_CLOB_URL=https://clob.polymarket.com
   ```

6. No public domain needed (background worker)

### 4.3 Dashboard

1. Click **"+ New"** → **"GitHub Repo"**
2. Select your ab-bot repository
3. Rename to `dashboard`
4. Configure in **Settings** tab:

   **Config-as-code:** set **Railway Config File** to `/railway/dashboard.toml`
   (or by hand: Builder `Dockerfile`, Dockerfile Path `Dockerfile.dashboard`).

5. Add **Variables**:
   ```
   NEXT_PUBLIC_API_URL=https://<api-server-url>.railway.app
   NEXT_PUBLIC_WS_URL=wss://<api-server-url>.railway.app
   ```
   Replace `<api-server-url>` with your actual API server domain from step 4.1.

6. In **Networking**, click **"Generate Domain"** for public URL

## Step 5: Verify Deployment

### Check Service Status

1. All services should show **"Active"** in Railway dashboard
2. Click each service → **Deployments** to view logs

### Test API Server

```bash
curl https://<your-api-server-url>.railway.app/health
# Expected: {"status":"healthy",...}
```

### Test Dashboard

Open `https://<your-dashboard-url>.railway.app` in browser.

## Environment Variables Reference

### API Server

| Variable | Description | Example |
|----------|-------------|---------|
| `DATABASE_URL` | TimescaleDB connection | `${{TimescaleDB.DATABASE_URL}}` |
| `REDIS_URL` | Redis connection | `${{Redis.REDIS_URL}}` |
| `DYNAMIC_TUNER_REDIS_URL` | Redis URL used by dynamic tuner publisher | `${{Redis.REDIS_URL}}` |
| `DYNAMIC_CONFIG_REDIS_URL` | Redis URL used by dynamic-config subscribers | `${{Redis.REDIS_URL}}` |
| `JWT_SECRET` | JWT signing key (32+ chars) | `your-secret-key` |
| `RUST_LOG` | Log filter. Start with `warn`: a crate the filter leaves out is not logged at all, errors included | `warn,api_server=info,auth=info,risk_manager=info,trading_engine=info,tower_http=error` |
| `ENVIRONMENT` | `production` enables the strict secret and sign-in checks | `production` |
| `CORS_ALLOWED_ORIGINS` | Exact origins allowed to call the API (unset: all origins) | `https://app.example.com` |
| `APP_DOMAIN` / `APP_URI` | Site wallet sign-in messages are bound to (required for wallet sign-in outside development) | `app.example.com` / `https://app.example.com` |
| `TRUSTED_PROXY_HOPS` | Reverse proxies in front of the API (0 when exposed directly) | `1` |
| `SKIP_MIGRATIONS` | Skip auto-migrations | `false` |
| `API_PORT` | Server port | `3000` (default) |

### Arb Monitor

| Variable | Description | Example |
|----------|-------------|---------|
| `DATABASE_URL` | TimescaleDB connection | `${{TimescaleDB.DATABASE_URL}}` |
| `REDIS_URL` | Redis connection | `${{Redis.REDIS_URL}}` |
| `DYNAMIC_CONFIG_REDIS_URL` | Redis URL used by dynamic-config subscribers | `${{Redis.REDIS_URL}}` |
| `RUST_LOG` | Log filter (debug floods Railway's log limit) | `warn,arb_monitor=info` |
| `POLYMARKET_CLOB_URL` | Polymarket API | `https://clob.polymarket.com` |

### Dashboard

| Variable | Description | Example |
|----------|-------------|---------|
| `NEXT_PUBLIC_API_URL` | API server URL | `https://api-server-xxx.railway.app` |
| `NEXT_PUBLIC_WS_URL` | WebSocket URL | `wss://api-server-xxx.railway.app` |

## Redis Hardening for Dynamic Config

Recommended production posture is:

1. Only the API process writes `dynamic:config:update` (the dynamic tuner,
   and the manual dynamic-config endpoints, which publish their changes).
2. Subscribers use read-only pub/sub credentials.
3. Redis is private-network only (no public ingress).

This repository supports split credentials by environment variable:

- `REDIS_URL` for general app pub/sub traffic
- `DYNAMIC_TUNER_REDIS_URL` for tuner publish/read
- `DYNAMIC_CONFIG_REDIS_URL` for dynamic-config subscribers

If your managed Redis tier does not support ACL users, keep all three URLs pointed at the same private Redis endpoint and enforce network isolation at the platform layer.

## Local monitoring profile

`docker compose --profile monitoring up -d` adds Prometheus and Grafana, both
reachable from this machine only. Set `METRICS_TOKEN` (Prometheus scrapes the
API's `/metrics` with it), `GRAFANA_PASSWORD` (Grafana's admin login; it
refuses to start without one, or with "admin") and `GRAFANA_DB_PASSWORD`.

Grafana's PostgreSQL datasource reads as `grafana_reader`, never as the
database owner. On every start, once the API is healthy (its migrations
applied), the one-shot `grafana-db-reader` service creates or updates that
login from `docker/grafana/grafana-reader.sql`, as the compose database's
superuser: `SELECT` on exactly the listed trading and analytics tables and
nothing else, at most 5 connections, and by default read-only transactions
with a 30 s statement timeout (session defaults a query could change; the
grants are what holds). Users, credentials, sessions, signed
orders (`execution_intents`) and signed withdrawals (`wallet_withdrawals`)
stay out of its reach. To chart another table, add it to that file's list;
one taken off the list loses its grant on the next start.

## Railway config files

Railway reads one config file per service, by default `railway.toml` at the
repository root, so a single root file would give every service the same
Dockerfile. Each service has its own file under `railway/`, selected by the
service's **Railway Config File** setting (an absolute path such as
`/railway/api-server.toml`). The files follow
[Railway's schema](https://railway.com/railway.schema.json); the root
`railway.toml` this replaces used `[services.<name>]` tables, which the schema
does not define, so none of its settings ever applied.

Railway has deprecated Config as Code: existing files keep working for legacy
services until 2026-12-01
([docs](https://docs.railway.com/reference/config-as-code)). Before then,
`railway config migrate` turns these files into Infrastructure as Code
(`.railway/railway.ts`; see [the CLI docs](https://docs.railway.com/cli/config)).

## Troubleshooting

### "function uuid_generate_v4() does not exist"

The PostgreSQL uuid-ossp extension is not enabled.

**Fix:**
```bash
psql "$DATABASE_URL"
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
```

### Migrations run out of order

SQLx sorts migrations alphabetically. If all migrations have the same date prefix, they may run in wrong order.

**Fix:** Migrations should use sequential numbering (001_, 002_, etc.) rather than date prefixes.

### Dashboard build fails with missing dependencies

The dashboard requires devDependencies for building.

**Fix:** Ensure `Dockerfile.dashboard` uses `npm ci` (not `npm ci --only=production`).

### Service running wrong binary

Each service builds its own Dockerfile. Check that the service's **Railway
Config File** points at its file under `railway/` (or that its Dockerfile
Path is set to its Dockerfile).

### Dashboard can't connect to API

1. Verify `NEXT_PUBLIC_API_URL` matches your api-server domain exactly
2. Ensure api-server has a public domain generated
3. Check CORS settings on api-server

### Build timeout

Rust builds can be slow. Railway has a 2-hour build timeout which should be sufficient, but if builds are failing:
1. Check for compilation errors in logs

## Updating the Application

Once deployed, updates are automatic:

1. Push code to `main` branch
2. Railway detects changes via GitHub integration
3. Rebuilds and deploys affected services
4. Monitor progress in Railway dashboard

## Running Migrations Manually

If needed, run migrations via Railway CLI:

```bash
# Install Railway CLI
npm install -g @railway/cli

# Login and link project
railway login
railway link

# Run migrations
DATABASE_URL=$(railway variables get DATABASE_URL -s TimescaleDB) cargo sqlx migrate run
```

## Estimated Costs

| Service | Estimated Monthly Cost |
|---------|------------------------|
| TimescaleDB | ~$5-20 (usage-based) |
| Redis | ~$5-10 (usage-based) |
| API Server | ~$5-20 (usage-based) |
| Arb Monitor | ~$5-15 (usage-based) |
| Dashboard | ~$5-10 (usage-based) |
| **Total** | **~$25-75/month** |

*Costs vary based on usage. Railway charges for compute time and resources.*

## Multiple Environments (Staging + Production)

Railway supports multiple environments within a single project, allowing you to run staging and production side-by-side.

### Setting Up Staging Environment

1. **Create the environment:**
   - Go to Railway Dashboard → your project
   - Click the environment dropdown (top-left, shows "production")
   - Click **"+ New Environment"**
   - Name it `staging`

2. **Add databases to staging:**
   - Switch to **Staging** environment (dropdown)
   - Click **"+ New"** → **Database** → **TimescaleDB**
   - Click **"+ New"** → **Database** → **Redis**
   - Initialize database:
     ```bash
     psql "<staging-database-url>"
     CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
     ```

3. **Configure branch triggers:**
   - **Production:** Settings → Deploy → Branch: `main`
   - **Staging:** Settings → Deploy → Branch: `develop`

4. **Update staging variables:**

   | Service | Variable | Staging Value |
   |---------|----------|---------------|
   | api-server | `DATABASE_URL` | `${{TimescaleDB.DATABASE_URL}}` |
   | api-server | `REDIS_URL` | `${{Redis.REDIS_URL}}` |
   | api-server | `JWT_SECRET` | New unique 32+ char secret |
   | api-server | `RUST_LOG` | `warn,api_server=debug,auth=info,risk_manager=info,trading_engine=info,tower_http=error` |
   | dashboard | `NEXT_PUBLIC_API_URL` | `https://<staging-api>.railway.app` |

5. **Keep staging off the production wallet.** The trading lock is held
   in each environment's own database, so a staging deployment with the
   production signer would trade the same wallet beside production, under
   its own circuit breaker. In staging, unset (or point at a separate test
   wallet) `WALLET_PRIVATE_KEY`, `TRADING_WALLET_ADDRESS`, `VAULT_MASTER_KEY`
   and the vault file, and leave `LIVE_TRADING`, `ARB_AUTO_EXECUTE` and
   `QUANT_EXECUTOR_ENABLED` off unless that test wallet is meant to trade.

### Development Workflow

```
feature-branch → develop (staging) → main (production)
       ↓              ↓                    ↓
   Local dev     Auto-deploy          Auto-deploy
                 to staging           to production
```

1. Develop features on feature branches
2. Merge to `develop` → auto-deploys to staging
3. Test on staging environment
4. Merge `develop` to `main` → auto-deploys to production

### Running Migrations Per Environment

```bash
# Install Railway CLI
npm install -g @railway/cli

# Staging
railway environment staging
railway run -s api-server -- cargo sqlx migrate run

# Production
railway environment production
railway run -s api-server -- cargo sqlx migrate run
```

### Cost Impact

Adding staging roughly doubles costs (~$60-100/month total) due to separate database instances. Reduce staging costs by:
- Stopping staging services when not in use
- Using smaller database instances for staging

## Trading wallet selection

The bot selects its shared trading signer at API startup. Adding or removing a personal wallet, or changing a user's primary wallet, does not change that signer. Personal primary wallets control balance lookups and withdrawals.

Set `TRADING_WALLET_ADDRESS` in the API service configuration to select a wallet explicitly. Otherwise, startup uses the canonical workspace's saved `trading_wallet_address`. Its private key must be available in the configured vault or in `WALLET_PRIVATE_KEY`, and must derive the selected address. If no address is configured, `WALLET_PRIVATE_KEY` remains supported as the operator's explicit signer configuration.

A missing or mismatched key never causes fallback to another user's wallet. Exchange approvals use the same signer selected for order execution. Changes to signer configuration require an API restart; personal wallet endpoints do not provide a signer reload operation.

Startup rejects a change away from the saved trading wallet while positions are pending, open, recovering, or still hold tokens. Reconcile and close that exposure before rotating the wallet. Importing a key through personal wallet settings alone does not authorize its use by the bot.

Disconnecting a wallet in the vault is refused while the running server signs with it, and, for the saved trading wallet, while any exposure remains. Disconnecting the saved trading wallet once it is idle and flat also clears the saved selection, so the next start uses `TRADING_WALLET_ADDRESS` or `WALLET_PRIVATE_KEY` instead of looking for the erased key.

### Withdrawals that do not resolve

Every withdrawal is journaled (hash, nonce, signed bytes) before it is sent,
and the reconciler settles it from the chain: confirmed or reverted on a final
receipt, or failed when another transaction used its nonce (seen twice, five
minutes apart). Until then it holds back its amount from live buys and, while
it is still `submitting`, blocks further withdrawals from that wallet.

- Before the first deploy of this journal, list older unresolved rows, which
  the reconciler leaves to an operator because they carry no hash:
  `SELECT id, wallet_address, amount, status, requested_at FROM wallet_withdrawals WHERE status IN ('pending','submitting','submitted') AND tx_hash IS NULL AND NOT (status = 'pending' AND journaled);`
  Check each against the wallet's history on Polygonscan and set it to
  `confirmed` or `failed` by hand.
- A transfer still unmined five minutes after it was signed, while the
  network asks more for gas than it pays (for example after a gas spike), is
  signed again at the same nonce for the network's price plus a margin
  (migration 091). The replacement is journaled before it is sent and every
  earlier hash is kept in `replaced_tx_hashes`: only one transaction per
  nonce can be mined, and whichever is settles the withdrawal under its
  hash. This needs the wallet's key in the vault and enough POL for the new
  gas price; the log says "Raised the gas price of a waiting withdrawal" when
  it happens. Run every API replica on a version with migration 091:
  an older reconciler checks only the latest hash and could fail a
  withdrawal whose earlier transaction was mined (the 24-hour receipt watch
  then confirms it).
- A transfer that still does not resolve (no key or POL for a replacement,
  or a node that refuses it for another reason) is logged as "Node did not
  take a re-sent withdrawal" with how long it has been unresolved. To release
  it, send any transaction from the wallet at the same nonce (a zero-value
  transfer to itself at a higher gas price); the reconciler then fails the
  stuck transfer once that nonce is final. Never mark it `failed` by hand
  while its nonce is unused: it can still be mined.

The file vault replaces `VAULT_FILE_PATH` by writing a sibling file and renaming it over the old one. Mount a writable directory (for example a Railway volume) and point `VAULT_FILE_PATH` inside it; a bind mount of the single file cannot be replaced this way.

### Review fix migrations and client compatibility

Apply the repository's migrations before starting the updated API. Migrations 069–072 add durable signed-order intents, exchange order identity/provenance, recovery of closed rows with explicit holdings, and persistent session revocation. Migration 068 now preserves unresolved positions instead of closing them by age. If a deployment already recorded the original 068 checksum, SQLx will require a reviewed migration-history repair; back up the database, reconcile the affected positions, and review migration 071 before accepting the revised checksum. Do not disable checksum validation globally.

REST order placement requires a stable `client_order_id`; withdrawals require a stable `client_request_id`. Reuse the same key and original payload through retries, including lost responses. An order refused before it was sent (its error says nothing was sent) stays refused under its key: send it again under a new one. A changed payload requires a new intent. Private WebSocket clients must send `{"action":"authenticate","token":"<JWT>"}` as their first frame within five seconds; only orderbook-only streams remain public. Browser wallet orders also require a fresh wallet-signed ClobAuth proof, which the dashboard supplies.

Stop all trading workers before changing the pinned signer. A permitted startup selection is saved to the workspace so later restarts recognize the same signer after positions are opened.

A signed live order is journaled before submission. `submitting` or `unknown` intents block further submissions for that wallet after a timeout, process failure or uncertain response. Reconcile the stored exchange identity and signed payload against exchange fills and actual holdings before marking an intent reconciled; do not clear the fence merely to retry. Historical orders with unknown execution provenance cannot be marked cancelled locally. These changes do not submit orders, migrate production databases, or rotate production keys automatically.

### Efficiency improvements

Apply migrations 073–075 with the updated API. They add the complete market-catalog publication timestamp, inventory refresh status, and a covering index for pending or held exposure. Catalog refreshes publish a persisted markets projection; list requests return 503 until its first completed publication and then include observed-at/stale headers. The shared token refresh uses the faster of `ARB_CACHE_REFRESH_SECS` and `QUANT_CACHE_REFRESH_SECS`, with a minimum of one second.

Recovery previews read the last completed inventory snapshot. Use the dashboard's **Refresh inventory** action, or `POST /api/v1/workspaces/{id}/recovery/preview?refresh=true`, to request a scan. Recovery runs require a successful fresh scan. Failed balance reads preserve previous observations and expose failure status; they never imply zero holdings. CLOB balances use six-decimal base units, converted to shares or USDC before valuation and capacity checks. Scans coordinate across replicas and publish inventory batches atomically.


### Correctness repair upgrade (076–078)

Stop the API and arb workers before applying migrations 076–078, then deploy all
workers together. Older writers do not enforce position versions, so do not run
mixed versions during this upgrade. Keep the existing migration-068 checksum
repair procedure above; do not change recorded migration history wholesale.

076 adds position execution mode, wallet identity and optimistic versions. 077
adds wallet-scoped manual inventory with transactional fill accounting. 078 links
position orders to their durable fill receipts. New live positions require a
wallet address. Historical positions intentionally default to `unknown` and cannot
submit automatic exits until their execution provenance has been verified.

Inventory requiring operator verification can be listed with:

```sql
SELECT id, market_id, state, held_yes_qty, held_no_qty
FROM positions WHERE execution_mode = 'unknown';
SELECT execution_mode, wallet_address, market_id, outcome, quantity, cost_basis
FROM manual_inventory WHERE wallet_address = 'unknown' AND quantity > 0;
SELECT id, market_id, exchange_order_id, status
FROM execution_intents WHERE status IN ('submitting', 'unknown');
```

Verify historical position mode and wallet against saved trade events, signed
orders and wallet inventory before explicitly assigning provenance. Do not relabel
all rows from the current runtime mode. Assign each verified position in its own
statement, bumping its version so a stale writer cannot overwrite it:

```sql
UPDATE positions SET execution_mode = 'live', wallet_address = '<verified wallet>',
    version = version + 1
WHERE id = '<position id>' AND execution_mode = 'unknown';
```

A position left `unknown` still settles at its record once its market resolves
(no order is sent): its `closed_via_resolution` event notes the provenance as
unknown, and its result is left out of the circuit breaker.

Historical manual orders with missing
wallet identity require matching their order history and ledger to the verified
wallet in one transaction; their conservative exposure remains reserved meanwhile.
Unlinked historical uncertain intents need verified position/leg or manual-order
association. A missing exchange order or a 404 is not proof of non-execution and
must not be used to delete a fence. New linked intents reconcile automatically
from terminal exchange status and confirmed trade quantities/prices.

Manual trades and automated positions retain separate inventory ownership.
Use position exits for automated holdings and manual sell orders for manual
holdings. Orphan recovery excludes both, including stale orphan flags. Missing
manual fill prices reserve up to $1 per share; historical purchases retain a
conservative basis rather than guessing their remaining lot allocation.

`QUANT_TAKE_PROFIT_PCT` and `QUANT_STOP_LOSS_PCT` are fractions: `0.15` is 15%,
`0.10` is 10%. Review existing overrides before enabling exits. Stop-loss and
take-profit API order types now return a validation error; they are not durable
protective orders. Dashboard development, CI and Docker use Node 24.

### Market data and signals

- The order book stream applies only `book` events as whole books (and
  `price_change` events to them): trade, tick-size and best-bid/ask events
  no longer replace a token's book with an empty one.
- An arbitrage must clear its edge under both the fee terms it was detected
  with (from the market listing) and the exchange's own (`/clob-markets`,
  reread every ten minutes, read alongside the books with a 5 s limit); the
  highest published rate stands in when the exchange's cannot be read. The
  position books the exchange's terms, or the listing's when unread.
- The arb monitor keeps its market universe when a refresh lists none, and
  does not start on an empty one.
- Quant positions book the taker fee each fill pays under the market's own
  terms (read from the exchange; the highest published rate when they
  cannot be read), as arbitrage positions do. Before, they deducted a flat
  2% of cost on each side, which booked a winning buy of a favourite as a
  loss; positions opened earlier keep the flat model. Arbitrage must clear
  both its listed and the exchange's fee terms, and books the exchange's.
- After a quant position in a market closes or fails to enter, no quant
  entry is made there for `QUANT_REENTRY_COOLDOWN_MINUTES` (default 60): the
  generators re-fire on trailing windows that still hold the same evidence.
- The quant generators (flow, mean reversion, cross-market, resolution) read
  `orderbook_hourly`, the hourly aggregate of `orderbook_snapshots`. Nothing
  in this repository writes `orderbook_snapshots`: without an external feed
  the generators emit no signals.
- The risk page's signal thresholds are the arb monitor's own (published
  with its runtime stats); depth and cooldown show a dash while the monitor
  is not reporting. `ARB_SIGNAL_COOLDOWN_SECS` and `ARB_TRADING_FEE_PCT`
  were never read by the monitor and are no longer reported.

- A blank `VAULT_MASTER_KEY` counts as unset (it was used as an empty
  key). A vault file written under an empty key no longer opens: reconnect
  those wallets after setting a real key.

### One trading process (094)

- Only one API process trades: the one holding the Postgres advisory lock
  `trading-leader` on a connection of its own. Another process (the new
  version while a deploy overlaps the two, or a second replica) serves
  requests and waits; it starts the executors, exits, reconcilers and the
  circuit breaker's bookkeeping when the first one stops, reloading the
  breaker's stored state, its saved limits, the stored dynamic config and
  the workspace's service toggles first. Its service status says so.
- The leader re-reads every 5 s what operators change through any process:
  circuit breaker trips, resets and limits, and the service toggles.
  A trip or reset made through the waiting process is stored at once
  (`circuit_breaker_state.operator_epoch` counts these) and the leader adopts
  it rather than overwriting it with its own next save.
- A trip or reset that cannot be stored answers 503 (the trip still holds in
  the process that took it). The saved breaker limits must be readable at
  start; a start that cannot read them stops.
- If the leader's lock connection fails, it stops trading and exits non-zero
  at once (without draining, mid-shutdown too), so a restart waits its turn.
- Recovery runs only on the trading process (503 elsewhere).
- The first deploy of this change still overlaps: the outgoing version does
  not take the lock. Stop the outgoing API before the new one starts if no
  trade may run on two processes at once (a trip stops entries, not exits).
  While both run, each one's breaker saves overwrite the other's counters,
  and the new leader's breaker does not see the losses the outgoing process
  books after the new one took over (until the next takeover's replay).

### Operator controls on every process; exits that do not get out

- Every API process re-reads trips, resets, breaker limits, service toggles
  and the stored dynamic config every 5 s; the trading leader looks for a
  trip made elsewhere every second. A live manual order is refused (409)
  once live trading is switched off, whichever process takes it.
- Breaker limit updates apply only the limits sent, on top of the stored
  ones. Dynamic config updates store all their keys in one transaction; a
  Redis publish that fails is a warning, as the processes re-read the
  database.
- New live entries (arb and quant) wait, with skip reason
  `live_exits_failing`, while three or more live positions are stuck at
  once: each has failed to exit three times since its last fill, or has sat
  in exit-ready or closing for 30 minutes. That is selling failing broadly
  (an allowance, the wallet, the venue); one market with no bids keeps only
  its own position stuck. Entries resume once fewer than three are stuck:
  a position leaves the count when it sells, resolves or is closed.
- A manual close answers 409 while the exit handler is off. Asking again
  for a position already exiting changes nothing.
- A matched trade that ends `FAILED` counts as no fill, so an order whose
  trades failed settles instead of holding the wallet's order fence.
- A process that loses the trading lock stops at once; the next leader's
  reconcilers book what it had in flight. The lock's session uses TCP
  keepalives, and only an error, or three slow checks in a row, counts as
  losing it. A process that waits more than ten minutes for the lock logs
  an error.
- Winning shares are not redeemed by this system: redeem them on
  Polymarket. Until then inventory values a token of a market a position
  saw resolve at its payout ($1 or $0), and such tokens are not offered for
  orphan sale.
- Audit log entries now cover manual orders and cancellations, manual
  closes, wallet-signed orders, submitted withdrawals, cash flows, and every
  workspace update (`workspace/<id>`).

### Reported numbers, tuning and signal transport (095)

- While trading live, the positions summary, trade-flow strategy summaries,
  and the strategy P&L and health snapshots leave paper positions out, as
  the account valuation does. A paper account still sees everything.
- The account summary's `cash_balance` and `total_equity` are `null`, not
  0, when the trading wallet's balance cannot be read (a failed RPC read no
  longer fails the request); `equity_complete` says whether the equity is a
  complete live valuation. Snapshots without the cash are not stored, so
  equity history no longer charts a missing balance as a loss.
- Arb execution telemetry, the arb market scorecard and the
  `canonical_arb_learning_attempts` view (migration 095) count arb entries
  that failed (`entry_failed`) or were left holding one leg
  (`one_legged_exit_ready`) as failed attempts, the second as one-legged.
  They counted none before.
- The positions summary adds `closed_list_count`, the rows of
  `status=closed` (failed entries included); the Closed tab pages by it.
- The arb monitor's entries reach the API only on the Redis signal channel;
  the public orderbook WebSocket no longer carries them. An entry whose
  publish fails or takes over 5 s is not counted and does not start its
  market's cooldown. The API's Redis subscriber reconnects after 3 minutes
  without a message (the monitor publishes its runtime stats every minute).
- The arb monitor re-reads its stored dynamic config every 30 s, so it
  catches up on a Redis update it missed.
- An arb signal stamped more than 2 s in the future is skipped with reason
  `clock_skew` and a warning: the arb monitor's and the API's clocks
  disagree, so no arb trades until they are synced (NTP).
- The dynamic tuner leaves a value set by hand for 24 hours, never changes a
  key while its previous change is being evaluated, and stores a change only
  if the value is still the one it read. It relaxes the minimum book depth
  no lower than 60% of its default, and grows the quant base size only
  after ten closed quant trades in seven days with a net profit, up to twice
  its default.
- `ARB_POSITION_SIZE` defaults to $10, inside its $5-$25 bounds (the
  example env had $50, which the tuner held to $25). Arb executor updates
  through the workspace API also store the key's bounds.
- A WebSocket client that does not take a message within 10 s is
  disconnected.
- The withdraw card shows the most one transfer can take now (USDC.e or
  pUSD, less withdrawals in flight and open buys) and what blocks it.
- The trading leader's lock session also sets a 30 s TCP user timeout, so
  the server ends it when the host dies with data unacknowledged.

### Settled fills, learning attempts and the equity feed (096)

- Reading or cancelling a live manual order books its fills only from
  trades that have settled (confirmed or failed), at their average price.
  While its trades are still settling nothing is booked and the order stays
  open, its cash reserved: a cancel then answers 409 "awaiting exchange
  confirmation" and the reconciler books the fills. A manual market buy was
  costed at $1 a share before, and never corrected.
- Withdrawals hold back, besides open buy orders, the USDC of the trading
  wallet's matched buys whose trades may not have settled on chain: a bot
  entry's buy for 30 minutes after its fill is booked, a manual buy's fills
  until its trades are confirmed; each with the most taker fee it can carry.
  The preflight takes `amount` as optional: without it, it reports the
  wallet's capacity alone.
- An arb entry whose exchange fee terms could not be read books fees at the
  highest published rate (the entry gate already used it), not the
  listing's.
- The circuit breaker's equity feed scans the wallet's inventory afresh
  each snapshot. A reading taken while the trading wallet placed a live
  order (or within 30 s before) feeds no reference, as the scan and the
  cash read could straddle its trade, but still trips on a drawdown its
  best case (the reading plus that order's cash) shows; after three such
  readings in a row the next one feeds it regardless.
- `canonical_arb_learning_attempts` (migration 096) takes each attempt's id
  and telemetry from its entry request, so opened and failed arb attempts
  join their shadow predictions and the rollout guardrails' failure and
  one-legged rates see them. A failed entry later found holding a leg is one
  one-legged attempt, here and in the arb telemetry.
- Strategy performance and health show the strategies of the calculators'
  latest cycle only, and nothing once that cycle is older than two
  intervals: a strategy that traded only in paper no longer shows its old
  numbers once trading is live. In live mode, quant signals acted on in
  paper are left out of their counts. The health failure rate is failed
  entries and exits over entries tried (failed entries were counted twice).
- The dynamic tuner stores values at the column's scale, so the revert after
  a failed publish matches; an evaluation or rollback that finds the value
  changed since it read it leaves it (and announces nothing); and the
  startup seed no longer rewrites an unchanged row, which renewed manual
  holds at every restart.
- After a failed or slow publish, the arb monitor waits 5 s before
  publishing another entry signal.
- A manual close that races the exit handler's update of the position
  retries instead of answering 400.

### Restarts, outages and operator safeguards (097)

- A process that loses its database at start waits for it (logging an
  error every 15 s) instead of exiting; so does the arb monitor for its
  database and Redis. Each exit spent one of Railway's five restarts, and a
  long outage left the services down once it ended.
- While live trading is on without a loaded trading signer (the exchange or
  database failed at start, or live trading was switched on after a paper
  start), every API process tries to load it every 30 s. Before, nothing
  traded live, exits included, until a restart.
- An exit sale refused before it was sent (another live order still held
  the wallet's submission fence, or preparing it ran out of time) goes back
  to the exit queue for the next pass instead of counting as a failed exit
  and slowing the position's retries.
- Trading live needs the Polygon RPC: after three snapshots in a row (15
  minutes by default) without a complete account valuation, the circuit
  breaker trips on connectivity (it cools down and trips again while the
  valuation stays incomplete), since its drawdown guard cannot see equity.
- Strategy health counts an arb entry left holding one leg as a failed
  entry, and "executed" as the entries that opened. Strategy snapshots of a
  period are stored together.
- The arb monitor's runtime stats publish is bounded at 5 s, and its pause
  after a failed entry publish runs from the failure.
- Dashboard and API safeguards:
  - Turning live trading or automatic arbitrage on, pausing or resuming from
    the mobile menu, and starting recovery (which sells orphans at market)
    each ask for confirmation; recovery starts only from a preview that
    loaded.
  - Workspace settings (wallet connection, trading switches, pause and
    resume) are shown to the owner only, as only the owner may change them.
  - Cash flows offer "Transfer in" and "Transfer out" and record each with
    its sign; a fee is money out, and the API refuses a positive one.
  - The risk form no longer resets while it is edited and sends only the
    limits changed. The API refuses a cooldown under a minute and a drawdown
    limit at or past the hard kill.
  - A learning rollout guardrail that is not a plain number is refused
    rather than dropped. The tuner's minimum net profit is labelled in
    dollars per share.
- Migration 097 indexes `execution_intents` by creation time and, for
  reconciled ones, update time.

### Configuration and operator safeguards

- Settings are read the same way everywhere. Yes/no settings take true, 1,
  yes or on and false, 0, no or off (before, `1` or `TRUE` switched off
  settings that default on, among them `FLOW_REQUIRE_CALIBRATION` and the
  account snapshot that feeds the circuit breaker). The switches that let
  the bot spend money (`LIVE_TRADING`, `ARB_AUTO_EXECUTE`,
  `ARB_ALLOW_LIVE_EXECUTION`, `QUANT_EXECUTOR_ENABLED`,
  `EXIT_HANDLER_ENABLED`, ...) take only exactly `true` or `false`.
  Scientific notation such as `1e3` is read. `QUANT_STOP_LOSS_PCT` must be
  a fraction above 0 and at most 1, `QUANT_TAKE_PROFIT_PCT` above 0 and at
  most 10 (1.5 takes profit at +150%), and the `QUANT_*_ALLOCATION_PCT`
  shares 0 to 1. A value that cannot be read halts entries (see below).
- docker-compose passes every setting in `.env` to the API (`env_file`); it
  used to pass a fixed list, dropping the circuit breaker's `CB_*`, the
  quant and tuner settings among others. **Before upgrading, read `.env`
  through**: a setting that compose used to drop now applies, such as a
  `QUANT_EXECUTOR_ENABLED=true`, an `ARB_ALLOW_LIVE_EXECUTION=true`, a
  `CB_*` limit or an `ENCRYPTION_KEY` left there. The API logs the trading
  switches and the circuit breaker limits in effect at start. `env_file`
  with `required: false` needs Docker Compose 2.24 or later.
- `.env.example` documents `ARB_ALLOW_LIVE_EXECUTION` (live arbitrage is
  skipped without it), `ARB_MAX_TOTAL_EXPOSURE`, the `CB_*` limits and the
  quant allocations, and no longer ships a placeholder Alchemy key that
  would be used as one.
- The service status shows strategies as trading reads the switches (the
  environment's as well as the workspace's), and arbitrage as disabled while
  live arbitrage is not acknowledged.
- Railway restarts the API and the arb monitor always (it was on failure, at
  most five times): set Restart Policy to Always on services configured by
  hand. A database or Redis URL that cannot be read stops the process at
  once instead of being waited on; a stop request ends a wait for the
  database; the arb monitor writes its health file once it has started.
- A position whose sales go unsent for ten minutes in a row counts a failed
  exit and is alerted, rather than being deferred indefinitely.
- A tripped circuit breaker refuses manual buys but lets manual sales
  through. A connectivity trip for an account that cannot be valued is
  made once, not on every snapshot; starting live without any Polygon RPC
  logs an error saying so.
- A manual buy's filled cash is held back from withdrawals for at most 30
  minutes after its last fill was booked.
- The dashboard offers Polygon only (it also listed a testnet), and its
  image build takes `NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID`. The images no
  longer set `RUST_LOG=info`, which logged every database query.
- An `ENCRYPTION_KEY` set but empty is logged as an error at start.
- Staging: see "Keep staging off the production wallet" above.

### Settings checked at start, the arb monitor and signal analytics

- **A setting that cannot be read, or is out of its range, halts new
  entries**: the API logs it, naming the setting and what it expects,
  holds new entries in every API process (manual buys included), and its
  trading leader trips the circuit breaker (latched, as an operator trip)
  while exits keep running; correct the setting, restart, and reset the
  breaker. A reset is refused (409) while the hold stands: it would clear
  the trip and leave entries held. The arb monitor does not start. Refused: a yes/no value such
  as `disabled`, a money switch other than `true`/`false` (`TRUE`, `1`), a
  number with a comma or unit, a fraction written as a percentage, a
  decimal where a whole number goes, or a value that cannot be meant: a
  zero loop interval (it would spin), `QUANT_MAX_HOLD_HOURS=0` (every quant
  entry sold at once), `ARB_UPDATE_TIMEOUT_SECS` under 10 (reconnecting
  without pause), a negative size, exposure or depth, a non-positive daily
  loss, `QUANT_TAKE_PROFIT_PCT` above 10, `ARB_MIN_POSITION_SIZE` above
  `ARB_MAX_POSITION_SIZE` (either side at its default when unset), or
  `VAULT_PROVIDER` other than `file` or `environment` (`aws` is not
  implemented). `ARB_MAX_TOTAL_EXPOSURE=0` and `QUANT_MAX_POSITIONS=0`
  stay valid: they refuse new entries. Every numeric setting is checked,
  about 150 in all (`SETTINGS` in `crates/polymarket-core/src/config.rs`
  lists them with their ranges); a test keeps that list in step with the
  code and checks `.env.example` against it. Before, such a value fell back
  to the setting's default, which left a service meant off running. A blank
  value is unset. `ARB_ALLOW_LIVE_EXECUTION=1` was accepted as on and is
  now refused: write `true`.
- Live trading with `ACCOUNT_SNAPSHOT_ENABLED=false` logs an error when the
  trading leader starts: the circuit breaker then gets no equity readings.
- The status page reads `DYNAMIC_TUNER_ENABLED`/`DYNAMIC_TUNER_APPLY` as
  the tuner does (`on` is on; blank is the default). A blank
  `DYNAMIC_CONFIG_ALLOWED_SOURCES` is unset rather than an empty list that
  refused every dynamic update.
- The default log filters now start with `warn`
  (`warn,api_server=info,auth=info,risk_manager=info,trading_engine=info,...`):
  the old filters dropped the circuit breaker's and the order executor's
  messages entirely, errors included. Update a `RUST_LOG` set on a service
  the same way.
- `ENCRYPTION_KEY` can be set after secrets were saved: a workspace secret
  saved under `JWT_SECRET` is still read (and logged; save it again to move
  it to the new key). A secret that neither key decrypts is reported and
  not used, rather than its ciphertext being sent as the Alchemy key.
- docker-compose passes the arb monitor only the settings it reads (it had
  every `.env` secret). The arb monitor exits when its loop stops for five
  minutes (Railway and compose restart it), and its compose health check
  fails when the health file is five minutes old. A stop signal during
  migrations or startup ends the API's start.
- `.env.example` no longer sets `TRUSTED_PROXY_HOPS=1`: copied as is under
  compose, which publishes the API directly, it let clients choose the
  address they are rate limited by.
- Arb monitor: YES and NO books are paired only from the same websocket
  connection (after a reconnect, a fresh side no longer pairs with the other
  side's pre-gap book); a book that turns crossed is dropped until the next
  snapshot; each price-change event is applied whole before it is
  published; an entry signal no API is subscribed to counts as not sent; a
  failed market listing is retried after two minutes, doubling up to the
  refresh interval, and one under half the size of the last is applied
  only once a second listing confirms it at about its size, or after three
  such listings in a row; one malformed Gamma market no longer fails its
  page.
- Quant: the flow strategy's calibration measures returns on what each
  position cost (it used the signal's suggested $30, reading a 4% gain as
  1.2%) against the uncalibrated edge, and leaves out unconfirmed
  resolution wins. Calibrated flow edges were understated in proportion
  to how far positions were sized below $30; expect more flow signals to
  clear the minimum edge. Mean reversion trades only active
  markets that do not end within a day, priced 0.08-0.92. The resolution
  strategy's default allocation is 0: it follows the favourite with no
  estimate of its own, so it has no modelled edge. Set
  `QUANT_RESOLUTION_ALLOCATION_PCT` to trade it.
- Bot scores: a wallet is scored again a day after its last score, one
  with too few transfers after six hours, and one whose analysis failed
  after half an hour; before, each wallet was scored once per process.
- Market metadata marks a market inactive once two complete Gamma listings
  in a row leave it out (a listing with unreadable rows, or under half the
  size of the last, does not count); before, closed markets stayed active
  for good, and the signal generators reading `active` traded them.
- The market regime averages only wallets with five or more closed trades
  in 30 days (buy-only wallets read as losers and held it bearish), and the
  tuner's volatility term uses hourly price movement rather than wallets'
  annualized return volatility, which held the arbitrage threshold at its
  ceiling.
- Backtests: volume fee tiers advance with traded volume (they used fees),
  and Sortino ratios divide downside over every day, as wallet metrics now
  do.

### Live order and close safeguards (098)

- A live order whose connection to the exchange never opened is recorded as
  not sent (it was recorded as of unknown outcome, which held every live
  order of the wallet, exits included, for at least ten minutes until the
  reconciler cleared it). An exit sale not sent this way is retried next pass.
- Migration 098 adds `positions.breaker_recorded_at`. Each close is marked
  in the same transaction that saves the circuit breaker's counters with it
  in them; a new trading leader records the closes a stopped process booked
  but never saved (today's; earlier days' are only marked) before it
  trades. Existing closes are marked by the migration; the first takeover
  marks, without counting, those the previous version makes while handing
  over (see migration 100 below). The 24-hour realized
  P&L is now dated by close time, so neither the migration nor the marks
  move closes into it.
- Orphan recovery no longer sells tokens a position sold in the last 30
  minutes (the balance can still show them until the sale settles), nor
  tokens of a market a position closed at resolution with equal legs.
- An exit whose market tokens cannot be read stays ExitReady for the next
  pass instead of sitting in Closing until housekeeping; one that cannot be
  put back in the queue is recorded as a failed exit.
- An arbitrage pair's buying-power check holds back the cash the wallet's
  resting manual buys reserve at the exchange, so the second leg is not
  refused after the first has filled.

### Entry holds, effective switches and removals

- A new trading leader holds new entries until it has recorded the closes a
  stopped process left uncounted; if it cannot (three tries), it starts
  trading with entries held and retries every 30 seconds, looking only at
  closes made before it began (it records its own). A resolution win counts
  only when its close event says the payout was measured.
- A close is saved with its mark all or nothing: a mark that fails fails the
  save, so the close stays counted only in the process's memory and is
  retried within seconds (a close is never in the stored counters while
  unmarked, so a new leader cannot count it twice); meanwhile what halts
  trading (the trip, latches, recovery, equity references, holds) is stored
  on its own, so a trip still reaches every process and survives a restart.
  A graceful shutdown stores what is still owed. A close the switched-off
  breaker does not count is marked at once.
- Migration 099 adds `circuit_breaker_state.leader_epoch` and `entry_holds`.
  Each process taking over trading counts the leader epoch up before it
  reads the state, and a save carrying an older epoch is refused: the
  process that made it stops saving and exits (its lock watcher sees the
  breaker step down), so a save stalled across a change of leader can no
  longer overwrite the new leader's counters. The leader stores its entry
  holds there (the missed-close hold right after its claim), so every API
  process refuses entries, and a reset, while they stand.
- Migration 100 adds `circuit_breaker_state.marked_write_at`, which this
  version sets equal to `updated_at` whenever it stores the loss counters.
  A claim that finds the two apart (an earlier version, which counts its
  closes without marking them, wrote the row since: the first takeover
  after the upgrade, or after a rollback and a return to this version)
  marks every close left unmarked, in the claim's own transaction, so no
  takeover (a retried one included) counts them again. A close that earlier
  version booked without saving it is not counted. A hand edit of the row
  (anything but this version's code moving `updated_at`) is read the same
  way: the next claim marks rather than counts the closes left unmarked,
  a close this version counted but had not stored when its process died
  included. Stop this version gracefully before rolling back (it stores
  what it owes as it stops).
- A process taking over trading saves nothing until it has read the stored
  state (a takeover retried after a failed read could save its earlier view
  over it). A process that does not lead resumes after a trip's cooldown,
  and advances recovery stages, only when the leader has stored it: on its
  own it could resume while the leader stays tripped. The leader does both
  on time (every 5 s, after re-reading the limits), whether or not anything
  asks it to trade (it used to wait for a trade attempt, which never came
  with the executors off), except while it holds entries (the 5 s pass also
  skips it when it cannot read the limits); a recovery stage that entries
  were held during starts again, its trades and profit not counted.
- The bound of 20 wallet orders awaiting a signature per user now holds
  for requests made at once (checked again under a per-user lock as the
  order is stored).
- Cancelling an order is no longer refused while the server shuts down
  (placing one still is); the shutdown waits for it.
- A process that loses trading leadership stops saving the circuit breaker
  and starting trades at once, and exits, even mid-shutdown (it used to
  finish draining, its background tasks still booking closes beside the
  next leader). A failed takeover stops acting as leader the same way.
- The dashboard shows trading as the server runs it: switches the
  environment holds on (`LIVE_TRADING`, `ARB_AUTO_EXECUTE`,
  `EXIT_HANDLER_ENABLED`) read as on and cannot be switched off there, and
  Pause/Resume wait until the server's switches are read.
- Removed: the unfinished CEX latency arbitrage module (`LATENCY_ARB_*`
  settings, `BINANCE_WS_URL`), which placed no orders, and the Kelly sizing
  helper only it used. Its table `latency_arb_signals` (065) stays; drop it
  with a new migration if its rows are not wanted.

### Dynamic config

- A tuner change whose Redis publish fails stays stored (every process
  re-reads the stored config within seconds); it used to be reverted, so a
  Redis outage froze the tuner.
- With `DYNAMIC_TUNER_ENABLED=false`, every API process removes at start,
  before it applies the stored config, the dynamic config rows still
  holding their seed (an earlier run's or a migration's, such as
  `QUANT_BASE_POSITION_SIZE=30`), which it applied over its environment;
  the environment applies instead. Values an operator set, or the tuner set
  when it ran, stay. Restart the arb monitor after the API when switching
  the tuner off: it keeps the values it read at start for removed rows.
- Migration 101 deletes the `QUANT_MIN_CONFIDENCE`, `FLOW_MIN_IMBALANCE`
  and `MEAN_REV_MIN_MOVE_PCT` rows: nothing read them (their environment
  settings apply, unchanged), yet the tuner table listed them.

### Positions trade in their own mode; replay proof; paper fidelity

- Switching live trading off (Pause) stops new live entries only, including
  one that chose live just before (its first buy is not sent; a pair whose
  first leg went out still sends its second). Exits, recovery sales and
  orphan sales of live positions keep running (they used to be refused,
  leaving live positions unsold): the trading signer is loaded at start, and
  kept loaded, while live positions, orders or unresolved submissions
  remain, whatever the switch. Paper positions likewise still exit while
  live trading is on. Their trade events carry the position's own mode.
- Paper manual orders are recorded without a wallet, and migration 102
  merges paper inventory kept under a wallet address (paper orders placed
  while a signer was loaded) into the single paper key.
- Open paper positions no longer block pinning or changing the trading
  signer at start (they hold nothing in any wallet).
- A live arbitrage pair checks its cash again, after its entry is
  recorded, when a withdrawal was reserved since the first check (from then
  on new withdrawals wait for the entry), so a withdrawal can no longer
  take the second leg's cash in between. A buy placed elsewhere at that
  moment still can; the pair's one-legged recovery then sells the first
  leg.
- Withdrawal timers (the second look at a superseded transfer, rebroadcast
  and fee bump) measure ages on the database's clock, so a process clock
  ahead of it no longer cuts the wait short.
- A sale or close the order reconciler settles is booked under the strategy
  that opened the position (it was booked as `recovery`).
- The circuit breaker's replay of missed closes (and the quant strategies'
  own breakers) counts a win only when its close event was written and
  does not say the payout went unmeasured; losses always count.
- Paper manual orders: a pending one abandoned for ten minutes is rejected
  (it held exposure and inventory for good), and a paper limit order fills
  only when the book can fill all of it now at its limit or better, at the
  book's prices; one that would rest is refused (it used to fill at its own
  limit price, whatever the book).

### Entries, access checks and monitor health

- Arbitrage and quant entries skip a market that has closed or stopped
  taking orders (`market_not_tradable`) since the last market catalog
  refresh or single-market read; they used to buy from cached tokens until
  the next refresh and have the order refused, or a pair left one-legged.
  Exits still find those markets' tokens.
- Routes that spend shared resources now need membership in the trading
  workspace (403 otherwise): storing a wallet key needs trading rights there
  (owner, admin or member, with a trading account); a wallet's balance (RPC
  reads), a market missing from the database and its order book, and a
  wallet's trade history (exchange reads) need any membership. Any
  self-registered account could use them before, and grow the shared vault.
- The wallet research endpoints (`/api/v1/discover/trades`,
  `/api/v1/discover/wallets`, `/api/v1/discover/wallets/:address`) need
  sign-in and workspace membership; they were public, and live trades read
  the exchange's data API on every request. `/api/v1/regime/current` stays
  public. The dashboard does not call the discover endpoints.
- A workspace invite lapses (410) once its sender could no longer send it:
  removed from the workspace, demoted below admin, or (for an admin invite)
  not the owner. Removing or demoting a member also withdraws the invites
  they sent that are still pending. An invite from a removed admin used to
  grant membership for its full seven days. An owner invite (sent by a
  platform admin creating a workspace for someone without an account) lapses
  once its sender is no longer a platform admin.
- Outside development (`ENVIRONMENT` other than development/dev/local/test),
  a startup with no `CORS_ALLOWED_ORIGINS` logs an error: every origin may
  call the API. Set it to the dashboard's origin.
- The mean-reversion and cross-market generators read hourly bars as the
  materialized aggregate holds them: the newest bar usually starts two to
  four hours ago (it is refreshed hourly, an hour behind). Mean reversion
  looked back only two hours, so it never found two bars and emitted
  nothing; it now looks back six hours and compares the two newest bars only
  when they are adjacent hours. Cross-market measured its "four hour" moves
  from NOW(), which left one or two bars, so a market with a single bar read
  as "did not move"; each move is now the four hours to the market's newest
  bar, needs both bars, and a pair compares the same hours.
- The arbitrage monitor exits (to be restarted by its supervisor) when no
  order book data has arrived for five `ARB_UPDATE_TIMEOUT_SECS` periods
  (ten minutes by default) while it is subscribed to markets. A subscription
  that delivered nothing was resubscribed forever while the health file
  stayed fresh, so the service looked healthy with no signals.
- The periodic data hygiene run also deletes session revocations a day
  past their token's expiry (the table only grew).
- Browser-wallet orders (`/api/v1/orders/prepare` and `/submit`) from the
  bot's trading wallet (the loaded signer or a pinned trading wallet) are
  refused (400), at prepare and again at submit. Their fills were recorded
  nowhere the bot reads: a buy was sold as orphan inventory and its loss
  booked against the circuit breaker, a sell took shares from under the
  bot's positions. Use the trading page for that wallet. The submit answer
  now says the app does not track fills for browser-wallet orders, and the
  signing summary shows the exact cost.
- Live buys hold back the cash other recorded entries are about to spend:
  every live buy, and a pair's cash check, counts each other pending live
  entry's legs not yet bought (at their expected prices, plus the most taker
  fee). A pair also checks its cash again every time once its entry is
  recorded, not only after a withdrawal. A quant entry or manual buy could
  take a pair's second-leg cash between its check and its second leg,
  leaving it one-legged; now the entry checking second refuses instead.
- Per-strategy quant breakers (migration 103): each close is counted once,
  marked on the position (`strategy_breaker_recorded_at`) in the same
  transaction that saves the breaker state. The executor used to replay the
  day's closes over the saved state on every start, which could re-halt a
  strategy whose losing streak had already been reset and restarted a
  halt's cooldown. The migration marks closes made before it as counted.
- The order reconciler treats an order the exchange reports MATCHED as done
  even when its matched size is a rounding short of its size (it stayed
  unresolved, fencing every live order of the wallet), and measures order
  ages on the database's clock.

### API contract changes

- Service status adds `live_entries_held`: why the trading process refuses
  new live entries now whatever the switches say (exit handler off or not
  running, live exits failing), null otherwise; live strategy modes carry
  the same reason. Risk status drops `stop_loss` (always zeros since the
  stop-loss rules were removed). `POST /workspaces/:id/switch` is removed
  (there is one canonical workspace; nothing called it).
- The tuner status's watchdog fields read what the tuner stores
  (`attempts_last_window`, `successful_fill_rate`) and its own verdict (the
  card always showed idle). `GET .../dynamic-tuning/arb-executor` returns
  `max_signal_age_secs` (it was always null).
- Service status adds `workspace_id` (the canonical workspace it describes)
  and `environment_switches`. Risk status adds `entry_holds`; a breaker
  reset answers 409 while any stands.
- Positions add `execution_mode` (`live`, `paper`, `unknown`) and `source`
  (`arb`, `quant`, `copy_trade`, `manual`); `outcome` is read from the legs
  entered when none is stored (it was `both` for every position).
- A placed order's response adds `rejection_reason` when it was rejected;
  `filled_at` is set only on a complete fill, and `remaining_quantity` is
  never negative.
- Quant summaries count a signal skipped for its expiry as expired (the
  status `expired` was never written), and a strategy's skip rate counts
  both.

- `GET .../risk/status` and the trip/reset responses add `drawdown_peak`
  (the soft drawdown trip's reference), `equity_seeded`, and
  `config.hard_kill_drawdown_pct`.
- Positions add `is_open`: a one-legged failed entry still holds a leg the
  exit flow manages and counts as open, as in the `status=open` list.
- `PUT .../dynamic-tuning/arb-executor` answers 400 for a value outside its
  bounds instead of storing the nearest bound.
- Learning rollout `bounds` and `guardrails` accept only their known keys,
  each a number in range (rates and drawdown are fractions; latency in ms);
  a misspelt or string-valued guardrail used to be skipped silently. The
  unused `bounds.threshold` is dropped on save.
- Backtest `total_return_pct` and `win_rate` are fractions (0.10 = 10%),
  `max_drawdown_pct` is in percent; the dashboard now shows them so.

### Controls, audit and sessions

- Circuit breaker limits are checked at startup: a drawdown or hard-kill
  limit outside 0–1 (they are fractions: `CB_MAX_DRAWDOWN_PCT=0.20` is 20%;
  a hard kill of 0 is off;
  20 would never trip), a run-of-losses limit below 1, a negative daily loss
  or cooldown, or a `CB_*` value that does not parse stops the start with
  the reason. The API refuses the same values.
- Switching the breaker off stops its limits but not a latched trip (manual
  or hard kill): only a reset lifts that. Breaker settings are stored before
  they take effect, and every change, trip and reset is written to the
  audit log with who made it, as are wallet connects, disconnects and
  primary changes, recovery runs, manual dynamic-config changes, and each
  withdrawal's outcome. These audit records are written before the request
  is answered (no longer queued, where a full queue or a crash lost them).
- Workspace toggles for live trading, the exit handler and arb execution
  change the running services only on the canonical workspace, and a
  service the environment switches on (`LIVE_TRADING`,
  `EXIT_HANDLER_ENABLED`, `ARB_AUTO_EXECUTE`) stays on, as at startup:
  switching one off in the workspace then answers 409 and changes nothing
  (trip the circuit breaker to stop trading at once). Status endpoints
  report live trading from what the executor does.
- An environment variable set but empty counts as unset (the trading
  wallet, its key, the RPC settings, `CB_*` limits, the vault): `docker
  compose` passes those through only when they are set.
- Stored dynamic-config values (manual overrides and tuned values) apply
  before the executors start, whether or not Redis is reachable. With
  dynamic arb sizing on (the default), the configured arb position size is
  a ceiling on each pair. The tuner sizes on the results of the mode it
  trades in only.
- Deleting a user with connected wallets or withdrawal records, and
  deleting the canonical workspace, are refused.
- An account connects at most 20 wallets and holds at most 20 prepared
  browser-wallet orders awaiting a signature; expired ones never submitted
  are cleared (a submitted one is kept: it answers a retry).
- Rate limits are per minute as configured (auth 5, config 10, admin 30
  per client), not one request a minute after the burst.
- Request logs carry the matched route template (`/api/v1/invites/:token`),
  never the request path or query, which can hold invite tokens.
- A session renews by refresh for at most 7 days from sign-in; each refresh
  revokes the token it replaces.
- `PG_MEMORY_TUNING` (default on only with `ENVIRONMENT=production`) runs
  the `ALTER SYSTEM` memory settings after migrations; a local database is
  no longer reconfigured.
- The dashboard asks before sending a withdrawal, disconnecting a wallet
  (its key is erased), tripping or resetting the breaker, and saving it
  switched off. It defaults to the API on port 3000 and runs on 3002
  (`npm run dev`). `ENCRYPTION_KEY` (default `JWT_SECRET`) encrypts stored
  workspace secrets: set it separately before rotating `JWT_SECRET`.
  `DASHBOARD_URL` sets where invite links point.
- `docker compose` passes the live trading settings (`EXIT_HANDLER_ENABLED`,
  `TRADING_WALLET_ADDRESS`, `WALLET_PRIVATE_KEY`, `POLYGON_RPC_URL`,
  `ALCHEMY_API_KEY`, `VAULT_*`) to the API and keeps a file vault in the
  `api_data` volume.

### Restored tables, withdrawal and fee accounting (089–092)

- 089 records when a withdrawal is first seen mined, so the circuit breaker
  keeps holding its equity feed until the transfer's cash flow is applied.
- 090 recreates `workspace_invites` and `pending_wallet_orders`, which 044
  dropped although invites and browser-signed orders still use them.
- 091 keeps every hash broadcast for a withdrawal (fee bumps, above).
- 092 records an arbitrage position's taker fee terms and the fees charged to
  its fills (`fees_paid`), which its P&L now deducts: Polymarket charges a
  taker `C × rate × p(1 − p)` USDC per fill on top of the order amount.
  Positions opened before 092 were all bought on the V1 exchange, which kept
  buy fees in shares: the wallet alignment before a sale books those as
  shares lost. They take their market's terms from the exchange
  (`/clob-markets`, `fd`) at their first exit sale, so the fees of their
  sales are charged; quant positions keep the flat 2% fee model. A share
  shortfall is never credited back as a fee: the V2 exchange charges every
  fee in collateral. Live buys also keep room for
  the highest published fee on top of their amount, so a buy cannot spend
  USDC held back for an unsettled withdrawal.

The fee is charged apart from the fill amounts. The CTF Exchange V2 contract
(`_settleTakerOrder` in Polymarket's ctf-exchange-v2 `Trading.sol`) pulls a
taker buy's fee on top of the collateral the fill costs and deducts a taker
sell's fee from its proceeds, and its `OrderFilled` event reports the fill
amounts and the fee separately. The accounting books exactly that: fills at
their pre-fee amounts, fees charged on top.

### Circuit breaker during withdrawals

While a withdrawal settles the breaker holds its equity feed (the balance may
already lack the cash), but it still checks drawdown against the most equity
can be: the snapshot plus every withdrawal and outbound flow not yet applied.
It trips only on a loss even that shows, so withdrawals one after another
cannot keep the breaker from seeing a real loss for longer than they last.

### CLOB V2 and pUSD collateral (093)

Polymarket moved the CLOB to V2 on 2026-04-28 and no longer accepts orders
signed for the V1 exchange. Orders are now signed for the CTF Exchange V2
(`0xE111180000d2663C0091e4f400237545B87B996B`) or, in neg-risk markets, the
Neg Risk CTF Exchange V2 (`0xe2222d279d744050d28e00520010520000310F59`),
with EIP-712 domain version `"2"`. The signed order carries a creation time
in milliseconds, metadata and builder fields, and no longer a nonce, fee rate,
taker or expiration (a good-till-date order still sends its expiration,
unsigned). Fees are set by the exchange when an order matches.

- The exchange's collateral is pUSD (`0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB`),
  backed 1:1 by USDC.e. At startup the API approves pUSD and the Conditional
  Tokens for both V2 exchanges and the Neg Risk Adapter, USDC.e for the
  Collateral Onramp and pUSD for the Collateral Offramp. Fund the wallet with
  a little more POL than before (eight approvals on a fresh wallet).
  Approvals are sent only under the wallet's spend lock: waited for up to 30
  seconds at startup, then in the background (startup goes on); one left
  unmined is signed again at its nonce for more gas (up to three times the
  network's price), but only while the node holds one of its transactions
  or counts nothing pending at its nonce. Approvals and wraps read the chain
  through `POLYGON_RPC_URL`/`ALCHEMY_API_KEY`, else the canonical
  workspace's RPC settings, else the public endpoint (with a warning).
- Nothing the API signs pays more than 10,000 gwei per unit of gas: a node
  reporting more (a unit error) gets nothing signed.
- The API then wraps the trading wallet's USDC.e into pUSD, at startup and
  every five minutes, so deposits become tradable without a restart. A wrap
  waits until no withdrawal from the wallet is unresolved and nothing it sent
  is pending, and keeps the wallet's spend lock until it is mined and balance
  reads include it (normally seconds): a withdrawal planned meanwhile would
  draw on USDC.e the wrap is about to take. A withdrawal request waits up to
  20 seconds for the lock, then answers 503 to retry; nothing is sent. A wrap
  that waits is looked after every 30 seconds: sent again if the node dropped
  it, its gas raised when the network asks more or the node refuses it (up
  to three times the network's price), until its nonce is used, by it or, as
  checks over two minutes find, by another transaction. Nothing is sent
  while the node counts the wrap's nonce pending without holding the wrap
  (another transaction holds it). A held lock whose
  database session stops answering is given up, that session is ended from
  another connection (a proxy can keep it alive at the server, holding the
  lock), and the lock is taken again. Any spend lock given up this way, or
  whose release does not answer within ten seconds, is freed the same way.
  After half an hour the
  lock is let go with a warning; a restart lets go of it too. From then on
  the lock is taken only for one check (about ten seconds), when free, and
  a lock that cannot be read leaves the wrap watched: the wrap's gas is still
  raised while the node holds it, but it is never sent again. One the node
  drops (as checks over two minutes agree) is let go, and a later look wraps
  afresh under the lock. Without the lock nothing is sent. A withdrawal
  still waits (503) while the node holds the wrap, and goes out once the
  node has mined or dropped it. The log says "Wrapping USDC.e into pUSD";
  USDC.e left waiting for an hour is logged as a warning. The sweeper wraps
  for the signer selected at startup.
- Cash, for equity, the circuit breaker and the vault balance, is the
  wallet's USDC.e plus its pUSD, both read at one block.
- A withdrawal still pays the recipient USDC.e: from the wallet's USDC.e with
  a transfer when that covers it, otherwise from its pUSD through the
  Offramp's `unwrap`, which pays the recipient directly. One withdrawal draws
  on one of the two, so the most one can take is the larger capacity. Every
  withdrawal is simulated before it is signed: one the node expects to fail
  (the token already wrapped, the Offramp paused) is refused with 409 and
  nothing is sent. Destinations that are token or Polymarket contracts are
  refused. While the wallet has a transaction waiting to be mined that is
  not one of its own sent withdrawals (a wrap, an approval, anything sent
  from elsewhere), a withdrawal answers 503 to retry and nothing is sent.
  A withdrawal's gas bump re-signs its journaled transaction exactly,
  whichever kind it is.
- Which exchange a token's orders are signed for is looked up from the CLOB
  and cached; a failed lookup fails the order instead of guessing, and an
  arbitrage looks up both legs before buying either. The CLOB's V2
  balance-allowance answer (an allowance per spender) and order answer
  (`success`, `errorMsg`, `transactionsHashes`) are read. Only an answer
  with `success: false`, no order id and no status saying one was placed
  proves no order exists, and only `success` with `matched`, `live` or
  `delayed` proves one was taken. Any other answer (a refusal that names an
  order, a missing or contradictory status, or `unmatched`: "marketable but
  failed to delay. Placement still succeeded", per Polymarket's order
  creation reference) leaves the order unknown: the wallet's next live order waits until the reconciler
  has read it from the exchange, under the order's own hash and any id the
  answer named, and a manual order it finds resting stops holding the
  wallet back (its row keeps the cash reserved). A fill-or-kill order that
  cannot fill is expected to be refused with an HTTP error, as V1 did. An
  order the exchange reports `CANCELED`, `CANCELED_MARKET_RESOLVED` (its
  unfilled rest cancelled when the market resolved) or `INVALID`, with or
  without the `ORDER_STATUS_` prefix, is closed: its confirmed fills are
  applied and nothing more is waited for.
- Browser-wallet orders cannot be read back from the exchange (the server
  keeps no credentials for those wallets). One whose outcome is unknown
  holds back further orders from its wallet until ten minutes after its
  expiry (at most a day), and never the trading wallet's withdrawals.
- Before a live position is sold or closed at resolution, the legs that pay
  (the winner's, or both without a winner) are measured against the
  wallet's balance: a buy whose fee the V1 exchange kept in shares left the
  record above it. A small gap is written off from all the wallet's
  positions in that market at once (in one transaction), each by its
  holding. A winner is taken only from one outcome flagged the winner; without
  one, a pair pays a dollar a pair while its measured legs are equal, and
  otherwise waits for the winner. A resolution close waits, and logs
  why, while an order of the wallet on the market may still settle, and for
  up to an hour while the balance or the market's tokens cannot be read (a
  live executor whose signer failed to load counts as unreadable: a restart
  may load it). A
  balance far below the record (shares redeemed or moved), one still
  unreadable after that hour, a position of another wallet than the
  trading wallet, or a paper executor (it never loads a wallet) leaves the
  record standing: the position closes at it, with a warning,
  and its `closed_via_resolution` event lists the legs not measured and why
  (metadata `unmeasured`). The hour counts only an unbroken run of
  unreadable balances. A position whose market resolved with no single
  winner flagged and whose legs differ (a resolution not final yet: trading
  can stop days before it; flags not yet set; or a split payout) is never
  booked at a guessed payout: it waits for the winner, with a warning at
  each look. The first such look is recorded as a
  `resolution_waiting_for_winner` trade event for an operator; a split
  payout must be settled by one.
- A break-even close neither extends nor ends the circuit breaker's run of
  losses. The breaker guards real money: only a live position's close counts
  in it, live trading switched off or not. Paper results (and those of
  unknown provenance) never do, so a paper win cannot end or offset a run
  of live losses; paper trading no longer trips the breaker by its own
  losses. A resolution close the wallet could not confirm feeds the breaker
  a loss but never a win.
- A daily loss trip holds for the rest of its UTC day, cooldown or not. The
  API does not start while the breaker's stored state cannot be read (it
  retries for a few seconds first): a fresh breaker would trade through a
  latched trip and overwrite it.
- Losses recorded while the breaker is already tripped still count. One
  that breaches a stronger loss limit than the trip's own (the day's loss
  over a run of losses, a run of losses over a drawdown) gives the trip
  that reason: the cooldown restarts from then (never ends sooner), and a
  daily loss trip holds to the end of the UTC day. A manual or kill-switch
  trip is never relabelled. When a cooldown ends, the day's loss is weighed
  again first, so a trip never resumes into a day already over its limit.
- Markets are checked for resolution, and resolved positions settled,
  whether or not the exit handler is enabled: settling sends no order, and
  an unsettled position keeps its exposure counted and its market taken.
- A quant signal enters only a market where no position (of any source) is
  active, and only while the quant executor holds fewer open positions than
  its limit. Both are decided with the entry, under the lock every entry
  takes, so concurrent signals or API processes cannot exceed them.
- An arb signal is skipped while any position in its market is still active,
  read from the database (an entry still settling included), so a restart
  or a reconciled entry never lets a second pair into the same market. The
  entry itself is refused under the lock every entry takes when its market
  is taken meanwhile, so two API processes acting on one signal open one
  pair. Each leg must be at least the exchange's one-dollar minimum market
  buy, or the pair is skipped: the YES leg at its ask, and the NO leg (sent
  second) at the shares the YES leg's whole cents buy, priced at an ask the
  default slippage (1%) below its own. The NO leg waits up to 30 seconds
  (other orders five) for another order of the wallet still being posted,
  so a slow answer to another strategy's order does not leave the pair
  one-legged.
- A buy the exchange fills with more shares than it signed for (a better
  price passed on to the taker) counts as filled with every share; a sell
  never fills beyond its size. A manual market order matched in full reads
  as filled though its signed size was rounded.
- An order of unknown outcome holds back the wallet's live orders until it
  is reconciled, a position's order that stays unresolved holds its
  position (its exposure and its market), and a manual order that cannot be
  settled keeps its cash reserved. Any of them still unresolved after a
  quarter of an hour (a manual order resting on the exchange aside) is
  warned about every ten minutes ("An order still unresolved holds
  back..."). A live entry stopped before linking its first
  order (it never sent one) fails after five minutes.
- The reconciler releases an order the exchange never received only after
  ten minutes and only when no trade of the wallet names it. It cannot
  decide an order the exchange acknowledged and no longer returns, or one
  whose trades name an order the exchange cannot find; those wait for an
  operator, and the warning above names each (`intent_id`). Look it up with
  `SELECT id, position_id, exchange_order_id, status, created_at FROM execution_intents WHERE id = '<intent id>';`
  then in the exchange's open orders for the wallet (a resting order is
  not stuck: cancel it first if it should go), its trade history (the
  CLOB's `/data/trades`, by `exchange_order_id`) and its token transfers on
  Polygonscan. Only when the order is not open and filled nothing, mark it
  refused, and the reconciler settles the rest (the position's link, a
  pending order row, and an entry that holds nothing, which fails):
  `UPDATE execution_intents SET status = 'rejected', updated_at = NOW() WHERE id = '<intent id>' AND status IN ('submitting','unknown','acknowledged');`
  A manual order's row is closed in the same transaction, which releases
  the cash it reserved:
  `UPDATE orders SET status = 'cancelled', updated_at = NOW() WHERE id = '<intent id>' AND COALESCE(filled_quantity, 0) = 0;`
  An order that filled is never marked refused: stop the API, correct the
  position's held quantity, entry price and fees from the trades (bumping
  `version`), set the intent `reconciled` and its `position_order_links`
  row `settled` in the same transaction, then start the API. A missing
  order or a 404 alone proves nothing.
- The circuit breaker's equity feed reads the wallet through the same chain
  client as withdrawals (environment, else the canonical workspace); trading
  live without an observed balance is warned about at every snapshot. A
  withdrawal failed by an earlier verdict and then seen mined holds the
  feed like an unsettled one.
- A withdrawal keeps back what open live buys can still spend, their taker
  fee included (at the highest published rate), so a withdrawal cannot
  leave a resting buy short of its fee.
- A vault wallet cannot be disconnected while a withdrawal from it is
  unconfirmed (re-sending or raising its gas needs the key) or while a
  transaction from it is being sent; retry once it settles.
- RPC errors are logged without the endpoint's URL, which carries the
  provider's API key.
- A withdrawal destination written in mixed case must carry a valid EIP-55
  checksum (a mistyped address is refused); an all-lowercase or
  all-uppercase address is taken as written.
- The wallet inventory's chain scan advances its cursor only once the
  tokens it found are stored, and scans only to a block that will not be
  reorganised. A token held by several positions (or a position and manual
  inventory) carries all their cost. Orphan sales and inventory refreshes
  of one wallet do not run at once: a sale waits for a refresh in flight.
  A token the wallet held none of at its last read, and that no position,
  manual holding or new transfer points at, is read again only hourly.
- Orphan recovery never sells a token whose market or outcome is unknown
  (it stays counted among the inventory summary's orphans for an operator),
  nor one in a market where a position's tokens could not be looked up
  (they may be that position's shares), nor one matching manual inventory
  that awaits wallet verification (`wallet_address = 'unknown'`). An orphan
  sale's P&L charges the market's taker fee terms (the highest published
  rate when they cannot be read).
- The circuit breaker's drawdown guard is fed only from a snapshot whose
  inventory scan completed: one that failed values only the positions'
  shares and would read orphan, manual and unredeemed shares as a loss.
  The snapshot's 24-hour realized P&L leaves paper results out when
  trading live.
- Live arb and quant entries are made only while the exit handler is
  enabled (`EXIT_HANDLER_ENABLED`, or the workspace setting) and running:
  it flattens a pair whose second leg failed and takes quant profits and
  losses. An exit or resolution pass that panics holds its heartbeat at
  zero, so entries wait until both run cleanly (a resolution pass every
  `EXIT_RESOLUTION_CHECK_SECS`); a heartbeat older than three exit polls
  (at least five minutes, which a long run of orphan sales can reach) also
  holds entries back. Paper entries are unaffected.
- A quant entry is refused when the token's round trip (the spread, as its
  position is marked at the best bid, plus the taker fee of buying and of
  selling under the market's fee terms) would be half the stop-loss or
  more, or when the token has no bids.
- Manual holdings under one sellable lot (0.01 shares, left by rounded
  sales) are not exposure and do not pin the trading wallet. Manual shares
  redeemed or moved outside the bot stay recorded until an operator clears
  them, after checking the wallet holds none:
  `UPDATE manual_inventory SET quantity = 0, cost_basis = 0 WHERE wallet_address = '<wallet>' AND market_id = '<market>' AND outcome = '<yes|no>';`
- The quant executor's per-strategy limits count only results of the mode
  it trades in, and never a resolution win the wallet could not confirm.
- A circuit breaker cooldown is at most a week (`cooldown_minutes` above
  10080 is refused; a larger configured value is held to a week).
- A panic in one pass of a background task (order and withdrawal
  reconcilers, account snapshots, the collateral sweeper, wallet inventory,
  exits and resolutions) is logged as "A background pass panicked" and the
  task goes on with its next pass.
- Manual cash-flow entries take an `idempotency_key` (the dashboard sends
  one per entry): an entry sent again records nothing more. Withdrawals made
  through the API are recorded automatically; recording them by hand too
  counts them twice. A deposit's amount must be positive and a withdrawal's
  negative (the dashboard signs them by type): the circuit breaker reads
  outflows by their sign.
- Preparing a browser-wallet order reads the market's tick size and refuses
  a price outside one tick of either end or with more decimals than the
  tick has (as Polymarket's own clients round), before the wallet signs.
- The activity feed shows an exit that keeps failing for the same reason
  once; a new reason shows again. The trade event log keeps every attempt.
- 093 drops browser-wallet orders prepared for the V1 exchange (they can no
  longer be submitted) and stores the signed creation time of new ones.
- The bot-scanner indexer now reads trades from both V2 exchanges.
- The dashboard's unmounted manual-order components (order form, order
  panel, manual positions) and their wallet-signing hook were removed: no
  page used them, and the form could not run without the wallet provider.
  The browser-wallet order endpoints (`/api/v1/orders/prepare`, `/submit`)
  remain for API clients and sign V2 orders.
- Polymarket's documentation says a plain EOA trades on the CLOB "if your EOA
  is allowlisted for trading". The bot signs as an EOA (signature type 0);
  confirm the trading wallet is allowed before enabling live trading. Markets
  of Polymarket Protocol V2 (position ids, ExchangeV3, domain version `"3"`)
  are not traded by this bot; CTF markets, which it trades by token id, stay
  on the exchanges above.
