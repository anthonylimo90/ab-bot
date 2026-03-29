//! Error types for the Polymarket Scanner system.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("WebSocket error: {0}")]
    WebSocket(Box<tokio_tungstenite::tungstenite::Error>),

    #[error("HTTP request error: {0}")]
    Http(Box<reqwest::Error>),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Database error: {0}")]
    Database(Box<sqlx::Error>),

    #[error("Database migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("Redis error: {0}")]
    Redis(Box<redis::RedisError>),

    #[error("Configuration file error: {0}")]
    ConfigFile(#[from] config::ConfigError),

    #[error("Configuration error: {message}")]
    Config { message: String },

    #[error("Invalid market data: {0}")]
    InvalidMarket(String),

    #[error("Position error: {0}")]
    Position(String),

    #[error("API error: {message}")]
    Api {
        message: String,
        status: Option<u16>,
    },

    #[error("Signing error: {message}")]
    Signing { message: String },

    #[error("Order error: {message}")]
    Order { message: String },

    #[error("Authentication error: {message}")]
    Auth { message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

// Manual From impls for boxed variants (thiserror #[from] doesn't support Box).
impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::WebSocket(Box::new(e))
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(Box::new(e))
    }
}

impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(Box::new(e))
    }
}

impl From<redis::RedisError> for Error {
    fn from(e: redis::RedisError) -> Self {
        Self::Redis(Box::new(e))
    }
}
