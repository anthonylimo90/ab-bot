//! Canonical account snapshot worker and shared account valuation helpers.

use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use sqlx::{FromRow, PgPool};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time;
use tracing::{info, warn};
use uuid::Uuid;

use crate::state::AppState;
use crate::wallet_inventory::{
    load_canonical_inventory_summary, refresh_canonical_wallet_inventory,
};
use crate::workspace_scope::resolve_canonical_workspace_id;

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
    trading_wallet_address: Option<String>,
}

#[derive(Debug, Default)]
struct AccountValuationRow {
    open_positions: i64,
    open_markets: i64,
    position_value: Decimal,
    unrealized_pnl: Decimal,
    unpriced_open_positions: i64,
    unpriced_position_cost_basis: Decimal,
}

#[derive(Debug, Clone, Copy, Default)]
struct ObservedConditionalBalances {
    yes: Option<Decimal>,
    no: Option<Decimal>,
}

#[derive(Debug, FromRow)]
#[allow(dead_code)] // Fields are required by the SQL query mapping (FromRow)
struct EffectiveOpenPositionRow {
    market_id: String,
    quantity: Decimal,
    current_price: Option<Decimal>,
    yes_price: Option<Decimal>,
    no_price: Option<Decimal>,
    yes_entry_price: Decimal,
    no_entry_price: Decimal,
    yes_exit_price: Option<Decimal>,
    no_exit_price: Option<Decimal>,
    entry_value: Decimal,
    unrealized_pnl: Decimal,
}

impl EffectiveOpenPositionRow {
    fn holds_yes(&self) -> bool {
        self.yes_entry_price > Decimal::ZERO && self.yes_exit_price.is_none()
    }

    fn holds_no(&self) -> bool {
        self.no_entry_price > Decimal::ZERO && self.no_exit_price.is_none()
    }

    fn fallback_cost_basis(&self) -> Decimal {
        (if self.holds_yes() {
            self.quantity * self.yes_entry_price
        } else {
            Decimal::ZERO
        }) + (if self.holds_no() {
            self.quantity * self.no_entry_price
        } else {
            Decimal::ZERO
        })
    }

    /// Returns `(value, used_fallback)` — value is always computed (never None),
    /// and `used_fallback` is true when no live market price was available and
    /// the entry price was substituted to keep the equity display stable.
    fn marked_value(&self, yes_qty: Decimal, no_qty: Decimal) -> (Decimal, bool) {
        let has_yes = yes_qty > Decimal::ZERO;
        let has_no = no_qty > Decimal::ZERO;

        match (has_yes, has_no) {
            (false, false) => (Decimal::ZERO, false),
            (true, false) => {
                if let Some(price) = self.yes_price.or(self.current_price) {
                    (yes_qty * price, false)
                } else {
                    (yes_qty * self.yes_entry_price, true)
                }
            }
            (false, true) => {
                if let Some(price) = self.no_price.or(self.current_price) {
                    (no_qty * price, false)
                } else {
                    (no_qty * self.no_entry_price, true)
                }
            }
            (true, true) => {
                if let (Some(yes_price), Some(no_price)) = (self.yes_price, self.no_price) {
                    ((yes_qty * yes_price) + (no_qty * no_price), false)
                } else {
                    let yes_price = self.yes_price.unwrap_or(self.yes_entry_price);
                    let no_price = self.no_price.unwrap_or(self.no_entry_price);
                    ((yes_qty * yes_price) + (no_qty * no_price), true)
                }
            }
        }
    }
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
    if let Err(error) = snapshot_canonical_workspace(&state).await {
        warn!(error = %error, "Initial account snapshot cycle failed");
    }

    let interval = time::Duration::from_secs(config.interval_secs);
    loop {
        tokio::time::sleep(interval).await;
        if let Err(error) = snapshot_canonical_workspace(&state).await {
            warn!(error = %error, "Account snapshot cycle failed");
        }
    }
}

