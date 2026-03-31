//! CEX latency arbitrage module.
//!
//! Monitors Binance real-time price feeds via WebSocket, detects when
//! Polymarket short-duration contract odds diverge from CEX-implied
//! probabilities, and executes FOK orders on the implied-correct side
//! before the market corrects (~2.7s lag).

pub mod binance_ws;
pub mod latency_arb_executor;
pub mod market_mapper;
pub mod price_tracker;

pub use binance_ws::{spawn_binance_ws_client, BinanceWsConfig};
pub use latency_arb_executor::{spawn_latency_arb_executor, LatencyArbExecutorConfig};
pub use market_mapper::MarketMapper;
pub use price_tracker::{CexPriceTick, CexSymbol, PriceMovement, PriceTracker, PriceTrackerConfig};
