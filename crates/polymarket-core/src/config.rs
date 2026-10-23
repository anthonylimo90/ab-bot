//! Configuration management for the Polymarket Scanner system.

use crate::{Error, Result};
use serde::Deserialize;
use std::env;

/// The value of `name`, unless it is unset or blank: a deployment that
/// passes a variable through empty (`VAR=`) has not configured it.
pub fn env_set(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

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
    pub max_retries: u32,
    pub retry_base_delay_ms: u64,
    pub retry_max_delay_ms: u64,
    pub acquire_timeout_secs: Option<u64>,
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
                max_retries: env::var("DB_RETRY_MAX_ATTEMPTS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(5),
                retry_base_delay_ms: env::var("DB_RETRY_BASE_DELAY_MS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1000),
                retry_max_delay_ms: env::var("DB_RETRY_MAX_DELAY_MS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(30000),
                acquire_timeout_secs: env::var("DB_ACQUIRE_TIMEOUT_SECS")
                    .ok()
                    .and_then(|s| s.parse().ok()),
            },
            redis: RedisConfig {
                url: env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
            },
            polygon: PolygonConfig {
                rpc_url: env_set("POLYGON_RPC_URL"),
                alchemy_api_key: env_set("ALCHEMY_API_KEY"),
            },
            polymarket: PolymarketConfig {
                clob_url: env_set("POLYMARKET_CLOB_URL"),
                ws_url: env_set("POLYMARKET_WS_URL"),
            },
            alerts: AlertsConfig {
                telegram_bot_token: env_set("TELEGRAM_BOT_TOKEN"),
                telegram_chat_id: env_set("TELEGRAM_CHAT_ID"),
                discord_webhook_url: env_set("DISCORD_WEBHOOK_URL"),
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
                max_retries: 1,
                retry_base_delay_ms: 100,
                retry_max_delay_ms: 500,
                acquire_timeout_secs: Some(5),
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
