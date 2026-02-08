//! Bot Scanner
//!
//! Identifies automated trading wallets on Polymarket through behavioral analysis.

mod feature_extractor;
mod indexer;
mod scorer;

use anyhow::Result;
use polymarket_core::config::Config;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const HEALTH_FILE: &str = "/tmp/healthy";

fn touch_health_file() {
    let _ = std::fs::write(HEALTH_FILE, format!("{}", chrono::Utc::now().timestamp()));
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bot_scanner=info,polymarket_core=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Bot Scanner");
    touch_health_file();

    // Load configuration
    let config = Config::from_env()?;

    // For now, run in single-wallet analysis mode
    // Future: continuous indexing mode
    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 {
        // Analyze specific wallet
        let wallet_address = &args[1];
        analyze_wallet(&config, wallet_address).await?;
    } else {
        // Run continuous indexer
        run_indexer(&config).await?;
    }

    Ok(())
}

/// Analyze a specific wallet for bot-like behavior.
async fn analyze_wallet(config: &Config, address: &str) -> Result<()> {
    use polymarket_core::api::PolygonClient;
    use polymarket_core::db;
    use polymarket_core::types::BotScore;

    info!("Analyzing wallet: {}", address);

    // Initialize services
    let pool = db::create_pool(&config.database).await?;
    let polygon = match config.polygon.get_rpc_url() {
        Some(url) => PolygonClient::new(url),
        None => {
            anyhow::bail!("No Polygon RPC URL configured. Set ALCHEMY_API_KEY or POLYGON_RPC_URL.");
        }
    };

    // Fetch wallet transactions
    let transfers = polygon.get_asset_transfers(address, None, None).await?;
    info!("Found {} transfers for wallet", transfers.len());

    // Extract features
    let features = feature_extractor::extract_features(address, &transfers)?;
    info!(
        "Extracted features: {} trades, interval_cv={:?}, win_rate={:?}",
        features.total_trades, features.interval_cv, features.win_rate
    );

    // Calculate bot score
    let score = BotScore::new(address.to_string(), &features);

    info!("Bot Score Analysis:");
    info!("  Total Score: {}", score.total_score);
    info!("  Classification: {:?}", score.classification);
    info!("  Signals:");
    for signal in &score.signals {
        info!("    - {:?}", signal);
    }

    // Save to database
    let wallet_repo = polymarket_core::db::wallets::WalletRepository::new(pool.clone());
    wallet_repo.upsert_features(&features).await?;
    wallet_repo.insert_score(&score).await?;

    Ok(())
}

/// Run the continuous trade indexer.
async fn run_indexer(config: &Config) -> Result<()> {
    info!("Starting continuous indexer mode...");

    let mut indexer = indexer::TradeIndexer::new(config).await?;
    indexer.run().await?;

    Ok(())
}
