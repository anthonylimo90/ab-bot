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
   RUST_LOG=info,api_server=debug
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
   RUST_LOG=info,arb_monitor=debug
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
  Approvals are sent under the wallet's spend lock (waited for up to 30
  seconds); one left unmined is signed again at its nonce for more gas (up
  to three times the network's price), but only while the node holds one of
  its transactions or counts nothing pending at its nonce.
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
  up to an hour while the balance cannot be read (a live executor whose
  signer failed to load counts as unreadable: a restart may load it). A
  balance far below the record (shares redeemed or moved), one still
  unreadable after that hour, a position of another wallet than the
  trading wallet, or a paper executor (it never loads a wallet) leaves the
  record standing: the position closes at it, with a warning,
  and its `closed_via_resolution` event lists the legs not measured and why
  (metadata `unmeasured`). The hour counts only an unbroken run of
  unreadable balances. A position whose market resolved with no single
  winner flagged and whose legs differ (a split payout, or flags not yet
  set) is never booked at a guessed payout: it waits a day for the winner,
  from the first look that found none (a `resolution_waiting_for_winner`
  trade event, so restarts keep the count), then closes at the least it can
  pay: a dollar a pair held, the shares without a pair valued at nothing
  (they stay in the wallet) and listed in its `closed_via_resolution` event
  (metadata `unpaired`).
- A break-even close neither extends nor ends the circuit breaker's run of
  losses. The breaker guards real money: only a live position's close counts
  in it, live trading switched off or not. Paper results (and those of
  unknown provenance) never do, so a paper win cannot end or offset a run
  of live losses; paper trading no longer trips the breaker by its own
  losses.
- An arb signal is skipped while any position in its market is still active,
  read from the database (an entry still settling included), so a restart
  or a reconciled entry never lets a second pair into the same market.
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
