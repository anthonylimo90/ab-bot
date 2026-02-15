//! API Server binary entrypoint.

use api_server::{ApiServer, ServerConfig};
use clap::{Parser, Subcommand};
use polymarket_core::config::DatabaseConfig;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod seed;

/// Polymarket Trading API Server
#[derive(Parser)]
#[command(name = "api-server")]
#[command(about = "REST and WebSocket API for the Polymarket trading platform")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the API server (default)
    Serve,

    /// Seed the initial platform admin user
    SeedAdmin {
        /// Admin email address
        #[arg(long)]
        email: String,

        /// Admin password (min 8 characters)
        #[arg(long)]
        password: String,
    },
}

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

    let cli = Cli::parse();

    // Create database connection pool with retry
    let db_config = DatabaseConfig {
        url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
        max_connections: std::env::var("DATABASE_MAX_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(20),
        max_retries: std::env::var("DB_RETRY_MAX_ATTEMPTS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5),
        retry_base_delay_ms: std::env::var("DB_RETRY_BASE_DELAY_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1000),
        retry_max_delay_ms: std::env::var("DB_RETRY_MAX_DELAY_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30000),
        acquire_timeout_secs: Some(
            std::env::var("DB_ACQUIRE_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
        ),
    };
    let pool = polymarket_core::db::create_pool(&db_config).await?;

    // Run migrations
    let skip_migrations = std::env::var("SKIP_MIGRATIONS")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    if !skip_migrations {
        tracing::info!("Running database migrations...");
        sqlx::migrate!("../../migrations").run(&pool).await?;
    }

    match cli.command {
        Some(Commands::SeedAdmin { email, password }) => {
            seed::seed_admin(&pool, &email, &password).await?;
        }
        Some(Commands::Serve) | None => {
            run_server(pool).await?;
        }
    }

    Ok(())
}

async fn run_server(pool: sqlx::PgPool) -> anyhow::Result<()> {
    tracing::info!("API Server starting up...");

    // Validate JWT_SECRET for security
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_default();
    if jwt_secret.is_empty() || jwt_secret == "development-secret-change-in-production" {
        tracing::error!("JWT_SECRET must be set to a secure value (not the default)");
        tracing::error!("Generate a secure secret: openssl rand -base64 32");
        anyhow::bail!("JWT_SECRET environment variable must be set to a secure value");
    }
    if jwt_secret.len() < 32 {
        tracing::error!(
            "JWT_SECRET must be at least 32 characters long (current: {})",
            jwt_secret.len()
        );
        anyhow::bail!("JWT_SECRET must be at least 32 characters long");
    }
    tracing::info!("JWT secret validation passed");

    tracing::info!("Database connection established");

    // Try to seed admin from environment (for first-time setup via Docker)
    if let Err(e) = seed::seed_admin_from_env(&pool).await {
        // Only log as debug if admin already exists, error for other issues
        if e.to_string().contains("already exists") {
            tracing::debug!("Admin user already exists, skipping seed");
        } else {
            tracing::warn!("Failed to seed admin from env: {}", e);
        }
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
