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
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const HEALTH_FILE: &str = "/tmp/healthy";

fn touch_health_file() {
    let _ = std::fs::write(HEALTH_FILE, format!("{}", chrono::Utc::now().timestamp()));
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    // Filter out noisy crates to avoid hitting Railway's 500 logs/sec limit
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "arb_monitor=info,polymarket_core=warn,tungstenite=warn,hyper=warn".into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Arbitrage Monitor");
    touch_health_file();

    // As PID 1 in its container the monitor must handle SIGTERM itself.
    let mut stop = polymarket_core::shutdown::StopSignals::install()?;

    // Load configuration
    let config = Config::from_env()?;

    // Initialize the monitor. Its database or Redis being down at start is
    // waited out rather than exited on: each exit spends one of the
    // platform's restarts, and a long outage would spend them all.
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

    // Run the monitoring loop until it fails or the process is asked to stop.
    // The monitor only publishes signals, so stopping needs no draining.
    tokio::select! {
        result = monitor.run() => result?,
        () = stop.recv() => info!("Shutdown requested; stopping arbitrage monitor"),
    }

    Ok(())
}
