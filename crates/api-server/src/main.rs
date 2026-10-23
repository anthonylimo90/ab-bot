//! API Server binary entrypoint.

#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use api_server::{is_development_environment, ApiServer, ServerConfig};
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
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            // Every crate's warnings and errors, and the trading crates'
            // decisions: a target left out of the filter is not logged
            // at all, errors included.
            "warn,api_server=info,auth=info,risk_manager=info,trading_engine=info,tower_http=error"
                .into()
        }))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();
    let serving = matches!(cli.command, Some(Commands::Serve) | None);

    // A setting that cannot be read stops the start: run on a default the
    // operator did not choose, a service meant off could trade.
    if serving {
        let unreadable = polymarket_core::config::unreadable_settings();
        if !unreadable.is_empty() {
            anyhow::bail!(
                "Settings that cannot be read (correct or unset them): {}",
                unreadable.join("; ")
            );
        }
        use polymarket_core::config::env_switch;
        tracing::info!(
            live_trading = env_switch("LIVE_TRADING"),
            arb_auto_execute = env_switch("ARB_AUTO_EXECUTE"),
            arb_allow_live_execution = env_switch("ARB_ALLOW_LIVE_EXECUTION"),
            quant_executor_enabled = env_switch("QUANT_EXECUTOR_ENABLED"),
            exit_handler_enabled = env_switch("EXIT_HANDLER_ENABLED"),
            latency_arb_enabled = env_switch("LATENCY_ARB_ENABLED"),
            cross_market_signal_enabled = env_switch("CROSS_MARKET_SIGNAL_ENABLED"),
            "Trading switches set in the environment (the workspace's settings can also \
             switch services on)"
        );
    }

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
                .unwrap_or(5),
        ),
    };
    // The server waits for a database that is down at start rather than
    // exiting: each exit spends one of the platform's restarts, and a long
    // outage would spend them all, leaving the service down once the
    // database is back. A one-off command gives up as before.
    // As PID 1 the process ignores SIGTERM unless it listens: a stop while
    // waiting ends the wait.
    // An unreadable URL is a configuration error, not an outage: no wait.
    db_config
        .url
        .parse::<sqlx::postgres::PgConnectOptions>()
        .map_err(|error| anyhow::anyhow!("DATABASE_URL cannot be read: {error}"))?;
    let mut stop = polymarket_core::shutdown::StopSignals::install()?;
    let pool = loop {
        tokio::select! {
            created = polymarket_core::db::create_pool(&db_config) => match created {
                Ok(pool) => break pool,
                Err(error) if serving => {
                    tracing::error!(%error, "The database is unavailable at startup; waiting for it");
                }
                Err(error) => return Err(error.into()),
            },
            () = stop.recv() => {
                tracing::info!("Stop requested while waiting for the database");
                return Ok(());
            }
        }
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_secs(15)) => {}
            () = stop.recv() => {
                tracing::info!("Stop requested while waiting for the database");
                return Ok(());
            }
        }
    };

    // A stop while migrating or starting ends the start.
    tokio::select! {
        migrated = migrate(&pool) => migrated?,
        () = stop.recv() => {
            tracing::info!("Stop requested while migrating");
            return Ok(());
        }
    }

    match cli.command {
        Some(Commands::SeedAdmin { email, password }) => {
            seed::seed_admin(&pool, &email, &password).await?;
        }
        Some(Commands::Serve) | None => {
            let server = tokio::select! {
                server = build_server(pool) => server?,
                () = stop.recv() => {
                    tracing::info!("Stop requested while starting");
                    return Ok(());
                }
            };
            // The server listens for stop signals itself from its first step.
            drop(stop);
            server.run().await?;
        }
    }

    Ok(())
}

async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    if !polymarket_core::config::env_flag("SKIP_MIGRATIONS", false) {
        tracing::info!("Running database migrations...");
        sqlx::migrate!("../../migrations").run(pool).await?;
        // ALTER SYSTEM cannot run inside a transaction (sqlx wraps migrations
        // in transactions), so we apply memory tuning here in autocommit mode.
        // It rewrites the server's own settings (sized for the deployed 2 GB
        // database), so it runs only where asked: PG_MEMORY_TUNING=true, or
        // by default in production.
        let production = std::env::var("ENVIRONMENT").is_ok_and(|env| env == "production");
        if polymarket_core::config::env_flag("PG_MEMORY_TUNING", production) {
            apply_pg_memory_tuning(pool).await;
        }
    } else {
        tracing::warn!("Skipping database migrations because SKIP_MIGRATIONS is enabled");
    }
    Ok(())
}

