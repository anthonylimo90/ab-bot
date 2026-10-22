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

    // Load configuration
    let config = Config::from_env()?;

    // Initialize the monitor
    let mut monitor = monitor::ArbMonitor::new(config).await?;

    // Run the monitoring loop until it fails or the process is asked to stop.
    // The monitor only publishes signals, so stopping needs no draining; as
    // PID 1 in its container it must still handle SIGTERM itself.
    tokio::select! {
        result = monitor.run() => result?,
        () = shutdown_signal() => info!("Shutdown requested; stopping arbitrage monitor"),
    }

    Ok(())
}

/// Resolve on SIGTERM (container stop, deploy) or Ctrl-C.
async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
}
