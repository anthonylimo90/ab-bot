//! Canonical account snapshot worker and shared account valuation helpers.

use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use sqlx::{FromRow, PgPool};
use std::sync::Arc;
use std::time;
use tracing::{info, warn};
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct AccountSnapshotConfig {
    pub enabled: bool,
    pub interval_secs: u64,
}

impl AccountSnapshotConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("ACCOUNT_SNAPSHOT_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
            interval_secs: std::env::var("ACCOUNT_SNAPSHOT_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AccountSnapshotRecord {
    pub workspace_id: Uuid,
    pub snapshot_time: DateTime<Utc>,
    pub wallet_address: Option<String>,
    pub cash_balance: Decimal,
    pub position_value: Decimal,
    pub total_equity: Decimal,
    pub unrealized_pnl: Decimal,
    pub realized_pnl_24h: Decimal,
    pub net_cash_flows_24h: Decimal,
    pub open_positions: i64,
    pub open_markets: i64,
    pub unpriced_open_positions: i64,
    pub unpriced_position_cost_basis: Decimal,
}

#[derive(Debug, FromRow)]
struct WorkspaceSnapshotTarget {
    id: Uuid,
    trading_wallet_address: Option<String>,
}

#[derive(Debug, FromRow)]
struct AccountValuationRow {
    open_positions: i64,
    open_markets: i64,
    position_value: Decimal,
    unrealized_pnl: Decimal,
    unpriced_open_positions: i64,
    unpriced_position_cost_basis: Decimal,
}

pub fn spawn_account_snapshot_calculator(config: AccountSnapshotConfig, state: Arc<AppState>) {
    if !config.enabled {
        info!("Account snapshot calculator disabled (ACCOUNT_SNAPSHOT_ENABLED != true)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        "Spawning account snapshot calculator"
    );

    tokio::spawn(run_loop(config, state));
}

async fn run_loop(config: AccountSnapshotConfig, state: Arc<AppState>) {
    if let Err(error) = snapshot_all_workspaces(&state).await {
        warn!(error = %error, "Initial account snapshot cycle failed");
    }

    let interval = time::Duration::from_secs(config.interval_secs);
    loop {
        tokio::time::sleep(interval).await;
        if let Err(error) = snapshot_all_workspaces(&state).await {
            warn!(error = %error, "Account snapshot cycle failed");
        }
    }
}

pub async fn load_live_account_snapshot(
    state: &AppState,
    workspace_id: Uuid,
) -> anyhow::Result<Option<AccountSnapshotRecord>> {
    let target = sqlx::query_as::<_, WorkspaceSnapshotTarget>(
        r#"
        SELECT id, trading_wallet_address
        FROM workspaces
        WHERE id = $1
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await?;

    let Some(target) = target else {
        return Ok(None);
    };

    let valuation = load_account_valuation(&state.pool).await?;
    let realized_pnl_24h = load_realized_pnl_24h(&state.pool).await?;
    let net_cash_flows_24h = load_net_cash_flows_24h(&state.pool, workspace_id).await?;
    let cash_balance = load_wallet_balance(state, target.trading_wallet_address.as_deref()).await?;
    let total_equity = cash_balance + valuation.position_value;

    Ok(Some(AccountSnapshotRecord {
        workspace_id,
        snapshot_time: Utc::now(),
        wallet_address: target.trading_wallet_address,
        cash_balance,
        position_value: valuation.position_value,
        total_equity,
        unrealized_pnl: valuation.unrealized_pnl,
        realized_pnl_24h,
        net_cash_flows_24h,
        open_positions: valuation.open_positions,
        open_markets: valuation.open_markets,
        unpriced_open_positions: valuation.unpriced_open_positions,
        unpriced_position_cost_basis: valuation.unpriced_position_cost_basis,
    }))
}

async fn snapshot_all_workspaces(state: &Arc<AppState>) -> anyhow::Result<()> {
    let workspaces = sqlx::query_as::<_, WorkspaceSnapshotTarget>(
        r#"
        SELECT id, trading_wallet_address
        FROM workspaces
        ORDER BY updated_at DESC, created_at DESC
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    for workspace in workspaces {
        if let Some(snapshot) = load_live_account_snapshot(state, workspace.id).await? {
            persist_snapshot(&state.pool, &snapshot).await?;
        }
    }

    Ok(())
}

async fn persist_snapshot(pool: &PgPool, snapshot: &AccountSnapshotRecord) -> anyhow::Result<()> {
    let bucket_time = snapshot.snapshot_time
        - Duration::seconds((snapshot.snapshot_time.timestamp() % 60).max(0));

    sqlx::query(
        r#"
        INSERT INTO account_snapshots (
            id, workspace_id, snapshot_time, wallet_address, cash_balance,
            position_value, total_equity, unrealized_pnl, realized_pnl_24h,
            net_cash_flows_24h, open_positions, open_markets,
            unpriced_open_positions, unpriced_position_cost_basis
        )
        VALUES (
            $1, $2, $3, $4, $5,
            $6, $7, $8, $9,
            $10, $11, $12,
            $13, $14
        )
        ON CONFLICT (workspace_id, snapshot_time) DO UPDATE SET
            wallet_address = EXCLUDED.wallet_address,
            cash_balance = EXCLUDED.cash_balance,
            position_value = EXCLUDED.position_value,
            total_equity = EXCLUDED.total_equity,
            unrealized_pnl = EXCLUDED.unrealized_pnl,
            realized_pnl_24h = EXCLUDED.realized_pnl_24h,
            net_cash_flows_24h = EXCLUDED.net_cash_flows_24h,
            open_positions = EXCLUDED.open_positions,
            open_markets = EXCLUDED.open_markets,
            unpriced_open_positions = EXCLUDED.unpriced_open_positions,
            unpriced_position_cost_basis = EXCLUDED.unpriced_position_cost_basis
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(snapshot.workspace_id)
    .bind(bucket_time)
    .bind(&snapshot.wallet_address)
    .bind(snapshot.cash_balance)
    .bind(snapshot.position_value)
    .bind(snapshot.total_equity)
    .bind(snapshot.unrealized_pnl)
    .bind(snapshot.realized_pnl_24h)
    .bind(snapshot.net_cash_flows_24h)
    .bind(snapshot.open_positions as i32)
    .bind(snapshot.open_markets as i32)
    .bind(snapshot.unpriced_open_positions as i32)
    .bind(snapshot.unpriced_position_cost_basis)
    .execute(pool)
    .await
    .context("failed to persist account snapshot")?;

    Ok(())
}

async fn load_wallet_balance(
    state: &AppState,
    wallet_address: Option<&str>,
) -> anyhow::Result<Decimal> {
    let Some(address) = wallet_address else {
        return Ok(Decimal::ZERO);
    };

    let Some(polygon_client) = state.polygon_client.as_ref() else {
        return Ok(Decimal::ZERO);
    };

    let balance = polygon_client
        .get_usdc_balance(address)
        .await
        .with_context(|| format!("failed to fetch USDC balance for {address}"))?;

    Decimal::from_f64_retain(balance).context("failed to convert wallet balance into Decimal")
}

async fn load_realized_pnl_24h(pool: &PgPool) -> anyhow::Result<Decimal> {
    let value: Option<Decimal> = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(realized_pnl), 0)
        FROM positions
        WHERE state = 4
          AND COALESCE(updated_at, exit_timestamp, entry_timestamp) >= NOW() - INTERVAL '24 hours'
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(value.unwrap_or(Decimal::ZERO))
}

async fn load_net_cash_flows_24h(pool: &PgPool, workspace_id: Uuid) -> anyhow::Result<Decimal> {
    let value: Option<Decimal> = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(amount), 0)
        FROM cash_flow_events
        WHERE workspace_id = $1
          AND occurred_at >= NOW() - INTERVAL '24 hours'
        "#,
    )
    .bind(workspace_id)
    .fetch_one(pool)
    .await?;

    Ok(value.unwrap_or(Decimal::ZERO))
}

async fn load_account_valuation(pool: &PgPool) -> anyhow::Result<AccountValuationRow> {
    let row = sqlx::query_as::<_, AccountValuationRow>(
        r#"
        WITH active_positions AS (
            SELECT
                id,
                market_id,
                COALESCE(source, 0) AS source,
                quantity,
                current_price,
                COALESCE(entry_price, (yes_entry_price + no_entry_price), 0) AS entry_price,
                (quantity * COALESCE(entry_price, (yes_entry_price + no_entry_price), 0)) AS entry_value,
                unrealized_pnl,
                COALESCE(last_updated, updated_at, entry_timestamp) AS sort_updated
            FROM positions
            WHERE is_open = TRUE
        ),
        ranked_active AS (
            SELECT
                *,
                ROW_NUMBER() OVER (
                    PARTITION BY market_id, source
                    ORDER BY sort_updated DESC, id DESC
                ) AS rn
            FROM active_positions
        ),
        effective_active AS (
            SELECT *
            FROM ranked_active
            WHERE rn = 1
        )
        SELECT
            COUNT(*)::bigint AS open_positions,
            COUNT(DISTINCT market_id)::bigint AS open_markets,
            COALESCE(SUM(
                CASE
                    WHEN current_price IS NOT NULL THEN quantity * current_price
                    ELSE GREATEST(entry_value + COALESCE(unrealized_pnl, 0), 0)
                END
            ), 0) AS position_value,
            COALESCE(SUM(unrealized_pnl), 0) AS unrealized_pnl,
            COUNT(*) FILTER (WHERE current_price IS NULL)::bigint AS unpriced_open_positions,
            COALESCE(SUM(quantity * entry_price) FILTER (WHERE current_price IS NULL), 0) AS unpriced_position_cost_basis
        FROM effective_active
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(row)
}
