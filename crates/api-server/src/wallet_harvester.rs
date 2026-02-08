//! Background wallet harvester — discovers wallets from CLOB trade feed.
//!
//! Periodically fetches recent trades from the Polymarket CLOB API,
//! extracts wallet addresses, analyzes them with feature extraction,
//! and stores results in the database.

use polymarket_core::api::{ClobClient, PolygonClient};
use polymarket_core::db::wallets::WalletRepository;
use polymarket_core::types::BotScore;
use sqlx::PgPool;
use std::collections::HashSet;
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

/// Spawn the wallet harvester as a background task.
pub fn spawn_wallet_harvester(
    config: WalletHarvesterConfig,
    clob_client: Arc<ClobClient>,
    pool: PgPool,
    polygon_client: Option<Arc<PolygonClient>>,
) {
    if !config.enabled {
        info!("Wallet harvester is disabled");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        trades_per_fetch = config.trades_per_fetch,
        max_new = config.max_new_per_cycle,
        has_polygon = polygon_client.is_some(),
        "Spawning wallet harvester"
    );

    tokio::spawn(async move {
        harvester_loop(config, clob_client, pool, polygon_client).await;
    });
}

async fn harvester_loop(
    config: WalletHarvesterConfig,
    clob_client: Arc<ClobClient>,
    pool: PgPool,
    polygon_client: Option<Arc<PolygonClient>>,
) {
    let wallet_repo = WalletRepository::new(pool.clone());
    let interval = Duration::from_secs(config.interval_secs);

    // Initial delay to let the server finish starting up
    tokio::time::sleep(Duration::from_secs(10)).await;

    loop {
        if let Err(e) = harvest_cycle(
            &config,
            &clob_client,
            &pool,
            &wallet_repo,
            polygon_client.as_deref(),
        )
        .await
        {
            warn!(error = %e, "Wallet harvest cycle failed");
        }

        tokio::time::sleep(interval).await;
    }
}

async fn harvest_cycle(
    config: &WalletHarvesterConfig,
    clob_client: &ClobClient,
    pool: &PgPool,
    wallet_repo: &WalletRepository,
    polygon_client: Option<&PolygonClient>,
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

    // 2. Extract unique wallet addresses
    let mut addresses: HashSet<String> = HashSet::new();
    for trade in &trades {
        let addr = trade.maker_address.to_lowercase();
        if !addr.is_empty() && addr.starts_with("0x") {
            addresses.insert(addr);
        }
        if let Some(taker) = &trade.trader_address {
            let addr = taker.to_lowercase();
            if !addr.is_empty() && addr.starts_with("0x") {
                addresses.insert(addr);
            }
        }
    }

    // 3. Filter to addresses NOT already in wallet_features
    let mut new_addresses = Vec::new();
    for addr in &addresses {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM wallet_features WHERE LOWER(address) = $1)",
        )
        .bind(addr)
        .fetch_one(pool)
        .await
        .unwrap_or(false);

        if !exists {
            new_addresses.push(addr.clone());
        }

        if new_addresses.len() >= config.max_new_per_cycle {
            break;
        }
    }

    if new_addresses.is_empty() {
        debug!(
            total_addresses = addresses.len(),
            "All discovered addresses already in DB"
        );
        return Ok(());
    }

    // 4. Analyze each new address
    let mut harvested = 0u32;
    for addr in &new_addresses {
        match analyze_and_store(addr, wallet_repo, polygon_client).await {
            Ok(()) => harvested += 1,
            Err(e) => {
                debug!(address = %addr, error = %e, "Failed to analyze wallet");
            }
        }
    }

    info!(
        harvested = harvested,
        total_clob_trades = trades.len(),
        unique_addresses = addresses.len(),
        "Harvested {} new wallets from {} CLOB trades",
        harvested,
        trades.len()
    );

    Ok(())
}

async fn analyze_and_store(
    address: &str,
    wallet_repo: &WalletRepository,
    polygon_client: Option<&PolygonClient>,
) -> anyhow::Result<()> {
    let features = if let Some(polygon) = polygon_client {
        // Full analysis with on-chain data
        let transfers = polygon
            .get_asset_transfers(address, None, None)
            .await
            .map_err(|e| anyhow::anyhow!("Polygon fetch failed: {}", e))?;

        polymarket_core::feature_extractor::extract_features(address, &transfers)?
    } else {
        // Minimal features from CLOB data alone (no Polygon RPC)
        use polymarket_core::types::WalletFeatures;
        WalletFeatures {
            address: address.to_string(),
            total_trades: 1,
            first_trade: Some(chrono::Utc::now()),
            last_trade: Some(chrono::Utc::now()),
            ..Default::default()
        }
    };

    // Store features
    wallet_repo
        .upsert_features(&features)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to upsert features: {}", e))?;

    // Compute and store bot score
    let score = BotScore::new(address.to_string(), &features);
    wallet_repo
        .insert_score(&score)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to insert bot score: {}", e))?;

    Ok(())
}
