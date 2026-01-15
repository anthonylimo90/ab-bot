//! API Server binary entrypoint.

use api_server::{ApiServer, ServerConfig};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load environment variables from .env file
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "api_server=debug,tower_http=debug,axum=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Get database URL
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    // Create database connection pool
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&database_url)
        .await?;

    // Run migrations (can be disabled via SKIP_MIGRATIONS=true for manual migration management)
    let skip_migrations = std::env::var("SKIP_MIGRATIONS")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    if !skip_migrations {
        tracing::info!("Running database migrations...");
        sqlx::migrate!("../../migrations").run(&pool).await?;
    } else {
        tracing::info!("Skipping migrations (SKIP_MIGRATIONS=true)");
    }

    // Create server config from environment
    let config = ServerConfig::from_env();

    // Create and run server
    let server = ApiServer::new(config, pool).await?;
    server.run().await?;

    Ok(())
}
