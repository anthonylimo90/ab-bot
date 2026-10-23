//! Background job to populate wallet_success_metrics table.
//!
//! This module runs a periodic background task that discovers wallets
//! and calculates their profitability metrics using ProfitabilityAnalyzer.

use anyhow::Result;
use chrono::{Duration, Utc};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tokio::time;
use tracing::{debug, error, info, warn};
use wallet_tracker::profitability::{ProfitabilityAnalyzer, TimePeriod};
use wallet_tracker::strategy_classifier::StrategyClassifier;
use wallet_tracker::{MarketConditionAnalyzer, MarketRegime};

/// Internal row type for Pearson correlation query results.
#[derive(Debug, sqlx::FromRow)]
struct CorrelationRow {
    condition_id_a: String,
    condition_id_b: String,
    correlation: Option<f64>,
    sample_size: i32,
}

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
            interval_secs: 900, // 15 minutes — was 1 hour, too slow for rotation
            batch_size: 200,    // was 50 — process more wallets per cycle
            recalc_after_hours: 6, // was 24 — stale metrics block good candidates
        }
    }
}

impl MetricsCalculatorConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        Self {
            enabled: polymarket_core::config::env_flag("METRICS_CALCULATOR_ENABLED", true),
            interval_secs: polymarket_core::config::env_count(
                "METRICS_CALCULATOR_INTERVAL_SECS",
                900,
            ),
            batch_size: polymarket_core::config::env_count("METRICS_CALCULATOR_BATCH_SIZE", 50),
            recalc_after_hours: polymarket_core::config::env_count("METRICS_RECALC_AFTER_HOURS", 6),
        }
    }
}

/// Background metrics calculator service.
pub struct MetricsCalculator {
    pool: PgPool,
    config: MetricsCalculatorConfig,
    analyzer: Arc<ProfitabilityAnalyzer>,
    /// Shared market regime state, updated each cycle.
    regime_state: Option<Arc<RwLock<MarketRegime>>>,
    /// Shared semaphore to coordinate heavy DB tasks.
    db_semaphore: Option<Arc<Semaphore>>,
}

impl MetricsCalculator {
    /// Create a new metrics calculator.
    pub fn new(pool: PgPool, config: MetricsCalculatorConfig) -> Self {
        let analyzer = Arc::new(ProfitabilityAnalyzer::new(pool.clone()));
        Self {
            pool,
            config,
            analyzer,
            regime_state: None,
            db_semaphore: None,
        }
    }

    /// Create a metrics calculator that also updates the shared market regime.
    pub fn with_regime(
        pool: PgPool,
        config: MetricsCalculatorConfig,
        regime_state: Arc<RwLock<MarketRegime>>,
    ) -> Self {
        let analyzer = Arc::new(ProfitabilityAnalyzer::new(pool.clone()));
        Self {
            pool,
            config,
            analyzer,
            regime_state: Some(regime_state),
            db_semaphore: None,
        }
    }

    /// Attach a shared DB semaphore for backpressure coordination.
    pub fn with_db_semaphore(mut self, semaphore: Arc<Semaphore>) -> Self {
        self.db_semaphore = Some(semaphore);
        self
    }

