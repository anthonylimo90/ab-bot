//! Background job to populate wallet_success_metrics table.
//!
//! This module runs a periodic background task that discovers wallets
//! and calculates their profitability metrics using ProfitabilityAnalyzer.

use anyhow::Result;
use chrono::{Duration, Utc};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::time;
use tracing::{debug, error, info, warn};
use wallet_tracker::profitability::{ProfitabilityAnalyzer, TimePeriod};

/// Configuration for the metrics calculator background job.
#[derive(Debug, Clone)]
pub struct MetricsCalculatorConfig {
    /// Whether the background job is enabled.
    pub enabled: bool,
    /// Interval between calculation cycles in seconds.
    pub interval_secs: u64,
    /// Number of wallets to process per batch.
    pub batch_size: usize,
    /// Recalculate metrics if older than this many hours.
    pub recalc_after_hours: i64,
}

impl Default for MetricsCalculatorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 3600, // 1 hour
            batch_size: 50,
            recalc_after_hours: 24,
        }
    }
}

impl MetricsCalculatorConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("METRICS_CALCULATOR_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            interval_secs: std::env::var("METRICS_CALCULATOR_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3600),
            batch_size: std::env::var("METRICS_CALCULATOR_BATCH_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(50),
            recalc_after_hours: std::env::var("METRICS_RECALC_AFTER_HOURS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(24),
        }
    }
}

/// Background metrics calculator service.
pub struct MetricsCalculator {
    pool: PgPool,
    config: MetricsCalculatorConfig,
    analyzer: Arc<ProfitabilityAnalyzer>,
}

impl MetricsCalculator {
    /// Create a new metrics calculator.
    pub fn new(pool: PgPool, config: MetricsCalculatorConfig) -> Self {
        let analyzer = Arc::new(ProfitabilityAnalyzer::new(pool.clone()));
        Self {
            pool,
            config,
            analyzer,
        }
    }

    /// Start the background calculation loop.
    pub async fn run(self: Arc<Self>) {
        if !self.config.enabled {
            info!("Metrics calculator is disabled");
            return;
        }

        info!(
            interval_secs = self.config.interval_secs,
            batch_size = self.config.batch_size,
            recalc_after_hours = self.config.recalc_after_hours,
            "Starting metrics calculator background job"
        );

        let mut interval = time::interval(time::Duration::from_secs(self.config.interval_secs));

        loop {
            interval.tick().await;

            if let Err(e) = self.calculate_batch().await {
                error!(error = %e, "Failed to calculate metrics batch");
            }
        }
    }

    /// Calculate metrics for a batch of wallets.
    async fn calculate_batch(&self) -> Result<()> {
        let start_time = Utc::now();
        debug!("Starting metrics calculation cycle");

        // Get wallets that need metrics calculated
        let wallets = self.get_wallets_to_process().await?;

        if wallets.is_empty() {
            debug!("No wallets need metrics calculation");
            return Ok(());
        }

        info!(
            wallet_count = wallets.len(),
            "Processing wallets for metrics calculation"
        );

        let mut success_count = 0;
        let mut error_count = 0;

        for address in wallets {
            match self.calculate_wallet_metrics(&address).await {
                Ok(_) => {
                    success_count += 1;
                    debug!(address = %address, "Successfully calculated metrics");
                }
                Err(e) => {
                    error_count += 1;
                    warn!(address = %address, error = %e, "Failed to calculate metrics");
                }
            }
        }

        let duration = (Utc::now() - start_time).num_seconds();
        info!(
            success_count,
            error_count,
            duration_secs = duration,
            "Metrics calculation cycle complete"
        );

        Ok(())
    }

    /// Get addresses of wallets that need metrics calculated.
    async fn get_wallets_to_process(&self) -> Result<Vec<String>> {
        let cutoff = Utc::now() - Duration::hours(self.config.recalc_after_hours);

        // Priority 1: Wallets that have never been calculated
        // Priority 2: Wallets with stale metrics (older than recalc_after_hours)
        // Priority 3: Active wallets (have traded recently)
        let rows = sqlx::query_scalar::<_, String>(
            r#"
            SELECT wf.address
            FROM wallet_features wf
            LEFT JOIN wallet_success_metrics wsm ON wsm.address = wf.address
            WHERE
                -- Never calculated OR stale metrics
                (wsm.last_computed IS NULL OR wsm.last_computed < $1)
                -- Has enough trades to be meaningful
                AND wf.total_trades >= 10
                -- Active in the last 90 days
                AND wf.last_trade >= NOW() - INTERVAL '90 days'
            ORDER BY
                -- Prioritize never-calculated
                CASE WHEN wsm.last_computed IS NULL THEN 0 ELSE 1 END,
                -- Then by staleness
                COALESCE(wsm.last_computed, '1970-01-01'::timestamptz) ASC,
                -- Then by recent activity
                wf.last_trade DESC
            LIMIT $2
            "#,
        )
        .bind(cutoff)
        .bind(self.config.batch_size as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Calculate and store metrics for a single wallet.
    async fn calculate_wallet_metrics(&self, address: &str) -> Result<()> {
        // Calculate 30-day metrics
        let metrics = self
            .analyzer
            .calculate_metrics(address, TimePeriod::Month)
            .await?;

        // Store in database
        self.analyzer.store_metrics(&metrics).await?;

        debug!(
            address = %address,
            roi = metrics.roi_percentage,
            sharpe = metrics.sharpe_ratio,
            win_rate = metrics.win_rate,
            "Stored wallet metrics"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = MetricsCalculatorConfig::default();
        assert!(config.enabled);
        assert_eq!(config.interval_secs, 3600);
        assert_eq!(config.batch_size, 50);
        assert_eq!(config.recalc_after_hours, 24);
    }

    #[test]
    fn test_config_from_env() {
        std::env::set_var("METRICS_CALCULATOR_ENABLED", "false");
        std::env::set_var("METRICS_CALCULATOR_INTERVAL_SECS", "7200");
        std::env::set_var("METRICS_CALCULATOR_BATCH_SIZE", "100");
        std::env::set_var("METRICS_RECALC_AFTER_HOURS", "48");

        let config = MetricsCalculatorConfig::from_env();
        assert!(!config.enabled);
        assert_eq!(config.interval_secs, 7200);
        assert_eq!(config.batch_size, 100);
        assert_eq!(config.recalc_after_hours, 48);

        // Cleanup
        std::env::remove_var("METRICS_CALCULATOR_ENABLED");
        std::env::remove_var("METRICS_CALCULATOR_INTERVAL_SECS");
        std::env::remove_var("METRICS_CALCULATOR_BATCH_SIZE");
        std::env::remove_var("METRICS_RECALC_AFTER_HOURS");
    }
}
