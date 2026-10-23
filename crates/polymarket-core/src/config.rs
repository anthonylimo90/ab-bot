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

/// Each name reported once: settings read on every pass would flood the log.
fn report_once(name: &str, message: impl FnOnce() -> String) {
    static REPORTED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let reported = REPORTED.get_or_init(Default::default);
    if reported
        .lock()
        .map(|mut names| names.insert(name.to_string()))
        .unwrap_or(true)
    {
        tracing::error!("{}", message());
    }
}

/// A yes/no setting with `default` when unset: true, 1, yes or on, and
/// false, 0, no or off, in any case. Any other value keeps the default and
/// is reported: a safety check or a service that defaults on must not be
/// switched off by `1` or `TRUE`.
pub fn env_flag(name: &str, default: bool) -> bool {
    let Some(value) = env_set(name) else {
        return default;
    };
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        _ => {
            report_once(name, || {
                format!("{name}={value} is not a yes/no value; using its default ({default})")
            });
            default
        }
    }
}

/// A switch that turns on something that spends money (live trading,
/// automatic execution): on only for exactly `true`. Any other value keeps
/// it off, and one that is not `false` is reported, so `TRUE` or `1` is not
/// mistaken for on without a word.
pub fn env_switch(name: &str) -> bool {
    let Some(value) = env_set(name) else {
        return false;
    };
    if value == "true" {
        return true;
    }
    if value != "false" {
        report_once(name, || {
            format!("{name}={value} is not `true`: it stays off (write true to switch it on)")
        });
    }
    false
}

/// A decimal setting with `default` when unset. A value that is not a
/// number (a decimal comma, a unit) keeps the default and is reported;
/// scientific notation (`1e3`) is read.
pub fn env_decimal(name: &str, default: rust_decimal::Decimal) -> rust_decimal::Decimal {
    let Some(value) = env_set(name) else {
        return default;
    };
    match value
        .parse::<rust_decimal::Decimal>()
        .or_else(|_| rust_decimal::Decimal::from_scientific(&value))
    {
        Ok(parsed) => parsed,
        Err(_) => {
            report_once(name, || {
                format!("{name}={value} is not a number; using its default ({default})")
            });
            default
        }
    }
}

/// A fraction setting (above 0, at most 1: 0.10 for 10%) with `default`
/// when unset. A value out of that range (10 written for 10%) or not a
/// number keeps the default and is reported.
pub fn env_fraction(name: &str, default: rust_decimal::Decimal) -> rust_decimal::Decimal {
    let value = env_decimal(name, default);
    if value > rust_decimal::Decimal::ZERO && value <= rust_decimal::Decimal::ONE {
        return value;
    }
    report_once(name, || {
        format!(
            "{name}={value} is not a fraction above 0 and at most 1 (0.10 for 10%); using its \
             default ({default})"
        )
    });
    default
}

/// A share setting held as an `f64` (0 to 1: 0.40 for 40%; 0 turns its
/// use off) with `default` when unset. A value out of that range (40 written
/// for 40%) or not a number keeps the default and is reported.
pub fn env_share_f64(name: &str, default: f64) -> f64 {
    use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
    let Some(fallback) = rust_decimal::Decimal::from_f64(default) else {
        return default;
    };
    let value = env_decimal(name, fallback);
    if value >= rust_decimal::Decimal::ZERO && value <= rust_decimal::Decimal::ONE {
        return value.to_f64().unwrap_or(default);
    }
    report_once(name, || {
        format!("{name}={value} is not a share from 0 to 1 (0.40 for 40%); using its default ({default})")
    });
    default
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

#[cfg(test)]
mod env_tests {
    use super::*;
    use rust_decimal::Decimal;

    // Each test uses its own variable names: tests run in parallel.
    #[test]
    fn flags_read_the_usual_spellings_and_keep_the_default_otherwise() {
        for (value, expected) in [
            ("true", true),
            ("1", true),
            ("TRUE", true),
            (" yes ", true),
            ("on", true),
            ("false", false),
            ("0", false),
            ("No", false),
            ("off", false),
            ("enabled", true),
        ] {
            std::env::set_var("ENV_TESTS_FLAG", value);
            assert_eq!(env_flag("ENV_TESTS_FLAG", true), expected, "{value}");
        }
        std::env::set_var("ENV_TESTS_FLAG", "");
        assert!(!env_flag("ENV_TESTS_FLAG", false));
    }

    #[test]
    fn a_money_switch_is_on_only_for_true() {
        for (value, expected) in [
            ("true", true),
            ("TRUE", false),
            ("1", false),
            ("false", false),
        ] {
            std::env::set_var("ENV_TESTS_SWITCH", value);
            assert_eq!(env_switch("ENV_TESTS_SWITCH"), expected, "{value}");
        }
        std::env::remove_var("ENV_TESTS_SWITCH");
        assert!(!env_switch("ENV_TESTS_SWITCH"));
    }

    #[test]
    fn numbers_and_fractions_keep_their_default_when_unreadable() {
        let default = Decimal::new(25000, 0);
        std::env::set_var("ENV_TESTS_DECIMAL", "1e3");
        assert_eq!(
            env_decimal("ENV_TESTS_DECIMAL", default),
            Decimal::from(1000)
        );
        std::env::set_var("ENV_TESTS_DECIMAL", "0,01");
        assert_eq!(env_decimal("ENV_TESTS_DECIMAL", default), default);

        let tenth = Decimal::new(1, 1);
        std::env::set_var("ENV_TESTS_FRACTION", "0.25");
        assert_eq!(
            env_fraction("ENV_TESTS_FRACTION", tenth),
            Decimal::new(25, 2)
        );
        for unreadable in ["10", "0", "-0.1", "ten"] {
            std::env::set_var("ENV_TESTS_FRACTION", unreadable);
            assert_eq!(
                env_fraction("ENV_TESTS_FRACTION", tenth),
                tenth,
                "{unreadable}"
            );
        }

        std::env::set_var("ENV_TESTS_SHARE", "0");
        assert_eq!(env_share_f64("ENV_TESTS_SHARE", 0.4), 0.0);
        std::env::set_var("ENV_TESTS_SHARE", "40");
        assert_eq!(env_share_f64("ENV_TESTS_SHARE", 0.4), 0.4);
    }
}