    /// Compute metrics for a single wallet address (for on-demand recomputation).
    pub async fn compute_single_wallet(&self, address: &str) -> Result<()> {
        self.calculate_wallet_metrics(address).await
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

        // Wait for the harvester to populate wallet_features with initial data.
        // The harvester has a 10s startup delay + ~5s per cycle, so 90s gives it
        // several cycles to accumulate wallets before we try to compute metrics.
        info!("Metrics calculator waiting 90s for harvester to populate initial data");
        time::sleep(time::Duration::from_secs(90)).await;

        // Run the first batch immediately after the initial delay
        info!("Running initial metrics calculation batch");
        if let Err(e) = self.calculate_batch().await {
            error!(error = %e, "Failed to calculate initial metrics batch");
        }

        // Then resume the normal interval
        let mut interval =
            time::interval(time::Duration::from_secs(self.config.interval_secs.max(1)));
        // After an overrun, wait a full period instead of catching up.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consume the first immediate tick so we don't double-run
        interval.tick().await;

        let mut cycle_count: u64 = 0;
        // Run cleanup every 16 cycles (~4 hours at 15-min intervals)
        let cleanup_every: u64 = 16;
        // Run market correlation computation every 672 cycles (~7 days at 15-min intervals)
        let correlation_every: u64 = 672;

        loop {
            interval.tick().await;
            cycle_count += 1;

            if let Err(e) = self.calculate_batch().await {
                error!(error = %e, "Failed to calculate metrics batch");
            }

            // Periodic data hygiene — purge stale rows to keep tables lean
            if cycle_count.is_multiple_of(cleanup_every) {
                info!("Running periodic data hygiene cleanup");
                match sqlx::query("SELECT cleanup_stale_data()")
                    .execute(&self.pool)
                    .await
                {
                    Ok(_) => info!("Data hygiene cleanup completed successfully"),
                    Err(e) => warn!(error = %e, "Data hygiene cleanup failed (non-fatal)"),
                }
                // A revocation outlives its token by a day (a clock behind
                // the database's still honours the token that long), then
                // guards nothing.
                if let Err(e) = sqlx::query(
                    "DELETE FROM revoked_sessions WHERE expires_at < NOW() - INTERVAL '1 day'",
                )
                .execute(&self.pool)
                .await
                {
                    warn!(error = %e, "Pruning expired session revocations failed (non-fatal)");
                }
            }

            // Weekly market correlation computation for cross-market signal generator.
            // Computes Pearson r between same-category markets from orderbook_hourly.
            if cycle_count.is_multiple_of(correlation_every) {
                info!("Running weekly market correlation computation");
                if let Err(e) = self.compute_market_correlations().await {
                    warn!(error = %e, "Market correlation computation failed (non-fatal)");
                }
            }
        }
    }

    /// Calculate metrics for a batch of wallets.
    async fn calculate_batch(&self) -> Result<()> {
        // Acquire semaphore permit before heavy DB work (if configured)
        let _permit = if let Some(ref sem) = self.db_semaphore {
            let permit = sem.acquire().await.expect("semaphore closed");
            debug!("Metrics calculator acquired DB semaphore permit");
            Some(permit)
        } else {
            None
        };

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

        for (i, address) in wallets.iter().enumerate() {
            match self.calculate_wallet_metrics(address).await {
                Ok(_) => {
                    success_count += 1;
                    debug!(address = %address, "Successfully calculated metrics");
                }
                Err(e) => {
                    error_count += 1;
                    warn!(address = %address, error = %e, "Failed to calculate metrics");
                }
            }

            // Yield every 5 wallets to let other tasks acquire pool connections
            if (i + 1) % 5 == 0 {
                tokio::task::yield_now().await;
            }
        }

        // Release permit before the lighter classification + regime work
        drop(_permit);
        debug!("Metrics calculator released DB semaphore permit");

        let duration = (Utc::now() - start_time).num_seconds();
        info!(
            success_count,
            error_count,
            duration_secs = duration,
            "Metrics calculation cycle complete"
        );

        // Classify strategies for wallets that lack a classification.
        // This uses a lightweight rule-based classifier from wallet_features data.
        if let Err(e) = self.classify_wallet_strategies().await {
            warn!(error = %e, "Failed to classify wallet strategies");
        }

        // Update market regime detection after metrics are fresh
        if let Some(ref regime_state) = self.regime_state {
            let market_analyzer = MarketConditionAnalyzer::new(self.pool.clone());
            match market_analyzer.detect_regime_from_metrics().await {
                Ok(regime) => {
                    let mut current = regime_state.write().await;
                    if *current != regime {
                        info!(
                            previous = ?*current,
                            detected = ?regime,
                            "Market regime changed"
                        );
                    }
                    *current = regime;
                }
                Err(e) => {
                    warn!(error = %e, "Failed to detect market regime from metrics");
                }
            }
        }

        Ok(())
    }

    /// Compute Pearson correlation coefficients between same-category markets.
    ///
    /// For each active category in `market_metadata`, self-joins `orderbook_hourly`
    /// to compute pairwise correlations of **hourly price changes** (differences of
    /// consecutive hourly closes) over the last 90 days. Correlating price *levels*
    /// would report any two markets that both drifted over the period as correlated
    /// (spurious correlation between trending series); changes remove that trend.
    /// Pairs with `|r| > 0.50` and at least 168 overlapping hourly changes (≈ 1 week)
    /// are UPSERTed into `market_correlations`. Pairs of a computed category that no
    /// longer qualify, and any pair not refreshed in 14 days, are pruned.
    async fn compute_market_correlations(&self) -> Result<()> {
        let start = Utc::now();

        // Acquire semaphore — this is a heavy cross-join computation
        let _permit = if let Some(ref sem) = self.db_semaphore {
            let permit = sem.acquire().await.expect("semaphore closed");
            debug!("Correlation computation acquired DB semaphore permit");
            Some(permit)
        } else {
            None
        };

        // Get distinct active categories from market_metadata
        let categories: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT category FROM market_metadata WHERE category IS NOT NULL AND active = true",
        )
        .fetch_all(&self.pool)
        .await?;

