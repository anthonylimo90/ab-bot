//! Flow feature calculator.
//!
//! Background task that aggregates `wallet_trades` into `market_flow_features`
//! at multiple time windows (15min, 60min, 240min). Joins `bot_scores` to
//! weight smart money flows (non-bot wallets with bot_score < 30).

use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

/// Configuration for the flow feature calculator.
#[derive(Debug, Clone)]
pub struct FlowFeatureConfig {
    /// Whether the calculator is enabled.
    pub enabled: bool,
    /// Interval between computation cycles in seconds.
    pub interval_secs: u64,
    /// Time windows to compute (in minutes).
    pub windows: Vec<i32>,
    /// Bot score threshold — wallets below this are considered smart money.
    pub smart_money_threshold: i32,
}

impl FlowFeatureConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("FLOW_FEATURE_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
            interval_secs: std::env::var("FLOW_FEATURE_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            windows: vec![15, 60, 240],
            smart_money_threshold: std::env::var("SMART_MONEY_BOT_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
        }
    }
}

/// Spawn the flow feature calculator background task.
pub fn spawn_flow_feature_calculator(
    config: FlowFeatureConfig,
    pool: PgPool,
    db_semaphore: Arc<Semaphore>,
) {
    if !config.enabled {
        info!("Flow feature calculator disabled (FLOW_FEATURE_ENABLED != true)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        windows = ?config.windows,
        smart_money_threshold = config.smart_money_threshold,
        "Spawning flow feature calculator background task"
    );

    tokio::spawn(calculator_loop(config, pool, db_semaphore));
}

/// Main calculator loop.
async fn calculator_loop(config: FlowFeatureConfig, pool: PgPool, db_semaphore: Arc<Semaphore>) {
    let interval = Duration::from_secs(config.interval_secs);

    // Startup delay
    tokio::time::sleep(Duration::from_secs(20)).await;

    loop {
        match compute_cycle(&pool, &db_semaphore, &config).await {
            Ok(rows) => {
                info!(rows_upserted = rows, "Flow feature computation completed");
            }
            Err(e) => {
                warn!(error = %e, "Flow feature computation failed");
            }
        }

        tokio::time::sleep(interval).await;
    }
}

/// Execute a single computation cycle across all configured time windows.
async fn compute_cycle(
    pool: &PgPool,
    db_semaphore: &Semaphore,
    config: &FlowFeatureConfig,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let now = chrono::Utc::now();
    let mut total_rows = 0usize;

    for &window_minutes in &config.windows {
        let window_start = now - chrono::Duration::minutes(window_minutes as i64);
        let _permit = db_semaphore.acquire().await.expect("semaphore closed");
        debug!(
            window_minutes = window_minutes,
            "Flow feature calculator acquired DB semaphore permit"
        );

        // Step 1: Aggregate wallet_trades for this window.
        //
        // This query:
        // - Groups trades by condition_id within the time window
        // - Computes buy/sell volumes, net flow, imbalance ratio
        // - Looks up latest bot scores only for wallets active in this window
        // - Counts unique buyers and sellers
        let rows = sqlx::query_as::<_, FlowFeatureRow>(
            r#"
            WITH recent_trades AS (
                SELECT
                    condition_id,
                    wallet_address,
                    side,
                    value
                FROM wallet_trades
                WHERE condition_id IS NOT NULL
                  AND timestamp >= $4
                  AND timestamp <= $1
            ),
            latest_bot_scores AS (
                SELECT DISTINCT ON (bs.address)
                    bs.address,
                    bs.total_score
                FROM bot_scores bs
                INNER JOIN (
                    SELECT DISTINCT wallet_address
                    FROM recent_trades
                ) rw
                  ON rw.wallet_address = bs.address
                ORDER BY bs.address, bs.computed_at DESC
            )
            SELECT
                rt.condition_id,
                $1::timestamptz AS window_end,
                $2::int AS window_minutes,
                COALESCE(SUM(CASE WHEN rt.side = 'BUY' THEN rt.value ELSE 0 END), 0) AS buy_volume,
                COALESCE(SUM(CASE WHEN rt.side = 'SELL' THEN rt.value ELSE 0 END), 0) AS sell_volume,
                COALESCE(
                    SUM(CASE WHEN rt.side = 'BUY' THEN rt.value ELSE -rt.value END),
                    0
                ) AS net_flow,
                CASE
                    WHEN COALESCE(SUM(rt.value), 0) = 0 THEN 0
                    ELSE COALESCE(
                        SUM(CASE WHEN rt.side = 'BUY' THEN rt.value ELSE -rt.value END),
                        0
                    )::numeric / SUM(rt.value)
                END AS imbalance_ratio,
                COUNT(DISTINCT CASE WHEN rt.side = 'BUY' THEN rt.wallet_address END)::int AS unique_buyers,
                COUNT(DISTINCT CASE WHEN rt.side = 'SELL' THEN rt.wallet_address END)::int AS unique_sellers,
                COALESCE(
                    SUM(
                        CASE
                            WHEN bs.total_score IS NULL OR bs.total_score < $3
                            THEN CASE WHEN rt.side = 'BUY' THEN rt.value ELSE -rt.value END
                            ELSE 0
                        END
                    ),
                    0
                ) AS smart_money_flow,
                COUNT(*)::int AS trade_count
            FROM recent_trades rt
            LEFT JOIN latest_bot_scores bs ON bs.address = rt.wallet_address
            GROUP BY rt.condition_id
            HAVING COUNT(*) >= 2
            "#,
        )
        .bind(now)
        .bind(window_minutes)
        .bind(config.smart_money_threshold)
        .bind(window_start)
        .fetch_all(pool)
        .await?;

        if rows.is_empty() {
            debug!(window_minutes = window_minutes, "No flow data for window");
            continue;
        }

        // Step 2: Batch UPSERT into market_flow_features
        const BATCH_SIZE: usize = 100;
        for chunk in rows.chunks(BATCH_SIZE) {
            let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
                "INSERT INTO market_flow_features \
                 (condition_id, window_end, window_minutes, buy_volume, sell_volume, \
                  net_flow, imbalance_ratio, unique_buyers, unique_sellers, \
                  smart_money_flow, trade_count) ",
            );

            qb.push_values(chunk, |mut b, row| {
                b.push_bind(&row.condition_id)
                    .push_bind(row.window_end)
                    .push_bind(row.window_minutes)
                    .push_bind(row.buy_volume)
                    .push_bind(row.sell_volume)
                    .push_bind(row.net_flow)
                    .push_bind(row.imbalance_ratio)
                    .push_bind(row.unique_buyers)
                    .push_bind(row.unique_sellers)
                    .push_bind(row.smart_money_flow)
                    .push_bind(row.trade_count);
            });

            qb.push(
                " ON CONFLICT (condition_id, window_end, window_minutes) DO UPDATE SET \
                 buy_volume = EXCLUDED.buy_volume, \
                 sell_volume = EXCLUDED.sell_volume, \
                 net_flow = EXCLUDED.net_flow, \
                 imbalance_ratio = EXCLUDED.imbalance_ratio, \
                 unique_buyers = EXCLUDED.unique_buyers, \
                 unique_sellers = EXCLUDED.unique_sellers, \
                 smart_money_flow = EXCLUDED.smart_money_flow, \
                 trade_count = EXCLUDED.trade_count",
            );

            match qb.build().execute(pool).await {
                Ok(result) => {
                    total_rows += result.rows_affected() as usize;
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        window_minutes = window_minutes,
                        "Failed to upsert flow features batch"
                    );
                }
            }

            tokio::task::yield_now().await;
        }

        drop(_permit);
        debug!(
            window_minutes = window_minutes,
            "Flow feature calculator released DB semaphore permit"
        );

        debug!(
            window_minutes = window_minutes,
            markets = rows.len(),
            "Computed flow features for window"
        );
    }

    Ok(total_rows)
}

/// Row type for the flow aggregation query.
#[derive(Debug, sqlx::FromRow)]
struct FlowFeatureRow {
    condition_id: String,
    window_end: chrono::DateTime<chrono::Utc>,
    window_minutes: i32,
    buy_volume: rust_decimal::Decimal,
    sell_volume: rust_decimal::Decimal,
    net_flow: rust_decimal::Decimal,
    imbalance_ratio: rust_decimal::Decimal,
    unique_buyers: i32,
    unique_sellers: i32,
    smart_money_flow: rust_decimal::Decimal,
    trade_count: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = FlowFeatureConfig {
            enabled: true,
            interval_secs: 300,
            windows: vec![15, 60, 240],
            smart_money_threshold: 30,
        };
        assert!(config.enabled);
        assert_eq!(config.windows.len(), 3);
    }
}
