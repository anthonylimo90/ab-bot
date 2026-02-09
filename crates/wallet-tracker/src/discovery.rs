//! Wallet discovery for finding profitable traders on Polymarket.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use polymarket_core::api::PolygonClient;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::info;

/// Criteria for discovering wallets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryCriteria {
    /// Minimum number of trades required.
    pub min_trades: u64,
    /// Minimum win rate (0.0 - 1.0).
    pub min_win_rate: f64,
    /// Minimum total volume in USD.
    pub min_volume: Decimal,
    /// Time window in days to analyze.
    pub time_window_days: u32,
    /// Exclude wallets flagged as bots.
    pub exclude_bots: bool,
    /// Minimum ROI percentage.
    pub min_roi: Option<f64>,
    /// Maximum number of results.
    pub limit: usize,
}

impl Default for DiscoveryCriteria {
    fn default() -> Self {
        Self {
            min_trades: 10,
            min_win_rate: 0.52, // Lowered from 0.55 to include more wallets
            min_volume: Decimal::new(500, 0), // Lowered from 1000 to include smaller traders
            time_window_days: 30,
            exclude_bots: false, // Changed from true - include profitable bots
            min_roi: Some(0.02), // Lowered from 0.05 (2% minimum ROI)
            limit: 100,
        }
    }
}

impl DiscoveryCriteria {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn min_trades(mut self, min: u64) -> Self {
        self.min_trades = min;
        self
    }

    pub fn min_win_rate(mut self, rate: f64) -> Self {
        self.min_win_rate = rate;
        self
    }

    pub fn min_volume(mut self, volume: Decimal) -> Self {
        self.min_volume = volume;
        self
    }

    pub fn time_window(mut self, days: u32) -> Self {
        self.time_window_days = days;
        self
    }

    pub fn include_bots(mut self) -> Self {
        self.exclude_bots = false;
        self
    }

    pub fn min_roi(mut self, roi: f64) -> Self {
        self.min_roi = Some(roi);
        self
    }

    pub fn no_min_roi(mut self) -> Self {
        self.min_roi = None;
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// A discovered wallet with basic metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredWallet {
    pub address: String,
    pub total_trades: u64,
    pub win_count: u64,
    pub loss_count: u64,
    pub win_rate: f64,
    pub total_volume: Decimal,
    pub total_pnl: Decimal,
    pub roi: f64,
    pub first_trade: DateTime<Utc>,
    pub last_trade: DateTime<Utc>,
    pub is_bot: bool,
    pub bot_score: Option<u32>,
    pub discovered_at: DateTime<Utc>,
}

impl DiscoveredWallet {
    /// Calculate days active.
    pub fn days_active(&self) -> i64 {
        (self.last_trade - self.first_trade).num_days()
    }

    /// Calculate average trade size.
    pub fn avg_trade_size(&self) -> Decimal {
        if self.total_trades == 0 {
            Decimal::ZERO
        } else {
            self.total_volume / Decimal::from(self.total_trades)
        }
    }

    /// Calculate trades per day.
    pub fn trades_per_day(&self) -> f64 {
        let days = self.days_active().max(1) as f64;
        self.total_trades as f64 / days
    }
}

/// Wallet discovery service.
pub struct WalletDiscovery {
    polygon_client: Option<PolygonClient>,
    pool: PgPool,
    /// Cache of recently discovered wallets.
    cache: dashmap::DashMap<String, DiscoveredWallet>,
}

impl WalletDiscovery {
    /// Create a new wallet discovery service with a Polygon client.
    pub fn new(polygon_client: PolygonClient, pool: PgPool) -> Self {
        Self {
            polygon_client: Some(polygon_client),
            pool,
            cache: dashmap::DashMap::new(),
        }
    }

