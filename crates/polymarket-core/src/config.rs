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

fn parse_flag(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_decimal(value: &str) -> Option<rust_decimal::Decimal> {
    value
        .parse::<rust_decimal::Decimal>()
        .or_else(|_| rust_decimal::Decimal::from_scientific(value))
        .ok()
}

/// A yes/no setting with `default` when unset: true, 1, yes or on, and
/// false, 0, no or off, in any case. Any other value stops a start (see
/// [`unreadable_settings`]); read later it keeps the default and is
/// reported.
pub fn env_flag(name: &str, default: bool) -> bool {
    let Some(value) = env_set(name) else {
        return default;
    };
    match parse_flag(&value) {
        Some(flag) => flag,
        None => {
            report_once(name, || {
                format!("{name}={value} is not a yes/no value; using its default ({default})")
            });
            default
        }
    }
}

/// A switch that turns on something that spends money (live trading,
/// automatic execution): on only for exactly `true`. Any value but `true`
/// or `false` stops a start (see [`unreadable_settings`]); read later it
/// keeps the switch off and is reported.
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

/// A decimal setting with `default` when unset; scientific notation
/// (`1e3`) is read. A value that is not a number (a decimal comma, a unit)
/// stops a start (see [`unreadable_settings`]); read later it keeps the
/// default and is reported.
pub fn env_decimal(name: &str, default: rust_decimal::Decimal) -> rust_decimal::Decimal {
    let Some(value) = env_set(name) else {
        return default;
    };
    match parse_decimal(&value) {
        Some(parsed) => parsed,
        None => {
            report_once(name, || {
                format!("{name}={value} is not a number; using its default ({default})")
            });
            default
        }
    }
}

/// A fraction setting (above 0, at most 1: 0.10 for 10%) with `default`
/// when unset. A value out of that range (10 written for 10%) or not a
/// number stops a start; read later it keeps the default and is reported.
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

/// A positive multiple setting (0.15 for 15%, 1.5 for 150%) with `default`
/// when unset. Zero, a negative value or one that is not a number stops a
/// start; read later it keeps the default and is reported.
pub fn env_positive_decimal(name: &str, default: rust_decimal::Decimal) -> rust_decimal::Decimal {
    let value = env_decimal(name, default);
    if value > rust_decimal::Decimal::ZERO {
        return value;
    }
    report_once(name, || {
        format!("{name}={value} is not above 0; using its default ({default})")
    });
    default
}

/// A share setting held as an `f64` (0 to 1: 0.40 for 40%; 0 turns its
/// use off) with `default` when unset. A value out of that range (40 written
/// for 40%) or not a number stops a start; read later it keeps the default
/// and is reported.
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

/// How a setting read through the helpers above is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    /// [`env_flag`]
    Flag,
    /// [`env_switch`]
    Switch,
    /// [`env_decimal`]
    Decimal,
    /// [`env_fraction`]
    Fraction,
    /// [`env_positive_decimal`]
    PositiveDecimal,
    /// [`env_share_f64`]
    Share,
}

impl SettingKind {
    fn reads(self, value: &str) -> bool {
        use rust_decimal::Decimal;
        match self {
            SettingKind::Flag => parse_flag(value).is_some(),
            SettingKind::Switch => value == "true" || value == "false",
            SettingKind::Decimal => parse_decimal(value).is_some(),
            SettingKind::Fraction => {
                parse_decimal(value).is_some_and(|v| v > Decimal::ZERO && v <= Decimal::ONE)
            }
            SettingKind::PositiveDecimal => parse_decimal(value).is_some_and(|v| v > Decimal::ZERO),
            SettingKind::Share => {
                parse_decimal(value).is_some_and(|v| v >= Decimal::ZERO && v <= Decimal::ONE)
            }
        }
    }

    fn expected(self) -> &'static str {
        match self {
            SettingKind::Flag => "true or false (also 1/0, yes/no, on/off)",
            SettingKind::Switch => "exactly true or false",
            SettingKind::Decimal => "a number",
            SettingKind::Fraction => "a fraction above 0 and at most 1 (0.10 for 10%)",
            SettingKind::PositiveDecimal => "a number above 0 (0.15 for 15%)",
            SettingKind::Share => "a share from 0 to 1 (0.40 for 40%)",
        }
    }
}

