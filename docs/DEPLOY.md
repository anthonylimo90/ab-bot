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
   JWT_SECRET=<generate-32-char-secret>
   RUST_LOG=info,api_server=debug
   CORS_PERMISSIVE=false
   SKIP_MIGRATIONS=false
   ```

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
| `JWT_SECRET` | JWT signing key (32+ chars) | `your-secret-key` |
| `RUST_LOG` | Log level | `info,api_server=debug` |
| `CORS_PERMISSIVE` | Allow all CORS origins | `false` |
| `SKIP_MIGRATIONS` | Skip auto-migrations | `false` |
| `API_PORT` | Server port | `3000` (default) |

### Arb Monitor

| Variable | Description | Example |
|----------|-------------|---------|
| `DATABASE_URL` | TimescaleDB connection | `${{TimescaleDB.DATABASE_URL}}` |
| `REDIS_URL` | Redis connection | `${{Redis.REDIS_URL}}` |
| `RUST_LOG` | Log level | `info,arb_monitor=debug` |
| `POLYMARKET_CLOB_URL` | Polymarket API | `https://clob.polymarket.com` |

### Dashboard

| Variable | Description | Example |
|----------|-------------|---------|
| `NEXT_PUBLIC_API_URL` | API server URL | `https://api-server-xxx.railway.app` |
| `NEXT_PUBLIC_WS_URL` | WebSocket URL | `wss://api-server-xxx.railway.app` |

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
