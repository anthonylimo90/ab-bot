//! Background wallet harvester — discovers wallets from CLOB trade feed.
//!
//! Periodically fetches recent trades from the Polymarket CLOB API,
//! aggregates per-wallet trade statistics (count, volume, timestamps),
//! and accumulates results into the database.

use polymarket_core::api::ClobClient;
use polymarket_core::db::wallets::WalletRepository;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

/// Configuration for the wallet harvester.
#[derive(Debug, Clone)]
pub struct WalletHarvesterConfig {
    /// Whether the harvester is enabled.
    pub enabled: bool,
    /// Interval between harvest cycles in seconds.
    pub interval_secs: u64,
    /// Number of trades to fetch per page.
    pub trades_per_fetch: u32,
    /// Maximum new wallets to analyze per cycle.
    pub max_new_per_cycle: usize,
    /// Number of pages to fetch per cycle (pagination depth).
    pub pages_per_cycle: usize,
}

impl Default for WalletHarvesterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 120,     // was 300 — faster discovery cycles
            trades_per_fetch: 1000, // per page
            max_new_per_cycle: 200, // was 50 — process more wallets per cycle
            pages_per_cycle: 3,     // fetch up to 3 pages (3000 trades)
        }
    }
}

impl WalletHarvesterConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("HARVESTER_ENABLED")
                .map(|v| v != "false")
                .unwrap_or(true),
            interval_secs: std::env::var("HARVESTER_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(120),
            trades_per_fetch: std::env::var("HARVESTER_TRADES_PER_FETCH")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000),
            max_new_per_cycle: std::env::var("HARVESTER_MAX_NEW_PER_CYCLE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(200),
            pages_per_cycle: std::env::var("HARVESTER_PAGES_PER_CYCLE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
        }
    }
}

/// Per-wallet aggregated stats from a batch of CLOB trades.
struct WalletTradeStats {
    trade_count: i64,
    total_volume: Decimal,
    first_seen: chrono::DateTime<chrono::Utc>,
    last_seen: chrono::DateTime<chrono::Utc>,
}

/// Spawn the wallet harvester as a background task.
pub fn spawn_wallet_harvester(
    config: WalletHarvesterConfig,
    clob_client: Arc<ClobClient>,
    pool: PgPool,
    db_semaphore: Arc<Semaphore>,
) {
    if !config.enabled {
        info!("Wallet harvester is disabled");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        trades_per_fetch = config.trades_per_fetch,
        max_new = config.max_new_per_cycle,
        pages_per_cycle = config.pages_per_cycle,
        "Spawning wallet harvester"
    );

    tokio::spawn(async move {
        harvester_loop(config, clob_client, pool, db_semaphore).await;
    });
}

async fn harvester_loop(
    config: WalletHarvesterConfig,
    clob_client: Arc<ClobClient>,
    pool: PgPool,
    db_semaphore: Arc<Semaphore>,
) {
    let wallet_repo = WalletRepository::new(pool.clone());
    let interval = Duration::from_secs(config.interval_secs);

    // Persist cursor across cycles so each cycle fetches a NEW page of trades
    let mut last_offset: Option<u64> = None;

    // Initial delay to let the server finish starting up
    tokio::time::sleep(Duration::from_secs(10)).await;

    let mut first_cycle = true;
    loop {
        match harvest_cycle(
            &config,
            &clob_client,
            &wallet_repo,
            &db_semaphore,
            &mut last_offset,
        )
        .await
        {
            Ok(trade_count) => {
                if first_cycle {
                    if trade_count > 0 {
                        info!(
                            trades = trade_count,
                            "Data API connectivity verified, fetched {} trades on first cycle",
                            trade_count
                        );
                    } else {
                        warn!("Data API returned 0 trades on first cycle — wallet discovery may be delayed");
                    }
                    first_cycle = false;
                }
            }
            Err(e) => {
                warn!(error = %e, "Wallet harvest cycle failed");
                if first_cycle {
                    warn!("First harvest cycle failed — Data API may be unreachable");
                    first_cycle = false;
                }
                // Reset cursor on error to avoid getting stuck on a bad offset
                last_offset = None;
            }
        }

        tokio::time::sleep(interval).await;
    }
}