/// Every setting the services read through the helpers above. A test keeps
/// this list in step with the code.
pub const SETTINGS: &[(&str, SettingKind)] = &[
    ("ACCOUNT_SNAPSHOT_ENABLED", SettingKind::Flag),
    ("ARB_ALLOW_LIVE_EXECUTION", SettingKind::Switch),
    ("ARB_AUTO_EXECUTE", SettingKind::Switch),
    ("ARB_DYNAMIC_SIZING", SettingKind::Flag),
    ("ARB_MAX_POSITION_SIZE", SettingKind::Decimal),
    ("ARB_MAX_SIGNAL_AGE_SECS", SettingKind::Decimal),
    ("ARB_MAX_TOTAL_EXPOSURE", SettingKind::Decimal),
    ("ARB_MIN_BOOK_DEPTH", SettingKind::Decimal),
    ("ARB_MIN_NET_PROFIT", SettingKind::Decimal),
    ("ARB_MIN_POSITION_SIZE", SettingKind::Decimal),
    ("ARB_MIN_PROFIT_THRESHOLD", SettingKind::Decimal),
    ("ARB_MONITOR_EXPLORATION_SLOTS", SettingKind::Decimal),
    ("ARB_MONITOR_MAX_MARKETS", SettingKind::Decimal),
    ("ARB_POSITION_SIZE", SettingKind::Decimal),
    ("BACKTEST_AUTOMATION_ENABLED", SettingKind::Flag),
    ("CROSS_MARKET_SIGNAL_ENABLED", SettingKind::Switch),
    ("DYNAMIC_TUNER_APPLY", SettingKind::Flag),
    ("DYNAMIC_TUNER_BOOTSTRAP_ENABLED", SettingKind::Flag),
    ("DYNAMIC_TUNER_ENABLED", SettingKind::Flag),
    ("EXIT_HANDLER_ENABLED", SettingKind::Switch),
    ("FLOW_FEATURE_ENABLED", SettingKind::Flag),
    ("FLOW_REQUIRE_CALIBRATION", SettingKind::Flag),
    ("FLOW_REQUIRE_YES_PRICE", SettingKind::Flag),
    ("FLOW_SIGNAL_ENABLED", SettingKind::Flag),
    ("GAMMA_SYNCER_ENABLED", SettingKind::Flag),
    ("HARVESTER_ENABLED", SettingKind::Flag),
    ("LATENCY_ARB_ENABLED", SettingKind::Switch),
    ("LEARNING_EVALUATOR_ENABLED", SettingKind::Flag),
    ("LEARNING_ROLLOUT_OBSERVER_ENABLED", SettingKind::Flag),
    ("LEARNING_SHADOW_ENABLED", SettingKind::Flag),
    ("LIVE_TRADING", SettingKind::Switch),
    ("MEAN_REVERSION_SIGNAL_ENABLED", SettingKind::Flag),
    ("METRICS_CALCULATOR_ENABLED", SettingKind::Flag),
    ("PG_MEMORY_TUNING", SettingKind::Flag),
    ("POSITION_RECONCILER_ENABLED", SettingKind::Flag),
    ("QUANT_BASE_POSITION_SIZE", SettingKind::Decimal),
    ("QUANT_CROSS_MARKET_ALLOCATION_PCT", SettingKind::Share),
    ("QUANT_EXECUTOR_ENABLED", SettingKind::Switch),
    ("QUANT_FLOW_ALLOCATION_PCT", SettingKind::Share),
    ("QUANT_MEAN_REVERSION_ALLOCATION_PCT", SettingKind::Share),
    ("QUANT_MIN_BOOK_DEPTH", SettingKind::Decimal),
    ("QUANT_RESOLUTION_ALLOCATION_PCT", SettingKind::Share),
    ("QUANT_STOP_LOSS_PCT", SettingKind::Fraction),
    ("QUANT_STRATEGY_MAX_DAILY_LOSS", SettingKind::Decimal),
    ("QUANT_TAKE_PROFIT_PCT", SettingKind::PositiveDecimal),
    ("REDIS_SUBSCRIBE_ARB", SettingKind::Flag),
    ("REDIS_SUBSCRIBE_ORDERBOOK", SettingKind::Flag),
    ("RESOLUTION_SIGNAL_ENABLED", SettingKind::Flag),
    ("SKIP_MIGRATIONS", SettingKind::Flag),
    ("STRATEGY_HEALTH_ENABLED", SettingKind::Flag),
    ("STRATEGY_PNL_ENABLED", SettingKind::Flag),
    ("WALLET_INVENTORY_ENABLED", SettingKind::Flag),
];

