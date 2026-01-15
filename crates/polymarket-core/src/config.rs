//! Configuration management for the Polymarket Scanner system.

use crate::{Error, Result};
use serde::Deserialize;
use std::env;

/// Application configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub polygon: PolygonConfig,
    pub polymarket: PolymarketConfig,
    pub alerts: AlertsConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolygonConfig {
    pub rpc_url: Option<String>,
    pub alchemy_api_key: Option<String>,
}

impl PolygonConfig {
    pub fn get_rpc_url(&self) -> Option<String> {
        self.rpc_url.clone().or_else(|| {
            self.alchemy_api_key
                .as_ref()
                .map(|key| format!("https://polygon-mainnet.g.alchemy.com/v2/{}", key))
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolymarketConfig {
    pub clob_url: Option<String>,
    pub ws_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AlertsConfig {
    pub telegram_bot_token: Option<String>,
    pub telegram_chat_id: Option<String>,
    pub discord_webhook_url: Option<String>,
}

impl Config {
    /// Load configuration from environment variables.
    #[allow(clippy::result_large_err)]
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        Ok(Self {
            database: DatabaseConfig {
                url: env::var("DATABASE_URL").map_err(|_| Error::Config {
                    message: "DATABASE_URL environment variable not set".to_string(),
                })?,
                max_connections: env::var("DATABASE_MAX_CONNECTIONS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(5),
            },
            redis: RedisConfig {
                url: env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
            },
            polygon: PolygonConfig {
                rpc_url: env::var("POLYGON_RPC_URL").ok(),
                alchemy_api_key: env::var("ALCHEMY_API_KEY").ok(),
            },
            polymarket: PolymarketConfig {
                clob_url: env::var("POLYMARKET_CLOB_URL").ok(),
                ws_url: env::var("POLYMARKET_WS_URL").ok(),
            },
            alerts: AlertsConfig {
                telegram_bot_token: env::var("TELEGRAM_BOT_TOKEN").ok(),
                telegram_chat_id: env::var("TELEGRAM_CHAT_ID").ok(),
                discord_webhook_url: env::var("DISCORD_WEBHOOK_URL").ok(),
            },
        })
    }

    /// Load configuration for testing (with defaults).
    #[cfg(test)]
    pub fn test_config() -> Self {
        Self {
            database: DatabaseConfig {
                url: "postgres://localhost/polymarket_test".to_string(),
                max_connections: 2,
            },
            redis: RedisConfig {
                url: "redis://127.0.0.1:6379".to_string(),
            },
            polygon: PolygonConfig {
                rpc_url: None,
                alchemy_api_key: None,
            },
            polymarket: PolymarketConfig {
                clob_url: None,
                ws_url: None,
            },
            alerts: AlertsConfig::default(),
        }
    }
}
