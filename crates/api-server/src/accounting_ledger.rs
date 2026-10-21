//! Canonical account snapshot worker and shared account valuation helpers.

use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
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
    /// Whether `total_equity` is a complete live valuation (live trading and
    /// an observed cash balance) that may drive the circuit breaker.
    pub equity_complete: bool,
    /// Taken before the cash balance read: cash flows recorded up to this
    /// instant are reflected in `cash_balance`.
    pub cash_observed_at: DateTime<Utc>,
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

/// One position row holding explicit inventory.
#[derive(Debug, FromRow)]
struct HeldPositionRow {
    market_id: String,
    current_price: Option<Decimal>,
    yes_price: Option<Decimal>,
    no_price: Option<Decimal>,
    yes_entry_price: Decimal,
    no_entry_price: Decimal,
    held_yes_qty: Decimal,
    held_no_qty: Decimal,
}

/// Holdings of one market aggregated across its position rows.
#[derive(Debug, Default)]
struct MarketHoldings {
    positions: i64,
    yes_qty: Decimal,
    no_qty: Decimal,
    yes_cost: Decimal,
    no_cost: Decimal,
    yes_price: Option<Decimal>,
    no_price: Option<Decimal>,
    /// Row mark; usable as a leg price only when the market holds one leg.
    current_price: Option<Decimal>,
}

impl MarketHoldings {
    fn add(&mut self, row: &HeldPositionRow) {
        self.positions += 1;
        self.yes_qty += row.held_yes_qty;
        self.no_qty += row.held_no_qty;
        self.yes_cost += row.held_yes_qty * row.yes_entry_price;
        self.no_cost += row.held_no_qty * row.no_entry_price;
        self.yes_price = self.yes_price.or(row.yes_price);
        self.no_price = self.no_price.or(row.no_price);
        self.current_price = self.current_price.or(row.current_price);
    }

    /// Cost basis of `qty` shares of a leg holding `held` shares at `cost`:
    /// exact when the whole leg is valued, pro rata otherwise.
    fn leg_cost(qty: Decimal, held: Decimal, cost: Decimal) -> Decimal {
        if qty == held {
            cost
        } else if held > Decimal::ZERO {
            qty * cost / held
        } else {
            Decimal::ZERO
        }
    }

    /// Values `(yes_qty, no_qty)` shares: `(value, cost_basis, unpriced_cost)`.
    /// A leg without a market price is marked at its average entry cost and
    /// reported as unpriced.
    fn value(&self, yes_qty: Decimal, no_qty: Decimal) -> (Decimal, Decimal, Decimal) {
        let single_leg_mark = if (yes_qty > Decimal::ZERO) != (no_qty > Decimal::ZERO) {
            self.current_price
        } else {
            None
        };
        let mut value = Decimal::ZERO;
        let mut cost_basis = Decimal::ZERO;
        let mut unpriced = Decimal::ZERO;
        for (qty, price, leg_cost) in [
            (
                yes_qty,
                self.yes_price.or(single_leg_mark),
                Self::leg_cost(yes_qty, self.yes_qty, self.yes_cost),
            ),
            (
                no_qty,
                self.no_price.or(single_leg_mark),
                Self::leg_cost(no_qty, self.no_qty, self.no_cost),
            ),
        ] {
            if qty <= Decimal::ZERO {
                continue;
            }
            cost_basis += leg_cost;
            match price {
                Some(price) => value += qty * price,
                None => {
                    value += leg_cost;
                    unpriced += leg_cost;
                }
            }
        }
        (value, cost_basis, unpriced)
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

    let live = state.order_executor.is_live_ready().await;
    let inventory_refreshed = if live {
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
    let cash_observed_at = Utc::now();
    let observed_cash =
        load_wallet_balance(state, target.trading_wallet_address.as_deref()).await?;
    let cash_balance = observed_cash.unwrap_or(Decimal::ZERO);
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
        equity_complete: live && observed_cash.is_some(),
        cash_observed_at,
    }))
}

