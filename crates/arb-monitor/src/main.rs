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
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "arb_monitor=info,polymarket_core=warn,tungstenite=warn,hyper=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Arbitrage Monitor");
    touch_health_file();

    // Load configuration
    let config = Config::from_env()?;

    // Initialize the monitor
    let mut monitor = monitor::ArbMonitor::new(config).await?;

    // Run the monitoring loop
    monitor.run().await?;

    Ok(())
}