/// The settings in [`SETTINGS`] that are set to a value their helper cannot
/// read, each with what it expects. A service checks them before it starts
/// and refuses to start on any: the helpers would otherwise run on a default
/// the operator did not choose (`disabled` would leave a service on, `10`
/// meant as 10% would be replaced).
pub fn unreadable_settings() -> Vec<String> {
    SETTINGS
        .iter()
        .filter_map(|(name, kind)| {
            let value = env_set(name)?;
            (!kind.reads(&value)).then(|| format!("{name}={value} (expected {})", kind.expected()))
        })
        .collect()
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

    #[test]
    fn unreadable_settings_name_each_value_that_cannot_be_read() {
        for (kind, readable, unreadable) in [
            (SettingKind::Flag, "Off", "disabled"),
            (SettingKind::Switch, "false", "TRUE"),
            (SettingKind::Decimal, "1e3", "0,01"),
            (SettingKind::Fraction, "0.25", "10"),
            (SettingKind::PositiveDecimal, "1.5", "0"),
            (SettingKind::Share, "0", "40"),
        ] {
            assert!(kind.reads(readable), "{kind:?} {readable}");
            assert!(!kind.reads(unreadable), "{kind:?} {unreadable}");
        }
    }

    /// Every setting read through a helper is in `SETTINGS`, as that helper
    /// reads it, so a start checks it.
    #[test]
    fn the_settings_list_matches_the_code() {
        const HELPERS: &[(&str, SettingKind)] = &[
            ("env_flag(", SettingKind::Flag),
            ("env_switch(", SettingKind::Switch),
            // runtime_sync's wrapper for env_switch.
            ("env_enabled(", SettingKind::Switch),
            ("env_decimal(", SettingKind::Decimal),
            ("env_fraction(", SettingKind::Fraction),
            ("env_positive_decimal(", SettingKind::PositiveDecimal),
            ("env_share_f64(", SettingKind::Share),
        ];

        fn constant(text: &str, name: &str) -> Option<String> {
            let declaration = format!("const {name}: &str = \"");
            let start = text.find(&declaration)? + declaration.len();
            let end = text[start..].find('"')?;
            Some(text[start..start + end].to_string())
        }

        fn scan(text: &str, found: &mut Vec<(String, SettingKind)>) {
            for (helper, kind) in HELPERS {
                for (at, _) in text.match_indices(helper) {
                    // Not a longer name, nor this test's own list.
                    let before = text[..at].chars().next_back();
                    if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '"') {
                        continue;
                    }
                    let rest = text[at + helper.len()..].trim_start();
                    let name = if let Some(quoted) = rest.strip_prefix('"') {
                        quoted.split('"').next().map(str::to_string)
                    } else {
                        let ident: String = rest
                            .chars()
                            .take_while(|c| {
                                c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_'
                            })
                            .collect();
                        // A lowercase argument is a variable, read elsewhere.
                        (!ident.is_empty())
                            .then(|| constant(text, &ident))
                            .flatten()
                    };
                    if let Some(name) = name.filter(|name| !name.starts_with("ENV_TESTS_")) {
                        found.push((name, *kind));
                    }
                }
            }
        }

        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut found = Vec::new();
        let mut dirs = vec![crates];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    scan(&std::fs::read_to_string(&path).unwrap(), &mut found);
                }
            }
        }

        let listed: std::collections::BTreeMap<&str, SettingKind> =
            SETTINGS.iter().copied().collect();
        assert_eq!(listed.len(), SETTINGS.len(), "a setting is listed twice");
        let mut read: std::collections::BTreeMap<String, SettingKind> = Default::default();
        for (name, kind) in found {
            if let Some(other) = read.insert(name.clone(), kind) {
                assert_eq!(other, kind, "{name} is read as two kinds of setting");
            }
        }
        let read: Vec<(&str, SettingKind)> = read
            .iter()
            .map(|(name, kind)| (name.as_str(), *kind))
            .collect();
        let listed: Vec<(&str, SettingKind)> = listed.into_iter().collect();
        assert_eq!(read, listed);
    }
}
