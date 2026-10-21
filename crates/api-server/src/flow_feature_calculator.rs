//! Flow feature calculator.
//!
//! Background task that aggregates `wallet_trades` into `market_flow_features`
//! at multiple time windows (15min, 60min, 240min). Joins `bot_scores` to
//! weight smart money flows (scored, non-bot wallets with bot_score < 30).
//!
//! # Direction
//!
//! A trade moves the YES price depending on **which outcome token** was traded,
//! not only on BUY/SELL: buying NO is bearish on YES. Flow is therefore signed
//! towards YES:
//!
//! | outcome | BUY | SELL |
//! |---|---|---|
//! | YES | bullish (+) | bearish (-) |
//! | NO | bearish (-) | bullish (+) |
//!
//! In the stored columns `buy_volume` / `unique_buyers` mean *bullish-on-YES*
//! volume / wallets, `sell_volume` / `unique_sellers` the bearish side, and
//! `net_flow`, `imbalance_ratio` and `smart_money_flow` are positive when flow
//! favours YES. Trades whose outcome cannot be determined as YES or NO (the
//! `outcome` column is empty or names another outcome) are skipped.
//!
//! # Keys and retention
//!
//! `window_end` is truncated to the cycle boundary (the cycle interval, at most
//! one minute), so a re-run within the same bucket - a restart, an overlapping
//! deployment, a retry - updates the existing row instead of inserting a new
//! one. The bucket is deliberately small: consumers compare `window_end` with
//! "now" (the flow signal accepts features up to 10 minutes old), so a coarse
//! bucket would make the data they read staler than before. Rows older than the
//! retention period (14 days by default) are pruned.

use chrono::{DateTime, Duration as ChronoDuration, TimeZone, Utc};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

/// Largest cycle bucket, in seconds. `window_end` is a timestamp consumers
/// compare with "now", so it must not lag behind the data by more than this.
const MAX_BUCKET_SECS: u64 = 60;

/// How often old rows are pruned.
const PRUNE_EVERY: Duration = Duration::from_secs(3600);

/// Default number of days of flow features to keep.
const DEFAULT_RETENTION_DAYS: i64 = 14;

/// Configuration for the flow feature calculator.
#[derive(Debug, Clone)]
pub struct FlowFeatureConfig {
    /// Whether the calculator is enabled.
    pub enabled: bool,
    /// Interval between computation cycles in seconds.
    pub interval_secs: u64,
    /// Time windows to compute (in minutes).
    pub windows: Vec<i32>,
    /// Bot score threshold — scored wallets below this are considered smart money.
    pub smart_money_threshold: i32,
    /// Days of `market_flow_features` to keep (`FLOW_FEATURE_RETENTION_DAYS`).
    pub retention_days: i64,
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
            // Bounded so the cutoff date arithmetic can never overflow.
            retention_days: std::env::var("FLOW_FEATURE_RETENTION_DAYS")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|days| (1..=3650).contains(days))
                .unwrap_or(DEFAULT_RETENTION_DAYS),
        }
    }

    /// Bucket size used to truncate `window_end`.
    fn bucket_secs(&self) -> i64 {
        self.interval_secs.clamp(1, MAX_BUCKET_SECS) as i64
    }
}

/// Truncate `now` down to the start of its `bucket_secs`-sized bucket.
///
/// Every cycle that runs inside the same bucket computes the same `window_end`,
/// so its rows are upserted rather than accumulated.
pub(crate) fn cycle_boundary(now: DateTime<Utc>, bucket_secs: i64) -> DateTime<Utc> {
    let bucket = bucket_secs.max(1);
    let floored = now.timestamp().div_euclid(bucket) * bucket;
    Utc.timestamp_opt(floored, 0).single().unwrap_or(now)
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
        retention_days = config.retention_days,
        "Spawning flow feature calculator background task"
    );

    tokio::spawn(calculator_loop(config, pool, db_semaphore));
}

