//! Error types for the Polymarket Scanner system.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Database migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("Configuration error: {0}")]
    Config(#[from] config::ConfigError),

    #[error("Invalid market data: {0}")]
    InvalidMarket(String),

    #[error("Position error: {0}")]
    Position(String),

    #[error("API error: {message}")]
    Api { message: String, status: Option<u16> },
}

pub type Result<T> = std::result::Result<T, Error>;