async fn snapshot_canonical_workspace(state: &Arc<AppState>) -> anyhow::Result<()> {
    if let Some(snapshot) = load_live_account_snapshot(state).await? {
        persist_snapshot(&state.pool, &snapshot).await?;

        // Only a complete live valuation may drive drawdown: paper fills and
        // an unobserved cash balance do not describe real equity.
        if snapshot.equity_complete {
            feed_circuit_breaker(state, &snapshot).await?;
        }
    }

    Ok(())
}

/// Passes live equity to the circuit breaker together with the withdrawals
/// recorded since the previous snapshot, so moving cash out of the trading
/// wallet is not mistaken for a trading loss.
async fn feed_circuit_breaker(
    state: &AppState,
    snapshot: &AccountSnapshotRecord,
) -> anyhow::Result<()> {
    let through = snapshot.cash_observed_at;
    let (withdrawals, flows_through) =
        match state.circuit_breaker.cash_flows_applied_through().await {
            Some(applied) if applied >= through => (Decimal::ZERO, None),
            Some(applied) => (
                load_withdrawals_between(&state.pool, snapshot.workspace_id, applied, through)
                    .await?,
                Some(through),
            ),
            // First feed: start the cursor here; earlier withdrawals are
            // already reflected in the equity this snapshot seeds.
            None => (Decimal::ZERO, Some(through)),
        };

    if let Err(e) = state
        .circuit_breaker
        .update_portfolio_value_with_flows(snapshot.total_equity, withdrawals, flows_through)
        .await
    {
        warn!(
            error = %e,
            total_equity = %snapshot.total_equity,
            "Failed to update circuit breaker portfolio value from account snapshot"
        );
    }
    Ok(())
}

/// Sum of cash moved out of the trading wallet (withdrawals and outbound
/// transfers) recorded in `(after, through]`.
async fn load_withdrawals_between(
    pool: &PgPool,
    workspace_id: Uuid,
    after: DateTime<Utc>,
    through: DateTime<Utc>,
) -> anyhow::Result<Decimal> {
    let value: Option<Decimal> = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(amount), 0)
        FROM cash_flow_events
        WHERE workspace_id = $1
          AND event_type IN ('withdrawal', 'transfer')
          AND amount < 0
          AND created_at > $2
          AND created_at <= $3
        "#,
    )
    .bind(workspace_id)
    .bind(after)
    .bind(through)
    .fetch_one(pool)
    .await?;

    Ok(value.unwrap_or(Decimal::ZERO))
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

/// Observed USDC balance of the trading wallet; `None` when no wallet or RPC
/// client is configured, so the balance is unknown rather than zero.
async fn load_wallet_balance(
    state: &AppState,
    wallet_address: Option<&str>,
) -> anyhow::Result<Option<Decimal>> {
    let Some(address) = wallet_address else {
        return Ok(None);
    };

    let Some(polygon_client) = state.polygon_client.as_ref() else {
        return Ok(None);
    };

    let balance = polygon_client
        .get_usdc_balance(address)
        .await
        .with_context(|| format!("failed to fetch USDC balance for {address}"))?;

    Decimal::from_f64_retain(balance)
        .context("failed to convert wallet balance into Decimal")
        .map(Some)
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
    let live = state.order_executor.is_live_ready().await;
    // Explicit held quantities are the only evidence of inventory; every row
    // holding shares counts, in whatever lifecycle state it is.
    let rows = sqlx::query_as::<_, HeldPositionRow>(
        r#"
        SELECT
            p.market_id,
            p.current_price,
            m.yes_price,
            m.no_price,
            p.yes_entry_price,
            p.no_entry_price,
            p.held_yes_qty,
            p.held_no_qty
        FROM positions p
        LEFT JOIN markets m
          ON m.id = p.market_id
        WHERE (p.held_yes_qty > 0 OR p.held_no_qty > 0)
          AND ($1 OR p.execution_mode <> 'paper')
        "#,
    )
    .bind(!live)
    .fetch_all(&state.pool)
    .await?;

    let mut markets: HashMap<String, MarketHoldings> = HashMap::new();
    for row in &rows {
        markets.entry(row.market_id.clone()).or_default().add(row);
    }

    let observed_balances = if live {
        load_observed_conditional_balances(state, &markets).await?
    } else {
        HashMap::new()
    };

    let mut valuation = AccountValuationRow::default();
    for (market_id, holdings) in &markets {
        valuation.open_positions += holdings.positions;
        valuation.open_markets += 1;

        // A wallet balance covers every row of the market, so it replaces
        // the summed row quantities once per market.
        let observed = observed_balances
            .get(market_id)
            .copied()
            .unwrap_or_default();
        let yes_qty = observed.yes.unwrap_or(holdings.yes_qty);
        let no_qty = observed.no.unwrap_or(holdings.no_qty);

        let (value, cost_basis, unpriced) = holdings.value(yes_qty, no_qty);
        valuation.position_value += value;
        valuation.unrealized_pnl += value - cost_basis;
        if unpriced > Decimal::ZERO {
            valuation.unpriced_open_positions += holdings.positions;
            valuation.unpriced_position_cost_basis += unpriced;
        }
    }

    Ok(valuation)
}

