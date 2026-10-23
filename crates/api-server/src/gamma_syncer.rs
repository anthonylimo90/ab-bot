//! Gamma API market metadata syncer.
//!
//! Background task that periodically fetches market metadata from the Polymarket
//! Gamma API and upserts it into `market_metadata`. Also backfills NULL
//! `condition_id` values in `wallet_trades` using the `token_condition_cache`.

use polymarket_core::api::gamma::GammaClient;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

/// Configuration for the Gamma syncer background task.
#[derive(Debug, Clone)]
pub struct GammaSyncerConfig {
    /// Whether the syncer is enabled.
    pub enabled: bool,
    /// Interval between sync cycles in seconds.
    pub interval_secs: u64,
    /// Page size for Gamma API pagination.
    pub page_size: u32,
}

impl GammaSyncerConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        Self {
            enabled: polymarket_core::config::env_flag("GAMMA_SYNCER_ENABLED", true),
            interval_secs: polymarket_core::config::env_count("GAMMA_SYNCER_INTERVAL_SECS", 3600),
            page_size: polymarket_core::config::env_count("GAMMA_SYNCER_PAGE_SIZE", 100),
        }
    }
}

/// Spawn the Gamma syncer background task.
///
/// Spawns two independent loops:
/// - API sync loop: fetches Gamma market metadata every `interval_secs` (default 1h).
/// - Backfill loop: resolves `wallet_trades.condition_id IS NULL` every 5 minutes
///   regardless of Gamma API availability. Decoupling ensures the flow feature
///   calculator has live data every cycle instead of waiting up to 59 minutes
///   for the next hourly API sync to trigger the backfill.
pub fn spawn_gamma_syncer(config: GammaSyncerConfig, pool: PgPool, db_semaphore: Arc<Semaphore>) {
    if !config.enabled {
        info!("Gamma syncer disabled (GAMMA_SYNCER_ENABLED is off)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        page_size = config.page_size,
        "Spawning Gamma syncer background task"
    );

    // Hourly Gamma API sync
    tokio::spawn(syncer_loop(config, pool.clone(), db_semaphore));

    // 5-minute condition_id backfill (no API dependency, no semaphore needed)
    tokio::spawn(backfill_loop(pool));
}

/// Standalone backfill loop. Resolves wallet_trades.condition_id IS NULL rows
/// using token_condition_cache every 5 minutes. When the backlog is empty the
/// query hits idx_wallet_trades_null_condition (partial index) and returns
/// instantly — negligible overhead.
async fn backfill_loop(pool: PgPool) {
    // Short startup delay to let the hourly sync populate token_condition_cache
    tokio::time::sleep(Duration::from_secs(30)).await;

    let mut interval = tokio::time::interval(Duration::from_secs(300));

    // After an overrun, wait a full period instead of catching up.

    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await; // consume the first immediate tick

    loop {
        interval.tick().await;
        match backfill_condition_ids(&pool).await {
            Ok(0) => {} // backlog empty — silent no-op
            Ok(n) => info!(rows = n, "Backfilled condition_ids in wallet_trades"),
            Err(e) => debug!(error = %e, "condition_id backfill failed (non-fatal)"),
        }
    }
}

/// Main syncer loop. Runs until the process shuts down.
async fn syncer_loop(config: GammaSyncerConfig, pool: PgPool, db_semaphore: Arc<Semaphore>) {
    let interval = Duration::from_secs(config.interval_secs);
    let gamma_client = GammaClient::new(None);

    // Startup delay to let the server finish booting
    tokio::time::sleep(Duration::from_secs(15)).await;

    loop {
        match sync_cycle(&gamma_client, &pool, &db_semaphore, config.page_size).await {
            Ok(stats) => {
                info!(
                    upserted = stats.upserted,
                    backfilled = stats.backfilled,
                    "Gamma sync cycle completed"
                );
            }
            Err(e) => {
                warn!(error = %e, "Gamma sync cycle failed");
            }
        }

        tokio::time::sleep(interval).await;
    }
}

/// Stats from a single sync cycle.
struct SyncStats {
    upserted: usize,
    backfilled: i64,
}

/// Execute a single sync cycle: fetch from Gamma API and upsert to DB.
async fn sync_cycle(
    gamma_client: &GammaClient,
    pool: &PgPool,
    db_semaphore: &Semaphore,
    page_size: u32,
) -> Result<SyncStats, Box<dyn std::error::Error + Send + Sync>> {
    // Step 1: Fetch all active markets from Gamma API (no semaphore — network I/O)
    let markets = gamma_client.get_all_markets(page_size).await?;
    let total_fetched = markets.len();
    debug!(count = total_fetched, "Fetched markets from Gamma API");

    if markets.is_empty() {
        return Ok(SyncStats {
            upserted: 0,
            backfilled: 0,
        });
    }

    // Step 2: Parse markets into storage-ready structs
    let parsed: Vec<_> = markets
        .into_iter()
        .map(polymarket_core::api::gamma::ParsedGammaMarket::from)
        .collect();

    // Step 3: Acquire semaphore before DB writes
    let _permit = db_semaphore.acquire().await.expect("semaphore closed");

    // Step 4: Batch UPSERT into market_metadata (chunks of 50 to stay under param limit)
    const BATCH_SIZE: usize = 50;
    let mut upserted = 0usize;
    let mut all_stored = true;

    for chunk in parsed.chunks(BATCH_SIZE) {
        match upsert_market_metadata(pool, chunk).await {
            Ok(changed) => {
                upserted += changed as usize;
            }
            Err(e) => {
                all_stored = false;
                warn!(
                    error = %e,
                    chunk_size = chunk.len(),
                    "Failed to upsert market_metadata batch"
                );
            }
        }

        // Yield between chunks to prevent blocking
        tokio::task::yield_now().await;
    }

    // Markets that left the active listing are closed: without this their
    // rows stayed active for good, and the signal generators reading
    // `active` kept treating resolved markets as tradable.
    if all_stored {
        if let Err(error) = deactivate_unlisted(pool, &parsed).await {
            warn!(%error, "Failed to mark closed markets inactive in market_metadata");
        }
    }

    // Step 5: Release semaphore (explicit drop for clarity)
    drop(_permit);

    // Backfill is now handled by the dedicated backfill_loop (every 5 min)
    Ok(SyncStats {
        upserted,
        backfilled: 0,
    })
}

/// Marks active rows missing from a complete listing inactive, unless the
/// listing is under half the active rows: more likely a partial answer than
/// half the markets closing at once.
async fn deactivate_unlisted(
    pool: &PgPool,
    listed: &[polymarket_core::api::gamma::ParsedGammaMarket],
) -> Result<(), sqlx::Error> {
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM market_metadata WHERE active")
        .fetch_one(pool)
        .await?;
    if (listed.len() as i64) * 2 < active {
        warn!(
            listed = listed.len(),
            active, "Gamma listing is under half the active markets; not marking any closed"
        );
        return Ok(());
    }
    let ids: Vec<&str> = listed.iter().map(|m| m.condition_id.as_str()).collect();
    let closed = sqlx::query(
        "UPDATE market_metadata SET active = FALSE WHERE active AND NOT (condition_id = ANY($1))",
    )
    .bind(ids)
    .execute(pool)
    .await?
    .rows_affected();
    if closed > 0 {
        debug!(
            closed,
            "Marked markets that left the Gamma listing inactive"
        );
    }
    Ok(())
}

/// Upsert a batch of market metadata, rewriting only rows whose values
/// changed; returns how many rows were written.
async fn upsert_market_metadata(
    pool: &PgPool,
    chunk: &[polymarket_core::api::gamma::ParsedGammaMarket],
) -> Result<u64, sqlx::Error> {
    let mut query_builder: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "INSERT INTO market_metadata (condition_id, question, category, tags, end_date, volume, liquidity, active, fetched_at) ",
    );

    query_builder.push_values(chunk, |mut b, market| {
        b.push_bind(&market.condition_id)
            .push_bind(&market.question)
            .push_bind(&market.category)
            .push_bind(&market.tags)
            .push_bind(market.end_date)
            .push_bind(market.volume)
            .push_bind(market.liquidity)
            .push_bind(market.active)
            .push_bind(chrono::Utc::now());
    });

    query_builder.push(
        " ON CONFLICT (condition_id) DO UPDATE SET \
         question = EXCLUDED.question, \
         category = EXCLUDED.category, \
         tags = EXCLUDED.tags, \
         end_date = EXCLUDED.end_date, \
         volume = EXCLUDED.volume, \
         liquidity = EXCLUDED.liquidity, \
         active = EXCLUDED.active, \
         fetched_at = EXCLUDED.fetched_at \
         WHERE (market_metadata.question, market_metadata.category, market_metadata.tags, \
                market_metadata.end_date, market_metadata.volume, market_metadata.liquidity, \
                market_metadata.active) \
               IS DISTINCT FROM \
               (EXCLUDED.question, EXCLUDED.category, EXCLUDED.tags, EXCLUDED.end_date, \
                EXCLUDED.volume, EXCLUDED.liquidity, EXCLUDED.active)",
    );
    Ok(query_builder.build().execute(pool).await?.rows_affected())
}

