//! API Server binary entrypoint.

use api_server::{ApiServer, ServerConfig};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load environment variables from .env file
    dotenvy::dotenv().ok();

    // Initialize tracing with production-friendly defaults
    // Filter out noisy crates to avoid hitting Railway's 500 logs/sec limit
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "api_server=info,tower_http=error,polymarket_core=warn,auth=info,sqlx=warn,hyper=warn,tungstenite=warn,h2=warn".into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("API Server starting up...");

    // Validate JWT_SECRET for security
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_default();
    if jwt_secret.is_empty() || jwt_secret == "development-secret-change-in-production" {
        tracing::error!("JWT_SECRET must be set to a secure value (not the default)");
        tracing::error!("Generate a secure secret: openssl rand -base64 32");
        anyhow::bail!("JWT_SECRET environment variable must be set to a secure value");
    }
    if jwt_secret.len() < 32 {
        tracing::error!("JWT_SECRET must be at least 32 characters long (current: {})", jwt_secret.len());
        anyhow::bail!("JWT_SECRET must be at least 32 characters long");
    }
    tracing::info!("JWT secret validation passed");

    // Get database URL
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    tracing::info!("Connecting to database...");

    // Create database connection pool
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(std::time::Duration::from_secs(30))
        .connect(&database_url)
        .await?;

    tracing::info!("Database connection established");

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
    tracing::info!(
        host = %config.host,
        port = %config.port,
        "Server configuration loaded"
    );

    // Create and run server
    tracing::info!("Starting API server...");
    let server = ApiServer::new(config, pool).await?;
    server.run().await?;

    Ok(())
}
