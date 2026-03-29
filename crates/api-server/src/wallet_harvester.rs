//! Background wallet harvester — discovers wallets from CLOB trade feed.
//!
//! Periodically fetches the latest trade snapshot from the Polymarket Data API,
//! aggregates per-wallet trade statistics (count, volume, timestamps),
//! and accumulates results into the database.

use polymarket_core::api::ClobClient;
use polymarket_core::db::wallets::WalletRepository;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
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
    /// Number of recent trades to fetch per cycle.
    pub trades_per_fetch: u32,
    /// Maximum new wallets to analyze per cycle.
    pub max_new_per_cycle: usize,
}

impl Default for WalletHarvesterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 120,
            trades_per_fetch: 1000,
            max_new_per_cycle: 1500,
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
                .unwrap_or(1500),
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

    // Initial delay to let the server finish starting up
    tokio::time::sleep(Duration::from_secs(10)).await;

    let mut first_cycle = true;
    loop {
        match harvest_cycle(&config, &clob_client, &wallet_repo, &db_semaphore).await {
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
) -> anyhow::Result<usize> {
    // 1. Fetch the latest public trade snapshot from the Data API.
    let all_trades = clob_client
        .get_recent_trades(config.trades_per_fetch)
        .await
        .map_err(|e| anyhow::anyhow!("CLOB trade fetch failed: {}", e))?;

    if all_trades.is_empty() {
        info!("No trades returned from Data API this cycle");
        return Ok(0);
    }

    let trade_count = all_trades.len();
    let now = chrono::Utc::now();
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

    // 2. Acquire semaphore before DB writes and downstream feature accumulation.
    let _permit = db_semaphore.acquire().await.expect("semaphore closed");
    debug!("Wallet harvester acquired DB semaphore permit");
    // 3. Store trades in bulk batches and track which hashes were genuinely new.
    let mut trades_inserted = 0u32;
    let mut inserted_hashes = HashSet::new();

    #[derive(sqlx::FromRow)]
    struct InsertedTradeRow {
        transaction_hash: String,
    }

    // Batch inserts — 200 rows per query reduces statement count without stressing Postgres.
    const BATCH_SIZE: usize = 200;
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
        // Conflict target includes timestamp because wallet_trades is a hypertable
        // (migration 062) and TimescaleDB requires unique indexes to include the
        // partitioning column. In practice each blockchain tx_hash has exactly one
        // immutable timestamp, so this is equivalent to ON CONFLICT (transaction_hash).
        query_builder.push(
            " ON CONFLICT (transaction_hash, timestamp) DO NOTHING RETURNING transaction_hash",
        );

        match query_builder
            .build_query_as::<InsertedTradeRow>()
            .fetch_all(wallet_repo.pool())
            .await
        {
            Ok(rows) => {
                trades_inserted += rows.len() as u32;
                inserted_hashes.extend(rows.into_iter().map(|row| row.transaction_hash));
            }
            Err(e) => {
                warn!(batch_size = chunk.len(), error = %e, "Failed to insert trade batch");
            }
        }
    }

    // 4. Aggregate features only from newly inserted trades so repeated snapshots
    // do not inflate wallet_features totals.
    let mut stats_map: HashMap<String, WalletTradeStats> = HashMap::new();
    for (trade, wallet_addr, _price, _size, value, timestamp) in &valid_trades {
        if !inserted_hashes.contains(&trade.transaction_hash) {
            continue;
        }

        let entry = stats_map
            .entry(wallet_addr.clone())
            .or_insert(WalletTradeStats {
                trade_count: 0,
                total_volume: Decimal::ZERO,
                first_seen: *timestamp,
                last_seen: *timestamp,
            });
        entry.trade_count += 1;
        entry.total_volume += *value;
        if *timestamp < entry.first_seen {
            entry.first_seen = *timestamp;
        }
        if *timestamp > entry.last_seen {
            entry.last_seen = *timestamp;
        }
    }

    let wallet_limit = if config.max_new_per_cycle == 0 {
        usize::MAX
    } else {
        config.max_new_per_cycle
    };

    // Sort by last_seen DESC (most recently active wallets first) before truncating
    let mut sorted_wallets: Vec<_> = stats_map.iter().collect();
    sorted_wallets.sort_by(|a, b| b.1.last_seen.cmp(&a.1.last_seen));

    let batch_rows: Vec<_> = sorted_wallets
        .iter()
        .take(wallet_limit)
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
    if wallet_limit != usize::MAX && stats_map.len() > wallet_limit {
        warn!(
            unique_wallets = stats_map.len(),
            batch_cap = wallet_limit,
            dropped = stats_map.len() - wallet_limit,
            "Batch cap reached — {} wallets dropped (consider raising HARVESTER_MAX_NEW_PER_CYCLE)",
            stats_map.len() - wallet_limit
        );
    }

    let harvested = match wallet_repo.accumulate_features_batch(&batch_rows).await {
        Ok(rows) => rows as u32,
        Err(e) => {
            warn!(error = %e, "Failed to batch-accumulate wallet features");
            0
        }
    };

    // Release semaphore — all DB writes done
    drop(_permit);
    debug!("Wallet harvester released DB semaphore permit");

    info!(
        harvested = harvested,
        total_clob_trades = trade_count,
        unique_addresses = stats_map.len(),
        new_wallets = batch_rows.len(),
        trades_inserted = trades_inserted,
        "Harvested {} new wallets from {} trades ({} trades stored)",
        harvested,
        trade_count,
        trades_inserted
    );

    Ok(trade_count)
}