/// Main calculator loop.
async fn calculator_loop(config: FlowFeatureConfig, pool: PgPool, db_semaphore: Arc<Semaphore>) {
    let interval = Duration::from_secs(config.interval_secs);

    // Startup delay
    tokio::time::sleep(Duration::from_secs(20)).await;

    let mut last_prune: Option<std::time::Instant> = None;

    loop {
        match compute_cycle(&pool, &db_semaphore, &config).await {
            Ok(rows) => {
                info!(rows_upserted = rows, "Flow feature computation completed");
            }
            Err(e) => {
                warn!(error = %e, "Flow feature computation failed");
            }
        }

        if last_prune.is_none_or(|at| at.elapsed() >= PRUNE_EVERY) {
            let cutoff = Utc::now() - ChronoDuration::days(config.retention_days);
            match prune_old_features(&pool, cutoff).await {
                Ok(()) => debug!(%cutoff, "Pruned old flow features"),
                Err(e) => warn!(error = %e, "Failed to prune old flow features"),
            }
            last_prune = Some(std::time::Instant::now());
        }

        tokio::time::sleep(interval).await;
    }
}

/// Delete flow features older than `cutoff`.
///
/// On TimescaleDB whole chunks are dropped (cheap, and works on compressed
/// chunks), which removes data older than `cutoff` at chunk (one day)
/// granularity; on plain PostgreSQL, or if dropping chunks fails, the rows are
/// deleted individually.
pub(crate) async fn prune_old_features(
    pool: &PgPool,
    cutoff: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let dropped = sqlx::query("SELECT drop_chunks('market_flow_features', older_than => $1)")
        .bind(cutoff)
        .fetch_all(pool)
        .await;
    if dropped.is_ok() {
        return Ok(());
    }

    sqlx::query("DELETE FROM market_flow_features WHERE window_end < $1")
        .bind(cutoff)
        .execute(pool)
        .await?;
    Ok(())
}

