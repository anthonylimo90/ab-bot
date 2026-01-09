//! Database access layer for PostgreSQL/TimescaleDB.

pub mod positions;
pub mod wallets;

use crate::config::DatabaseConfig;
use crate::Result;
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::path::Path;

/// Create a PostgreSQL connection pool.
pub async fn create_pool(config: &DatabaseConfig) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .connect(&config.url)
        .await?;

    Ok(pool)
}

/// Run database migrations from the migrations directory.
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    // Use sqlx's migrator for runtime migrations
    let migrator = sqlx::migrate::Migrator::new(Path::new("./migrations")).await?;
    migrator.run(pool).await?;
    Ok(())
}
