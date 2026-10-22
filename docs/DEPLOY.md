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

   **Build Settings:**
   - Builder: `Dockerfile`
   - Dockerfile Path: `Dockerfile`

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
   RUST_LOG=info,api_server=debug
   ENVIRONMENT=production
   CORS_ALLOWED_ORIGINS=https://<your-dashboard-domain>
   APP_DOMAIN=<your-dashboard-domain>
   APP_URI=https://<your-dashboard-domain>
   TRUSTED_PROXY_HOPS=1
   SKIP_MIGRATIONS=false
   ```

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

   **Build Settings:**
   - Builder: `Dockerfile`
   - Dockerfile Path: `Dockerfile.arb-monitor`

5. Add **Variables**:
   ```
   DATABASE_URL=${{TimescaleDB.DATABASE_URL}}
   REDIS_URL=${{Redis.REDIS_URL}}
   DYNAMIC_CONFIG_REDIS_URL=${{Redis.REDIS_URL}}
   RUST_LOG=info,arb_monitor=debug
   POLYMARKET_CLOB_URL=https://clob.polymarket.com
   ```

6. No public domain needed (background worker)

### 4.3 Dashboard

1. Click **"+ New"** → **"GitHub Repo"**
2. Select your ab-bot repository
3. Rename to `dashboard`
4. Configure in **Settings** tab:

   **Build Settings:**
   - Builder: `Dockerfile`
   - Dockerfile Path: `Dockerfile.dashboard`

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
# Expected: {"status":"ok"}
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
| `RUST_LOG` | Log level | `info,api_server=debug` |
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
| `RUST_LOG` | Log level | `info,arb_monitor=debug` |
| `POLYMARKET_CLOB_URL` | Polymarket API | `https://clob.polymarket.com` |

### Dashboard

| Variable | Description | Example |
|----------|-------------|---------|
| `NEXT_PUBLIC_API_URL` | API server URL | `https://api-server-xxx.railway.app` |
| `NEXT_PUBLIC_WS_URL` | WebSocket URL | `wss://api-server-xxx.railway.app` |

## Redis Hardening for Dynamic Config

Recommended production posture is:

1. `dynamic_tuner` is the sole writer to `dynamic:config:update`.
2. Subscribers use read-only pub/sub credentials.
3. Redis is private-network only (no public ingress).

This repository supports split credentials by environment variable:

- `REDIS_URL` for general app pub/sub traffic
- `DYNAMIC_TUNER_REDIS_URL` for tuner publish/read
- `DYNAMIC_CONFIG_REDIS_URL` for dynamic-config subscribers

If your managed Redis tier does not support ACL users, keep all three URLs pointed at the same private Redis endpoint and enforce network isolation at the platform layer.

## railway.toml Configuration

The repository includes `railway.toml` with service-specific settings:

```toml
[services.api-server.build]
builder = "dockerfile"
dockerfilePath = "./Dockerfile"

[services.api-server.deploy]
healthcheckPath = "/health"
restartPolicyType = "ON_FAILURE"
restartPolicyMaxRetries = 5

[services.arb-monitor.build]
builder = "dockerfile"
dockerfilePath = "./Dockerfile.arb-monitor"

[services.dashboard.build]
builder = "dockerfile"
dockerfilePath = "./Dockerfile.dashboard"
```

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

If using a shared Dockerfile for multiple services, ensure:
1. Each service has the correct `dockerfilePath` in railway.toml
2. Or use service-specific Dockerfiles (recommended)

### Dashboard can't connect to API

1. Verify `NEXT_PUBLIC_API_URL` matches your api-server domain exactly
2. Ensure api-server has a public domain generated
3. Check CORS settings on api-server

### Build timeout

Rust builds can be slow. Railway has a 2-hour build timeout which should be sufficient, but if builds are failing:
1. Check for compilation errors in logs
2. Ensure `cargo update home --precise 0.5.9` is in Dockerfile (compatibility fix)

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
   | api-server | `RUST_LOG` | `debug,api_server=debug` |
   | dashboard | `NEXT_PUBLIC_API_URL` | `https://<staging-api>.railway.app` |

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

### Review fix migrations and client compatibility

Apply the repository's migrations before starting the updated API. Migrations 069–072 add durable signed-order intents, exchange order identity/provenance, recovery of closed rows with explicit holdings, and persistent session revocation. Migration 068 now preserves unresolved positions instead of closing them by age. If a deployment already recorded the original 068 checksum, SQLx will require a reviewed migration-history repair; back up the database, reconcile the affected positions, and review migration 071 before accepting the revised checksum. Do not disable checksum validation globally.

REST order placement requires a stable `client_order_id`; withdrawals require a stable `client_request_id`. Reuse the same key and original payload through retries, including lost responses. A changed payload requires a new intent. Private WebSocket clients must send `{"action":"authenticate","token":"<JWT>"}` as their first frame within five seconds; only orderbook-only streams remain public. Browser wallet orders also require a fresh wallet-signed ClobAuth proof, which the dashboard supplies.

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
all rows from the current runtime mode. Historical manual orders with missing
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
