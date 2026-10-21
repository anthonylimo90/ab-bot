//! Canonical wallet inventory reconciliation and orphan recovery helpers.

use anyhow::Context;
use chrono::Utc;
use polymarket_core::api::{ClobClient, PolygonClient};
use polymarket_core::db::inventory::{
    WalletInventoryEntry, WalletInventoryRepository, WalletInventorySummary, WalletInventoryUpsert,
};
use polymarket_core::types::{MarketOrder, OrderSide};
use rust_decimal::Decimal;
use sqlx::FromRow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time;
use tokio::sync::broadcast;
use tracing::{info, warn};
use uuid::Uuid;

use risk_manager::circuit_breaker::CircuitBreaker;
use trading_engine::OrderExecutor;

use crate::state::AppState;
mod refresh;
use crate::trade_events::{NewTradeEvent, TradeEventRecorder};
use crate::websocket::{SignalType, SignalUpdate};
use crate::workspace_scope::resolve_canonical_workspace_id;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct WalletInventoryConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub initial_phase2_lookback_blocks: u64,
    pub phase2_backfill_start_block: u64,
    pub phase2_chunk_blocks: u64,
    pub phase2_backfill_chunks_per_refresh: u64,
}

impl WalletInventoryConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("WALLET_INVENTORY_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
            interval_secs: std::env::var("WALLET_INVENTORY_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(180),
            initial_phase2_lookback_blocks: std::env::var(
                "WALLET_INVENTORY_INITIAL_LOOKBACK_BLOCKS",
            )
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2_000_000),
            phase2_backfill_start_block: std::env::var(
                "WALLET_INVENTORY_PHASE2_BACKFILL_START_BLOCK",
            )
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
            phase2_chunk_blocks: std::env::var("WALLET_INVENTORY_PHASE2_CHUNK_BLOCKS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2_000_000),
            phase2_backfill_chunks_per_refresh: std::env::var(
                "WALLET_INVENTORY_PHASE2_BACKFILL_CHUNKS_PER_REFRESH",
            )
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WalletInventoryRefreshResult {
    pub wallet_address: String,
    pub candidate_tokens: usize,
    pub nonzero_tokens: usize,
    pub orphan_tokens: usize,
    pub phase2_from_block: Option<u64>,
    pub phase2_to_block: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct OrphanRecoveryResult {
    pub attempted: usize,
    pub succeeded: usize,
}

#[derive(Debug, Clone, Default)]
struct InventoryCandidate {
    token_id: String,
    condition_id: Option<String>,
    outcome: Option<String>,
    linked_position_id: Option<Uuid>,
    cost_basis: Option<Decimal>,
    discovery_source: String,
}

#[derive(Debug, Clone, FromRow)]
struct WalletTradeCandidateRow {
    token_id: String,
    condition_id: Option<String>,
    outcome: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
struct EffectiveOpenPositionRow {
    id: Uuid,
    market_id: String,
    held_yes_qty: Decimal,
    held_no_qty: Decimal,
    yes_entry_price: Decimal,
    no_entry_price: Decimal,
}

#[derive(Debug, Clone, FromRow)]
struct MarketPriceRow {
    id: String,
    yes_price: Option<Decimal>,
    no_price: Option<Decimal>,
}

#[derive(Debug, Clone, Default)]
struct Phase2DiscoveryResult {
    token_ids: Vec<String>,
    scanned_from_block: Option<u64>,
    scanned_to_block: Option<u64>,
}

pub fn spawn_wallet_inventory_reconciler(config: WalletInventoryConfig, state: Arc<AppState>) {
    if !config.enabled {
        info!("Wallet inventory reconciler disabled");
        return;
    }

    tokio::spawn(async move {
        if let Err(error) = refresh_canonical_wallet_inventory(state.as_ref()).await {
            warn!(error = %error, "Initial wallet inventory refresh failed");
        }

        let interval = time::Duration::from_secs(config.interval_secs);
        loop {
            tokio::time::sleep(interval).await;
            if let Err(error) = refresh_canonical_wallet_inventory(state.as_ref()).await {
                warn!(error = %error, "Wallet inventory refresh failed");
            }
        }
    });
}

pub async fn load_canonical_inventory_summary(
    state: &AppState,
) -> anyhow::Result<Option<WalletInventorySummary>> {
    let Some(wallet_address) = resolve_canonical_wallet_address(state).await? else {
        return Ok(None);
    };

    let repo = WalletInventoryRepository::new(state.pool.clone());
    Ok(Some(repo.summarize_wallet(&wallet_address).await?))
}

pub async fn refresh_canonical_wallet_inventory(
    state: &AppState,
) -> anyhow::Result<Option<WalletInventoryRefreshResult>> {
    refresh_inventory(state, time::Duration::from_secs(30)).await
}

pub async fn force_refresh_canonical_wallet_inventory(
    state: &AppState,
) -> anyhow::Result<Option<WalletInventoryRefreshResult>> {
    refresh_inventory(state, time::Duration::ZERO).await
}

async fn refresh_inventory(
    state: &AppState,
    max_age: time::Duration,
) -> anyhow::Result<Option<WalletInventoryRefreshResult>> {
    let Some(wallet_address) = resolve_canonical_wallet_address(state).await? else {
        return Ok(None);
    };
    if !state.order_executor.is_live_ready().await {
        return Ok(None);
    }
    refresh::coordinate(
        &state.pool,
        &state.inventory_refresh,
        &wallet_address,
        max_age,
        || refresh_wallet_inventory(state, &wallet_address),
    )
    .await
}

pub(crate) async fn canonical_refresh_status(
    state: &AppState,
) -> anyhow::Result<Option<refresh::RefreshStatus>> {
    let Some(wallet) = resolve_canonical_wallet_address(state).await? else {
        return Ok(None);
    };
    refresh::status(&state.pool, &wallet).await
}

async fn refresh_wallet_inventory(
    state: &AppState,
    wallet_address: &str,
) -> anyhow::Result<Option<WalletInventoryRefreshResult>> {
    let repo = WalletInventoryRepository::new(state.pool.clone());
    let existing_entries = repo.list_wallet_entries(wallet_address).await?;
    let existing_by_token: HashMap<String, WalletInventoryEntry> = existing_entries
        .into_iter()
        .map(|entry| (entry.token_id.clone(), entry))
        .collect();

    let now = Utc::now();
    let mut candidates = HashMap::<String, InventoryCandidate>::new();

    for token_id in repo.list_wallet_token_ids(wallet_address).await? {
        merge_candidate(
            &mut candidates,
            InventoryCandidate {
                token_id,
                discovery_source: "inventory".to_string(),
                ..InventoryCandidate::default()
            },
        );
    }

    for trade in load_wallet_trade_candidates(&state.pool, wallet_address).await? {
        merge_candidate(
            &mut candidates,
            InventoryCandidate {
                token_id: trade.token_id,
                condition_id: trade.condition_id,
                outcome: normalize_outcome(trade.outcome.as_deref()),
                discovery_source: "wallet_trade".to_string(),
                ..InventoryCandidate::default()
            },
        );
    }

    let manual: Vec<(String,String,Decimal)> = sqlx::query_as("SELECT market_id,outcome,cost_basis FROM manual_inventory WHERE execution_mode='live' AND wallet_address=LOWER($1) AND quantity>0")
        .bind(wallet_address).fetch_all(&state.pool).await?;
    let manual_keys: HashSet<(String, String)> = manual
        .iter()
        .map(|(market, outcome, _)| (market.clone(), outcome.clone()))
        .collect();
    for (market, outcome, basis) in manual {
        if let Some((yes, no)) = state.market_catalog.hydrate_market(&market).await? {
            merge_candidate(
                &mut candidates,
                InventoryCandidate {
                    token_id: if outcome == "yes" { yes } else { no },
                    condition_id: Some(market),
                    outcome: Some(outcome),
                    cost_basis: Some(basis),
                    discovery_source: "manual_order".into(),
                    ..Default::default()
                },
            );
        }
    }

    for position in load_effective_open_positions(&state.pool, wallet_address).await? {
        let Some((yes_token_id, no_token_id)) = state
            .market_catalog
            .hydrate_market(&position.market_id)
            .await?
        else {
            continue;
        };

        if holds_yes(&position) {
            merge_candidate(
                &mut candidates,
                InventoryCandidate {
                    token_id: yes_token_id,
                    condition_id: Some(position.market_id.clone()),
                    outcome: Some("yes".to_string()),
                    linked_position_id: Some(position.id),
                    cost_basis: Some(position.held_yes_qty * position.yes_entry_price),
                    discovery_source: "open_position".to_string(),
                },
            );
        }

        if holds_no(&position) {
            merge_candidate(
                &mut candidates,
                InventoryCandidate {
                    token_id: no_token_id,
                    condition_id: Some(position.market_id.clone()),
                    outcome: Some("no".to_string()),
                    linked_position_id: Some(position.id),
                    cost_basis: Some(position.held_no_qty * position.no_entry_price),
                    discovery_source: "open_position".to_string(),
                },
            );
        }
    }

    let mut phase2_from_block = None;
    let mut phase2_to_block = None;
    if let Some(polygon_client) = state.polygon_client.as_ref() {
        let inventory_config = WalletInventoryConfig::from_env();
        match discover_phase2_token_ids(polygon_client, &repo, wallet_address, &inventory_config)
            .await
        {
            Ok(discovery) => {
                phase2_from_block = discovery.scanned_from_block;
                phase2_to_block = discovery.scanned_to_block;
                for token_id in discovery.token_ids {
                    merge_candidate(
                        &mut candidates,
                        InventoryCandidate {
                            token_id,
                            discovery_source: "chain_log".to_string(),
                            ..InventoryCandidate::default()
                        },
                    );
                }
            }
            Err(error) => {
                warn!(
                    wallet_address = %wallet_address,
                    error = %error,
                    "Phase 2 CTF transfer discovery failed"
                );
            }
        }
    }

    if candidates.is_empty() {
        return Ok(Some(WalletInventoryRefreshResult {
            wallet_address: wallet_address.to_owned(),
            phase2_from_block,
            phase2_to_block,
            ..WalletInventoryRefreshResult::default()
        }));
    }

    hydrate_condition_ids(&state.pool, &mut candidates).await?;
    hydrate_outcomes(state.market_catalog.as_ref(), &mut candidates).await?;
    let market_prices = load_market_prices(&state.pool, &candidates).await?;

    // Complete all reads before publishing any balances. A failed token read
    // retains the previous snapshot, including previously observed zero balances.
    let balances = refresh::balances(candidates.keys().cloned().collect(), |token| async move {
        load_conditional_balance(state, &token).await
    })
    .await?;
    let mut rows = Vec::with_capacity(candidates.len());
    let mut nonzero_tokens = 0_usize;
    let mut orphan_tokens = 0_usize;

    for (_, candidate) in candidates {
        let quantity = balances[&candidate.token_id];
        if quantity > Decimal::ZERO {
            nonzero_tokens += 1;
        }

        let current_price = candidate
            .condition_id
            .as_ref()
            .and_then(|market_id| market_prices.get(market_id))
            .and_then(|market| match candidate.outcome.as_deref() {
                Some("yes") => market.yes_price,
                Some("no") => market.no_price,
                _ => None,
            });
        let marked_value = current_price.map(|price| price * quantity);
        let is_manual = candidate
            .condition_id
            .as_ref()
            .zip(candidate.outcome.as_ref())
            .is_some_and(|(market, outcome)| {
                manual_keys.contains(&(market.clone(), outcome.clone()))
            });
        let is_orphan =
            quantity > Decimal::ZERO && candidate.linked_position_id.is_none() && !is_manual;
        if is_orphan {
            orphan_tokens += 1;
        }

        let previous = existing_by_token.get(&candidate.token_id);
        let recovery_status = derive_recovery_status(previous, quantity, is_orphan);
        let last_exit_error = if recovery_status == "sell_failed" {
            previous.and_then(|entry| entry.last_exit_error.clone())
        } else {
            None
        };
        let last_exit_attempted_at = if is_orphan {
            previous.and_then(|entry| entry.last_exit_attempted_at)
        } else {
            None
        };

        rows.push(WalletInventoryUpsert {
            wallet_address: wallet_address.to_owned(),
            token_id: candidate.token_id.clone(),
            condition_id: candidate.condition_id.clone(),
            outcome: candidate.outcome.clone(),
            linked_position_id: candidate.linked_position_id,
            quantity,
            cost_basis: candidate
                .cost_basis
                .or_else(|| previous.and_then(|entry| entry.cost_basis)),
            current_price,
            marked_value,
            is_orphan,
            discovery_source: candidate.discovery_source.clone(),
            recovery_status,
            last_exit_error,
            last_exit_attempted_at,
            first_observed_at: previous.map(|entry| entry.first_observed_at).unwrap_or(now),
            last_observed_at: now,
        });
    }

    repo.upsert_entries(&rows).await?;

    Ok(Some(WalletInventoryRefreshResult {
        wallet_address: wallet_address.to_owned(),
        candidate_tokens: rows.len(),
        nonzero_tokens,
        orphan_tokens,
        phase2_from_block,
        phase2_to_block,
    }))
}

pub async fn recover_canonical_orphan_inventory(
    state: &AppState,
    retry_backoff_secs: u64,
) -> anyhow::Result<OrphanRecoveryResult> {
    let Some(wallet_address) = resolve_canonical_wallet_address(state).await? else {
        return Ok(OrphanRecoveryResult::default());
    };

    let recorder = TradeEventRecorder::new(state.pool.clone(), state.trade_event_tx.clone());
    recover_wallet_orphan_inventory(
        &state.pool,
        state.order_executor.as_ref(),
        state.clob_client.as_ref(),
        &wallet_address,
        retry_backoff_secs,
        Some(&recorder),
        Some(&state.signal_tx),
        Some(state.circuit_breaker.as_ref()),
    )
    .await
}

pub async fn load_canonical_orphan_inventory(
    state: &AppState,
) -> anyhow::Result<Vec<WalletInventoryEntry>> {
    let Some(wallet_address) = resolve_canonical_wallet_address(state).await? else {
        return Ok(Vec::new());
    };

    let repo = WalletInventoryRepository::new(state.pool.clone());
    Ok(repo.get_recoverable_orphans(&wallet_address, 0).await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn recover_wallet_orphan_inventory(
    pool: &sqlx::PgPool,
    order_executor: &OrderExecutor,
    clob_client: &ClobClient,
    wallet_address: &str,
    retry_backoff_secs: u64,
    trade_event_recorder: Option<&TradeEventRecorder>,
    signal_tx: Option<&broadcast::Sender<SignalUpdate>>,
    circuit_breaker: Option<&CircuitBreaker>,
) -> anyhow::Result<OrphanRecoveryResult> {
    if !order_executor.is_live_ready().await {
        return Ok(OrphanRecoveryResult::default());
    }

    let mut recovery_fence = pool.begin().await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext('orphan-inventory-recovery'),hashtext(LOWER($1)))")
        .bind(wallet_address).fetch_one(&mut *recovery_fence).await?;
    if !acquired {
        return Ok(OrphanRecoveryResult::default());
    }
    let observed = refresh::status(pool, wallet_address).await?;
    let freshness = WalletInventoryConfig::from_env()
        .interval_secs
        .saturating_mul(2)
        .max(60);
    anyhow::ensure!(
        observed.as_ref().is_some_and(|s| s.last_error.is_none()
            && s.finished_at.is_some()
            && s.completed_at.is_some_and(
                |at| Utc::now().signed_duration_since(at).num_seconds() <= freshness as i64
            )),
        "Inventory must complete a successful refresh before orphan recovery"
    );
    let repo = WalletInventoryRepository::new(pool.clone());
    let entries = repo
        .get_recoverable_orphans(wallet_address, retry_backoff_secs)
        .await?;

    let mut result = OrphanRecoveryResult::default();
    for entry in entries {
        let Some(market_id) = entry.condition_id.clone() else {
            repo.record_recovery_attempt(
                wallet_address,
                &entry.token_id,
                Some("missing condition_id for orphan inventory"),
            )
            .await?;
            continue;
        };

        if entry.quantity <= Decimal::ZERO {
            continue;
        }

        result.attempted += 1;

        let expected_price = match load_expected_sell_price(clob_client, &entry.token_id).await {
            Ok(Some(price)) => price,
            Ok(None) => {
                repo.record_recovery_attempt(
                    wallet_address,
                    &entry.token_id,
                    Some("no bids available for orphan inventory exit"),
                )
                .await?;
                continue;
            }
            Err(error) => {
                repo.record_recovery_attempt(
                    wallet_address,
                    &entry.token_id,
                    Some(&format!("failed to load orphan exit orderbook: {error}")),
                )
                .await?;
                continue;
            }
        };

        let order = MarketOrder::new(
            market_id.clone(),
            entry.token_id.clone(),
            OrderSide::Sell,
            entry.quantity,
        )
        .with_expected_price(expected_price)
        .with_slippage(order_executor.default_slippage());

        match order_executor
            .execute_reserved_market_order(order, "live", Some(wallet_address))
            .await
        {
            Ok(report) if report.is_success() => {
                repo.record_recovery_attempt(wallet_address, &entry.token_id, None)
                    .await?;
                result.succeeded += 1;

                // A partial fill only disposes of its share of the cost basis.
                let realized_pnl = entry.cost_basis.and_then(|cost_basis| {
                    (entry.quantity > Decimal::ZERO).then(|| {
                        let filled = report.filled_quantity.min(entry.quantity);
                        report.total_value()
                            - report.fees_paid
                            - cost_basis * filled / entry.quantity
                    })
                });

                if let Some(recorder) = trade_event_recorder {
                    recorder
                        .record_warn(
                            NewTradeEvent::new(
                                "inventory_recovery",
                                if order_executor.is_live() {
                                    "live"
                                } else {
                                    "paper"
                                },
                                "inventory",
                                market_id.clone(),
                                "orphan_inventory_exit_filled",
                            )
                            .with_position(None)
                            .with_reason(Some(format!(
                                "Recovered orphan inventory token {}",
                                entry.token_id
                            )))
                            .with_fill(report.average_price, report.total_value())
                            .with_realized_pnl(realized_pnl)
                            .with_metadata(serde_json::json!({
                                "token_id": entry.token_id,
                                "wallet_address": wallet_address,
                                "quantity": entry.quantity,
                                "recovery_status": entry.recovery_status,
                            })),
                        )
                        .await;
                }

                if let (Some(cb), Some(realized_pnl)) = (circuit_breaker, realized_pnl) {
                    let _ = cb
                        .record_trade(realized_pnl, realized_pnl > Decimal::ZERO)
                        .await;
                }

                if let Some(tx) = signal_tx {
                    let _ = tx.send(SignalUpdate {
                        signal_id: Uuid::new_v4(),
                        signal_type: SignalType::Alert,
                        market_id: market_id.clone(),
                        outcome_id: entry
                            .outcome
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string()),
                        action: "orphan_inventory_recovered".to_string(),
                        confidence: 1.0,
                        timestamp: Utc::now(),
                        metadata: serde_json::json!({
                            "token_id": entry.token_id,
                            "wallet_address": wallet_address,
                            "quantity": entry.quantity,
                        }),
                    });
                }
            }
            Ok(report) => {
                let message = report
                    .error_message
                    .unwrap_or_else(|| "orphan inventory exit rejected".to_string());
                repo.record_recovery_attempt(wallet_address, &entry.token_id, Some(&message))
                    .await?;
            }
            Err(error) => {
                repo.record_recovery_attempt(
                    wallet_address,
                    &entry.token_id,
                    Some(&format!("orphan inventory exit failed: {error}")),
                )
                .await?;
            }
        }
    }

    Ok(result)
}

trait NewTradeEventExt {
    fn with_position(self, position_id: Option<Uuid>) -> Self;
    fn with_reason(self, reason: Option<String>) -> Self;
    fn with_fill(self, fill_price: Decimal, filled_size_usd: Decimal) -> Self;
    fn with_realized_pnl(self, realized_pnl: Option<Decimal>) -> Self;
    fn with_metadata(self, metadata: serde_json::Value) -> Self;
}

impl NewTradeEventExt for NewTradeEvent {
    fn with_position(mut self, position_id: Option<Uuid>) -> Self {
        self.position_id = position_id;
        self
    }

    fn with_reason(mut self, reason: Option<String>) -> Self {
        self.reason = reason;
        self
    }

    fn with_fill(mut self, fill_price: Decimal, filled_size_usd: Decimal) -> Self {
        self.fill_price = Some(fill_price);
        self.filled_size_usd = Some(filled_size_usd);
        self
    }

    fn with_realized_pnl(mut self, realized_pnl: Option<Decimal>) -> Self {
        self.realized_pnl = realized_pnl;
        self
    }

    fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

async fn resolve_canonical_wallet_address(state: &AppState) -> anyhow::Result<Option<String>> {
    if let Some(address) = state.order_executor.wallet_address().await {
        return Ok(Some(address.to_lowercase()));
    }

    let Some(workspace_id) = resolve_canonical_workspace_id(&state.pool).await? else {
        return Ok(None);
    };

    let row: Option<(Option<String>,)> = sqlx::query_as(
        r#"
        SELECT trading_wallet_address
        FROM workspaces
        WHERE id = $1
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await?;

    Ok(row.and_then(|(address,)| address.map(|value| value.to_lowercase())))
}

async fn load_wallet_trade_candidates(
    pool: &sqlx::PgPool,
    wallet_address: &str,
) -> anyhow::Result<Vec<WalletTradeCandidateRow>> {
    let rows = sqlx::query_as::<_, WalletTradeCandidateRow>(
        r#"
        SELECT DISTINCT ON (asset_id)
            asset_id AS token_id,
            condition_id,
            outcome
        FROM wallet_trades
        WHERE LOWER(wallet_address) = LOWER($1)
          AND asset_id ~ '^[0-9]+$'
        ORDER BY asset_id, timestamp DESC
        "#,
    )
    .bind(wallet_address)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

async fn load_effective_open_positions(
    pool: &sqlx::PgPool,
    wallet: &str,
) -> anyhow::Result<Vec<EffectiveOpenPositionRow>> {
    Ok(sqlx::query_as("SELECT id,market_id,held_yes_qty,held_no_qty,yes_entry_price,no_entry_price FROM positions WHERE (held_yes_qty>0 OR held_no_qty>0) AND execution_mode<>'paper' AND (LOWER(wallet_address)=LOWER($1) OR execution_mode='unknown')")
        .bind(wallet).fetch_all(pool).await?)
}

async fn discover_phase2_token_ids(
    polygon_client: &PolygonClient,
    repo: &WalletInventoryRepository,
    wallet_address: &str,
    config: &WalletInventoryConfig,
) -> anyhow::Result<Phase2DiscoveryResult> {
    let current_block = polygon_client.get_block_number().await?;
    let sync_state = repo.get_sync_state(wallet_address).await?;
    let recent_from_block = sync_state
        .as_ref()
        .map(|state| {
            let last_scanned = (state.last_scanned_block.max(0)) as u64;
            last_scanned.saturating_add(1)
        })
        .unwrap_or_else(|| current_block.saturating_sub(config.initial_phase2_lookback_blocks));

    let mut token_ids = HashSet::new();
    let mut scanned_from_block = None;
    let mut scanned_to_block = None;

    scan_phase2_forward_range(
        polygon_client,
        wallet_address,
        recent_from_block,
        current_block,
        config.phase2_chunk_blocks,
        &mut token_ids,
        &mut scanned_from_block,
        &mut scanned_to_block,
    )
    .await?;

    let mut backfill_cursor_block = sync_state
        .as_ref()
        .and_then(|state| state.backfill_cursor_block)
        .map(|value| value.max(0) as u64)
        .or_else(|| recent_from_block.checked_sub(1));
    let mut backfill_complete = sync_state
        .as_ref()
        .and_then(|state| state.backfill_completed_at)
        .is_some()
        || backfill_cursor_block.is_none()
        || backfill_cursor_block.is_some_and(|cursor| cursor < config.phase2_backfill_start_block);

    if !backfill_complete {
        let outcome = scan_phase2_backfill_chunks(
            polygon_client,
            wallet_address,
            config.phase2_backfill_start_block,
            config.phase2_chunk_blocks,
            config.phase2_backfill_chunks_per_refresh,
            backfill_cursor_block,
            &mut token_ids,
            &mut scanned_from_block,
            &mut scanned_to_block,
        )
        .await?;
        backfill_cursor_block = outcome.next_cursor_block;
        backfill_complete = outcome.completed;
    }

    repo.upsert_sync_state(
        wallet_address,
        current_block,
        backfill_cursor_block,
        backfill_complete.then(Utc::now),
    )
    .await?;

    Ok(Phase2DiscoveryResult {
        token_ids: token_ids.into_iter().collect(),
        scanned_from_block,
        scanned_to_block,
    })
}

#[derive(Debug, Clone, Copy)]
struct BackfillScanOutcome {
    next_cursor_block: Option<u64>,
    completed: bool,
}

#[allow(clippy::too_many_arguments)]
async fn scan_phase2_forward_range(
    polygon_client: &PolygonClient,
    wallet_address: &str,
    from_block: u64,
    to_block: u64,
    chunk_blocks: u64,
    token_ids: &mut HashSet<String>,
    scanned_from_block: &mut Option<u64>,
    scanned_to_block: &mut Option<u64>,
) -> anyhow::Result<()> {
    if from_block > to_block {
        return Ok(());
    }

    let chunk_blocks = chunk_blocks.max(1);
    let mut chunk_from = from_block;
    loop {
        let chunk_to = chunk_from
            .saturating_add(chunk_blocks.saturating_sub(1))
            .min(to_block);
        append_phase2_token_ids(
            polygon_client,
            wallet_address,
            chunk_from,
            chunk_to,
            token_ids,
            scanned_from_block,
            scanned_to_block,
        )
        .await?;

        if chunk_to >= to_block {
            break;
        }
        chunk_from = chunk_to.saturating_add(1);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn scan_phase2_backfill_chunks(
    polygon_client: &PolygonClient,
    wallet_address: &str,
    start_block: u64,
    chunk_blocks: u64,
    max_chunks: u64,
    cursor_block: Option<u64>,
    token_ids: &mut HashSet<String>,
    scanned_from_block: &mut Option<u64>,
    scanned_to_block: &mut Option<u64>,
) -> anyhow::Result<BackfillScanOutcome> {
    let chunk_blocks = chunk_blocks.max(1);
    let mut cursor_block = match cursor_block {
        Some(block) if block >= start_block => block,
        _ => {
            return Ok(BackfillScanOutcome {
                next_cursor_block: None,
                completed: true,
            });
        }
    };

    let max_chunks = max_chunks.max(1);
    for _ in 0..max_chunks {
        let chunk_to = cursor_block;
        let chunk_from = chunk_to
            .saturating_sub(chunk_blocks.saturating_sub(1))
            .max(start_block);

        append_phase2_token_ids(
            polygon_client,
            wallet_address,
            chunk_from,
            chunk_to,
            token_ids,
            scanned_from_block,
            scanned_to_block,
        )
        .await?;

        if chunk_from == start_block {
            return Ok(BackfillScanOutcome {
                next_cursor_block: None,
                completed: true,
            });
        }

        cursor_block = chunk_from.saturating_sub(1);
    }

    Ok(BackfillScanOutcome {
        next_cursor_block: Some(cursor_block),
        completed: false,
    })
}

async fn append_phase2_token_ids(
    polygon_client: &PolygonClient,
    wallet_address: &str,
    from_block: u64,
    to_block: u64,
    token_ids: &mut HashSet<String>,
    scanned_from_block: &mut Option<u64>,
    scanned_to_block: &mut Option<u64>,
) -> anyhow::Result<()> {
    if from_block > to_block {
        return Ok(());
    }

    for token_id in polygon_client
        .get_ctf_transfer_token_ids(wallet_address, from_block, to_block)
        .await?
    {
        if looks_like_token_id(&token_id) {
            token_ids.insert(token_id);
        }
    }

    *scanned_from_block = Some(
        scanned_from_block
            .map(|existing| existing.min(from_block))
            .unwrap_or(from_block),
    );
    *scanned_to_block = Some(
        scanned_to_block
            .map(|existing| existing.max(to_block))
            .unwrap_or(to_block),
    );

    Ok(())
}

async fn hydrate_condition_ids(
    pool: &sqlx::PgPool,
    candidates: &mut HashMap<String, InventoryCandidate>,
) -> anyhow::Result<()> {
    let unresolved = candidates
        .values()
        .filter(|candidate| candidate.condition_id.is_none())
        .map(|candidate| candidate.token_id.clone())
        .collect::<Vec<_>>();

    if unresolved.is_empty() {
        return Ok(());
    }

    let rows = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT token_id, condition_id
        FROM token_condition_cache
        WHERE token_id = ANY($1)
        "#,
    )
    .bind(&unresolved)
    .fetch_all(pool)
    .await?;

    for (token_id, condition_id) in rows {
        if let Some(candidate) = candidates.get_mut(&token_id) {
            if candidate.condition_id.is_none() {
                candidate.condition_id = Some(condition_id);
            }
        }
    }

    Ok(())
}

async fn hydrate_outcomes(
    catalog: &crate::market_catalog::OutcomeTokenCache,
    candidates: &mut HashMap<String, InventoryCandidate>,
) -> anyhow::Result<()> {
    let market_ids = candidates
        .values()
        .filter(|candidate| candidate.outcome.is_none())
        .filter_map(|candidate| candidate.condition_id.clone())
        .collect::<HashSet<_>>();
    let mut market_cache = HashMap::<String, (String, String)>::new();

    for market_id in market_ids {
        if market_cache.contains_key(&market_id) {
            continue;
        }

        if let Some(tokens) = catalog.hydrate_market(&market_id).await? {
            market_cache.insert(market_id, tokens);
        }
    }

    for candidate in candidates.values_mut() {
        if candidate.outcome.is_some() {
            continue;
        }
        let Some(condition_id) = candidate.condition_id.as_ref() else {
            continue;
        };
        let Some((yes_token_id, no_token_id)) = market_cache.get(condition_id) else {
            continue;
        };

        if &candidate.token_id == yes_token_id {
            candidate.outcome = Some("yes".to_string());
        } else if &candidate.token_id == no_token_id {
            candidate.outcome = Some("no".to_string());
        }
    }

    Ok(())
}

async fn load_market_prices(
    pool: &sqlx::PgPool,
    candidates: &HashMap<String, InventoryCandidate>,
) -> anyhow::Result<HashMap<String, MarketPriceRow>> {
    let market_ids = candidates
        .values()
        .filter_map(|candidate| candidate.condition_id.clone())
        .collect::<Vec<_>>();

    if market_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query_as::<_, MarketPriceRow>(
        r#"
        SELECT id, yes_price, no_price
        FROM markets
        WHERE id = ANY($1)
        "#,
    )
    .bind(&market_ids)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|row| (row.id.clone(), row)).collect())
}

async fn load_expected_sell_price(
    clob_client: &ClobClient,
    token_id: &str,
) -> anyhow::Result<Option<Decimal>> {
    let book = clob_client
        .get_order_book(token_id)
        .await
        .with_context(|| format!("failed to load orderbook for {}", token_id))?;
    Ok(book.best_bid())
}

async fn load_conditional_balance(state: &AppState, token_id: &str) -> anyhow::Result<Decimal> {
    let snapshot = state
        .order_executor
        .get_live_balance_allowance("CONDITIONAL", Some(token_id))
        .await
        .with_context(|| format!("Balance unavailable for token {token_id}"))?;
    snapshot
        .balance_in_units()
        .with_context(|| format!("Invalid balance for token {token_id}"))
}

fn merge_candidate(
    candidates: &mut HashMap<String, InventoryCandidate>,
    incoming: InventoryCandidate,
) {
    let entry = candidates
        .entry(incoming.token_id.clone())
        .or_insert_with(|| incoming.clone());

    if entry.condition_id.is_none() {
        entry.condition_id = incoming.condition_id.clone();
    }
    if entry.outcome.is_none() {
        entry.outcome = incoming.outcome.clone();
    }
    if entry.linked_position_id.is_none() {
        entry.linked_position_id = incoming.linked_position_id;
    }
    if entry.cost_basis.is_none() {
        entry.cost_basis = incoming.cost_basis;
    }
    if entry.discovery_source == "inventory" || incoming.discovery_source == "open_position" {
        entry.discovery_source = incoming.discovery_source;
    }
}

fn derive_recovery_status(
    previous: Option<&WalletInventoryEntry>,
    quantity: Decimal,
    is_orphan: bool,
) -> String {
    if quantity <= Decimal::ZERO {
        return if previous.is_some_and(|entry| entry.quantity > Decimal::ZERO) {
            "recovered".to_string()
        } else {
            previous
                .map(|entry| entry.recovery_status.clone())
                .unwrap_or_else(|| "linked".to_string())
        };
    }

    if !is_orphan {
        return "linked".to_string();
    }

    match previous.map(|entry| entry.recovery_status.as_str()) {
        Some("sell_failed") => "sell_failed".to_string(),
        _ => "observed".to_string(),
    }
}

fn normalize_outcome(outcome: Option<&str>) -> Option<String> {
    match outcome?.trim().to_ascii_lowercase().as_str() {
        "yes" => Some("yes".to_string()),
        "no" => Some("no".to_string()),
        _ => None,
    }
}

fn holds_yes(position: &EffectiveOpenPositionRow) -> bool {
    position.held_yes_qty > Decimal::ZERO
}

fn holds_no(position: &EffectiveOpenPositionRow) -> bool {
    position.held_no_qty > Decimal::ZERO
}

fn looks_like_token_id(token_id: &str) -> bool {
    !token_id.is_empty() && token_id.chars().all(|ch| ch.is_ascii_digit())
}
