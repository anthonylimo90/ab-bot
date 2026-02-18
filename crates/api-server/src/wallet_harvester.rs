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
use tracing::{info, warn};

/// Configuration for the wallet harvester.
#[derive(Debug, Clone)]
pub struct WalletHarvesterConfig {
    /// Whether the harvester is enabled.
    pub enabled: bool,
    /// Interval between harvest cycles in seconds.
    pub interval_secs: u64,
    /// Number of trades to fetch per cycle.
    pub trades_per_fetch: u32,
    /// Maximum new wallets to analyze per cycle.
    pub max_new_per_cycle: usize,
}

impl Default for WalletHarvesterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 300,
            trades_per_fetch: 500,
            max_new_per_cycle: 50,
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
                .unwrap_or(300),
            trades_per_fetch: std::env::var("HARVESTER_TRADES_PER_FETCH")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(500),
            max_new_per_cycle: std::env::var("HARVESTER_MAX_NEW_PER_CYCLE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(50),
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
        harvester_loop(config, clob_client, pool).await;
    });
}

async fn harvester_loop(config: WalletHarvesterConfig, clob_client: Arc<ClobClient>, pool: PgPool) {
    let wallet_repo = WalletRepository::new(pool.clone());
    let interval = Duration::from_secs(config.interval_secs);

    // Initial delay to let the server finish starting up
    tokio::time::sleep(Duration::from_secs(10)).await;

    let mut first_cycle = true;
    loop {
        match harvest_cycle(&config, &clob_client, &wallet_repo).await {
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
) -> anyhow::Result<usize> {
    // 1. Fetch recent trades from CLOB
    let trades = clob_client
        .get_recent_trades(config.trades_per_fetch, None)
        .await
        .map_err(|e| anyhow::anyhow!("CLOB trade fetch failed: {}", e))?;

    if trades.is_empty() {
        info!("No trades returned from Data API this cycle");
        return Ok(0);
    }

    let trade_count = trades.len();

    // 2. Aggregate per-wallet stats from the trade batch
    let mut stats_map: HashMap<String, WalletTradeStats> = HashMap::new();
    let now = chrono::Utc::now();

    for trade in &trades {
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

    // 3. Accumulate stats into the database (cap at max_new_per_cycle)
    let mut harvested = 0u32;
    for (addr, stats) in stats_map.iter().take(config.max_new_per_cycle) {
        match wallet_repo
            .accumulate_features(
                addr,
                stats.trade_count,
                stats.total_volume,
                stats.first_seen,
                stats.last_seen,
            )
            .await
        {
            Ok(()) => harvested += 1,
            Err(e) => {
                warn!(address = %addr, error = %e, "Failed to accumulate wallet features");
            }
        }
    }

    // 4. Store individual trades for profitability analysis
    let mut trades_inserted = 0u32;
    for trade in &trades {
        let wallet_addr = trade.wallet_address.to_lowercase();
        if wallet_addr.is_empty() || !wallet_addr.starts_with("0x") {
            continue;
        }

        let price = Decimal::from_f64_retain(trade.price).unwrap_or(Decimal::ZERO);
        let size = Decimal::from_f64_retain(trade.size).unwrap_or(Decimal::ZERO);
        let value = price * size;
        let timestamp = chrono::DateTime::from_timestamp(trade.timestamp, 0).unwrap_or(now);

        // Insert trade with ON CONFLICT DO NOTHING for deduplication
        // Using sqlx::query (not query!) to avoid offline mode requirement
        let result = sqlx::query(
            r#"
            INSERT INTO wallet_trades (
                transaction_hash, wallet_address, asset_id, condition_id,
                side, price, quantity, value, timestamp,
                title, slug, outcome
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (transaction_hash) DO NOTHING
            "#,
        )
        .bind(&trade.transaction_hash)
        .bind(&wallet_addr)
        .bind(&trade.asset_id)
        .bind(&trade.condition_id)
        .bind(&trade.side)
        .bind(price)
        .bind(size)
        .bind(value)
        .bind(timestamp)
        .bind(&trade.title)
        .bind(&trade.slug)
        .bind(&trade.outcome)
        .execute(wallet_repo.pool())
        .await;

        match result {
            Ok(_) => trades_inserted += 1,
            Err(e) => {
                warn!(tx_hash = %trade.transaction_hash, error = %e, "Failed to insert trade");
            }
        }
    }

    info!(
        harvested = harvested,
        total_clob_trades = trade_count,
        unique_addresses = stats_map.len(),
        trades_inserted = trades_inserted,
        "Harvested {} new wallets from {} trades ({} trades stored)",
        harvested,
        trade_count,
        trades_inserted
    );

    Ok(trade_count)
}
