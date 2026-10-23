//! Arbitrage Monitor
//!
//! Real-time detection of mispriced Polymarket prediction markets.

#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod monitor;
mod position_tracker;
mod signals;

use anyhow::Result;
use polymarket_core::config::Config;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Instant;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const HEALTH_FILE: &str = "/tmp/healthy";

/// How long the monitoring loop may go without showing life before the
/// process exits for its supervisor to restart it. The loop shows life at
/// least once a minute.
const LOOP_STALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(300);

/// When the monitoring loop last showed life, in milliseconds since
/// `PROCESS_START`, plus one; zero until the loop starts.
static LOOP_ALIVE_AT: AtomicU64 = AtomicU64::new(0);
static PROCESS_START: LazyLock<Instant> = LazyLock::new(Instant::now);

fn write_health_file() {
    let _ = std::fs::write(HEALTH_FILE, format!("{}", chrono::Utc::now().timestamp()));
}

/// Records that the monitoring loop is alive, for the watchdog and the
/// container health check.
fn touch_health_file() {
    let elapsed = PROCESS_START.elapsed().as_millis() as u64;
    LOOP_ALIVE_AT.store(elapsed.saturating_add(1), Ordering::Relaxed);
    write_health_file();
}

/// Exits the process once the monitoring loop stops showing life. A loop
/// stuck on an await would otherwise leave arbitrage without signals while
/// the process still looks alive. On its own thread, so a stuck runtime
/// cannot hold it up; it measures on the monotonic clock.
fn spawn_loop_watchdog() {
    LazyLock::force(&PROCESS_START);
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(30));
        let alive_at = LOOP_ALIVE_AT.load(Ordering::Relaxed);
        if alive_at == 0 {
            continue;
        }
        let now = PROCESS_START.elapsed().as_millis() as u64;
        let silent = std::time::Duration::from_millis(now.saturating_sub(alive_at - 1));
        if silent > LOOP_STALL_LIMIT {
            tracing::error!(
                silent_secs = silent.as_secs(),
                "Arbitrage monitor loop has stalled; exiting so it is restarted"
            );
            std::process::exit(1);
        }
    });
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    // Filter out noisy crates to avoid hitting Railway's 500 logs/sec limit
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                // Every crate's warnings and errors: a target left out of the
                // filter is not logged at all.
                "warn,arb_monitor=info".into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Arbitrage Monitor");

    // As PID 1 in its container the monitor must handle SIGTERM itself.
    let mut stop = polymarket_core::shutdown::StopSignals::install()?;

    // Load configuration (and a local .env file, when there is one).
    let config = Config::from_env()?;

    // A setting that cannot be read stops the start rather than run on a
    // default the operator did not choose.
    let unreadable = polymarket_core::config::unreadable_settings();
    if !unreadable.is_empty() {
        anyhow::bail!(
            "Settings that cannot be read (correct or unset them): {}",
            unreadable.join("; ")
        );
    }

    // A Redis URL that cannot be read is a configuration error, not an
    // outage: stop at once rather than wait for it.
    redis::Client::open(config.redis.url.as_str())
        .map_err(|error| anyhow::anyhow!("REDIS_URL cannot be read: {error}"))?;
    config
        .database
        .url
        .parse::<sqlx::postgres::PgConnectOptions>()
        .map_err(|error| anyhow::anyhow!("DATABASE_URL cannot be read: {error}"))?;

    // Initialize the monitor. Its database or Redis being down at start is
    // waited out rather than exited on: each exit spends one of the
    // platform's restarts, and a long outage would spend them all. The
    // health file is first written once it has started.
    let mut monitor = loop {
        tokio::select! {
            monitor = monitor::ArbMonitor::new(config.clone()) => match monitor {
                Ok(monitor) => break monitor,
                Err(error) => {
                    tracing::error!(%error, "Could not start the arbitrage monitor; retrying");
                }
            },
            () = stop.recv() => {
                info!("Shutdown requested while starting; stopping arbitrage monitor");
                return Ok(());
            }
        }
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_secs(15)) => {}
            () = stop.recv() => {
                info!("Shutdown requested while starting; stopping arbitrage monitor");
                return Ok(());
            }
        }
    };
    write_health_file();
    spawn_loop_watchdog();

    // Run the monitoring loop until it fails or the process is asked to stop.
    // The monitor only publishes signals, so stopping needs no draining.
    tokio::select! {
        result = monitor.run() => result?,
        () = stop.recv() => info!("Shutdown requested; stopping arbitrage monitor"),
    }

    Ok(())
}