        if categories.is_empty() {
            info!("No active categories for correlation computation");
            return Ok(());
        }

        // Database clock: rows upserted below are stamped with the database's
        // NOW(), so the "not refreshed in this run" cut-off must come from the same
        // clock rather than from this process's.
        let run_started: chrono::DateTime<Utc> = sqlx::query_scalar("SELECT NOW()")
            .fetch_one(&self.pool)
            .await?;

        let mut total_pairs: u64 = 0;

        for category in &categories {
            // Compute Pearson correlation for all market pairs within this category.
            // Uses orderbook_hourly `close` (last yes_mid) as the price series.
            // Canonical ordering (a < b) avoids duplicate pairs and matches the
            // CHECK constraint on market_correlations.
            let rows: Vec<CorrelationRow> = sqlx::query_as(
                r#"
                WITH hourly_prices AS (
                    SELECT
                        oh.market_id,
                        oh.bucket,
                        oh.close AS price
                    FROM orderbook_hourly oh
                    JOIN market_metadata mm ON mm.condition_id = oh.market_id
                    WHERE mm.category = $1
                      AND mm.active = true
                      AND oh.bucket >= NOW() - INTERVAL '90 days'
                      AND oh.close > 0
                ),
                hourly_changes AS (
                    SELECT
                        market_id,
                        bucket,
                        price - LAG(price) OVER w AS change,
                        bucket - LAG(bucket) OVER w AS gap
                    FROM hourly_prices
                    WINDOW w AS (PARTITION BY market_id ORDER BY bucket)
                ),
                changes AS (
                    -- Only changes between *consecutive* hours: a missing hour would
                    -- make the difference span several hours.
                    SELECT market_id, bucket, change
                    FROM hourly_changes
                    WHERE change IS NOT NULL
                      AND gap = INTERVAL '1 hour'
                )
                SELECT
                    a.market_id AS condition_id_a,
                    b.market_id AS condition_id_b,
                    CORR(a.change, b.change) AS correlation,
                    COUNT(*)::INT AS sample_size
                FROM changes a
                JOIN changes b
                    ON a.bucket = b.bucket
                    AND a.market_id < b.market_id
                GROUP BY a.market_id, b.market_id
                HAVING COUNT(*) >= 168
                   AND ABS(CORR(a.change, b.change)) > 0.50
                "#,
            )
            .bind(category)
            .fetch_all(&self.pool)
            .await?;

            // Filter out NULL correlations (degenerate pairs with zero variance)
            let valid_rows: Vec<&CorrelationRow> =
                rows.iter().filter(|r| r.correlation.is_some()).collect();

            // Batch UPSERT into market_correlations
            for chunk in valid_rows.chunks(50) {
                let mut qb = sqlx::QueryBuilder::new(
                    "INSERT INTO market_correlations (condition_id_a, condition_id_b, correlation, category, sample_size, computed_at) ",
                );
                qb.push_values(chunk, |mut b, row| {
                    // Safe unwrap: filtered above
                    let corr = row.correlation.unwrap();
                    b.push_bind(&row.condition_id_a)
                        .push_bind(&row.condition_id_b)
                        .push_bind(corr)
                        .push_bind(category)
                        .push_bind(row.sample_size)
                        .push("NOW()");
                });
                qb.push(
                    " ON CONFLICT (condition_id_a, condition_id_b) DO UPDATE SET \
                     correlation = EXCLUDED.correlation, \
                     category = EXCLUDED.category, \
                     sample_size = EXCLUDED.sample_size, \
                     computed_at = EXCLUDED.computed_at",
                );
                qb.build().execute(&self.pool).await?;
                total_pairs += chunk.len() as u64;
            }

            // Drop this category's pairs that were not refreshed above: they no
            // longer qualify (e.g. pairs stored from the old level-based
            // correlation, or whose changes decorrelated) and must not linger
            // until the 14-day prune.
            sqlx::query("DELETE FROM market_correlations WHERE category = $1 AND computed_at < $2")
                .bind(category)
                .bind(run_started)
                .execute(&self.pool)
                .await?;

            // Yield between categories to avoid blocking the pool
            tokio::task::yield_now().await;
        }

