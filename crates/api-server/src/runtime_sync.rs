//! Runtime reconciliation helpers for copy-trading services.
//!
//! Keeps DB-backed workspace allocation state, tracked_wallets flags, and
//! in-memory TradeMonitor/CopyTrader state aligned.

use anyhow::Result;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};
use trading_engine::copy_trader::{CopyTrader, TrackedWallet};
use wallet_tracker::trade_monitor::TradeMonitor;

#[derive(Debug, Clone)]
pub struct CopyRuntimeSyncStats {
    pub desired_wallets: usize,
    pub monitor_wallets: usize,
    pub trader_wallets: usize,
    pub open_copy_positions: usize,
}

#[derive(Debug, sqlx::FromRow)]
struct RuntimeWalletRow {
    address: String,
    label: Option<String>,
    allocation_pct: Decimal,
    copy_delay_ms: i32,
    max_position_size: Option<Decimal>,
}

#[derive(Debug, sqlx::FromRow)]
struct DuplicateActiveWalletRow {
    wallet_address: String,
    workspace_count: i64,
}

/// Returns true when at least one workspace has copy-trading enabled.
pub async fn any_workspace_copy_enabled(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let enabled: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM workspaces
            WHERE COALESCE(copy_trading_enabled, FALSE) = TRUE
        )
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(enabled.map(|(v,)| v).unwrap_or(false))
}

/// Returns true when at least one workspace has arb auto-execution enabled.
pub async fn any_workspace_arb_enabled(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let enabled: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM workspaces
            WHERE COALESCE(arb_auto_execute, FALSE) = TRUE
        )
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(enabled.map(|(v,)| v).unwrap_or(false))
}

/// Returns true when at least one workspace has exit handler enabled.
pub async fn any_workspace_exit_handler_enabled(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let enabled: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM workspaces
            WHERE COALESCE(exit_handler_enabled, FALSE) = TRUE
        )
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(enabled.map(|(v,)| v).unwrap_or(false))
}

/// Returns true when at least one workspace has live-trading enabled.
pub async fn any_workspace_live_enabled(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let enabled: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM workspaces
            WHERE COALESCE(live_trading_enabled, FALSE) = TRUE
        )
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(enabled.map(|(v,)| v).unwrap_or(false))
}