async fn load_observed_conditional_balances(
    state: &AppState,
    markets: &HashMap<String, MarketHoldings>,
) -> anyhow::Result<HashMap<String, ObservedConditionalBalances>> {
    let mut market_tokens = HashMap::new();
    let mut balances = HashMap::new();

    for (market_id, holdings) in markets {
        let tokens = match resolve_market_tokens(state, &mut market_tokens, market_id).await {
            Ok(Some(tokens)) => tokens,
            Ok(None) => continue,
            Err(error) => {
                warn!(
                    market_id = %market_id,
                    error = %error,
                    "Failed to resolve market tokens for live equity valuation"
                );
                continue;
            }
        };

        let observe = |token_id: String, held: Decimal, leg: &'static str| {
            let market_id = market_id.clone();
            async move {
                if held <= Decimal::ZERO {
                    return None;
                }
                load_conditional_balance(state, &token_id)
                    .await
                    .map_err(|error| {
                        warn!(
                            market_id = %market_id,
                            token_id = %token_id,
                            leg,
                            error = %error,
                            "Failed to load observed conditional balance"
                        );
                        error
                    })
                    .ok()
                    .flatten()
            }
        };
        let yes = observe(tokens.0, holdings.yes_qty, "yes").await;
        let no = observe(tokens.1, holdings.no_qty, "no").await;

        balances.insert(market_id.clone(), ObservedConditionalBalances { yes, no });
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

    Ok(snapshot?.balance_in_units())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held_row(market: &str, entry: (i64, i64), held: (i64, i64)) -> HeldPositionRow {
        HeldPositionRow {
            market_id: market.to_string(),
            current_price: Some(Decimal::new(90, 2)),
            yes_price: None,
            no_price: None,
            yes_entry_price: Decimal::new(entry.0, 2),
            no_entry_price: Decimal::new(entry.1, 2),
            held_yes_qty: Decimal::from(held.0),
            held_no_qty: Decimal::from(held.1),
        }
    }

    #[test]
    fn holdings_value_every_row_and_use_weighted_cost() {
        let mut holdings = MarketHoldings::default();
        holdings.add(&HeldPositionRow {
            yes_price: Some(Decimal::new(50, 2)),
            ..held_row("m", (40, 0), (10, 0))
        });
        holdings.add(&held_row("m", (60, 0), (5, 0)));

        let (value, cost, unpriced) = holdings.value(holdings.yes_qty, holdings.no_qty);
        assert_eq!(value, Decimal::new(750, 2));
        assert_eq!(cost, Decimal::new(700, 2));
        assert_eq!(unpriced, Decimal::ZERO);
    }

    #[test]
    fn blended_row_mark_is_never_used_for_paired_legs() {
        let mut holdings = MarketHoldings::default();
        holdings.add(&held_row("m", (48, 50), (100, 100)));

        // No per-leg market price: both legs fall back to cost and are unpriced.
        let (value, cost, unpriced) = holdings.value(holdings.yes_qty, holdings.no_qty);
        assert_eq!(value, Decimal::new(98, 0));
        assert_eq!(cost, Decimal::new(98, 0));
        assert_eq!(unpriced, Decimal::new(98, 0));

        // A single held leg may use the row mark.
        let mut single = MarketHoldings::default();
        single.add(&held_row("m", (40, 0), (10, 0)));
        assert_eq!(
            single.value(single.yes_qty, single.no_qty).0,
            Decimal::new(9, 0)
        );
    }

    #[sqlx::test(migrations = false)]
    async fn fallback_valuation_counts_explicit_holdings_of_every_row(pool: PgPool) {
        crate::exit_handler::correctness_tests::setup(&pool).await;
        let state = AppState::for_tests(pool.clone());
        sqlx::query("INSERT INTO markets(id, yes_price, no_price) VALUES ('m', 0.5, 0.5)")
            .execute(&pool)
            .await
            .unwrap();
        for (yes_entry, held_yes, yes_exit, open) in [
            // Partly exited: an exit price exists, but 10 shares are still held.
            ("0.40", "10", Some("0.55"), true),
            // A second position in the same market and source.
            ("0.60", "5", None, true),
            // Flat history.
            ("0.30", "0", Some("0.70"), false),
        ] {
            sqlx::query(
                "INSERT INTO positions(id, market_id, yes_entry_price, no_entry_price, quantity, \
                 held_yes_qty, held_no_qty, yes_exit_price, is_open, source) \
                 VALUES ($1, 'm', $2::numeric, 0, 20, $3::numeric, 0, $4::numeric, $5, 1)",
            )
            .bind(Uuid::new_v4())
            .bind(yes_entry)
            .bind(held_yes)
            .bind(yes_exit)
            .bind(open)
            .execute(&pool)
            .await
            .unwrap();
        }

        let valuation = load_position_valuation_from_positions(&state)
            .await
            .unwrap();
        assert_eq!(valuation.open_positions, 2);
        assert_eq!(valuation.open_markets, 1);
        assert_eq!(valuation.position_value, Decimal::new(750, 2));
        assert_eq!(valuation.unrealized_pnl, Decimal::new(50, 2));
    }

    #[sqlx::test(migrations = false)]
    async fn only_outbound_cash_in_the_window_counts_as_withdrawal(pool: PgPool) {
        sqlx::raw_sql(
            "CREATE TABLE cash_flow_events (id UUID PRIMARY KEY, workspace_id UUID NOT NULL, \
             event_type TEXT NOT NULL, amount DECIMAL(18, 8) NOT NULL, \
             created_at TIMESTAMPTZ NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let workspace = Uuid::new_v4();
        let start = Utc::now() - Duration::minutes(10);
        let end = Utc::now();
        for (ws, kind, amount, minutes) in [
            (workspace, "withdrawal", "-100", 5),
            (workspace, "transfer", "-20", 5),
            (workspace, "deposit", "500", 5),
            (workspace, "fee", "-3", 5),
            (workspace, "withdrawal", "-7", 15),
            (Uuid::new_v4(), "withdrawal", "-50", 5),
        ] {
            sqlx::query("INSERT INTO cash_flow_events VALUES ($1, $2, $3, $4::numeric, $5)")
                .bind(Uuid::new_v4())
                .bind(ws)
                .bind(kind)
                .bind(amount)
                .bind(end - Duration::minutes(minutes))
                .execute(&pool)
                .await
                .unwrap();
        }

        let total = load_withdrawals_between(&pool, workspace, start, end)
            .await
            .unwrap();
        assert_eq!(total, Decimal::new(-120, 0));
    }
}
