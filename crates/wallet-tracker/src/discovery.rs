//! Wallet discovery for finding profitable traders on Polymarket.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use polymarket_core::api::PolygonClient;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use tracing::{info, warn};

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
    /// Maximum days since last trade before wallet is considered stale.
    pub max_staleness_days: u32,
}

impl Default for DiscoveryCriteria {
    fn default() -> Self {
        Self {
            min_trades: 30,                    // 30+ trades for statistical significance (was 10)
            min_win_rate: 0.52, // 52%+ — lower bar to find more candidates (was 0.60)
            min_volume: Decimal::new(5000, 0), // $5,000+ volume (was $500)
            time_window_days: 90, // 90-day window for longer track record (was 30)
            exclude_bots: true, // Exclude bots by default (was false)
            min_roi: Some(0.10), // 10%+ ROI (was 2%)
            limit: 50,          // Focus on top 50 (was 100)
            max_staleness_days: 14, // Reject wallets inactive > 14 days (was 60)
        }
    }
}

impl DiscoveryCriteria {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create discovery criteria adapted to the current market regime.
    ///
    /// During bear/volatile markets, thresholds are relaxed to prevent empty candidate
    /// pools while still maintaining quality standards. During bull/calm markets,
    /// defaults (or slightly stricter thresholds) are used.
    pub fn from_market_regime(regime: crate::market_regime::MarketRegime) -> Self {
        use crate::market_regime::MarketRegime;
        let base = Self::default();
        match regime {
            MarketRegime::BullCalm => base, // Keep strict defaults — best conditions
            MarketRegime::BullVolatile => Self {
                min_win_rate: 0.58,  // Slight relaxation; more variance expected
                min_roi: Some(0.08), // 8% ROI in volatile uptrend
                ..base
            },
            MarketRegime::BearCalm => Self {
                min_win_rate: 0.55,
                min_roi: Some(0.05),
                min_volume: Decimal::new(3000, 0), // Lower volume threshold
                ..base
            },
            MarketRegime::BearVolatile => Self {
                min_win_rate: 0.52, // Most relaxed — hard conditions
                min_roi: Some(0.03),
                min_volume: Decimal::new(2000, 0),
                min_trades: 20, // Fewer trades available in bear
                ..base
            },
            MarketRegime::Ranging | MarketRegime::Uncertain => Self {
                min_win_rate: 0.57,
                min_roi: Some(0.07),
                ..base
            },
        }
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

    pub fn max_staleness_days(mut self, days: u32) -> Self {
        self.max_staleness_days = days;
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
    /// Composite score from success prediction (0.0 - 1.0), used for multi-factor ranking.
    /// Falls back to ROI-based ranking when unavailable.
    pub composite_score: Option<f64>,
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

        // Filter by criteria, tracking rejection reasons for observability
        let total_candidates = wallets.len();
        let mut discovered: Vec<DiscoveredWallet> = Vec::new();
        let mut rejection_reasons: HashMap<&str, usize> = HashMap::new();

        for wallet in wallets {
            if wallet.total_trades < criteria.min_trades {
                *rejection_reasons.entry("low_trades").or_insert(0) += 1;
            } else if wallet.win_rate < criteria.min_win_rate {
                *rejection_reasons.entry("low_win_rate").or_insert(0) += 1;
            } else if wallet.total_volume < criteria.min_volume {
                *rejection_reasons.entry("low_volume").or_insert(0) += 1;
            } else if criteria.exclude_bots && wallet.bot_score.is_some_and(|s| s >= 50) {
                *rejection_reasons.entry("bot_detected").or_insert(0) += 1;
            } else if criteria.min_roi.is_some_and(|min| wallet.roi < min) {
                *rejection_reasons.entry("low_roi").or_insert(0) += 1;
            } else if criteria.max_staleness_days > 0 {
                let cutoff = Utc::now() - Duration::days(criteria.max_staleness_days as i64);
                if wallet.last_trade < cutoff {
                    *rejection_reasons.entry("stale").or_insert(0) += 1;
                } else {
                    discovered.push(wallet);
                }
            } else {
                discovered.push(wallet);
            }
        }

        if discovered.is_empty() && total_candidates > 0 {
            warn!(
                total_candidates,
                rejections = ?rejection_reasons,
                "Discovery found 0 wallets passing criteria"
            );
        }

        // Rank by composite score (0 when not computed), then ROI, then address.
        // This is the same key the query orders by; the composite score (a
        // probability) is never mixed with ROI (an unbounded ratio) as one number.
        discovered.sort_by(|a, b| {
            let score_a = a.composite_score.unwrap_or(0.0);
            let score_b = b.composite_score.unwrap_or(0.0);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.roi
                        .partial_cmp(&a.roi)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.address.cmp(&b.address))
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
                wallets.sort_by_key(|wallet| std::cmp::Reverse(wallet.total_volume));
            }
            RankingMetric::TotalPnl => {
                wallets.sort_by_key(|wallet| std::cmp::Reverse(wallet.total_pnl));
            }
            RankingMetric::TradeCount => {
                wallets.sort_by_key(|wallet| std::cmp::Reverse(wallet.total_trades));
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
        // Every criterion is applied in SQL so that LIMIT keeps the best wallets
        // that actually qualify (filtering after the LIMIT could return fewer, or
        // none, even though qualifying wallets exist further down the ranking).
        // The latest bot score per wallet is picked with a LATERAL lookup so a
        // wallet with several historical scores is one row, judged on its newest
        // score. Rank key: composite score (predicted success probability, 0 when
        // not computed); ROI and address only break ties deterministically.
        let stale_cutoff = (criteria.max_staleness_days > 0)
            .then(|| Utc::now() - Duration::days(criteria.max_staleness_days as i64));
        let rows = sqlx::query(
            r#"
            SELECT
                wf.address,
                wf.total_trades,
                COALESCE(wsm.win_rate_30d, 0)::FLOAT8 as win_rate,
                wf.total_volume,
                wf.first_trade,
                wf.last_trade,
                bs.total_score::INT4 as bot_score,
                COALESCE(bs.classification, 0)::INT2 as classification,
                COALESCE(wsm.roi_30d, 0)::FLOAT8 as roi,
                COALESCE(wsm.roi_30d * wf.total_volume, 0) as total_pnl,
                COALESCE(wsm.predicted_success_prob, 0)::FLOAT8 as composite_score
            FROM wallet_features wf
            LEFT JOIN LATERAL (
                SELECT b.total_score, b.classification
                FROM bot_scores b
                WHERE b.address = wf.address
                ORDER BY b.computed_at DESC
                LIMIT 1
            ) bs ON TRUE
            LEFT JOIN wallet_success_metrics wsm ON wsm.address = wf.address
            WHERE wf.last_trade >= $1
              AND wf.total_trades >= $2
              AND wf.total_volume >= $3
              AND ($5::timestamptz IS NULL OR wf.last_trade >= $5)
              AND COALESCE(wsm.win_rate_30d, 0)::FLOAT8 >= $6
              AND ($7::FLOAT8 IS NULL OR COALESCE(wsm.roi_30d, 0)::FLOAT8 >= $7)
              AND (NOT $8 OR bs.total_score IS NULL OR bs.total_score < 50)
            ORDER BY COALESCE(wsm.predicted_success_prob, 0) DESC,
                     COALESCE(wsm.roi_30d, 0) DESC,
                     wf.address ASC
            LIMIT $4
            "#,
        )
        .bind(cutoff_date)
        .bind(criteria.min_trades as i64)
        .bind(criteria.min_volume)
        .bind(i64::try_from(criteria.limit).unwrap_or(i64::MAX))
        .bind(stale_cutoff)
        .bind(criteria.min_win_rate)
        .bind(criteria.min_roi)
        .bind(criteria.exclude_bots)
        .fetch_all(&self.pool)
        .await?;

        let wallets: Vec<DiscoveredWallet> = rows
            .iter()
            .map(|row| {
                use sqlx::Row;
                let total_trades: i64 = row.get("total_trades");
                let win_rate: f64 = row.get("win_rate");
                let win_count = (total_trades as f64 * win_rate) as u64;
                let raw_composite: f64 = row.get("composite_score");
                let composite_score = if raw_composite > 0.0 {
                    Some(raw_composite)
                } else {
                    None
                };

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
                    bot_score: row
                        .get::<Option<i32>, _>("bot_score")
                        .map(|score| score as u32),
                    discovered_at: Utc::now(),
                    composite_score,
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
                bs.total_score::INT4 as bot_score,
                COALESCE(bs.classification, 0)::INT2 as classification,
                COALESCE(wsm.roi_30d, 0)::FLOAT8 as roi,
                COALESCE(wsm.roi_30d * wf.total_volume, 0) as total_pnl,
                COALESCE(wsm.predicted_success_prob, 0)::FLOAT8 as composite_score
            FROM wallet_features wf
            LEFT JOIN LATERAL (
                SELECT b.total_score, b.classification
                FROM bot_scores b
                WHERE b.address = wf.address
                ORDER BY b.computed_at DESC
                LIMIT 1
            ) bs ON TRUE
            LEFT JOIN wallet_success_metrics wsm ON wsm.address = wf.address
            WHERE LOWER(wf.address) = $1
            ORDER BY wf.address
            LIMIT 1
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
            let raw_composite: f64 = r.get("composite_score");
            let composite_score = if raw_composite > 0.0 {
                Some(raw_composite)
            } else {
                None
            };

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
                bot_score: r
                    .get::<Option<i32>, _>("bot_score")
                    .map(|score| score as u32),
                discovered_at: Utc::now(),
                composite_score,
            }
        }))
    }

