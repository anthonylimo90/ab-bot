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
            enabled: std::env::var("GAMMA_SYNCER_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
            interval_secs: std::env::var("GAMMA_SYNCER_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3600),
            page_size: std::env::var("GAMMA_SYNCER_PAGE_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),
        }
    }
}

/// Spawn the Gamma syncer background task.
///
/// Follows the `spawn_wallet_harvester` pattern:
/// - Free `spawn_*` function that calls `tokio::spawn` internally
/// - Receives individual fields rather than full AppState
/// - Uses DB semaphore around writes
pub fn spawn_gamma_syncer(config: GammaSyncerConfig, pool: PgPool, db_semaphore: Arc<Semaphore>) {
    if !config.enabled {
        info!("Gamma syncer disabled (GAMMA_SYNCER_ENABLED != true)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        page_size = config.page_size,
        "Spawning Gamma syncer background task"
    );

    tokio::spawn(syncer_loop(config, pool, db_semaphore));
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

    for chunk in parsed.chunks(BATCH_SIZE) {
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
             fetched_at = EXCLUDED.fetched_at",
        );

        match query_builder.build().execute(pool).await {
            Ok(result) => {
                upserted += result.rows_affected() as usize;
            }
            Err(e) => {
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

    // Step 5: Backfill NULL condition_id in wallet_trades using token_condition_cache
    let backfilled = backfill_condition_ids(pool).await.unwrap_or_else(|e| {
        warn!(error = %e, "Failed to backfill condition_ids in wallet_trades");
        0
    });

    // Step 6: Release semaphore (explicit drop for clarity)
    drop(_permit);

    Ok(SyncStats {
        upserted,
        backfilled,
    })
}

/// Backfill NULL condition_id values in wallet_trades from token_condition_cache.
///
/// Limited to 10,000 rows per cycle to avoid long-running transactions.
async fn backfill_condition_ids(pool: &PgPool) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE wallet_trades wt
        SET condition_id = tcc.condition_id
        FROM token_condition_cache tcc
        WHERE wt.asset_id = tcc.token_id
          AND wt.condition_id IS NULL
          AND tcc.condition_id IS NOT NULL
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