/// Backfill NULL condition_id values in wallet_trades from token_condition_cache.
///
/// Limited to 10,000 rows per cycle to avoid long-running transactions.
async fn backfill_condition_ids(pool: &PgPool) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE wallet_trades wt
        SET condition_id = pick.condition_id
        FROM (
            SELECT t.transaction_hash, t.timestamp, c.condition_id
            FROM wallet_trades t
            JOIN token_condition_cache c ON c.token_id = t.asset_id
            WHERE t.condition_id IS NULL
              AND c.condition_id IS NOT NULL
            LIMIT 10000
        ) pick
        WHERE wt.transaction_hash = pick.transaction_hash
          AND wt.timestamp = pick.timestamp
          AND wt.condition_id IS NULL
        "#,
    )
    .execute(pool)
    .await?;

    let rows = result.rows_affected() as i64;
    if rows > 0 {
        debug!(rows = rows, "Backfilled condition_ids in wallet_trades");
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    #[sqlx::test(migrations = false)]
    async fn unchanged_market_metadata_is_not_rewritten(pool: PgPool) {
        sqlx::raw_sql(include_str!("../../../migrations/038_market_metadata.sql"))
            .execute(&pool)
            .await
            .unwrap();
        let market = |volume: &str| polymarket_core::api::gamma::ParsedGammaMarket {
            condition_id: "c1".into(),
            question: "Will it?".into(),
            category: Some("crypto".into()),
            tags: vec!["btc".into()],
            end_date: None,
            // More places than the column keeps: compared after rounding.
            volume: volume.parse().unwrap(),
            liquidity: Decimal::ONE,
            active: true,
        };
        let upsert = |volume| {
            let pool = pool.clone();
            async move {
                upsert_market_metadata(&pool, &[market(volume)])
                    .await
                    .unwrap()
            }
        };
        assert_eq!(upsert("12.345678901234").await, 1);
        assert_eq!(upsert("12.345678901234").await, 0);
        assert_eq!(upsert("13").await, 1);
    }

    #[sqlx::test(migrations = false)]
    async fn markets_that_leave_the_listing_become_inactive(pool: PgPool) {
        sqlx::raw_sql(include_str!("../../../migrations/038_market_metadata.sql"))
            .execute(&pool)
            .await
            .unwrap();
        let market = |id: &str| polymarket_core::api::gamma::ParsedGammaMarket {
            condition_id: id.into(),
            question: "Will it?".into(),
            category: None,
            tags: Vec::new(),
            end_date: None,
            volume: Decimal::ONE,
            liquidity: Decimal::ONE,
            active: true,
        };
        let listing: Vec<_> = ["c1", "c2", "c3"].into_iter().map(market).collect();
        upsert_market_metadata(&pool, &listing).await.unwrap();
        let active = |pool: PgPool| async move {
            sqlx::query_scalar::<_, String>(
                "SELECT condition_id FROM market_metadata WHERE active ORDER BY condition_id",
            )
            .fetch_all(&pool)
            .await
            .unwrap()
        };

        // c3 closed: it leaves the listing.
        deactivate_unlisted(&pool, &listing[..2]).await.unwrap();
        assert_eq!(active(pool.clone()).await, ["c1", "c2"]);
        // A listing under half the active markets marks nothing.
        deactivate_unlisted(&pool, &[]).await.unwrap();
        assert_eq!(active(pool.clone()).await, ["c1", "c2"]);
    }

    #[sqlx::test(migrations = false)]
    async fn backfill_fills_missing_condition_ids_a_bounded_batch_at_a_time(pool: PgPool) {
        sqlx::raw_sql(
            "CREATE TABLE wallet_trades (transaction_hash TEXT, timestamp TIMESTAMPTZ, \
                 asset_id TEXT, condition_id TEXT, UNIQUE (transaction_hash, timestamp)); \
             CREATE TABLE token_condition_cache (token_id TEXT PRIMARY KEY, condition_id TEXT); \
             INSERT INTO token_condition_cache VALUES ('t1', 'c1'), ('t2', NULL); \
             INSERT INTO wallet_trades \
                 SELECT 'h' || n, NOW() - make_interval(secs => n), 't1', NULL \
                 FROM generate_series(1, 10005) n; \
             INSERT INTO wallet_trades VALUES \
                 ('h1', NOW() - INTERVAL '1 day', 't2', NULL), \
                 ('h2', NOW() - INTERVAL '1 day', 't1', 'kept')",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(backfill_condition_ids(&pool).await.unwrap(), 10_000);
        assert_eq!(backfill_condition_ids(&pool).await.unwrap(), 5);
        assert_eq!(backfill_condition_ids(&pool).await.unwrap(), 0);
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT asset_id, condition_id FROM wallet_trades \
             WHERE condition_id IS DISTINCT FROM 'c1' ORDER BY asset_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        // Unknown tokens stay unfilled; existing ids are never overwritten.
        assert_eq!(
            rows,
            vec![
                ("t1".to_string(), Some("kept".to_string())),
                ("t2".to_string(), None)
            ]
        );
    }

    #[test]
    fn test_config_defaults() {
        let config = GammaSyncerConfig {
            enabled: true,
            interval_secs: 3600,
            page_size: 100,
        };
        assert!(config.enabled);
        assert_eq!(config.interval_secs, 3600);
        assert_eq!(config.page_size, 100);
    }
}