/// Apply PostgreSQL memory tuning via ALTER SYSTEM (runs outside transactions).
///
/// ALTER SYSTEM writes to `postgresql.auto.conf` which takes precedence over
/// `postgresql.conf` (including timescaledb-tune's settings). Most settings
/// take effect after `pg_reload_conf()`; `shared_buffers` and `max_connections`
/// require a full server restart.
///
/// This is idempotent — safe to run on every startup.
async fn apply_pg_memory_tuning(pool: &sqlx::PgPool) {
    let statements = [
        // Tuned for 2GB container limit. Total PG RSS should stay under ~1.5GB,
        // leaving headroom for OS page cache and background workers.
        "ALTER SYSTEM SET shared_buffers = '256MB'",
        "ALTER SYSTEM SET work_mem = '2MB'",
        "ALTER SYSTEM SET maintenance_work_mem = '64MB'",
        "ALTER SYSTEM SET effective_cache_size = '768MB'",
        "ALTER SYSTEM SET wal_buffers = '8MB'",
        "ALTER SYSTEM SET checkpoint_completion_target = '0.9'",
        "ALTER SYSTEM SET max_wal_size = '512MB'",
        "ALTER SYSTEM SET min_wal_size = '128MB'",
        "ALTER SYSTEM SET max_connections = '50'",
        // 7 hypertables × 2 policies (compression + retention) + 1 CA refresh = 15 jobs.
        // 8 workers gives headroom for simultaneous policy runs without exhausting slots.
        "ALTER SYSTEM SET timescaledb.max_background_workers = '8'",
        // SSD-optimised I/O settings (Railway volumes are SSD-backed).
        // Default random_page_cost=4.0 and effective_io_concurrency=1 assume HDD.
        "ALTER SYSTEM SET random_page_cost = '1.1'",
        "ALTER SYSTEM SET effective_io_concurrency = '200'",
        // JIT compilation adds overhead for short OLTP queries (<10 ms); disable it.
        "ALTER SYSTEM SET jit = 'off'",
        "SELECT pg_reload_conf()",
    ];

    for stmt in &statements {
        if let Err(e) = sqlx::query(stmt).execute(pool).await {
            tracing::warn!(stmt, error = %e, "pg memory tuning statement failed (non-fatal)");
        }
    }
    tracing::info!("PostgreSQL memory tuning applied (restart needed for shared_buffers)");
}

/// Secret the server library falls back to when `JWT_SECRET` is unset.
const DEV_DEFAULT_JWT_SECRET: &str = "development-secret-change-in-production";

/// Minimum JWT secret length.
const MIN_JWT_SECRET_LEN: usize = 32;

/// A secret with fewer distinct characters than this is trivially guessable.
const MIN_JWT_SECRET_DISTINCT_CHARS: usize = 10;

/// Placeholder values that ship in `.env.example`, docs and defaults. Compared
/// case-insensitively. Some are shorter than the minimum length; they are listed
/// anyway so the more specific "placeholder" message wins.
const PLACEHOLDER_JWT_SECRETS: &[&str] = &[
    DEV_DEFAULT_JWT_SECRET,
    "your-secure-random-secret-at-least-32-chars",
    "change-me-in-production",
    "your-secret-key",
    "replace-me",
    "changeme",
];

/// Fragments that mark a value as a template someone forgot to fill in.
const PLACEHOLDER_JWT_MARKERS: &[&str] = &[
    "your-secure",
    "your-secret",
    "change-me",
    "changeme",
    "replace-me",
    "placeholder",
    "secret-at-least",
    "development-secret",
];

/// Why a secret is weak despite being long enough, if it is.
fn weak_jwt_secret_reason(secret: &str) -> Option<&'static str> {
    let lowered = secret.to_ascii_lowercase();
    if PLACEHOLDER_JWT_SECRETS.contains(&lowered.as_str())
        || PLACEHOLDER_JWT_MARKERS.iter().any(|m| lowered.contains(m))
    {
        return Some("it is a placeholder value from the example configuration");
    }
    let distinct = secret
        .chars()
        .collect::<std::collections::HashSet<_>>()
        .len();
    if distinct < MIN_JWT_SECRET_DISTINCT_CHARS {
        return Some("it has too few distinct characters to be a random secret");
    }
    None
}

