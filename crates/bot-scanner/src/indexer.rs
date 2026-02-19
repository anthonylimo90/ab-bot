//! Trade indexer for continuous wallet monitoring.

use anyhow::Result;
use polymarket_core::api::PolygonClient;
use polymarket_core::config::Config;
use polymarket_core::db;
use polymarket_core::db::wallets::WalletRepository;
use polymarket_core::types::BotScore;
use sqlx::PgPool;
use std::collections::HashSet;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Polymarket contract addresses for trade event monitoring.
pub mod contracts {
    /// CTF Exchange contract on Polygon.
    pub const CTF_EXCHANGE: &str = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";
}

/// Continuous trade indexer that monitors Polymarket for new trades.
pub struct TradeIndexer {
    polygon: PolygonClient,
    _pool: PgPool,
    wallet_repo: WalletRepository,
    /// Last processed block.
    last_block: u64,
    /// Wallets we've already analyzed.
    analyzed_wallets: HashSet<String>,
    /// Polling interval.
    poll_interval: Duration,
}

impl TradeIndexer {
    /// Create a new trade indexer.
    pub async fn new(config: &Config) -> Result<Self> {
        let pool = db::create_pool(&config.database).await?;
        let wallet_repo = WalletRepository::new(pool.clone());

        let polygon = match config.polygon.get_rpc_url() {
            Some(url) => PolygonClient::new(url),
            None => {
                anyhow::bail!("No Polygon RPC URL configured");
            }
        };

        // Get current block as starting point
        let current_block = polygon.get_block_number().await?;

        Ok(Self {
            polygon,
            _pool: pool,
            wallet_repo,
            last_block: current_block,
            analyzed_wallets: HashSet::new(),
            poll_interval: Duration::from_secs(12), // ~Polygon block time
        })
    }

    /// Run the indexing loop.
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting indexer from block {}", self.last_block);

        loop {
            if let Err(e) = self.poll_new_blocks().await {
                error!("Error polling blocks: {}", e);
            }

            crate::touch_health_file();
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// Poll for new blocks and process trades.
    async fn poll_new_blocks(&mut self) -> Result<()> {
        let current_block = self.polygon.get_block_number().await?;

        if current_block <= self.last_block {
            return Ok(());
        }

        debug!(
            "Processing blocks {} to {}",
            self.last_block + 1,
            current_block
        );

        // Fetch logs from Polymarket exchange contract
        let logs = self
            .polygon
            .get_logs(
                contracts::CTF_EXCHANGE,
                self.last_block + 1,
                current_block,
                None, // All events for now
            )
            .await?;

        if !logs.is_empty() {
            info!(
                "Found {} trade events in blocks {}-{}",
                logs.len(),
                self.last_block + 1,
                current_block
            );
        }

        // Extract unique wallet addresses from logs
        let wallets: HashSet<String> = logs
            .iter()
            .flat_map(|log| {
                // Topics typically contain indexed event parameters
                // Topic 0 = event signature, Topics 1+ = indexed params (often addresses)
                log.topics.iter().skip(1).filter_map(|topic| {
                    // Convert topic to address (last 40 hex chars)
                    if topic.len() >= 42 {
                        Some(format!("0x{}", &topic[topic.len() - 40..]))
                    } else {
                        None
                    }
                })
            })
            .collect();

        // Analyze new wallets
        for wallet in wallets {
            if !self.analyzed_wallets.contains(&wallet) {
                if let Err(e) = self.analyze_wallet(&wallet).await {
                    warn!("Failed to analyze wallet {}: {}", wallet, e);
                }
                self.analyzed_wallets.insert(wallet);
            }
        }

        self.last_block = current_block;
        Ok(())
    }

    /// Analyze a single wallet.
    async fn analyze_wallet(&self, address: &str) -> Result<()> {
        debug!("Analyzing wallet: {}", address);

        // Fetch wallet transactions
        let transfers = self
            .polygon
            .get_asset_transfers(address, None, None)
            .await?;

        if transfers.len() < 10 {
            // Skip wallets with too few trades
            return Ok(());
        }

        // Extract features
        let features = crate::feature_extractor::extract_features(address, &transfers)?;

        // Calculate bot score
        let score = BotScore::new(address.to_string(), &features);

        // Log interesting findings
        if score.total_score >= 25 {
            info!(
                "Interesting wallet {}: score={}, classification={:?}",
                address, score.total_score, score.classification
            );
        }

        // Save to database
        self.wallet_repo.upsert_features(&features).await?;
        self.wallet_repo.insert_score(&score).await?;

        Ok(())
    }
}