        // Prune stale correlation pairs not refreshed in the last 14 days
        let deleted = sqlx::query(
            "DELETE FROM market_correlations WHERE computed_at < NOW() - INTERVAL '14 days'",
        )
        .execute(&self.pool)
        .await?;

        let duration = (Utc::now() - start).num_seconds();
        info!(
            categories = categories.len(),
            total_pairs,
            stale_deleted = deleted.rows_affected(),
            duration_secs = duration,
            "Market correlation computation complete"
        );

        Ok(())
    }

    /// Get addresses of wallets that need metrics calculated.
    async fn get_wallets_to_process(&self) -> Result<Vec<String>> {
        let cutoff = Utc::now() - Duration::hours(self.config.recalc_after_hours);

        // Priority 0: Wallets currently in workspace_wallet_allocations (actively tracked)
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
                AND wf.total_trades >= 5
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

    /// Classify trading strategies for wallets that haven't been classified yet.
    ///
    /// Fetches wallets from `wallet_features` where `strategy_type IS NULL`,
    /// runs the rule-based classifier, and stores the primary strategy label.
    async fn classify_wallet_strategies(&self) -> Result<()> {
        use polymarket_core::types::WalletFeatures;

        // Fetch wallets missing a strategy classification (limit per batch)
        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            String,
            i64,
            Option<f64>,
            Option<f64>,
            Option<f64>,
            bool,
            i64,
            f64,
        )> = sqlx::query_as(
            r#"
                SELECT address, total_trades, win_rate, avg_latency_ms, interval_cv,
                       has_opposing_positions, opposing_position_count, activity_spread
                FROM wallet_features
                WHERE strategy_type IS NULL
                  AND total_trades >= 10
                ORDER BY last_trade DESC NULLS LAST
                LIMIT 100
                "#,
        )
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            debug!("No wallets need strategy classification");
            return Ok(());
        }

        let classifier = StrategyClassifier::new();

        // Collect all classifications first, then batch-update in a single round-trip.
        let mut addresses: Vec<String> = Vec::with_capacity(rows.len());
        let mut labels: Vec<String> = Vec::with_capacity(rows.len());

        for (
            address,
            total_trades,
            win_rate,
            avg_latency_ms,
            interval_cv,
            has_opposing,
            opposing_count,
            activity_spread,
        ) in &rows
        {
            let features = WalletFeatures {
                address: address.clone(),
                total_trades: *total_trades as u64,
                win_rate: *win_rate,
                avg_latency_ms: *avg_latency_ms,
                interval_cv: *interval_cv,
                has_opposing_positions: *has_opposing,
                opposing_position_count: *opposing_count as u64,
                activity_spread: *activity_spread,
                ..Default::default()
            };

            let classification = classifier.classify_basic(&features);
            labels.push(format!("{:?}", classification.primary_strategy));
            addresses.push(address.clone());
        }

        let classified = addresses.len() as u32;

        if classified > 0 {
            // Single UNNEST batch UPDATE — eliminates N round-trips.
            if let Err(e) = sqlx::query(
                "UPDATE wallet_features wf \
                 SET strategy_type = data.label \
                 FROM UNNEST($1::text[], $2::text[]) AS data(address, label) \
                 WHERE wf.address = data.address",
            )
            .bind(&addresses)
            .bind(&labels)
            .execute(&self.pool)
            .await
            {
                warn!(error = %e, "Failed to batch-store strategy classifications");
            } else {
                info!(classified, "Classified wallet strategies");
            }
        }

        Ok(())
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
        assert_eq!(config.interval_secs, 900);
        assert_eq!(config.batch_size, 200);
        assert_eq!(config.recalc_after_hours, 6);
    }

    #[test]
    fn test_correlation_cycle_gate() {
        // 672 cycles × 15 min = ~7 days
        let correlation_every: u64 = 672;
        assert!(672_u64.is_multiple_of(correlation_every));
        assert!(!671_u64.is_multiple_of(correlation_every));
        assert!(1344_u64.is_multiple_of(correlation_every)); // 2 weeks
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

/// Market correlations are computed on hourly price changes, not on levels.
#[cfg(test)]
mod correlation_tests {
    use super::*;
    use chrono::DurationRound;

    /// The query used before: Pearson correlation of price LEVELS.
    const LEVELS_SQL: &str = r#"
        WITH hourly_prices AS (
            SELECT oh.market_id, oh.bucket, oh.close AS price
            FROM orderbook_hourly oh
            JOIN market_metadata mm ON mm.condition_id = oh.market_id
            WHERE mm.category = $1 AND mm.active = true
              AND oh.bucket >= NOW() - INTERVAL '90 days' AND oh.close > 0
        )
        SELECT a.market_id AS condition_id_a, b.market_id AS condition_id_b,
               CORR(a.price, b.price) AS correlation, COUNT(*)::INT AS sample_size
        FROM hourly_prices a
        JOIN hourly_prices b ON a.bucket = b.bucket AND a.market_id < b.market_id
        GROUP BY a.market_id, b.market_id
        HAVING COUNT(*) >= 168 AND ABS(CORR(a.price, b.price)) > 0.50
    "#;

    async fn create_tables(pool: &PgPool) {
        sqlx::raw_sql(
            r#"
            CREATE TABLE market_metadata (
                condition_id TEXT PRIMARY KEY,
                category TEXT,
                active BOOLEAN NOT NULL DEFAULT TRUE
            );
            CREATE TABLE orderbook_hourly (
                market_id TEXT NOT NULL,
                bucket TIMESTAMPTZ NOT NULL,
                close NUMERIC
            );
            CREATE TABLE market_correlations (
                condition_id_a  TEXT NOT NULL,
                condition_id_b  TEXT NOT NULL,
                correlation     DOUBLE PRECISION NOT NULL,
                category        TEXT,
                sample_size     INT NOT NULL,
                computed_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                PRIMARY KEY (condition_id_a, condition_id_b),
                CONSTRAINT market_correlations_ordered CHECK (condition_id_a < condition_id_b)
            );
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    /// Deterministic uniform noise in [-0.01, 0.01).
    fn noise(seed: u64, n: usize) -> Vec<f64> {
        let mut state = seed;
        (0..n)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((state >> 33) as f64 / (1u64 << 31) as f64 - 0.5) * 0.02
            })
            .collect()
    }

    const POINTS: usize = 400;

    /// `level(i)` per hourly bucket, ending at the current hour.
    async fn insert_series(
        pool: &PgPool,
        market: &str,
        category: &str,
        values: &[f64],
        skip: Option<usize>,
    ) {
        sqlx::query("INSERT INTO market_metadata VALUES ($1, $2, TRUE) ON CONFLICT DO NOTHING")
            .bind(market)
            .bind(category)
            .execute(pool)
            .await
            .unwrap();

        let now_hour = Utc::now()
            .duration_trunc(Duration::hours(1))
            .expect("truncate to the hour");
        let n = values.len();
        let mut buckets = Vec::new();
        let mut closes = Vec::new();
        for (i, value) in values.iter().enumerate() {
            if Some(i) == skip {
                continue;
            }
            buckets.push(now_hour - Duration::hours((n - 1 - i) as i64));
            closes.push(*value);
        }
        sqlx::query(
            "INSERT INTO orderbook_hourly (market_id, bucket, close) \
             SELECT $1, b, c::numeric FROM UNNEST($2::timestamptz[], $3::float8[]) AS t(b, c)",
        )
        .bind(market)
        .bind(&buckets)
        .bind(&closes)
        .execute(pool)
        .await
        .unwrap();
    }

    /// a and b drift upwards independently; c shares a's shocks but has its
    /// own drift. Levels: all three look strongly correlated (shared uptrend).
    /// Hourly changes: only a and c are related.
    async fn seed_politics(pool: &PgPool) {
        let na = noise(1, POINTS);
        let nb = noise(2, POINTS);
        let a: Vec<f64> = (0..POINTS)
            .map(|i| 0.20 + 0.0004 * i as f64 + na[i])
            .collect();
        let b: Vec<f64> = (0..POINTS)
            .map(|i| 0.30 + 0.0003 * i as f64 + nb[i])
            .collect();
        let c: Vec<f64> = (0..POINTS)
            .map(|i| 0.35 + 0.0002 * i as f64 + na[i])
            .collect();
        insert_series(pool, "a", "politics", &a, None).await;
        insert_series(pool, "b", "politics", &b, None).await;
        insert_series(pool, "c", "politics", &c, None).await;
    }

    fn calculator(pool: &PgPool) -> MetricsCalculator {
        MetricsCalculator::new(pool.clone(), MetricsCalculatorConfig::default())
    }

    async fn stored(pool: &PgPool) -> Vec<(String, String, f64, i32)> {
        sqlx::query_as(
            "SELECT condition_id_a, condition_id_b, correlation, sample_size \
             FROM market_correlations ORDER BY condition_id_a, condition_id_b",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = false)]
    async fn trending_markets_are_not_reported_as_correlated(pool: PgPool) {
        create_tables(&pool).await;
        seed_politics(&pool).await;

        // Control: the old level-based query flags every pair (spurious).
        let old: Vec<CorrelationRow> = sqlx::query_as(LEVELS_SQL)
            .bind("politics")
            .fetch_all(&pool)
            .await
            .unwrap();
        let old_pairs: Vec<_> = old
            .iter()
            .map(|r| (r.condition_id_a.as_str(), r.condition_id_b.as_str()))
            .collect();
        assert!(old_pairs.contains(&("a", "b")), "levels: {old_pairs:?}");
        assert!(old_pairs.contains(&("b", "c")), "levels: {old_pairs:?}");

        calculator(&pool)
            .compute_market_correlations()
            .await
            .unwrap();

        // Only the pair whose hourly changes really move together remains.
        let rows = stored(&pool).await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        let (a, c, correlation, sample_size) = &rows[0];
        assert_eq!((a.as_str(), c.as_str()), ("a", "c"));
        assert!(*correlation > 0.99, "got {correlation}");
        assert_eq!(
            *sample_size,
            (POINTS - 1) as i32,
            "one change per consecutive hour pair"
        );
    }

    #[sqlx::test(migrations = false)]
    async fn missing_hours_do_not_create_multi_hour_changes(pool: PgPool) {
        create_tables(&pool).await;
        let na = noise(1, POINTS);
        let a: Vec<f64> = (0..POINTS)
            .map(|i| 0.20 + 0.0004 * i as f64 + na[i])
            .collect();
        let c: Vec<f64> = (0..POINTS)
            .map(|i| 0.35 + 0.0002 * i as f64 + na[i])
            .collect();
        // Drop hour 200 from `a`: the change into it does not exist and the
        // change out of it spans two hours, so both are excluded.
        insert_series(&pool, "a", "politics", &a, Some(200)).await;
        insert_series(&pool, "c", "politics", &c, None).await;

        calculator(&pool)
            .compute_market_correlations()
            .await
            .unwrap();

        let rows = stored(&pool).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].3, (POINTS - 3) as i32);
        assert!(rows[0].2 > 0.99);
    }

    #[sqlx::test(migrations = false)]
    async fn short_histories_are_not_enough(pool: PgPool) {
        create_tables(&pool).await;
        let na = noise(1, 100);
        let a: Vec<f64> = (0..100).map(|i| 0.20 + 0.0004 * i as f64 + na[i]).collect();
        let c: Vec<f64> = (0..100).map(|i| 0.35 + 0.0002 * i as f64 + na[i]).collect();
        insert_series(&pool, "a", "politics", &a, None).await;
        insert_series(&pool, "c", "politics", &c, None).await;

        calculator(&pool)
            .compute_market_correlations()
            .await
            .unwrap();
        assert!(stored(&pool).await.is_empty(), "fewer than 168 changes");
    }

    #[sqlx::test(migrations = false)]
    async fn pairs_that_no_longer_qualify_are_removed_but_other_categories_are_kept(pool: PgPool) {
        create_tables(&pool).await;
        seed_politics(&pool).await;
        // A pair stored by the old level-based computation, and one in another
        // category that this run does not recompute.
        sqlx::query(
            "INSERT INTO market_correlations VALUES \
             ('a', 'b', 0.97, 'politics', 400, NOW() - INTERVAL '1 day'), \
             ('x', 'y', 0.80, 'sports', 300, NOW() - INTERVAL '1 day')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO market_metadata VALUES ('x', 'sports', FALSE), ('y', 'sports', FALSE)",
        )
        .execute(&pool)
        .await
        .unwrap();

        calculator(&pool)
            .compute_market_correlations()
            .await
            .unwrap();

        let pairs: Vec<(String, String)> = stored(&pool)
            .await
            .into_iter()
            .map(|(a, b, _, _)| (a, b))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("a".to_string(), "c".to_string()),
                ("x".to_string(), "y".to_string())
            ]
        );
    }
}