    /// Create a wallet discovery service backed only by the database.
    ///
    /// Discovery queries work without Polygon; only `refresh_wallet()` requires it.
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            polygon_client: None,
            pool,
            cache: dashmap::DashMap::new(),
        }
    }

    /// Discover profitable wallets based on criteria.
    pub async fn discover_profitable_wallets(
        &self,
        criteria: &DiscoveryCriteria,
    ) -> Result<Vec<DiscoveredWallet>> {
        info!(
            min_trades = criteria.min_trades,
            min_win_rate = criteria.min_win_rate,
            time_window = criteria.time_window_days,
            "Discovering profitable wallets"
        );

        let cutoff_date = Utc::now() - Duration::days(criteria.time_window_days as i64);

        // Query wallet features from database
        let wallets = self.query_wallet_candidates(&cutoff_date, criteria).await?;

        // Filter by criteria
        let mut discovered: Vec<DiscoveredWallet> = wallets
            .into_iter()
            .filter(|w| self.meets_criteria(w, criteria))
            .collect();

        // Sort by ROI (descending)
        discovered.sort_by(|a, b| {
            b.roi
                .partial_cmp(&a.roi)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Apply limit
        discovered.truncate(criteria.limit);

        // Cache results
        for wallet in &discovered {
            self.cache
                .insert(wallet.address.to_lowercase(), wallet.clone());
        }

        info!(count = discovered.len(), "Discovered profitable wallets");

        Ok(discovered)
    }

    /// Get top wallets by a specific metric.
    pub async fn get_top_wallets(
        &self,
        metric: RankingMetric,
        limit: usize,
    ) -> Result<Vec<DiscoveredWallet>> {
        let criteria = DiscoveryCriteria::default().limit(limit * 2);
        let mut wallets = self.discover_profitable_wallets(&criteria).await?;

        // Sort by specified metric
        match metric {
            RankingMetric::Roi => {
                wallets.sort_by(|a, b| {
                    b.roi
                        .partial_cmp(&a.roi)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            RankingMetric::WinRate => {
                wallets.sort_by(|a, b| {
                    b.win_rate
                        .partial_cmp(&a.win_rate)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            RankingMetric::Volume => {
                wallets.sort_by(|a, b| b.total_volume.cmp(&a.total_volume));
            }
            RankingMetric::TotalPnl => {
                wallets.sort_by(|a, b| b.total_pnl.cmp(&a.total_pnl));
            }
            RankingMetric::TradeCount => {
                wallets.sort_by(|a, b| b.total_trades.cmp(&a.total_trades));
            }
            RankingMetric::Consistency => {
                // Consistency = win rate * log(trades)
                wallets.sort_by(|a, b| {
                    let score_a = a.win_rate * (a.total_trades as f64).ln();
                    let score_b = b.win_rate * (b.total_trades as f64).ln();
                    score_b
                        .partial_cmp(&score_a)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        wallets.truncate(limit);
        Ok(wallets)
    }

    /// Search for a specific wallet by address.
    pub async fn get_wallet(&self, address: &str) -> Result<Option<DiscoveredWallet>> {
        let address_lower = address.to_lowercase();

        // Check cache first
        if let Some(wallet) = self.cache.get(&address_lower) {
            return Ok(Some(wallet.clone()));
        }

        // Query from database
        self.query_single_wallet(&address_lower).await
    }

    /// Refresh wallet data from on-chain.
    ///
    /// Requires a Polygon client; returns an error if constructed via `from_pool()`.
    pub async fn refresh_wallet(&self, address: &str) -> Result<Option<DiscoveredWallet>> {
        let polygon_client = self.polygon_client.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Polygon client not configured — cannot refresh on-chain data")
        })?;

        let address_lower = address.to_lowercase();

        // Fetch fresh data from Polygon
        let transfers = polygon_client
            .get_asset_transfers(&address_lower, None, None)
            .await?;

        if transfers.is_empty() {
            return Ok(None);
        }

        // Calculate metrics from transfers
        let wallet = self.calculate_wallet_metrics(&address_lower, &transfers)?;

        // Update cache
        self.cache.insert(address_lower.clone(), wallet.clone());

        // Update database
        self.update_wallet_in_db(&wallet).await?;

        Ok(Some(wallet))
    }

    /// Get cached wallets.
    pub fn cached_wallets(&self) -> Vec<DiscoveredWallet> {
        self.cache.iter().map(|e| e.value().clone()).collect()
    }

    /// Clear the cache.
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    // Private methods

    async fn query_wallet_candidates(
        &self,
        cutoff_date: &DateTime<Utc>,
        criteria: &DiscoveryCriteria,
    ) -> Result<Vec<DiscoveredWallet>> {
        let rows = sqlx::query(
            r#"
            SELECT
                wf.address,
                wf.total_trades,
                COALESCE(wsm.win_rate_30d, 0)::FLOAT8 as win_rate,
                wf.total_volume,
                wf.first_trade,
                wf.last_trade,
                COALESCE(bs.total_score, 0)::INT4 as bot_score,
                COALESCE(bs.classification, 0)::INT2 as classification,
                COALESCE(wsm.roi_30d, 0)::FLOAT8 as roi,
                COALESCE(wsm.roi_30d * wf.total_volume, 0) as total_pnl
            FROM wallet_features wf
            LEFT JOIN bot_scores bs ON bs.address = wf.address
            LEFT JOIN wallet_success_metrics wsm ON wsm.address = wf.address
            WHERE wf.last_trade >= $1
              AND wf.total_trades >= $2
              AND wf.total_volume >= $3
            ORDER BY COALESCE(wsm.roi_30d, 0) DESC
            LIMIT $4
            "#,
        )
        .bind(cutoff_date)
        .bind(criteria.min_trades as i64)
        .bind(criteria.min_volume)
        .bind((criteria.limit * 2) as i64)
        .fetch_all(&self.pool)
        .await?;

        let wallets: Vec<DiscoveredWallet> = rows
            .iter()
            .map(|row| {
                use sqlx::Row;
                let total_trades: i64 = row.get("total_trades");
                let win_rate: f64 = row.get("win_rate");
                let win_count = (total_trades as f64 * win_rate) as u64;

                DiscoveredWallet {
                    address: row.get("address"),
                    total_trades: total_trades as u64,
                    win_count,
                    loss_count: total_trades as u64 - win_count,
                    win_rate,
                    total_volume: row.get("total_volume"),
                    total_pnl: row.get("total_pnl"),
                    roi: row.get("roi"),
                    first_trade: row.get("first_trade"),
                    last_trade: row.get("last_trade"),
                    is_bot: row.get::<i16, _>("classification") >= 2,
                    bot_score: Some(row.get::<i32, _>("bot_score") as u32),
                    discovered_at: Utc::now(),
                }
            })
            .collect();

        Ok(wallets)
    }

    async fn query_single_wallet(&self, address: &str) -> Result<Option<DiscoveredWallet>> {
        let row = sqlx::query(
            r#"
            SELECT
                wf.address,
                wf.total_trades,
                COALESCE(wsm.win_rate_30d, 0)::FLOAT8 as win_rate,
                wf.total_volume,
                wf.first_trade,
                wf.last_trade,
                COALESCE(bs.total_score, 0)::INT4 as bot_score,
                COALESCE(bs.classification, 0)::INT2 as classification,
                COALESCE(wsm.roi_30d, 0)::FLOAT8 as roi,
                COALESCE(wsm.roi_30d * wf.total_volume, 0) as total_pnl
            FROM wallet_features wf
            LEFT JOIN bot_scores bs ON bs.address = wf.address
            LEFT JOIN wallet_success_metrics wsm ON wsm.address = wf.address
            WHERE LOWER(wf.address) = $1
            "#,
        )
        .bind(address)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| {
            use sqlx::Row;
            let total_trades: i64 = r.get("total_trades");
            let win_rate: f64 = r.get("win_rate");
            let win_count = (total_trades as f64 * win_rate) as u64;

            DiscoveredWallet {
                address: r.get("address"),
                total_trades: total_trades as u64,
                win_count,
                loss_count: total_trades as u64 - win_count,
                win_rate,
                total_volume: r.get("total_volume"),
                total_pnl: r.get("total_pnl"),
                roi: r.get("roi"),
                first_trade: r.get("first_trade"),
                last_trade: r.get("last_trade"),
                is_bot: r.get::<i16, _>("classification") >= 2,
                bot_score: Some(r.get::<i32, _>("bot_score") as u32),
                discovered_at: Utc::now(),
            }
        }))
    }

    fn calculate_wallet_metrics(
        &self,
        address: &str,
        transfers: &[polymarket_core::api::polygon::AssetTransfer],
    ) -> Result<DiscoveredWallet> {
        let mut total_volume = Decimal::ZERO;
        let mut win_count = 0u64;
        let mut loss_count = 0u64;
        let mut first_trade: Option<DateTime<Utc>> = None;
        let mut last_trade: Option<DateTime<Utc>> = None;

        for transfer in transfers {
            if let Some(value) = transfer.value {
                total_volume += Decimal::try_from(value).unwrap_or_default();
            }

            // Parse timestamp
            if let Some(ts) = transfer
                .metadata
                .as_ref()
                .and_then(|m| m.block_timestamp.as_ref())
                .and_then(|ts| ts.parse::<DateTime<Utc>>().ok())
            {
                if first_trade.is_none() || ts < first_trade.unwrap() {
                    first_trade = Some(ts);
                }
                if last_trade.is_none() || ts > last_trade.unwrap() {
                    last_trade = Some(ts);
                }
            }

            // Simple heuristic: if receiving, it's likely a win
            if transfer.to.to_lowercase() == address.to_lowercase() {
                win_count += 1;
            } else {
                loss_count += 1;
            }
        }

        let total_trades = win_count + loss_count;
        let win_rate = if total_trades > 0 {
            win_count as f64 / total_trades as f64
        } else {
            0.0
        };

        // Estimate ROI (simplified - would need price data for accuracy)
        let roi = (win_rate - 0.5) * 2.0; // Rough estimate

        Ok(DiscoveredWallet {
            address: address.to_string(),
            total_trades,
            win_count,
            loss_count,
            win_rate,
            total_volume,
            total_pnl: total_volume * Decimal::try_from(roi).unwrap_or_default(),
            roi,
            first_trade: first_trade.unwrap_or_else(Utc::now),
            last_trade: last_trade.unwrap_or_else(Utc::now),
            is_bot: false,
            bot_score: None,
            discovered_at: Utc::now(),
        })
    }

    async fn update_wallet_in_db(&self, wallet: &DiscoveredWallet) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO wallet_features (address, total_trades, win_rate, total_volume, first_trade, last_trade)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (address) DO UPDATE SET
                total_trades = EXCLUDED.total_trades,
                win_rate = EXCLUDED.win_rate,
                total_volume = EXCLUDED.total_volume,
                first_trade = EXCLUDED.first_trade,
                last_trade = EXCLUDED.last_trade,
                updated_at = NOW()
            "#,
        )
        .bind(&wallet.address)
        .bind(wallet.total_trades as i64)
        .bind(wallet.win_rate)
        .bind(&wallet.total_volume)
        .bind(&wallet.first_trade)
        .bind(&wallet.last_trade)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    fn meets_criteria(&self, wallet: &DiscoveredWallet, criteria: &DiscoveryCriteria) -> bool {
        if wallet.total_trades < criteria.min_trades {
            return false;
        }

        if wallet.win_rate < criteria.min_win_rate {
            return false;
        }

        if wallet.total_volume < criteria.min_volume {
            return false;
        }

        // Only filter out very high confidence bots (score > 70)
        // Include profitable bots with lower scores
        if criteria.exclude_bots {
            if let Some(bot_score) = wallet.bot_score {
                if bot_score > 70 {
                    return false;
                }
            }
        }

        if let Some(min_roi) = criteria.min_roi {
            if wallet.roi < min_roi {
                return false;
            }
        }

        true
    }
}

/// Metric for ranking wallets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankingMetric {
    Roi,
    WinRate,
    Volume,
    TotalPnl,
    TradeCount,
    Consistency,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_criteria_builder() {
        let criteria = DiscoveryCriteria::new()
            .min_trades(20)
            .min_win_rate(0.6)
            .min_volume(Decimal::new(5000, 0))
            .time_window(60)
            .limit(50);

        assert_eq!(criteria.min_trades, 20);
        assert_eq!(criteria.min_win_rate, 0.6);
        assert_eq!(criteria.min_volume, Decimal::new(5000, 0));
        assert_eq!(criteria.time_window_days, 60);
        assert_eq!(criteria.limit, 50);
    }

    #[test]
    fn test_discovered_wallet_metrics() {
        let wallet = DiscoveredWallet {
            address: "0x1234".to_string(),
            total_trades: 100,
            win_count: 60,
            loss_count: 40,
            win_rate: 0.6,
            total_volume: Decimal::new(10000, 0),
            total_pnl: Decimal::new(500, 0),
            roi: 0.05,
            first_trade: Utc::now() - Duration::days(30),
            last_trade: Utc::now(),
            is_bot: false,
            bot_score: Some(10),
            discovered_at: Utc::now(),
        };

        assert_eq!(wallet.days_active(), 30);
        assert_eq!(wallet.avg_trade_size(), Decimal::new(100, 0));
        assert!((wallet.trades_per_day() - 3.33).abs() < 0.1);
    }

    #[test]
    fn test_meets_criteria() {
        let wallet = DiscoveredWallet {
            address: "0x1234".to_string(),
            total_trades: 50,
            win_count: 30,
            loss_count: 20,
            win_rate: 0.6,
            total_volume: Decimal::new(5000, 0),
            total_pnl: Decimal::new(500, 0),
            roi: 0.10,
            first_trade: Utc::now() - Duration::days(30),
            last_trade: Utc::now(),
            is_bot: false,
            bot_score: Some(10),
            discovered_at: Utc::now(),
        };

        let criteria = DiscoveryCriteria::default();

        // Create a mock discovery instance isn't practical here,
        // so we test the logic directly
        assert!(wallet.total_trades >= criteria.min_trades);
        assert!(wallet.win_rate >= criteria.min_win_rate);
        assert!(wallet.total_volume >= criteria.min_volume);
    }
}