/// Reconcile copy runtime state from workspace allocations.
///
/// Steps:
/// 1. Sync `tracked_wallets.copy_enabled` from active workspace allocations where
///    workspace copy-trading is enabled.
/// 2. Reconcile in-memory TradeMonitor wallet set.
/// 3. Reconcile in-memory CopyTrader wallet set + open position counter.
pub async fn reconcile_copy_runtime(
    pool: &PgPool,
    trade_monitor: Option<&Arc<TradeMonitor>>,
    copy_trader: Option<&Arc<RwLock<CopyTrader>>>,
) -> Result<CopyRuntimeSyncStats> {
    // Observability: catch multi-tenant overlap where one wallet is active in
    // multiple copy-enabled workspaces. Runtime is still global, so we surface it.
    let duplicates: Vec<DuplicateActiveWalletRow> = sqlx::query_as(
        r#"
        SELECT
            LOWER(wwa.wallet_address) AS wallet_address,
            COUNT(*)::bigint AS workspace_count
        FROM workspace_wallet_allocations wwa
        JOIN workspaces w ON w.id = wwa.workspace_id
        WHERE wwa.tier = 'active'
          AND COALESCE(w.copy_trading_enabled, FALSE) = TRUE
        GROUP BY LOWER(wwa.wallet_address)
        HAVING COUNT(*) > 1
        "#,
    )
    .fetch_all(pool)
    .await?;
    for duplicate in duplicates {
        warn!(
            wallet = %duplicate.wallet_address,
            workspaces = duplicate.workspace_count,
            "Active wallet appears in multiple copy-enabled workspaces; global runtime will merge this wallet"
        );
    }

    // Upsert desired active copy wallets from workspace allocations.
    // Use MIN for allocation to fail-safe to the smaller allocation when
    // a wallet appears in multiple workspaces.
    sqlx::query(
        r#"
        WITH desired AS (
            SELECT
                LOWER(wwa.wallet_address) AS address,
                MIN(wwa.allocation_pct) AS allocation_pct,
                MIN(wwa.max_position_size) AS max_position_size
            FROM workspace_wallet_allocations wwa
            JOIN workspaces w ON w.id = wwa.workspace_id
            WHERE wwa.tier = 'active'
              AND COALESCE(w.copy_trading_enabled, FALSE) = TRUE
            GROUP BY LOWER(wwa.wallet_address)
        )
        INSERT INTO tracked_wallets
            (address, label, copy_enabled, allocation_pct, copy_delay_ms, max_position_size)
        SELECT
            d.address,
            d.address,
            TRUE,
            d.allocation_pct,
            500,
            d.max_position_size
        FROM desired d
        ON CONFLICT (address) DO UPDATE
        SET
            copy_enabled = TRUE,
            allocation_pct = EXCLUDED.allocation_pct,
            max_position_size = COALESCE(EXCLUDED.max_position_size, tracked_wallets.max_position_size),
            label = COALESCE(tracked_wallets.label, EXCLUDED.label),
            updated_at = NOW()
        "#,
    )
    .execute(pool)
    .await?;

    // Disable wallets no longer active in any copy-enabled workspace.
    sqlx::query(
        r#"
        UPDATE tracked_wallets tw
        SET copy_enabled = FALSE, updated_at = NOW()
        WHERE tw.copy_enabled = TRUE
          AND NOT EXISTS (
              SELECT 1
              FROM workspace_wallet_allocations wwa
              JOIN workspaces w ON w.id = wwa.workspace_id
              WHERE wwa.tier = 'active'
                AND COALESCE(w.copy_trading_enabled, FALSE) = TRUE
                AND LOWER(wwa.wallet_address) = LOWER(tw.address)
          )
        "#,
    )
    .execute(pool)
    .await?;

    // Load desired runtime wallet set.
    let desired_rows: Vec<RuntimeWalletRow> = sqlx::query_as(
        r#"
        SELECT
            LOWER(address) AS address,
            label,
            allocation_pct,
            copy_delay_ms,
            max_position_size
        FROM tracked_wallets
        WHERE copy_enabled = TRUE
        ORDER BY success_score DESC NULLS LAST, added_at ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let desired_set: HashSet<String> = desired_rows.iter().map(|r| r.address.clone()).collect();

    let mut monitor_wallets = 0usize;
    if let Some(monitor) = trade_monitor {
        let current = monitor.monitored_wallets().await;
        let current_set: HashSet<String> = current.into_iter().map(|w| w.to_lowercase()).collect();

        for stale in current_set.difference(&desired_set) {
            monitor.remove_wallet(stale).await;
        }
        for missing in desired_set.difference(&current_set) {
            monitor.add_wallet(missing).await;
        }

        monitor_wallets = monitor.monitored_wallets().await.len();
    }

    let open_positions: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint
        FROM positions
        WHERE is_copy_trade = TRUE
          AND is_open = TRUE
        "#,
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let mut trader_wallets = 0usize;
    if let Some(trader_lock) = copy_trader {
        let mut trader = trader_lock.write().await;
        let current_wallets = trader.list_tracked_wallets();
        let current_set: HashSet<String> = current_wallets
            .iter()
            .map(|wallet| wallet.address.to_lowercase())
            .collect();

        for stale in current_set.difference(&desired_set) {
            trader.remove_tracked_wallet(stale);
        }

        for row in &desired_rows {
            let mut wallet = TrackedWallet::new(row.address.clone(), row.allocation_pct)
                .with_delay(row.copy_delay_ms.max(0) as u64);
            if let Some(label) = &row.label {
                wallet = wallet.with_alias(label.clone());
            }
            if let Some(max_size) = row.max_position_size {
                wallet = wallet.with_max_size(max_size);
            }
            trader.add_tracked_wallet(wallet);
        }

        trader.set_open_position_count(open_positions.max(0) as usize);
        trader_wallets = trader.list_tracked_wallets().len();
    }

    info!(
        desired_wallets = desired_rows.len(),
        monitor_wallets,
        trader_wallets,
        open_copy_positions = open_positions,
        "Reconciled copy runtime from workspace allocations"
    );

    Ok(CopyRuntimeSyncStats {
        desired_wallets: desired_rows.len(),
        monitor_wallets,
        trader_wallets,
        open_copy_positions: open_positions.max(0) as usize,
    })
}
