# Testing Guide: Risk-Focused Wallet Discovery & Allocation System

## Prerequisites

1. **Update .env file:**
   ```bash
   cp .env.example .env
   # Edit .env and set:
   DATABASE_URL=postgres://abbot:abbot_secret@localhost:5432/ab_bot
   JWT_SECRET=$(openssl rand -base64 32)
   ```

2. **Start Docker services:**
   ```bash
   docker compose up -d postgres redis
   ```

3. **Run migrations:**
   ```bash
   cargo sqlx migrate run
   ```

## Phase 1: Metrics Calculator Testing

### 1. Start the API Server

```bash
cargo run -p api-server
```

**Expected log output:**
- ✅ "Metrics calculator background job spawned"
- ✅ "Starting API server..."
- ✅ Server listening on 0.0.0.0:3000

### 2. Check Background Job Logs

After 1 hour (or set `METRICS_CALCULATOR_INTERVAL_SECS=60` for faster testing):

```bash
# Check logs for metrics calculation
tail -f logs/api-server.log | grep "metrics"
```

**Expected output:**
```
Metrics calculation cycle complete success_count=X error_count=Y
```

### 3. Verify Database Population

```bash
# Connect to database
docker exec -it $(docker ps -qf "name=postgres") psql -U abbot -d ab_bot

# Check metrics table
SELECT COUNT(*) FROM wallet_success_metrics;
-- Expected: > 0 after first cycle

# Check recent calculations
SELECT address, roi_30d, sharpe_30d, last_computed
FROM wallet_success_metrics
ORDER BY last_computed DESC
LIMIT 10;
-- Expected: Recent timestamps, non-zero ROI values

# Exit psql
\q
```

### 4. Test Discovery with Relaxed Criteria

```bash
# Check wallets now discoverable with lower thresholds
docker exec -it $(docker ps -qf "name=postgres") psql -U abbot -d ab_bot -c "
SELECT COUNT(*) FROM wallet_features wf
LEFT JOIN wallet_success_metrics wsm ON wsm.address = wf.address
WHERE wf.total_trades >= 10
  AND COALESCE(wsm.roi_30d, 0) >= 0.02
  AND COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0) >= 0.52;
"
-- Expected: > 5 wallets (more than before)
```

## Phase 2: Risk Allocation API Testing

### 1. Get Authentication Token

```bash
# Register or login to get JWT token
curl -X POST http://localhost:3000/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{
    "email": "admin@example.com",
    "password": "your_password"
  }' | jq -r '.token'

# Save token
export TOKEN="your_token_here"
```

### 2. Test Preview Mode (Read-Only)

```bash
# Preview allocation changes for active tier
curl -X POST http://localhost:3000/api/v1/allocations/risk/recalculate \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "tier": "active",
    "auto_apply": false
  }' | jq '.'
```

**Expected response:**
```json
{
  "previews": [
    {
      "address": "0x1234...",
      "current_allocation_pct": 20.0,
      "recommended_allocation_pct": 25.3,
      "change_pct": 5.3,
      "composite_score": 0.78,
      "components": {
        "sortino_normalized": 0.85,
        "consistency": 0.72,
        "roi_drawdown_ratio": 0.68,
        "win_rate": 0.63,
        "volatility": 0.15
      }
    }
  ],
  "applied": false,
  "wallet_count": 5
}
```

### 3. Verify No Database Changes (Preview Mode)

```bash
# Check allocations before and after preview
docker exec -it $(docker ps -qf "name=postgres") psql -U abbot -d ab_bot -c "
SELECT wallet_address, allocation_pct
FROM workspace_wallet_allocations
WHERE tier = 'active'
ORDER BY allocation_pct DESC;
"
# Values should be UNCHANGED after preview
```

### 4. Test Apply Mode (Actually Updates)

```bash
# Apply the recommended allocations
curl -X POST http://localhost:3000/api/v1/allocations/risk/recalculate \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "tier": "active",
    "auto_apply": true
  }' | jq '.'
```

**Expected response:**
```json
{
  "applied": true,
  "wallet_count": 5
}
```

### 5. Verify Database Updated

```bash
# Check allocations were actually updated
docker exec -it $(docker ps -qf "name=postgres") psql -U abbot -d ab_bot -c "
SELECT wallet_address, allocation_pct, updated_at
FROM workspace_wallet_allocations
WHERE tier = 'active'
ORDER BY allocation_pct DESC;
"
# Values should be CHANGED to match recommendations
```

### 6. Check Audit Trail

```bash
# Verify audit log entries
docker exec -it $(docker ps -qf "name=postgres") psql -U abbot -d ab_bot -c "
SELECT wallet_address, action_type, reason, created_at
FROM auto_rotation_history
WHERE action_type = 'allocation_adjustment'
ORDER BY created_at DESC
LIMIT 10;
"
# Expected: Recent entries with risk scores and allocations
```

## Phase 3: Frontend UI Testing

### 1. Start the Dashboard

```bash
cd dashboard
npm install  # if not already done
npm run dev
```

### 2. Open Browser and Navigate

```
http://localhost:3002/trading?tab=active
```

### 3. Test Risk-Based Allocation Panel

**Active Tab:**
1. ✅ See "Risk-Based Allocation" card at bottom
2. ✅ Click "Preview Changes" button
3. ✅ Table appears with:
   - Wallet addresses
   - Current vs Recommended allocations
   - Change indicators (green ↑ red ↓)
   - Risk scores