async fn harvest_cycle(
    config: &WalletHarvesterConfig,
    clob_client: &ClobClient,
    wallet_repo: &WalletRepository,
    db_semaphore: &Semaphore,
    last_offset: &mut Option<u64>,
) -> anyhow::Result<usize> {
    // 1. Fetch recent trades from Data API with pagination (no semaphore — this is network I/O)
    let mut all_trades = Vec::new();
    let mut current_offset = *last_offset;

    for page in 0..config.pages_per_cycle {
        let (trades, next_offset) = clob_client
            .get_recent_trades(config.trades_per_fetch, current_offset)
            .await
            .map_err(|e| anyhow::anyhow!("CLOB trade fetch failed (page {}): {}", page, e))?;

        let page_count = trades.len();
        all_trades.extend(trades);

        if let Some(next) = next_offset {
            current_offset = Some(next);
        } else {
            // No more pages — reset cursor for next cycle
            current_offset = None;
            break;
        }

        if page_count < config.trades_per_fetch as usize {
            // Partial page — no more data available
            current_offset = None;
            break;
        }
    }

    // Advance cursor for next cycle
    *last_offset = current_offset;

    if all_trades.is_empty() {
        info!("No trades returned from Data API this cycle");
        // Reset offset when API returns nothing — likely wrapped around
        *last_offset = None;
        return Ok(0);
    }

    let trade_count = all_trades.len();

    // 2. Aggregate per-wallet stats from the trade batch (in-memory, no DB)
    let mut stats_map: HashMap<String, WalletTradeStats> = HashMap::new();
    let now = chrono::Utc::now();

    for trade in &all_trades {
        // Data API returns f64 for price and size
        let price = Decimal::from_f64_retain(trade.price).unwrap_or(Decimal::ZERO);
        let size = Decimal::from_f64_retain(trade.size).unwrap_or(Decimal::ZERO);
        let volume = price * size;

        // Data API returns Unix timestamp as i64
        let timestamp = chrono::DateTime::from_timestamp(trade.timestamp, 0).unwrap_or(now);

        // Data API returns wallet_address (proxyWallet)
        let wallet_addr = trade.wallet_address.to_lowercase();
        if wallet_addr.is_empty() || !wallet_addr.starts_with("0x") {
            continue;
        }

        let entry = stats_map.entry(wallet_addr).or_insert(WalletTradeStats {
            trade_count: 0,
            total_volume: Decimal::ZERO,
            first_seen: timestamp,
            last_seen: timestamp,
        });
        entry.trade_count += 1;
        entry.total_volume += volume;
        if timestamp < entry.first_seen {
            entry.first_seen = timestamp;
        }
        if timestamp > entry.last_seen {
            entry.last_seen = timestamp;
        }
    }

    // 3. Acquire semaphore before DB writes (steps 3 + 4)
    let _permit = db_semaphore.acquire().await.expect("semaphore closed");
    debug!("Wallet harvester acquired DB semaphore permit");

    // Sort by last_seen DESC (most recently active wallets first) before truncating
    let mut sorted_wallets: Vec<_> = stats_map.iter().collect();
    sorted_wallets.sort_by(|a, b| b.1.last_seen.cmp(&a.1.last_seen));

    let batch_rows: Vec<_> = sorted_wallets
        .iter()
        .take(config.max_new_per_cycle)
        .map(|(addr, stats)| {
            (
                (*addr).clone(),
                stats.trade_count,
                stats.total_volume,
                stats.first_seen,
                stats.last_seen,
            )
        })
        .collect();

    // Warn when batch cap is hit — indicates we're truncating useful data
    if stats_map.len() > config.max_new_per_cycle {
        warn!(
            unique_wallets = stats_map.len(),
            batch_cap = config.max_new_per_cycle,
            dropped = stats_map.len() - config.max_new_per_cycle,
            "Batch cap reached — {} wallets dropped (consider raising HARVESTER_MAX_NEW_PER_CYCLE)",
            stats_map.len() - config.max_new_per_cycle
        );
    }

    let harvested = match wallet_repo.accumulate_features_batch(&batch_rows).await {
        Ok(rows) => rows as u32,
        Err(e) => {
            warn!(error = %e, "Failed to batch-accumulate wallet features");
            0
        }
    };

    // 4. Store trades in bulk batches (50 per INSERT) to reduce pool pressure
    let mut trades_inserted = 0u32;
    let valid_trades: Vec<_> = all_trades
        .iter()
        .filter_map(|trade| {
            let wallet_addr = trade.wallet_address.to_lowercase();
            if wallet_addr.is_empty() || !wallet_addr.starts_with("0x") {
                return None;
            }
            let price = Decimal::from_f64_retain(trade.price).unwrap_or(Decimal::ZERO);
            let size = Decimal::from_f64_retain(trade.size).unwrap_or(Decimal::ZERO);
            let value = price * size;
            let timestamp = chrono::DateTime::from_timestamp(trade.timestamp, 0).unwrap_or(now);
            Some((trade, wallet_addr, price, size, value, timestamp))
        })
        .collect();

    // Batch inserts — 50 rows per query keeps parameter count under PostgreSQL's limit
    const BATCH_SIZE: usize = 50;
    for chunk in valid_trades.chunks(BATCH_SIZE) {
        let mut query_builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            "INSERT INTO wallet_trades (
                transaction_hash, wallet_address, asset_id, condition_id,
                side, price, quantity, value, timestamp,
                title, slug, outcome
            ) ",
        );

        query_builder.push_values(
            chunk,
            |mut b, (trade, wallet_addr, price, size, value, timestamp)| {
                b.push_bind(&trade.transaction_hash)
                    .push_bind(wallet_addr)
                    .push_bind(&trade.asset_id)
                    .push_bind(&trade.condition_id)
                    .push_bind(&trade.side)
                    .push_bind(price)
                    .push_bind(size)
                    .push_bind(value)
                    .push_bind(timestamp)
                    .push_bind(&trade.title)
                    .push_bind(&trade.slug)
                    .push_bind(&trade.outcome);
            },
        );
        query_builder.push(" ON CONFLICT (transaction_hash) DO NOTHING");

        match query_builder.build().execute(wallet_repo.pool()).await {
            Ok(result) => trades_inserted += result.rows_affected() as u32,
            Err(e) => {
                warn!(batch_size = chunk.len(), error = %e, "Failed to insert trade batch");
            }
        }
    }

    // Release semaphore — all DB writes done
    drop(_permit);
    debug!("Wallet harvester released DB semaphore permit");

    info!(
        harvested = harvested,
        total_clob_trades = trade_count,
        unique_addresses = stats_map.len(),
        trades_inserted = trades_inserted,
        pages_fetched = config.pages_per_cycle,
        cursor_offset = ?last_offset,
        "Harvested {} new wallets from {} trades ({} trades stored)",
        harvested,
        trade_count,
        trades_inserted
    );

    Ok(trade_count)
}