/// Execute a single computation cycle across all configured time windows.
async fn compute_cycle(
    pool: &PgPool,
    db_semaphore: &Semaphore,
    config: &FlowFeatureConfig,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    // All windows of this cycle end on the same boundary, which is also the
    // `window_end` key: re-running inside the bucket upserts the same rows.
    let window_end = cycle_boundary(Utc::now(), config.bucket_secs());
    let mut total_rows = 0usize;

    for &window_minutes in &config.windows {
        let _permit = db_semaphore.acquire().await.expect("semaphore closed");
        debug!(
            window_minutes = window_minutes,
            "Flow feature calculator acquired DB semaphore permit"
        );

        let rows = aggregate_window(
            pool,
            window_end,
            window_minutes,
            config.smart_money_threshold,
        )
        .await?;

        if rows.is_empty() {
            debug!(window_minutes = window_minutes, "No flow data for window");
            continue;
        }

        total_rows += upsert_rows(pool, &rows, window_minutes).await;

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

/// Aggregate the trades of `(window_end - window_minutes, window_end]` per market.
///
/// - Only trades with a known YES/NO outcome are used; their volume is signed
///   towards YES (see the module docs).
/// - Looks up the latest bot score only for wallets active in the window.
///   `smart_money_flow` counts wallets that *have* a score below the threshold;
///   unscored wallets are unknown, not smart money.
pub(crate) async fn aggregate_window(
    pool: &PgPool,
    window_end: DateTime<Utc>,
    window_minutes: i32,
    smart_money_threshold: i32,
) -> Result<Vec<FlowFeatureRow>, sqlx::Error> {
    let window_start = window_end - ChronoDuration::minutes(window_minutes as i64);

    sqlx::query_as::<_, FlowFeatureRow>(
        r#"
        WITH recent_trades AS (
            SELECT
                condition_id,
                wallet_address,
                value,
                -- +1 when the trade is bullish on YES, -1 when bearish:
                -- BUY YES / SELL NO = +1, SELL YES / BUY NO = -1.
                (CASE WHEN side = 'BUY' THEN 1 ELSE -1 END)
                  * (CASE WHEN LOWER(outcome) = 'yes' THEN 1 ELSE -1 END) AS yes_sign
            FROM wallet_trades
            WHERE condition_id IS NOT NULL
              AND LOWER(outcome) IN ('yes', 'no')
              AND timestamp > $4
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
            COALESCE(SUM(CASE WHEN rt.yes_sign > 0 THEN rt.value ELSE 0 END), 0) AS buy_volume,
            COALESCE(SUM(CASE WHEN rt.yes_sign < 0 THEN rt.value ELSE 0 END), 0) AS sell_volume,
            COALESCE(SUM(rt.yes_sign * rt.value), 0) AS net_flow,
            CASE
                WHEN COALESCE(SUM(rt.value), 0) = 0 THEN 0
                ELSE COALESCE(SUM(rt.yes_sign * rt.value), 0)::numeric / SUM(rt.value)
            END AS imbalance_ratio,
            COUNT(DISTINCT CASE WHEN rt.yes_sign > 0 THEN rt.wallet_address END)::int AS unique_buyers,
            COUNT(DISTINCT CASE WHEN rt.yes_sign < 0 THEN rt.wallet_address END)::int AS unique_sellers,
            COALESCE(
                SUM(
                    CASE
                        WHEN bs.total_score IS NOT NULL AND bs.total_score < $3
                        THEN rt.yes_sign * rt.value
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
    .bind(window_end)
    .bind(window_minutes)
    .bind(smart_money_threshold)
    .bind(window_start)
    .fetch_all(pool)
    .await
}

/// Batch UPSERT rows into `market_flow_features`; returns rows written.
pub(crate) async fn upsert_rows(
    pool: &PgPool,
    rows: &[FlowFeatureRow],
    window_minutes: i32,
) -> usize {
    const BATCH_SIZE: usize = 100;
    let mut total_rows = 0usize;

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

    total_rows
}

/// Row type for the flow aggregation query.
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct FlowFeatureRow {
    pub(crate) condition_id: String,
    pub(crate) window_end: DateTime<Utc>,
    pub(crate) window_minutes: i32,
    pub(crate) buy_volume: rust_decimal::Decimal,
    pub(crate) sell_volume: rust_decimal::Decimal,
    pub(crate) net_flow: rust_decimal::Decimal,
    pub(crate) imbalance_ratio: rust_decimal::Decimal,
    pub(crate) unique_buyers: i32,
    pub(crate) unique_sellers: i32,
    pub(crate) smart_money_flow: rust_decimal::Decimal,
    pub(crate) trade_count: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> FlowFeatureConfig {
        FlowFeatureConfig {
            enabled: true,
            interval_secs: 300,
            windows: vec![15, 60, 240],
            smart_money_threshold: 30,
            retention_days: DEFAULT_RETENTION_DAYS,
        }
    }

    #[test]
    fn test_config_defaults() {
        let config = config();
        assert!(config.enabled);
        assert_eq!(config.windows.len(), 3);
        assert_eq!(config.retention_days, 14);
    }

    #[test]
    fn cycle_boundary_truncates_to_the_bucket() {
        let t = |h, m, s| Utc.with_ymd_and_hms(2025, 3, 1, h, m, s).unwrap();
        assert_eq!(cycle_boundary(t(12, 7, 42), 300), t(12, 5, 0));
        assert_eq!(cycle_boundary(t(12, 5, 0), 300), t(12, 5, 0));
        assert_eq!(cycle_boundary(t(12, 9, 59), 300), t(12, 5, 0));
        assert_eq!(cycle_boundary(t(12, 10, 0), 300), t(12, 10, 0));
        // Sub-second precision is dropped too: two runs in a bucket agree.
        let a = t(12, 7, 0) + chrono::Duration::milliseconds(123);
        let b = t(12, 8, 30) + chrono::Duration::milliseconds(987);
        assert_eq!(cycle_boundary(a, 300), cycle_boundary(b, 300));
        // Degenerate bucket sizes do not panic.
        assert_eq!(cycle_boundary(t(12, 7, 42), 0), t(12, 7, 42));
        assert_eq!(cycle_boundary(t(12, 7, 42), -5), t(12, 7, 42));
    }

    #[test]
    fn bucket_is_bounded_so_window_end_stays_fresh() {
        let mut c = config();
        c.interval_secs = 300;
        assert_eq!(c.bucket_secs(), 60);
        c.interval_secs = 3_600;
        assert_eq!(c.bucket_secs(), 60, "never coarser than one minute");
        c.interval_secs = 20;
        assert_eq!(c.bucket_secs(), 20);
        c.interval_secs = 0;
        assert_eq!(c.bucket_secs(), 1);
    }
}

/// Outcome-aware direction, smart-money definition, idempotent keys, retention.
#[cfg(test)]
mod flow_sql_tests {
    use super::*;
    use crate::signals::flow_signal::direction_from_imbalance;
    use polymarket_core::types::signal::SignalDirection;
    use rust_decimal::Decimal;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    async fn create_tables(pool: &PgPool) {
        sqlx::raw_sql(
            r#"
            CREATE TABLE wallet_trades (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                transaction_hash VARCHAR NOT NULL UNIQUE,
                wallet_address VARCHAR NOT NULL,
                asset_id VARCHAR NOT NULL,
                condition_id VARCHAR,
                side VARCHAR NOT NULL,
                price DECIMAL(20, 10) NOT NULL,
                quantity DECIMAL(20, 10) NOT NULL,
                value DECIMAL(20, 10) NOT NULL,
                timestamp TIMESTAMPTZ NOT NULL,
                outcome TEXT
            );
            CREATE TABLE bot_scores (
                id SERIAL PRIMARY KEY,
                address TEXT NOT NULL,
                total_score INTEGER NOT NULL,
                computed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );
            CREATE TABLE market_flow_features (
                condition_id     TEXT NOT NULL,
                window_end       TIMESTAMPTZ NOT NULL,
                window_minutes   INT NOT NULL,
                buy_volume       DECIMAL(20, 10) DEFAULT 0,
                sell_volume      DECIMAL(20, 10) DEFAULT 0,
                net_flow         DECIMAL(20, 10) DEFAULT 0,
                imbalance_ratio  DECIMAL(10, 6) DEFAULT 0,
                unique_buyers    INT DEFAULT 0,
                unique_sellers   INT DEFAULT 0,
                smart_money_flow DECIMAL(20, 10) DEFAULT 0,
                trade_count      INT DEFAULT 0,
                PRIMARY KEY (condition_id, window_end, window_minutes)
            );
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    fn boundary() -> DateTime<Utc> {
        cycle_boundary(Utc::now(), 300)
    }

    /// Insert a trade at `at`, valued at `value` dollars.
    #[allow(clippy::too_many_arguments)]
    async fn trade(
        pool: &PgPool,
        n: usize,
        wallet: &str,
        condition: &str,
        side: &str,
        outcome: Option<&str>,
        value: &str,
        at: DateTime<Utc>,
    ) {
        sqlx::query(
            "INSERT INTO wallet_trades (transaction_hash, wallet_address, asset_id, condition_id, \
             side, price, quantity, value, timestamp, outcome) \
             VALUES ($1, $2, 'asset', $3, $4, 0.5, 1, $5::numeric, $6, $7)",
        )
        .bind(format!("0xtx{n}"))
        .bind(wallet)
        .bind(condition)
        .bind(side)
        .bind(value)
        .bind(at)
        .bind(outcome)
        .execute(pool)
        .await
        .unwrap();
    }

    fn minutes(n: i64) -> chrono::Duration {
        chrono::Duration::minutes(n)
    }

    async fn run(pool: &PgPool, window_end: DateTime<Utc>) -> Vec<FlowFeatureRow> {
        aggregate_window(pool, window_end, 60, 30).await.unwrap()
    }

    fn market<'a>(rows: &'a [FlowFeatureRow], condition: &str) -> &'a FlowFeatureRow {
        rows.iter()
            .find(|r| r.condition_id == condition)
            .unwrap_or_else(|| panic!("no row for {condition}"))
    }

    #[sqlx::test(migrations = false)]
    async fn flow_is_signed_by_the_outcome_token_traded(pool: PgPool) {
        create_tables(&pool).await;
        let end = boundary();
        let at = end - minutes(5);
        trade(&pool, 1, "0xa", "c1", "BUY", Some("Yes"), "100", at).await; // bullish
        trade(&pool, 2, "0xb", "c1", "BUY", Some("No"), "100", at).await; // bearish
        trade(&pool, 3, "0xc", "c1", "SELL", Some("No"), "50", at).await; // bullish
        trade(&pool, 4, "0xd", "c1", "SELL", Some("Yes"), "20", at).await; // bearish

        let rows = run(&pool, end).await;
        let row = market(&rows, "c1");
        assert_eq!(row.buy_volume, d("150"), "BUY YES + SELL NO");
        assert_eq!(row.sell_volume, d("120"), "BUY NO + SELL YES");
        assert_eq!(row.net_flow, d("30"));
        assert!((row.imbalance_ratio - d("0.1111")).abs() < d("0.0001"));
        assert_eq!((row.unique_buyers, row.unique_sellers), (2, 2));
        assert_eq!(row.trade_count, 4);
        // The old BUY/SELL-only logic reported +130 net: every NO buy counted
        // as bullish YES flow.
    }

    #[sqlx::test(migrations = false)]
    async fn buying_no_alone_is_a_buy_no_signal(pool: PgPool) {
        create_tables(&pool).await;
        let end = boundary();
        for (i, wallet) in ["0xa", "0xb", "0xc"].iter().enumerate() {
            trade(
                &pool,
                i,
                wallet,
                "c1",
                "BUY",
                Some("No"),
                "200",
                end - minutes(3),
            )
            .await;
        }
        let rows = run(&pool, end).await;
        let row = market(&rows, "c1");
        assert_eq!(row.net_flow, d("-600"));
        assert_eq!(row.imbalance_ratio, d("-1"));
        assert_eq!(row.buy_volume, Decimal::ZERO);
        assert_eq!(
            direction_from_imbalance(row.imbalance_ratio),
            SignalDirection::BuyNo,
            "NO buying must never read as YES demand"
        );

        // And YES buying is a BuyYes.
        trade(
            &pool,
            10,
            "0xd",
            "c2",
            "BUY",
            Some("YES"),
            "300",
            end - minutes(2),
        )
        .await;
        trade(
            &pool,
            11,
            "0xe",
            "c2",
            "BUY",
            Some("yes"),
            "100",
            end - minutes(2),
        )
        .await;
        let rows = run(&pool, end).await;
        assert_eq!(
            direction_from_imbalance(market(&rows, "c2").imbalance_ratio),
            SignalDirection::BuyYes
        );
    }

    #[sqlx::test(migrations = false)]
    async fn trades_with_unknown_outcomes_are_skipped(pool: PgPool) {
        create_tables(&pool).await;
        let end = boundary();
        let at = end - minutes(5);
        trade(&pool, 1, "0xa", "c1", "BUY", Some("Yes"), "100", at).await;
        trade(&pool, 2, "0xb", "c1", "BUY", Some("Yes"), "100", at).await;
        // Unknown / non-binary outcomes: ignored rather than guessed.
        trade(&pool, 3, "0xc", "c1", "BUY", None, "1000", at).await;
        trade(&pool, 4, "0xd", "c1", "SELL", Some(""), "1000", at).await;
        trade(&pool, 5, "0xe", "c1", "BUY", Some("Up"), "1000", at).await;
        // A market with a single determinable trade has too little data.
        trade(&pool, 6, "0xf", "c2", "BUY", Some("No"), "100", at).await;
        trade(&pool, 7, "0xg", "c2", "BUY", None, "100", at).await;
        trade(&pool, 8, "0xh", "c2", "BUY", None, "100", at).await;

        let rows = run(&pool, end).await;
        assert_eq!(rows.len(), 1);
        let row = market(&rows, "c1");
        assert_eq!(row.trade_count, 2);
        assert_eq!(row.net_flow, d("200"));
        assert_eq!(row.imbalance_ratio, d("1"));
    }

    #[sqlx::test(migrations = false)]
    async fn unscored_wallets_are_not_smart_money(pool: PgPool) {
        create_tables(&pool).await;
        let end = boundary();
        let at = end - minutes(5);
        // smart: scored below the threshold; unscored: unknown; bot: scored above.
        for (wallet, score) in [("0xsmart", 10), ("0xbot", 80)] {
            sqlx::query("INSERT INTO bot_scores (address, total_score) VALUES ($1, $2)")
                .bind(wallet)
                .bind(score)
                .execute(&pool)
                .await
                .unwrap();
        }
        // An old score must not override the latest one.
        sqlx::query(
            "INSERT INTO bot_scores (address, total_score, computed_at) \
             VALUES ('0xsmart', 90, NOW() - INTERVAL '3 days')",
        )
        .execute(&pool)
        .await
        .unwrap();
        trade(&pool, 1, "0xsmart", "c1", "BUY", Some("Yes"), "100", at).await;
        trade(&pool, 2, "0xnew", "c1", "BUY", Some("Yes"), "100", at).await;
        trade(&pool, 3, "0xbot", "c1", "BUY", Some("Yes"), "100", at).await;

        let rows = run(&pool, end).await;
        let row = market(&rows, "c1");
        assert_eq!(row.net_flow, d("300"));
        assert_eq!(
            row.smart_money_flow,
            d("100"),
            "only the scored, below-threshold wallet (the old query also counted the unscored one: 200)"
        );
    }

    #[sqlx::test(migrations = false)]
    async fn smart_money_flow_is_signed_like_the_rest(pool: PgPool) {
        create_tables(&pool).await;
        let end = boundary();
        let at = end - minutes(5);
        sqlx::query("INSERT INTO bot_scores (address, total_score) VALUES ('0xsmart', 5)")
            .execute(&pool)
            .await
            .unwrap();
        // A smart wallet buying NO is smart money going bearish; selling NO is bullish.
        trade(&pool, 1, "0xsmart", "c1", "BUY", Some("No"), "60", at).await;
        trade(&pool, 2, "0xother", "c1", "BUY", Some("Yes"), "10", at).await;
        trade(&pool, 3, "0xsmart", "c2", "SELL", Some("No"), "40", at).await;
        trade(&pool, 4, "0xother", "c2", "BUY", Some("Yes"), "10", at).await;

        let rows = run(&pool, end).await;
        assert_eq!(market(&rows, "c1").smart_money_flow, d("-60"));
        assert_eq!(market(&rows, "c2").smart_money_flow, d("40"));
    }

    #[sqlx::test(migrations = false)]
    async fn window_is_half_open_and_ends_at_the_boundary(pool: PgPool) {
        create_tables(&pool).await;
        let end = boundary();
        trade(&pool, 1, "0xa", "c1", "BUY", Some("Yes"), "10", end).await; // included
        trade(
            &pool,
            2,
            "0xb",
            "c1",
            "BUY",
            Some("Yes"),
            "10",
            end - minutes(30),
        )
        .await;
        trade(
            &pool,
            3,
            "0xc",
            "c1",
            "BUY",
            Some("Yes"),
            "10",
            end - minutes(60),
        )
        .await; // excluded
        trade(
            &pool,
            4,
            "0xd",
            "c1",
            "BUY",
            Some("Yes"),
            "10",
            end + minutes(1),
        )
        .await; // future
        let rows = run(&pool, end).await;
        assert_eq!(market(&rows, "c1").trade_count, 2);
    }

    async fn feature_count(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM market_flow_features")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = false)]
    async fn rerunning_in_the_same_bucket_upserts_instead_of_inserting(pool: PgPool) {
        create_tables(&pool).await;
        let end = boundary();
        let at = end - minutes(5);
        trade(&pool, 1, "0xa", "c1", "BUY", Some("Yes"), "100", at).await;
        trade(&pool, 2, "0xb", "c1", "BUY", Some("Yes"), "100", at).await;

        // Two runs landing in the same one-minute bucket compute the same key
        // (the old code used the exact `now()`, so every run added new rows).
        let bucket = FlowFeatureConfig {
            enabled: true,
            interval_secs: 300,
            windows: vec![60],
            smart_money_threshold: 30,
            retention_days: DEFAULT_RETENTION_DAYS,
        }
        .bucket_secs();
        let first_cycle = cycle_boundary(end + chrono::Duration::seconds(5), bucket);
        let second_cycle = cycle_boundary(end + chrono::Duration::seconds(50), bucket);
        assert_eq!(first_cycle, second_cycle);

        let rows = run(&pool, first_cycle).await;
        assert_eq!(upsert_rows(&pool, &rows, 60).await, 1);
        assert_eq!(feature_count(&pool).await, 1);

        // A late-arriving trade inside the window changes the values, not the key.
        trade(&pool, 3, "0xc", "c1", "BUY", Some("No"), "50", at).await;
        let rows = run(&pool, second_cycle).await;
        upsert_rows(&pool, &rows, 60).await;
        assert_eq!(feature_count(&pool).await, 1);
        let net: Decimal = sqlx::query_scalar(
            "SELECT net_flow FROM market_flow_features WHERE condition_id = 'c1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(net, d("150"));

        // The next bucket is a new row, and consumers read the latest one.
        let next = first_cycle + minutes(5);
        trade(
            &pool,
            4,
            "0xd",
            "c1",
            "SELL",
            Some("Yes"),
            "10",
            next - minutes(1),
        )
        .await;
        let rows = run(&pool, next).await;
        upsert_rows(&pool, &rows, 60).await;
        assert_eq!(feature_count(&pool).await, 2);
        let latest_end: DateTime<Utc> = sqlx::query_scalar(
            "SELECT window_end FROM market_flow_features \
             WHERE condition_id = 'c1' AND window_minutes = 60 ORDER BY window_end DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(latest_end, next);
    }

    #[sqlx::test(migrations = false)]
    async fn old_features_are_pruned_and_recent_ones_kept(pool: PgPool) {
        create_tables(&pool).await;
        for (condition, days) in [("c1", 1), ("c2", 10), ("c3", 20), ("c4", 45)] {
            sqlx::query(
                "INSERT INTO market_flow_features (condition_id, window_end, window_minutes) \
                 VALUES ($1, NOW() - make_interval(days => $2::int), 60)",
            )
            .bind(condition)
            .bind(days)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Plain PostgreSQL: drop_chunks does not exist, so rows are deleted.
        prune_old_features(&pool, Utc::now() - chrono::Duration::days(14))
            .await
            .unwrap();

        let remaining: Vec<String> = sqlx::query_scalar(
            "SELECT condition_id FROM market_flow_features ORDER BY condition_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, vec!["c1", "c2"]);
        // Safe to repeat.
        prune_old_features(&pool, Utc::now() - chrono::Duration::days(14))
            .await
            .unwrap();
        assert_eq!(feature_count(&pool).await, 2);
    }
}