4. ✅ Click eye icon to view detailed risk breakdown
5. ✅ Dialog shows:
   - Composite score (0-100)
   - Component breakdowns (Sortino, Consistency, ROI/MaxDD, Win Rate)
   - Volatility metric
6. ✅ Click "Apply Changes" button
7. ✅ Toast notification: "Allocations updated"
8. ✅ Table disappears and data refreshes

**Watching Tab:**
1. ✅ Navigate to `/trading?tab=watching`
2. ✅ See "Risk-Based Allocation" card for bench wallets
3. ✅ Same functionality as Active tab

### 4. Verify Real-Time Updates

After applying changes:
1. ✅ Check wallet cards show new allocation percentages
2. ✅ No page refresh needed (React Query handles it)
3. ✅ Check browser console for no errors

## Performance Testing

### 1. Metrics Calculator Performance

```bash
# Monitor CPU/memory during calculation cycle
docker stats $(docker ps -qf "name=api-server")
```

**Expected:**
- CPU < 50% during calculation
- Memory stable (no leaks)
- Cycle completes in < 60 seconds

### 2. API Response Times

```bash
# Preview mode (should be fast)
time curl -X POST http://localhost:3000/api/v1/allocations/risk/recalculate \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"tier":"active","auto_apply":false}' > /dev/null
# Expected: < 500ms

# Apply mode (includes transaction)
time curl -X POST http://localhost:3000/api/v1/allocations/risk/recalculate \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"tier":"active","auto_apply":true}' > /dev/null
# Expected: < 1s
```

## Troubleshooting

### Issue: "No wallets with metrics found"

**Cause:** Metrics calculator hasn't run yet or table is empty.

**Solution:**
```bash
# Force immediate calculation (reduce interval)
# Add to .env:
METRICS_CALCULATOR_INTERVAL_SECS=60
METRICS_CALCULATOR_BATCH_SIZE=50

# Restart server and wait 1 minute
```

### Issue: "Preview returns empty array"

**Cause:** No wallets meet criteria or no metrics calculated.

**Solution:**
```sql
-- Check if wallets have metrics
SELECT COUNT(*) FROM wallet_success_metrics WHERE roi_30d > 0;

-- Check if wallets exist
SELECT COUNT(*) FROM wallet_features WHERE total_trades >= 10;
```

### Issue: "Apply changes doesn't update database"

**Cause:** Permission error or transaction rollback.

**Solution:**
```bash
# Check server logs for errors
tail -50 logs/api-server.log | grep -i error

# Verify user has trader role
docker exec -it $(docker ps -qf "name=postgres") psql -U abbot -d ab_bot -c "
SELECT email, role FROM users;
"
```

### Issue: "Frontend shows undefined values"

**Cause:** TypeScript type mismatch with backend response.

**Solution:**
```bash
# Check network tab in browser DevTools
# Verify response field names match TypeScript types
# Should see: active_wallet_count (not active_count)
```

## Environment Variables for Testing

Add these to `.env` for faster testing cycles:

```bash
# Metrics Calculator
METRICS_CALCULATOR_ENABLED=true
METRICS_CALCULATOR_INTERVAL_SECS=60        # 1 minute (instead of 1 hour)
METRICS_CALCULATOR_BATCH_SIZE=50
METRICS_RECALC_AFTER_HOURS=1              # Recalc after 1 hour (instead of 24)

# Logging
RUST_LOG=api_server=debug,wallet_tracker=debug,metrics_calculator=debug
```

## Success Criteria

### Phase 1: Discovery Pipeline
- [x] Metrics calculator runs automatically every hour
- [x] `wallet_success_metrics` table populated
- [x] Discovery returns wallets with ROI ≥ 2%
- [x] Profitable bots included (only excludes bot_score > 70)

### Phase 2: Risk Allocation API
- [x] Preview mode returns recommendations without DB changes
- [x] Apply mode updates `workspace_wallet_allocations` table
- [x] Allocations sum to 100%
- [x] Audit trail logged to `auto_rotation_history`

### Phase 3: Frontend UI
- [x] AllocationAdjustmentPanel visible on Active/Watching tabs
- [x] Preview button generates allocation table
- [x] Risk score dialog shows component breakdown
- [x] Apply button updates database
- [x] React Query invalidates and refetches data
- [x] No TypeScript errors in console

## Next Steps

1. **Deploy to staging:**
   ```bash
   git push origin develop
   # Railway will auto-deploy
   ```

2. **Monitor production:**
   - Check Railway logs for metrics calculator
   - Verify database growth (metrics table)
   - Monitor API response times

3. **User training:**
   - Document preview/apply workflow
   - Show risk score interpretation
   - Explain allocation normalization

## Files Changed

### Backend
- `crates/api-server/src/metrics_calculator.rs` - Background job
- `crates/api-server/src/handlers/risk_allocations.rs` - API endpoint
- `crates/wallet-tracker/src/risk_scorer.rs` - Risk scoring logic
- `crates/wallet-tracker/src/discovery.rs` - Relaxed criteria

### Frontend
- `dashboard/components/allocations/RiskScoreDisplay.tsx` - Risk UI
- `dashboard/components/allocations/AllocationAdjustmentPanel.tsx` - Preview/apply UI
- `dashboard/app/trading/page.tsx` - Integration
- `dashboard/types/api.ts` - Type fixes
- `dashboard/components/setup/SetupWizard.tsx` - Bug fix
