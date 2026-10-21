//! Quantitative signal generators.
//!
//! Each generator runs as an independent background task, polling feature tables
//! (flow features, orderbook aggregates, market metadata) at its own cadence.
//! When conditions are met, it emits a `QuantSignal` on the broadcast channel
//! for the `QuantSignalExecutor` to evaluate and potentially execute.
//!
//! Generators can be independently enabled/disabled via environment variables.

pub mod cross_market_signal;
pub mod flow_signal;
pub mod mean_reversion_signal;
pub mod resolution_signal;

pub use cross_market_signal::spawn_cross_market_signal_generator;
pub use flow_signal::spawn_flow_signal_generator;
pub use mean_reversion_signal::spawn_mean_reversion_signal_generator;
pub use resolution_signal::spawn_resolution_signal_generator;

/// Plain-table stand-in for the `orderbook_hourly` continuous aggregate, with
/// the column types TimescaleDB gives it, for generator tests.
#[cfg(test)]
pub(crate) mod hourly_bars {
    use chrono::{DateTime, Utc};
    use rust_decimal::Decimal;
    use sqlx::PgPool;

    pub(crate) async fn create(pool: &PgPool) {
        sqlx::query(
            "CREATE TABLE orderbook_hourly (
                market_id VARCHAR(255) NOT NULL,
                bucket TIMESTAMPTZ NOT NULL,
                open NUMERIC, high NUMERIC, low NUMERIC, close NUMERIC,
                avg_spread NUMERIC, avg_volume NUMERIC, tick_count BIGINT
            )",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    pub(crate) async fn insert(
        pool: &PgPool,
        market_id: &str,
        bucket: DateTime<Utc>,
        close: Decimal,
        avg_volume: Decimal,
    ) {
        sqlx::query(
            "INSERT INTO orderbook_hourly
                 (market_id, bucket, open, high, low, close, avg_spread, avg_volume, tick_count)
             VALUES ($1, $2, $3, $3, $3, $3, 0.01, $4, 10)",
        )
        .bind(market_id)
        .bind(bucket)
        .bind(close)
        .bind(avg_volume)
        .execute(pool)
        .await
        .unwrap();
    }
}