pub async fn load_live_account_snapshot(
    state: &AppState,
) -> anyhow::Result<Option<AccountSnapshotRecord>> {
    let Some(workspace_id) = resolve_canonical_workspace_id(&state.pool).await? else {
        return Ok(None);
    };

    let target = sqlx::query_as::<_, WorkspaceSnapshotTarget>(
        r#"
        SELECT trading_wallet_address
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

    let inventory_refreshed = if state.order_executor.is_live_ready().await {
        match refresh_canonical_wallet_inventory(state).await {
            Ok(_) => true,
            Err(error) => {
                warn!(error = %error, "Failed to refresh wallet inventory before account snapshot");
                false
            }
        }
    } else {
        false
    };

    let valuation = load_account_valuation(state, inventory_refreshed).await?;
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

async fn snapshot_canonical_workspace(state: &Arc<AppState>) -> anyhow::Result<()> {
    if let Some(snapshot) = load_live_account_snapshot(state).await? {
        persist_snapshot(&state.pool, &snapshot).await?;

        // Bridge equity to the circuit breaker so drawdown tracking uses real
        // portfolio value instead of cumulative P&L from zero.
        if snapshot.total_equity > rust_decimal::Decimal::ZERO {
            if let Err(e) = state
                .circuit_breaker
                .update_portfolio_value(snapshot.total_equity)
                .await
            {
                warn!(
                    error = %e,
                    total_equity = %snapshot.total_equity,
                    "Failed to update circuit breaker portfolio value from account snapshot"
                );
            }
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

async fn load_account_valuation(
    state: &AppState,
    inventory_refreshed: bool,
) -> anyhow::Result<AccountValuationRow> {
    if inventory_refreshed {
        if let Some(summary) = load_canonical_inventory_summary(state).await? {
            return Ok(AccountValuationRow {
                open_positions: summary.open_positions,
                open_markets: summary.open_markets,
                position_value: summary.position_value,
                unrealized_pnl: summary.unrealized_pnl,
                unpriced_open_positions: summary.unpriced_open_positions,
                unpriced_position_cost_basis: summary.unpriced_position_cost_basis,
            });
        }
    }

    load_position_valuation_from_positions(state).await
}

async fn load_position_valuation_from_positions(
    state: &AppState,
) -> anyhow::Result<AccountValuationRow> {
    let rows = sqlx::query_as::<_, EffectiveOpenPositionRow>(
        r#"
        WITH active_positions AS (
            SELECT
                p.id,
                p.market_id,
                COALESCE(p.source, 0) AS source,
                p.quantity,
                p.current_price,
                p.yes_entry_price,
                p.no_entry_price,
                p.yes_exit_price,
                p.no_exit_price,
                COALESCE(p.entry_price, (p.yes_entry_price + p.no_entry_price), 0) AS entry_price,
                (p.quantity * COALESCE(p.entry_price, (p.yes_entry_price + p.no_entry_price), 0)) AS entry_value,
                COALESCE(p.unrealized_pnl, 0) AS unrealized_pnl,
                COALESCE(p.last_updated, p.updated_at, p.entry_timestamp) AS sort_updated
            FROM positions p
            WHERE p.is_open = TRUE
        ),
        ranked_active AS (
            SELECT
                *,
                ROW_NUMBER() OVER (
                    PARTITION BY market_id, source
                    ORDER BY sort_updated DESC, id DESC
                ) AS rn
            FROM active_positions
        )
        SELECT
            ra.market_id,
            ra.quantity,
            ra.current_price,
            m.yes_price,
            m.no_price,
            ra.yes_entry_price,
            ra.no_entry_price,
            ra.yes_exit_price,
            ra.no_exit_price,
            ra.entry_value,
            ra.unrealized_pnl
        FROM ranked_active ra
        LEFT JOIN markets m
          ON m.id = ra.market_id
        WHERE ra.rn = 1
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    let observed_balances = if state.order_executor.is_live_ready().await {
        load_observed_conditional_balances(state, &rows).await?
    } else {
        HashMap::new()
    };

    let mut valuation = AccountValuationRow::default();
    let mut open_markets = HashSet::new();

    for row in rows {
        valuation.open_positions += 1;
        open_markets.insert(row.market_id.clone());

        let observed = observed_balances
            .get(&row.market_id)
            .copied()
            .unwrap_or_default();
        let yes_qty = observed.yes.unwrap_or_else(|| {
            if row.holds_yes() {
                row.quantity
            } else {
                Decimal::ZERO
            }
        });
        let no_qty = observed.no.unwrap_or_else(|| {
            if row.holds_no() {
                row.quantity
            } else {
                Decimal::ZERO
            }
        });
        let marked_cost_basis = (yes_qty * row.yes_entry_price) + (no_qty * row.no_entry_price);

        let (marked_value, used_fallback) = row.marked_value(yes_qty, no_qty);
        valuation.position_value += marked_value;
        valuation.unrealized_pnl += marked_value - marked_cost_basis;
        if used_fallback {
            valuation.unpriced_open_positions += 1;
            valuation.unpriced_position_cost_basis += row.fallback_cost_basis();
        }
    }

    valuation.open_markets = open_markets.len() as i64;
    Ok(valuation)
}

async fn load_observed_conditional_balances(
    state: &AppState,
    rows: &[EffectiveOpenPositionRow],
) -> anyhow::Result<HashMap<String, ObservedConditionalBalances>> {
    let mut market_tokens = HashMap::new();
    let mut balances = HashMap::new();

    for row in rows {
        if !row.holds_yes() && !row.holds_no() {
            continue;
        }

        let tokens = match resolve_market_tokens(state, &mut market_tokens, &row.market_id).await {
            Ok(Some(tokens)) => tokens,
            Ok(None) => continue,
            Err(error) => {
                warn!(
                    market_id = %row.market_id,
                    error = %error,
                    "Failed to resolve market tokens for live equity valuation"
                );
                continue;
            }
        };

        let yes = if row.holds_yes() {
            load_conditional_balance(state, &tokens.0)
                .await
                .map_err(|error| {
                    warn!(
                        market_id = %row.market_id,
                        token_id = %tokens.0,
                        error = %error,
                        "Failed to load observed YES conditional balance"
                    );
                    error
                })
                .ok()
                .flatten()
        } else {
            None
        };
        let no = if row.holds_no() {
            load_conditional_balance(state, &tokens.1)
                .await
                .map_err(|error| {
                    warn!(
                        market_id = %row.market_id,
                        token_id = %tokens.1,
                        error = %error,
                        "Failed to load observed NO conditional balance"
                    );
                    error
                })
                .ok()
                .flatten()
        } else {
            None
        };

        balances.insert(
            row.market_id.clone(),
            ObservedConditionalBalances { yes, no },
        );
    }

    Ok(balances)
}

async fn resolve_market_tokens(
    state: &AppState,
    cache: &mut HashMap<String, (String, String)>,
    market_id: &str,
) -> anyhow::Result<Option<(String, String)>> {
    if let Some(tokens) = cache.get(market_id) {
        return Ok(Some(tokens.clone()));
    }

    let market = match state.clob_client.get_market_by_id(market_id).await {
        Ok(market) => market,
        Err(error) => {
            warn!(
                market_id = %market_id,
                error = %error,
                "Failed to load market details for token resolution"
            );
            return Ok(None);
        }
    };

    if market.outcomes.len() != 2 {
        return Ok(None);
    }

    let (yes_token_id, no_token_id) = if market.outcomes[0].name.to_lowercase().contains("yes") {
        (
            market.outcomes[0].token_id.clone(),
            market.outcomes[1].token_id.clone(),
        )
    } else {
        (
            market.outcomes[1].token_id.clone(),
            market.outcomes[0].token_id.clone(),
        )
    };
    cache.insert(
        market_id.to_string(),
        (yes_token_id.clone(), no_token_id.clone()),
    );

    Ok(Some((yes_token_id, no_token_id)))
}

async fn load_conditional_balance(
    state: &AppState,
    token_id: &str,
) -> anyhow::Result<Option<Decimal>> {
    let snapshot = state
        .order_executor
        .get_live_balance_allowance("CONDITIONAL", Some(token_id))
        .await;

    Ok(snapshot.and_then(|snapshot| parse_decimal_string(&snapshot.balance)))
}

fn parse_decimal_string(raw: &str) -> Option<Decimal> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    Decimal::from_str(trimmed).ok()
}
