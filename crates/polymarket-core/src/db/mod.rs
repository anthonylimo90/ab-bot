//! Database access layer for PostgreSQL/TimescaleDB.

pub mod positions;
pub mod wallets;

use crate::config::DatabaseConfig;
use crate::Result;
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::path::Path;
use std::time::Duration;
use tracing::{error, info, warn};

/// Create a PostgreSQL connection pool with retry and exponential backoff.
pub async fn create_pool(config: &DatabaseConfig) -> Result<PgPool> {
    let mut last_error = None;

    for attempt in 0..=config.max_retries {
        let mut opts = PgPoolOptions::new().max_connections(config.max_connections);

        if let Some(timeout_secs) = config.acquire_timeout_secs {
            opts = opts.acquire_timeout(Duration::from_secs(timeout_secs));
        }

        match opts.connect(&config.url).await {
            Ok(pool) => {
                if attempt > 0 {
                    info!(
                        attempt = attempt + 1,
                        "Database connection established after retry"
                    );
                }
                return Ok(pool);
            }
            Err(e) => {
                if attempt < config.max_retries {
                    let delay_ms = std::cmp::min(
                        config.retry_base_delay_ms * 2u64.pow(attempt),
                        config.retry_max_delay_ms,
                    );
                    warn!(
                        attempt = attempt + 1,
                        max_retries = config.max_retries + 1,
                        delay_ms = delay_ms,
                        error = %e,
                        "Database connection failed, retrying after backoff"
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                } else {
                    error!(
                        attempts = config.max_retries + 1,
                        error = %e,
                        "Database connection failed after all retries"
                    );
                }
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap().into())
}

/// Run database migrations from the migrations directory.
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    // Use sqlx's migrator for runtime migrations
    let migrator = sqlx::migrate::Migrator::new(Path::new("./migrations")).await?;
    migrator.run(pool).await?;
    Ok(())
}
