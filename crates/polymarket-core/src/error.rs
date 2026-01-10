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

    #[error("Configuration file error: {0}")]
    ConfigFile(#[from] config::ConfigError),

    #[error("Configuration error: {message}")]
    Config { message: String },

    #[error("Invalid market data: {0}")]
    InvalidMarket(String),

    #[error("Position error: {0}")]
    Position(String),

    #[error("API error: {message}")]
    Api { message: String, status: Option<u16> },

    #[error("Signing error: {message}")]
    Signing { message: String },

    #[error("Order error: {message}")]
    Order { message: String },

    #[error("Authentication error: {message}")]
    Auth { message: String },
}

pub type Result<T> = std::result::Result<T, Error>;