    /// Look up a single wallet's discovery data by address.
    pub async fn query_single_wallet_public(
        &self,
        address: &str,
    ) -> Result<Option<DiscoveredWallet>> {
        self.query_single_wallet(address).await
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
            // `get_asset_transfers` returns both directions. This heuristic was
            // written for outgoing transfers only (the only ones previously
            // returned); keep it that way rather than counting every incoming
            // transfer as a "win".
            if transfer.from.to_lowercase() != address.to_lowercase() {
                continue;
            }

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
            composite_score: None, // Not available from on-chain refresh; computed by MetricsCalculator
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
        .bind(wallet.total_volume)
        .bind(wallet.first_trade)
        .bind(wallet.last_trade)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[cfg(test)]
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

        // Filter out likely bots (score >= 50, the standard threshold)
        if criteria.exclude_bots {
            if let Some(bot_score) = wallet.bot_score {
                if bot_score >= 50 {
                    return false;
                }
            }
        }

        if let Some(min_roi) = criteria.min_roi {
            if wallet.roi < min_roi {
                return false;
            }
        }

        // Reject stale wallets that haven't traded within max_staleness_days
        if criteria.max_staleness_days > 0 {
            let staleness_cutoff = Utc::now() - Duration::days(criteria.max_staleness_days as i64);
            if wallet.last_trade < staleness_cutoff {
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
            composite_score: Some(0.72),
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
            win_count: 35,
            loss_count: 15,
            win_rate: 0.70,
            total_volume: Decimal::new(10000, 0),
            total_pnl: Decimal::new(1500, 0),
            roi: 0.15,
            first_trade: Utc::now() - Duration::days(90),
            last_trade: Utc::now(),
            is_bot: false,
            bot_score: Some(10),
            discovered_at: Utc::now(),
            composite_score: Some(0.80),
        };

        let criteria = DiscoveryCriteria::default();

        // Verify the thresholds
        assert_eq!(criteria.min_trades, 30);
        assert_eq!(criteria.min_win_rate, 0.52);
        assert_eq!(criteria.min_volume, Decimal::new(5000, 0));
        assert_eq!(criteria.time_window_days, 90);
        assert!(criteria.exclude_bots);
        assert_eq!(criteria.min_roi, Some(0.10));
        assert_eq!(criteria.max_staleness_days, 14);

        // Wallet should pass the tightened criteria
        assert!(wallet.total_trades >= criteria.min_trades);
        assert!(wallet.win_rate >= criteria.min_win_rate);
        assert!(wallet.total_volume >= criteria.min_volume);
    }

    #[test]
    fn test_composite_score_sort_order() {
        // Wallet A: high ROI but low composite (one-hit wonder)
        let wallet_a = DiscoveredWallet {
            address: "0xaaaa".to_string(),
            total_trades: 50,
            win_count: 35,
            loss_count: 15,
            win_rate: 0.70,
            total_volume: Decimal::new(10000, 0),
            total_pnl: Decimal::new(2000, 0),
            roi: 0.20,
            first_trade: Utc::now() - Duration::days(30),
            last_trade: Utc::now(),
            is_bot: false,
            bot_score: Some(5),
            discovered_at: Utc::now(),
            composite_score: Some(0.45), // Low composite despite high ROI
        };

        // Wallet B: lower ROI but high composite (consistent performer)
        let wallet_b = DiscoveredWallet {
            address: "0xbbbb".to_string(),
            total_trades: 100,
            win_count: 65,
            loss_count: 35,
            win_rate: 0.65,
            total_volume: Decimal::new(15000, 0),
            total_pnl: Decimal::new(1500, 0),
            roi: 0.10,
            first_trade: Utc::now() - Duration::days(60),
            last_trade: Utc::now(),
            is_bot: false,
            bot_score: Some(8),
            discovered_at: Utc::now(),
            composite_score: Some(0.78), // High composite despite lower ROI
        };

        let mut wallets = [wallet_a.clone(), wallet_b.clone()];
        wallets.sort_by(|a, b| {
            let score_a = a.composite_score.unwrap_or(a.roi);
            let score_b = b.composite_score.unwrap_or(b.roi);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Wallet B should rank first due to higher composite despite lower ROI
        assert_eq!(wallets[0].address, "0xbbbb");
        assert_eq!(wallets[1].address, "0xaaaa");
    }

    #[tokio::test]
    async fn test_staleness_filter() {
        let discovery = WalletDiscovery::from_pool(
            // We only need the meets_criteria method which doesn't use the pool
            sqlx::PgPool::connect_lazy("postgres://fake").unwrap(),
        );

        // Stale wallet: last traded 70 days ago
        let stale_wallet = DiscoveredWallet {
            address: "0xstale".to_string(),
            total_trades: 50,
            win_count: 35,
            loss_count: 15,
            win_rate: 0.70,
            total_volume: Decimal::new(10000, 0),
            total_pnl: Decimal::new(1500, 0),
            roi: 0.15,
            first_trade: Utc::now() - Duration::days(120),
            last_trade: Utc::now() - Duration::days(70),
            is_bot: false,
            bot_score: Some(5),
            discovered_at: Utc::now(),
            composite_score: Some(0.80),
        };

        let criteria = DiscoveryCriteria::default(); // max_staleness_days = 60
        assert!(
            !discovery.meets_criteria(&stale_wallet, &criteria),
            "Wallet inactive for 70 days should be rejected with 60-day staleness limit"
        );

        // Recent wallet should pass
        let recent_wallet = DiscoveredWallet {
            last_trade: Utc::now() - Duration::days(10),
            ..stale_wallet
        };
        assert!(
            discovery.meets_criteria(&recent_wallet, &criteria),
            "Wallet active 10 days ago should pass staleness check"
        );
    }
}

/// Discovery queries: filters before LIMIT, one row per wallet, a single rank key.
#[cfg(test)]
mod sql_tests {
    use super::*;

    async fn create_tables(pool: &PgPool) {
        sqlx::raw_sql(
            r#"
            CREATE TABLE wallet_features (
                address TEXT PRIMARY KEY,
                total_trades BIGINT NOT NULL,
                total_volume NUMERIC NOT NULL,
                first_trade TIMESTAMPTZ NOT NULL,
                last_trade TIMESTAMPTZ NOT NULL
            );
            CREATE TABLE bot_scores (
                id SERIAL PRIMARY KEY,
                address TEXT NOT NULL,
                total_score INTEGER NOT NULL,
                classification SMALLINT NOT NULL,
                computed_at TIMESTAMPTZ NOT NULL
            );
            CREATE TABLE wallet_success_metrics (
                address TEXT PRIMARY KEY,
                win_rate_30d NUMERIC(5, 4) NOT NULL DEFAULT 0,
                roi_30d NUMERIC(20, 8) NOT NULL DEFAULT 0,
                predicted_success_prob NUMERIC(5, 4) NOT NULL DEFAULT 0
            );
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    struct Wallet<'a> {
        address: &'a str,
        trades: i64,
        last_trade_days_ago: i64,
        /// (win_rate, roi, predicted_success_prob); `None` = no metrics row.
        metrics: Option<(&'a str, &'a str, &'a str)>,
    }

    impl<'a> Wallet<'a> {
        fn new(address: &'a str) -> Self {
            Self {
                address,
                trades: 100,
                last_trade_days_ago: 1,
                metrics: Some(("0.60", "0.10", "0.50")),
            }
        }
    }

    async fn insert(pool: &PgPool, w: &Wallet<'_>) {
        sqlx::query(
            "INSERT INTO wallet_features VALUES ($1, $2, 10000, NOW() - INTERVAL '60 days', \
             NOW() - make_interval(days => $3::int))",
        )
        .bind(w.address)
        .bind(w.trades)
        .bind(w.last_trade_days_ago as i32)
        .execute(pool)
        .await
        .unwrap();
        if let Some((win_rate, roi, prob)) = w.metrics {
            sqlx::query(
                "INSERT INTO wallet_success_metrics VALUES ($1, $2::numeric, $3::numeric, $4::numeric)",
            )
            .bind(w.address)
            .bind(win_rate)
            .bind(roi)
            .bind(prob)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    async fn score(pool: &PgPool, address: &str, total: i32, days_ago: i64) {
        sqlx::query(
            "INSERT INTO bot_scores (address, total_score, classification, computed_at) \
             VALUES ($1, $2, CASE WHEN $2 >= 50 THEN 2 ELSE 0 END, \
                     NOW() - make_interval(days => $3::int))",
        )
        .bind(address)
        .bind(total)
        .bind(days_ago as i32)
        .execute(pool)
        .await
        .unwrap();
    }

    fn criteria() -> DiscoveryCriteria {
        DiscoveryCriteria::new()
            .min_trades(1)
            .min_win_rate(0.0)
            .min_volume(Decimal::ZERO)
            .no_min_roi()
            .time_window(90)
            .limit(50)
    }

    fn addresses(wallets: &[DiscoveredWallet]) -> Vec<&str> {
        wallets.iter().map(|w| w.address.as_str()).collect()
    }

    #[sqlx::test(migrations = false)]
    async fn wallets_with_many_scores_are_one_row_judged_by_the_latest(pool: PgPool) {
        create_tables(&pool).await;
        insert(&pool, &Wallet::new("0xa")).await;
        insert(&pool, &Wallet::new("0xb")).await;
        // 0xa: was flagged, but its newest score is human-like.
        score(&pool, "0xa", 90, 10).await;
        score(&pool, "0xa", 10, 1).await;
        // 0xb: was human-like, but its newest score flags it as a bot.
        score(&pool, "0xb", 10, 10).await;
        score(&pool, "0xb", 90, 1).await;

        let discovery = WalletDiscovery::from_pool(pool);
        let found = discovery
            .discover_profitable_wallets(&criteria())
            .await
            .unwrap();

        assert_eq!(addresses(&found), vec!["0xa"], "one row, newest score wins");
        assert_eq!(found[0].bot_score, Some(10));

        // Bots are only included when asked for - still one row each.
        let with_bots = discovery
            .discover_profitable_wallets(&criteria().include_bots())
            .await
            .unwrap();
        assert_eq!(with_bots.len(), 2);
        let b = with_bots.iter().find(|w| w.address == "0xb").unwrap();
        assert_eq!(b.bot_score, Some(90));
        assert!(b.is_bot);
    }

    #[sqlx::test(migrations = false)]
    async fn unscored_wallets_have_no_bot_score_instead_of_zero(pool: PgPool) {
        create_tables(&pool).await;
        insert(&pool, &Wallet::new("0xnew")).await;

        let discovery = WalletDiscovery::from_pool(pool);
        let found = discovery
            .discover_profitable_wallets(&criteria())
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].bot_score, None);
        assert!(!found[0].is_bot);
        let single = discovery.query_single_wallet_public("0xnew").await.unwrap();
        assert_eq!(single.unwrap().bot_score, None);
    }

    #[sqlx::test(migrations = false)]
    async fn criteria_are_applied_before_the_limit(pool: PgPool) {
        create_tables(&pool).await;
        // Six wallets rank highest by composite score but fail the win-rate
        // criterion; the two that qualify rank lower.
        for i in 0..6 {
            let address = format!("0xhigh{i}");
            let mut wallet = Wallet::new(&address);
            wallet.metrics = Some(("0.30", "0.50", "0.95"));
            insert(&pool, &wallet).await;
        }
        for (address, prob) in [("0xgood1", "0.40"), ("0xgood2", "0.30")] {
            let mut wallet = Wallet::new(address);
            wallet.metrics = Some(("0.70", "0.20", prob));
            insert(&pool, &wallet).await;
        }

        let discovery = WalletDiscovery::from_pool(pool);
        let found = discovery
            .discover_profitable_wallets(&criteria().min_win_rate(0.60).limit(2))
            .await
            .unwrap();
        // The old query took LIMIT 4 (limit * 2) of the top composite scores - all
        // failing wallets - and then filtered them down to nothing.
        assert_eq!(addresses(&found), vec!["0xgood1", "0xgood2"]);
    }

    #[sqlx::test(migrations = false)]
    async fn roi_staleness_and_bot_criteria_also_filter_in_sql(pool: PgPool) {
        create_tables(&pool).await;
        // High composite, but: low ROI / stale / bot.
        let mut low_roi = Wallet::new("0xlowroi");
        low_roi.metrics = Some(("0.70", "0.01", "0.99"));
        insert(&pool, &low_roi).await;
        let mut stale = Wallet::new("0xstale");
        stale.last_trade_days_ago = 30;
        stale.metrics = Some(("0.70", "0.20", "0.98"));
        insert(&pool, &stale).await;
        let mut bot = Wallet::new("0xbot");
        bot.metrics = Some(("0.70", "0.20", "0.97"));
        insert(&pool, &bot).await;
        score(&pool, "0xbot", 80, 1).await;
        // The only qualifying wallet, with the lowest composite score.
        let mut ok = Wallet::new("0xok");
        ok.metrics = Some(("0.70", "0.20", "0.10"));
        insert(&pool, &ok).await;

        let discovery = WalletDiscovery::from_pool(pool);
        let found = discovery
            .discover_profitable_wallets(&criteria().min_roi(0.05).max_staleness_days(14).limit(1))
            .await
            .unwrap();
        assert_eq!(addresses(&found), vec!["0xok"]);
    }

    #[sqlx::test(migrations = false)]
    async fn ranking_is_by_composite_score_then_roi_never_a_mix(pool: PgPool) {
        create_tables(&pool).await;
        let mut x = Wallet::new("0xx");
        x.metrics = Some(("0.60", "0.05", "0.60"));
        insert(&pool, &x).await;
        // No composite score yet, but an enormous ROI. The old ORDER BY used
        // COALESCE(probability, roi) and ranked this wallet first.
        let mut y = Wallet::new("0xy");
        y.metrics = Some(("0.60", "5.00", "0"));
        insert(&pool, &y).await;
        let mut z = Wallet::new("0xz");
        z.metrics = Some(("0.60", "0.10", "0.80"));
        insert(&pool, &z).await;
        // Same composite as 0xx: ROI breaks the tie.
        let mut w = Wallet::new("0xw");
        w.metrics = Some(("0.60", "0.07", "0.60"));
        insert(&pool, &w).await;

        let discovery = WalletDiscovery::from_pool(pool);
        let found = discovery
            .discover_profitable_wallets(&criteria())
            .await
            .unwrap();
        assert_eq!(addresses(&found), vec!["0xz", "0xw", "0xx", "0xy"]);

        // With a limit, the SQL ranking and the in-memory ranking agree.
        let top2 = discovery
            .discover_profitable_wallets(&criteria().limit(2))
            .await
            .unwrap();
        assert_eq!(addresses(&top2), vec!["0xz", "0xw"]);
    }

    #[sqlx::test(migrations = false)]
    async fn single_wallet_lookup_uses_the_latest_score(pool: PgPool) {
        create_tables(&pool).await;
        insert(&pool, &Wallet::new("0xabc")).await;
        score(&pool, "0xabc", 95, 20).await;
        score(&pool, "0xabc", 5, 2).await;
        score(&pool, "0xabc", 40, 1).await;

        let discovery = WalletDiscovery::from_pool(pool);
        let wallet = discovery
            .query_single_wallet_public("0xabc")
            .await
            .unwrap()
            .expect("wallet exists");
        assert_eq!(wallet.bot_score, Some(40));
        assert!(!wallet.is_bot);
        assert!(discovery
            .query_single_wallet_public("0xmissing")
            .await
            .unwrap()
            .is_none());
    }
}