/// Validate `JWT_SECRET`.
///
/// Empty, default-valued and too-short secrets are always rejected. Placeholder
/// and low-entropy secrets are rejected in staging/production (and when
/// `ENVIRONMENT` is unset) and only reported as a warning in development.
fn validate_jwt_secret(secret: &str, environment: &str) -> Result<Option<String>, String> {
    if secret.is_empty() || secret == DEV_DEFAULT_JWT_SECRET {
        return Err("JWT_SECRET must be set to a secure value (not the default)".to_string());
    }
    if secret.len() < MIN_JWT_SECRET_LEN {
        return Err(format!(
            "JWT_SECRET must be at least {MIN_JWT_SECRET_LEN} characters long (current: {})",
            secret.len()
        ));
    }
    match weak_jwt_secret_reason(secret) {
        None => Ok(None),
        Some(reason) if is_development_environment(environment) => Ok(Some(format!(
            "JWT_SECRET is weak ({reason}); acceptable only because ENVIRONMENT={environment}"
        ))),
        Some(reason) => Err(format!(
            "JWT_SECRET is not acceptable: {reason} (ENVIRONMENT={environment:?}; \
             only development tolerates this)"
        )),
    }
}

async fn build_server(pool: sqlx::PgPool) -> anyhow::Result<ApiServer> {
    tracing::info!("API Server starting up...");

    // Validate JWT_SECRET for security
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_default();
    let environment = std::env::var("ENVIRONMENT").unwrap_or_default();
    match validate_jwt_secret(&jwt_secret, &environment) {
        Ok(Some(warning)) => {
            tracing::warn!("{warning}");
            tracing::info!("JWT secret validation passed (development: weak secret tolerated)");
        }
        Ok(None) => tracing::info!("JWT secret validation passed"),
        Err(message) => {
            tracing::error!("{message}");
            tracing::error!("Generate a secure secret: openssl rand -base64 32");
            anyhow::bail!("{message}");
        }
    }

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

    tracing::info!("Starting API server...");
    ApiServer::new(config, pool).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 44 characters of base64 from `openssl rand -base64 32`.
    const GOOD: &str = "k3Vq9uYxT1mZb7PdLw0RhJ8sNcA4eGfU2oXyIvQ6tBE=";

    #[test]
    fn a_random_secret_is_accepted_everywhere() {
        for environment in ["production", "staging", "development", ""] {
            assert_eq!(validate_jwt_secret(GOOD, environment), Ok(None));
        }
    }

    #[test]
    fn empty_default_and_short_secrets_are_always_rejected() {
        for environment in ["production", "development"] {
            assert!(validate_jwt_secret("", environment).is_err());
            assert!(validate_jwt_secret(DEV_DEFAULT_JWT_SECRET, environment).is_err());
            let short = validate_jwt_secret("Ab1!Ab1!Ab1!", environment).unwrap_err();
            assert!(short.contains("at least 32"), "{short}");
        }
    }

    #[test]
    fn the_env_example_placeholder_is_rejected_outside_development() {
        let placeholder = "your-secure-random-secret-at-least-32-chars";
        // It passes the length rule, which is how it used to slip through.
        assert!(placeholder.len() >= MIN_JWT_SECRET_LEN);
        for environment in ["production", "staging", "Production", "", "prod"] {
            let err = validate_jwt_secret(placeholder, environment).unwrap_err();
            assert!(err.contains("placeholder"), "{environment}: {err}");
        }
        // Obvious variants of it and the other shipped placeholders too.
        for variant in [
            "YOUR-SECURE-RANDOM-SECRET-AT-LEAST-32-CHARS",
            "please-change-me-to-something-else-xyz-123",
            "REPLACE-ME-with-a-real-secret-0123456789ab",
        ] {
            assert!(
                validate_jwt_secret(variant, "production").is_err(),
                "{variant}"
            );
        }
    }

    #[test]
    fn low_entropy_secrets_are_rejected_outside_development() {
        for weak in [
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "abababababababababababababababababababab",
            "1234512345123451234512345123451234512345",
            "abcdefgh-abcdefgh-abcdefgh-abcdefgh-abcdefgh-",
        ] {
            assert!(weak.len() >= MIN_JWT_SECRET_LEN);
            let err = validate_jwt_secret(weak, "production").unwrap_err();
            assert!(err.contains("distinct"), "{weak}: {err}");
        }
        // Exactly the minimum number of distinct characters is accepted.
        let ten_distinct = "abcdefghij".repeat(4);
        assert_eq!(validate_jwt_secret(&ten_distinct, "production"), Ok(None));
    }

    #[test]
    fn development_only_warns_about_weak_secrets() {
        for environment in ["development", "Development", "dev", "local", "test"] {
            let warning =
                validate_jwt_secret("your-secure-random-secret-at-least-32-chars", environment)
                    .unwrap()
                    .expect("a warning is reported");
            assert!(warning.contains("placeholder"), "{warning}");
            assert!(validate_jwt_secret(&"a".repeat(40), environment)
                .unwrap()
                .is_some());
        }
    }
}
