//! Core arbitrage monitoring logic.

use crate::position_tracker::PositionTracker;
use crate::signals::SignalPublisher;
use anyhow::Result;
use polymarket_core::api::clob::OrderBookUpdate;
use polymarket_core::api::ClobClient;
use polymarket_core::config::Config;
use polymarket_core::db;
use polymarket_core::types::{ArbOpportunity, BinaryMarketBook, OrderBook};
use rust_decimal::Decimal;
use std::collections::HashMap;
use tracing::info;

/// Main arbitrage monitor service.
pub struct ArbMonitor {
    clob_client: ClobClient,
    position_tracker: PositionTracker,
    signal_publisher: SignalPublisher,
    /// Current order books by (market_id, outcome_id).
    order_books: HashMap<(String, String), OrderBook>,
    /// Market outcome pairings (market_id -> (yes_outcome_id, no_outcome_id)).
    market_outcomes: HashMap<String, (String, String)>,
    /// Minimum net profit threshold for entry signals.
    min_profit_threshold: Decimal,
}

impl ArbMonitor {
    /// Create a new arbitrage monitor.
    pub async fn new(config: Config) -> Result<Self> {
        // Initialize database pool
        let pool = db::create_pool(&config.database).await?;

        // Initialize Redis connection
        let redis_client = redis::Client::open(config.redis.url.as_str())?;

        // Create CLOB client
        let clob_client = ClobClient::new(config.polymarket.clob_url, config.polymarket.ws_url);

        // Create position tracker
        let position_tracker = PositionTracker::new(pool.clone());

        // Create signal publisher
        let signal_publisher = SignalPublisher::new(redis_client, config.alerts).await?;

        Ok(Self {
            clob_client,
            position_tracker,
            signal_publisher,
            order_books: HashMap::new(),
            market_outcomes: HashMap::new(),
            min_profit_threshold: Decimal::new(1, 3), // 0.001 = 0.1% minimum profit
        })
    }

    /// Run the monitoring loop.
    pub async fn run(&mut self) -> Result<()> {
        info!("Fetching active markets...");

        // Fetch markets and identify binary markets
        let markets = self.clob_client.get_markets().await?;
        let binary_markets: Vec<_> = markets
            .iter()
            .filter(|m| m.outcomes.len() == 2 && !m.resolved)
            .collect();

        info!("Found {} binary markets to monitor", binary_markets.len());

        // Build market outcome mappings
        for market in &binary_markets {
            if market.outcomes.len() == 2 {
                let (yes_id, no_id) = if market.outcomes[0].name.to_lowercase().contains("yes") {
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
                self.market_outcomes
                    .insert(market.id.clone(), (yes_id, no_id));
            }
        }

        // Subscribe to order book updates
        let market_ids: Vec<String> = binary_markets.iter().map(|m| m.id.clone()).collect();
        let mut updates = self.clob_client.subscribe_orderbook(market_ids).await?;

        info!("Subscribed to order book updates, monitoring for arbitrage...");

        // Process updates
        let mut health_tick = 0u64;
        while let Some(update) = updates.recv().await {
            self.process_update(update).await?;
            health_tick += 1;
            if health_tick % 100 == 0 {
                crate::touch_health_file();
            }
        }

        Ok(())
    }

    /// Process an order book update.
    async fn process_update(&mut self, update: OrderBookUpdate) -> Result<()> {
        // Store the updated order book
        let book = OrderBook {
            market_id: update.market_id.clone(),
            outcome_id: update.asset_id.clone(),
            timestamp: update.timestamp,
            bids: update.bids,
            asks: update.asks,
        };
        self.order_books
            .insert((update.market_id.clone(), update.asset_id.clone()), book);

        // Check if we have both sides for this market
        if let Some((yes_id, no_id)) = self.market_outcomes.get(&update.market_id) {
            let yes_key = (update.market_id.clone(), yes_id.clone());
            let no_key = (update.market_id.clone(), no_id.clone());

            if let (Some(yes_book), Some(no_book)) = (
                self.order_books.get(&yes_key),
                self.order_books.get(&no_key),
            ) {
                let binary_book = BinaryMarketBook {
                    market_id: update.market_id.clone(),
                    timestamp: update.timestamp,
                    yes_book: yes_book.clone(),
                    no_book: no_book.clone(),
                };

                // Calculate arbitrage opportunity
                if let Some(arb) =
                    ArbOpportunity::calculate(&binary_book, ArbOpportunity::DEFAULT_FEE)
                {
                    if arb.is_profitable() && arb.net_profit >= self.min_profit_threshold {
                        self.handle_arb_opportunity(&arb, &binary_book).await?;
                    }
                }

                // Update P&L for open positions in this market
                self.position_tracker
                    .update_market_positions(&update.market_id, &binary_book)
                    .await?;
            }
        }

        Ok(())
    }

    /// Handle a detected arbitrage opportunity.
    async fn handle_arb_opportunity(
        &mut self,
        arb: &ArbOpportunity,
        book: &BinaryMarketBook,
    ) -> Result<()> {
        info!(
            "ARB DETECTED: market={} cost={:.4} profit={:.4}",
            arb.market_id, arb.total_cost, arb.net_profit
        );

        // Publish entry signal
        self.signal_publisher.publish_entry_signal(arb).await?;

        // Check for exit opportunities on open positions
        self.position_tracker
            .check_exit_opportunities(&arb.market_id, book)
            .await?;

        Ok(())
    }
}
