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
use tracing::{debug, info, warn};

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
            trades_per_fetch: 200,
            max_new_per_cycle: 20,
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
                .unwrap_or(200),
            max_new_per_cycle: std::env::var("HARVESTER_MAX_NEW_PER_CYCLE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
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

    loop {
        if let Err(e) = harvest_cycle(&config, &clob_client, &wallet_repo).await {
            warn!(error = %e, "Wallet harvest cycle failed");
        }

        tokio::time::sleep(interval).await;
    }
}

async fn harvest_cycle(
    config: &WalletHarvesterConfig,
    clob_client: &ClobClient,
    wallet_repo: &WalletRepository,
) -> anyhow::Result<()> {
    // 1. Fetch recent trades from CLOB
    let trades = clob_client
        .get_recent_trades(config.trades_per_fetch, None)
        .await
        .map_err(|e| anyhow::anyhow!("CLOB trade fetch failed: {}", e))?;

    if trades.is_empty() {
        debug!("No trades returned from CLOB API");
        return Ok(());
    }

    let trade_count = trades.len();

    // 2. Aggregate per-wallet stats from the trade batch
    let mut stats_map: HashMap<String, WalletTradeStats> = HashMap::new();
    let now = chrono::Utc::now();

    for trade in &trades {
        let price: Decimal = trade.price.parse().unwrap_or(Decimal::ZERO);
        let size: Decimal = trade.size.parse().unwrap_or(Decimal::ZERO);
        let volume = price * size;

        let timestamp = trade
            .created_at
            .as_deref()
            .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
            .or_else(|| {
                trade
                    .match_time
                    .as_deref()
                    .and_then(|s| s.parse::<i64>().ok())
                    .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
            })
            .unwrap_or(now);

        // Collect all addresses from this trade
        let mut addrs = Vec::with_capacity(2);
        let maker = trade.maker_address.to_lowercase();
        if !maker.is_empty() && maker.starts_with("0x") {
            addrs.push(maker);
        }
        if let Some(taker) = &trade.trader_address {
            let taker = taker.to_lowercase();
            if !taker.is_empty() && taker.starts_with("0x") {
                addrs.push(taker);
            }
        }

        for addr in addrs {
            let entry = stats_map.entry(addr).or_insert(WalletTradeStats {
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
                debug!(address = %addr, error = %e, "Failed to accumulate wallet features");
            }
        }
    }

    info!(
        harvested = harvested,
        total_clob_trades = trade_count,
        unique_addresses = stats_map.len(),
        "Harvested {} new wallets from {} CLOB trades",
        harvested,
        trade_count
    );

    Ok(())
}
