//! Arbitrage Monitor
//!
//! Real-time detection of mispriced Polymarket prediction markets.

mod monitor;
mod position_tracker;
mod signals;

use anyhow::Result;
use polymarket_core::config::Config;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "arb_monitor=info,polymarket_core=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Arbitrage Monitor");

    // Load configuration
    let config = Config::from_env()?;

    // Initialize the monitor
    let mut monitor = monitor::ArbMonitor::new(config).await?;

    // Run the monitoring loop
    monitor.run().await?;

    Ok(())
}
