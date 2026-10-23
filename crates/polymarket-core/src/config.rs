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

/// A whole-number setting (a count, or seconds, minutes, hours) with
/// `default` when unset. A value that is not a whole number, or is outside
/// the range [`SETTINGS`] gives the name, stops a start; read later it keeps
/// the default and is reported.
pub fn env_count<T>(name: &str, default: T) -> T
where
    T: TryFrom<i64> + Copy + std::fmt::Display,
{
    env_count_opt(name).unwrap_or(default)
}

/// As [`env_count`], `None` when unset (or unreadable, which is reported).
pub fn env_count_opt<T>(name: &str) -> Option<T>
where
    T: TryFrom<i64> + Copy + std::fmt::Display,
{
    let value = env_set(name)?;
    let kind = setting_kind(name);
    let read = value
        .parse::<i64>()
        .ok()
        .filter(|_| kind.is_none_or(|kind| kind.reads(&value)))
        .and_then(|count| T::try_from(count).ok());
    if read.is_none() {
        report_once(name, || {
            format!(
                "{name}={value} is not {}; using its default",
                kind.map_or_else(|| "a whole number".to_string(), SettingKind::expected)
            )
        });
    }
    read
}

/// A number setting (an `f64` or a `Decimal`) with `default` when unset;
/// scientific notation is read. A value that is not a number, or is outside
/// the range [`SETTINGS`] gives the name, stops a start; read later it keeps
/// the default and is reported.
pub fn env_number<T>(name: &str, default: T) -> T
where
    T: TryFrom<rust_decimal::Decimal> + Copy + std::fmt::Display,
{
    let Some(value) = env_set(name) else {
        return default;
    };
    let kind = setting_kind(name);
    match parse_decimal(&value)
        .filter(|_| kind.is_none_or(|kind| kind.reads(&value)))
        .and_then(|number| T::try_from(number).ok())
    {
        Some(number) => number,
        None => {
            report_once(name, || {
                format!(
                    "{name}={value} is not {}; using its default ({default})",
                    kind.map_or_else(|| "a number".to_string(), SettingKind::expected)
                )
            });
            default
        }
    }
}

/// One of a setting's words (any case), lowercased, with `default` when
/// unset. Another word stops a start; read later it keeps the default and is
/// reported.
pub fn env_choice(name: &str, default: &str) -> String {
    let Some(value) = env_set(name) else {
        return default.to_string();
    };
    let kind = setting_kind(name);
    if kind.is_none_or(|kind| kind.reads(&value)) {
        return value.to_ascii_lowercase();
    }
    report_once(name, || {
        format!(
            "{name}={value} is not {}; using its default ({default})",
            kind.map_or_else(String::new, SettingKind::expected)
        )
    });
    default.to_string()
}

/// How a setting read through the helpers above is written.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// A whole number from `min` to `max` ([`env_count`], [`env_count_opt`]).
    Count { min: i64, max: i64 },
    /// One of the listed whole numbers ([`env_count`]).
    CountOf(&'static [i64]),
    /// A number at most `max`, and above `min` (when `above`) or at least it
    /// ([`env_number`]).
    Number { min: f64, max: f64, above: bool },
    /// One of the listed words, in any case ([`env_choice`]).
    Choice(&'static [&'static str]),
}

/// A number above `min`, at most `max`.
const fn above(min: f64, max: f64) -> SettingKind {
    SettingKind::Number {
        min,
        max,
        above: true,
    }
}

/// A number from `min` to `max`.
const fn between(min: f64, max: f64) -> SettingKind {
    SettingKind::Number {
        min,
        max,
        above: false,
    }
}

const fn count(min: i64, max: i64) -> SettingKind {
    SettingKind::Count { min, max }
}

/// Seconds between the passes of a background loop: at least one (zero would
/// spin it), at most a week.
const LOOP_SECS: SettingKind = count(1, 604_800);
/// A delay before a background loop's first pass, up to a day.
const STARTUP_DELAY_SECS: SettingKind = count(0, 86_400);

impl SettingKind {
    fn reads(self, value: &str) -> bool {
        use rust_decimal::prelude::ToPrimitive;
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
            SettingKind::Count { min, max } => value
                .parse::<i64>()
                .is_ok_and(|count| (min..=max).contains(&count)),
            SettingKind::CountOf(allowed) => value
                .parse::<i64>()
                .is_ok_and(|count| allowed.contains(&count)),
            SettingKind::Number { min, max, above } => parse_decimal(value)
                .and_then(|number| number.to_f64())
                .is_some_and(|number| {
                    (if above { number > min } else { number >= min }) && number <= max
                }),
            SettingKind::Choice(words) => words.iter().any(|word| value.eq_ignore_ascii_case(word)),
        }
    }

    fn expected(self) -> String {
        match self {
            SettingKind::Flag => "true or false (also 1/0, yes/no, on/off)".into(),
            SettingKind::Switch => "exactly true or false".into(),
            SettingKind::Decimal => "a number".into(),
            SettingKind::Fraction => "a fraction above 0 and at most 1 (0.10 for 10%)".into(),
            SettingKind::PositiveDecimal => "a number above 0".into(),
            SettingKind::Share => "a share from 0 to 1 (0.40 for 40%)".into(),
            SettingKind::Count { min, max } => format!("a whole number from {min} to {max}"),
            SettingKind::CountOf(allowed) => format!(
                "one of {}",
                allowed
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            SettingKind::Number { min, max, above } => {
                let low = if above { "above" } else { "at least" };
                if max.is_finite() {
                    format!("a number {low} {min} and at most {max}")
                } else {
                    format!("a number {low} {min}")
                }
            }
            SettingKind::Choice(words) => format!("one of {}", words.join(", ")),
        }
    }

    /// The helper that reads a setting of this kind.
    #[cfg(test)]
    fn helper(self) -> &'static str {
        match self {
            SettingKind::Flag => "env_flag",
            SettingKind::Switch => "env_switch",
            SettingKind::Decimal => "env_decimal",
            SettingKind::Fraction => "env_fraction",
            SettingKind::PositiveDecimal => "env_positive_decimal",
            SettingKind::Share => "env_share_f64",
            SettingKind::Count { .. } | SettingKind::CountOf(_) => "env_count",
            SettingKind::Number { .. } => "env_number",
            SettingKind::Choice(_) => "env_choice",
        }
    }
}

fn setting_kind(name: &str) -> Option<SettingKind> {
    SETTINGS
        .iter()
        .find(|(listed, _)| *listed == name)
        .map(|(_, kind)| *kind)
}

/// Every setting the services read through the helpers above, with the
/// values it takes. A range rejects only values that cannot be meant (a zero
/// interval, 15 written for 15%), not unusual ones; a 0 that stops new
/// entries is allowed. A test keeps this list in step with the code.
pub const SETTINGS: &[(&str, SettingKind)] = &[
    ("ACCOUNT_SNAPSHOT_ENABLED", SettingKind::Flag),
    ("ACCOUNT_SNAPSHOT_INTERVAL_SECS", count(30, 3_600)),
    ("API_PORT", count(1, 65_535)),
    ("ARB_ALLOW_LIVE_EXECUTION", SettingKind::Switch),
    ("ARB_AUTO_EXECUTE", SettingKind::Switch),
    ("ARB_CACHE_REFRESH_SECS", count(1, 3_600)),
    ("ARB_DYNAMIC_SIZING", SettingKind::Flag),
    ("ARB_EXPLORATION_HOLD_SECS", count(1, 604_800)),
    (
        "ARB_EXPLORATION_SWAP_MIN_SCORE_DELTA",
        between(0.0, f64::INFINITY),
    ),
    ("ARB_MAX_POSITION_SIZE", SettingKind::PositiveDecimal),
    ("ARB_MAX_SIGNAL_AGE_SECS", count(1, 3_600)),
    // 0 refuses every new entry: a stop an operator can set at once.
    ("ARB_MAX_TOTAL_EXPOSURE", between(0.0, f64::INFINITY)),
    ("ARB_MIN_BOOK_DEPTH", SettingKind::PositiveDecimal),
    ("ARB_MIN_NET_PROFIT", between(0.0, 0.5)),
    ("ARB_MIN_POSITION_SIZE", SettingKind::PositiveDecimal),
    ("ARB_MIN_PROFIT_THRESHOLD", above(0.0, 0.5)),
    (
        "ARB_MONITOR_AGGRESSIVENESS",
        SettingKind::Choice(&[
            "stable",
            "conservative",
            "balanced",
            "discovery",
            "aggressive",
        ]),
    ),
    ("ARB_MONITOR_EXPLORATION_SLOTS", count(0, 10_000)),
    ("ARB_MONITOR_MAX_MARKETS", count(1, 100_000)),
    ("ARB_MONITOR_MIN_BOOK_DEPTH", SettingKind::PositiveDecimal),
    ("ARB_POSITION_SIZE", SettingKind::PositiveDecimal),
    ("ARB_SELECTION_FORCE_REFRESH_SECS", count(1, 604_800)),
    (
        "ARB_SELECTION_RESUBSCRIBE_MIN_DELTA_RATIO",
        between(0.0, f64::INFINITY),
    ),
    (
        "ARB_SELECTION_RESUBSCRIBE_MIN_MARKET_DELTA",
        count(0, 100_000),
    ),
    ("ARB_UNIVERSE_REFRESH_SECS", LOOP_SECS),
    // Zero would reconnect the order book stream without pause.
    ("ARB_UPDATE_TIMEOUT_SECS", count(10, 3_600)),
    ("ARGON2_MAX_CONCURRENCY", count(0, 1_024)),
    ("BACKTEST_AUTOMATION_CLAIM_LIMIT", count(1, 1_000)),
    ("BACKTEST_AUTOMATION_ENABLED", SettingKind::Flag),
    ("BACKTEST_AUTOMATION_INTERVAL_SECS", LOOP_SECS),
    ("BACKTEST_AUTOMATION_STARTUP_DELAY_SECS", STARTUP_DELAY_SECS),
    ("BACKTEST_STALE_RUN_TIMEOUT_MINUTES", count(1, 43_200)),
    ("CLOB_MARKET_LIMIT", count(1, 10_000_000)),
    ("CLOB_WS_PING_INTERVAL_SECS", count(1, 600)),
    ("CLOB_WS_READ_TIMEOUT_SECS", count(5, 3_600)),
    ("CORS_PERMISSIVE", SettingKind::Flag),
    ("CROSS_MARKET_INTERVAL_SECS", LOOP_SECS),
    ("CROSS_MARKET_MAX_LAG_MOVE", above(0.0, 1.0)),
    ("CROSS_MARKET_MIN_CORRELATION", above(0.0, 1.0)),
    ("CROSS_MARKET_MIN_LEAD_MOVE", above(0.0, 1.0)),
    ("CROSS_MARKET_SIGNAL_ENABLED", SettingKind::Switch),
    ("DATABASE_MAX_CONNECTIONS", count(1, 1_000)),
    ("DB_ACQUIRE_TIMEOUT_SECS", count(1, 3_600)),
    ("DB_RETRY_BASE_DELAY_MS", count(0, 600_000)),
    ("DB_RETRY_MAX_ATTEMPTS", count(0, 30)),
    ("DB_RETRY_MAX_DELAY_MS", count(0, 3_600_000)),
    ("DYNAMIC_TUNER_APPLY", SettingKind::Flag),
    ("DYNAMIC_TUNER_BOOTSTRAP_ENABLED", SettingKind::Flag),
    ("DYNAMIC_TUNER_BOOTSTRAP_MAX_ATTEMPTS", count(0, 1_000_000)),
    ("DYNAMIC_TUNER_ENABLED", SettingKind::Flag),
    ("DYNAMIC_TUNER_EVAL_DELAY_MINUTES", count(0, 10_080)),
    ("DYNAMIC_TUNER_FILL_DEGRADE_DELTA", above(0.0, 1.0)),
    ("DYNAMIC_TUNER_FREEZE_DRAWDOWN", above(0.0, 1.0)),
    ("DYNAMIC_TUNER_INTERVAL_SECS", LOOP_SECS),
    ("DYNAMIC_TUNER_NO_TRADE_MIN_ATTEMPTS", count(1, 1_000_000)),
    ("DYNAMIC_TUNER_NO_TRADE_WINDOW_MINUTES", count(1, 10_080)),
    ("DYNAMIC_TUNER_PNL_DEGRADE_DELTA", above(0.0, f64::INFINITY)),
    ("DYNAMIC_TUNER_REGIME_STREAK", count(1, 1_000)),
    ("EXECUTOR_MIN_BOOK_DEPTH", between(0.0, f64::INFINITY)),
    ("EXIT_FAILED_RETRY_BACKOFF_SECS", count(1, 86_400)),
    ("EXIT_HANDLER_ENABLED", SettingKind::Switch),
    // Stop-losses and take-profits are checked once a pass.
    ("EXIT_POLL_INTERVAL_SECS", count(1, 3_600)),
    ("EXIT_RESOLUTION_CHECK_SECS", count(1, 86_400)),
    ("FLOW_CALIBRATION_LOOKBACK_DAYS", count(1, 365)),
    ("FLOW_CALIBRATION_MIN_CLOSED_TRADES", count(1, 100_000)),
    ("FLOW_FEATURE_ENABLED", SettingKind::Flag),
    ("FLOW_FEATURE_INTERVAL_SECS", LOOP_SECS),
    ("FLOW_FEATURE_RETENTION_DAYS", count(1, 3_650)),
    ("FLOW_MAX_SIGNALS_PER_CYCLE", count(1, 10_000)),
    ("FLOW_MIN_BOT_SCORE_COVERAGE", between(0.0, 1.0)),
    ("FLOW_MIN_EXPECTED_EDGE_BPS", between(0.0, 10_000.0)),
    ("FLOW_MIN_IMBALANCE", between(0.0, 1.0)),
    ("FLOW_MIN_SCORE", between(0.0, 1.0)),
    ("FLOW_MIN_SMART_MONEY_SHARE", between(0.0, 1.0)),
    ("FLOW_MIN_SMART_MONEY_USD", between(0.0, f64::INFINITY)),
    ("FLOW_MIN_TRADE_COUNT", count(0, 1_000_000)),
    ("FLOW_REQUIRE_CALIBRATION", SettingKind::Flag),
    ("FLOW_REQUIRE_YES_PRICE", SettingKind::Flag),
    ("FLOW_SIGNAL_ENABLED", SettingKind::Flag),
    ("FLOW_SIGNAL_INTERVAL_SECS", LOOP_SECS),
    // The windows the flow features are computed for.
    (
        "FLOW_SIGNAL_WINDOW_MINUTES",
        SettingKind::CountOf(&[15, 60, 240]),
    ),
    ("GAMMA_ARB_MARKET_PAGE_SIZE", count(1, 1_000)),
    ("GAMMA_MAX_PAGE_SIZE", count(1, 1_000)),
    ("GAMMA_PAGE_DELAY_MS", count(0, 60_000)),
    ("GAMMA_SYNCER_ENABLED", SettingKind::Flag),
    ("GAMMA_SYNCER_INTERVAL_SECS", LOOP_SECS),
    ("GAMMA_SYNCER_PAGE_SIZE", count(1, 1_000)),
    ("HARVESTER_ENABLED", SettingKind::Flag),
    ("HARVESTER_INTERVAL_SECS", LOOP_SECS),
    ("HARVESTER_MAX_NEW_PER_CYCLE", count(0, 1_000_000)),
    ("HARVESTER_TRADES_PER_FETCH", count(1, 100_000)),
    ("LEARNING_EVALUATOR_ENABLED", SettingKind::Flag),
    ("LEARNING_EVALUATOR_INTERVAL_SECS", LOOP_SECS),
    ("LEARNING_EVALUATOR_LOOKBACK_HOURS", count(1, 87_600)),
    ("LEARNING_EVALUATOR_MAX_MODELS", count(1, 10_000)),
    ("LEARNING_EVALUATOR_STARTUP_DELAY_SECS", STARTUP_DELAY_SECS),
    ("LEARNING_MODEL_REFRESH_SECS", count(1, 86_400)),
    ("LEARNING_ROLLOUT_OBSERVER_ENABLED", SettingKind::Flag),
    ("LEARNING_ROLLOUT_OBSERVER_INTERVAL_SECS", LOOP_SECS),
    (
        "LEARNING_ROLLOUT_OBSERVER_STARTUP_DELAY_SECS",
        STARTUP_DELAY_SECS,
    ),
    ("LEARNING_ROLLOUT_REFRESH_SECS", count(1, 86_400)),
    ("LEARNING_SHADOW_ENABLED", SettingKind::Flag),
    ("LIVE_TRADING", SettingKind::Switch),
    ("MEAN_REVERSION_INTERVAL_SECS", LOOP_SECS),
    ("MEAN_REVERSION_SIGNAL_ENABLED", SettingKind::Flag),
    ("MEAN_REV_MIN_MOVE_PCT", above(0.0, 1.0)),
    ("METRICS_CALCULATOR_BATCH_SIZE", count(1, 100_000)),
    ("METRICS_CALCULATOR_ENABLED", SettingKind::Flag),
    ("METRICS_CALCULATOR_INTERVAL_SECS", LOOP_SECS),
    ("METRICS_RECALC_AFTER_HOURS", count(0, 8_760)),
    ("PG_MEMORY_TUNING", SettingKind::Flag),
    ("PORT", count(1, 65_535)),
    ("POSITION_RECONCILER_ENABLED", SettingKind::Flag),
    ("POSITION_RECONCILER_INTERVAL_SECS", LOOP_SECS),
    ("POSITION_RECONCILER_STARTUP_DELAY_SECS", STARTUP_DELAY_SECS),
    ("QUANT_BASE_POSITION_SIZE", SettingKind::PositiveDecimal),
    ("QUANT_CACHE_REFRESH_SECS", count(1, 3_600)),
    ("QUANT_CROSS_MARKET_ALLOCATION_PCT", SettingKind::Share),
    ("QUANT_EXECUTOR_ENABLED", SettingKind::Switch),
    ("QUANT_FLOW_ALLOCATION_PCT", SettingKind::Share),
    // Zero would sell every quant position at its first exit pass.
    ("QUANT_MAX_HOLD_HOURS", count(1, 8_760)),
    // 0 refuses every new quant entry.
    ("QUANT_MAX_POSITIONS", count(0, 1_000)),
    ("QUANT_MAX_REFERENCE_AGE_SECS", count(1, 86_400)),
    ("QUANT_MAX_REFERENCE_DRIFT", above(0.0, 0.5)),
    ("QUANT_MAX_SIGNAL_AGE_SECS", count(1, 3_600)),
    ("QUANT_MEAN_REVERSION_ALLOCATION_PCT", SettingKind::Share),
    ("QUANT_MIN_BOOK_DEPTH", SettingKind::PositiveDecimal),
    ("QUANT_MIN_CONFIDENCE", between(0.0, 1.0)),
    ("QUANT_REENTRY_COOLDOWN_MINUTES", count(0, 10_080)),
    ("QUANT_RESOLUTION_ALLOCATION_PCT", SettingKind::Share),
    ("QUANT_STOP_LOSS_PCT", SettingKind::Fraction),
    ("QUANT_STRATEGY_HALT_COOLDOWN_SECS", count(0, 604_800)),
    ("QUANT_STRATEGY_MAX_CONSECUTIVE_LOSSES", count(1, 1_000)),
    (
        "QUANT_STRATEGY_MAX_DAILY_LOSS",
        SettingKind::PositiveDecimal,
    ),
    // A gain can exceed the cost (1.5 is +150%); 15 meant as 15% cannot.
    ("QUANT_TAKE_PROFIT_PCT", above(0.0, 10.0)),
    ("REDIS_RECONNECT_DELAY", count(1, 3_600)),
    ("REDIS_SUBSCRIBE_ARB", SettingKind::Flag),
    ("REDIS_SUBSCRIBE_ORDERBOOK", SettingKind::Flag),
    ("RESOLUTION_MIN_VOLUME_USD", between(0.0, f64::INFINITY)),
    ("RESOLUTION_SIGNAL_ENABLED", SettingKind::Flag),
    ("RESOLUTION_SIGNAL_INTERVAL_SECS", LOOP_SECS),
    ("SKIP_MIGRATIONS", SettingKind::Flag),
    ("SMART_MONEY_BOT_THRESHOLD", count(0, 100)),
    ("STRATEGY_HEALTH_ENABLED", SettingKind::Flag),
    ("STRATEGY_HEALTH_INTERVAL_SECS", LOOP_SECS),
    ("STRATEGY_PNL_ENABLED", SettingKind::Flag),
    ("STRATEGY_PNL_INTERVAL_SECS", LOOP_SECS),
    ("TRUSTED_PROXY_HOPS", count(0, 10)),
    ("WALLET_INVENTORY_ENABLED", SettingKind::Flag),
    ("WALLET_INVENTORY_INTERVAL_SECS", LOOP_SECS),
    (
        "WALLET_INVENTORY_PHASE2_CHUNK_BLOCKS",
        count(1, 1_000_000_000),
    ),
    ("WITHDRAWAL_DAILY_CAP_USD", SettingKind::PositiveDecimal),
    ("WITHDRAWAL_DESTINATION_DELAY_HOURS", count(0, 720)),
    ("WS_CHANNEL_CAPACITY", count(1, 1_000_000)),
];

/// Settings read by a crate that cannot use the helpers above (`auth`),
/// checked at start all the same.
const CHECKED_ELSEWHERE: &[(&str, SettingKind)] = &[
    // `aws` is not implemented; another word would quietly use the
    // read-only environment vault.
    (
        "VAULT_PROVIDER",
        SettingKind::Choice(&["file", "environment"]),
    ),
];

/// Pairs of settings where the first must not exceed the second, each with
/// its default (in the code that reads it), checked when either is set.
const ORDERED_PAIRS: &[(&str, &str, &str, &str)] = &[
    ("ARB_MIN_POSITION_SIZE", "5", "ARB_MAX_POSITION_SIZE", "25"),
    (
        "CLOB_WS_PING_INTERVAL_SECS",
        "10",
        "CLOB_WS_READ_TIMEOUT_SECS",
        "120",
    ),
];

/// The settings in [`SETTINGS`] that are set to a value their helper cannot
/// read, each with what it expects, and pairs out of order. A service checks
/// them before it starts: the API halts new entries on any, the arbitrage
/// monitor does not start. The helpers would otherwise run on a default the
/// operator did not choose (`disabled` would leave a service on, `10` meant
/// as 10% would be replaced).
pub fn unreadable_settings() -> Vec<String> {
    let mut problems: Vec<String> = SETTINGS
        .iter()
        .chain(CHECKED_ELSEWHERE)
        .filter_map(|(name, kind)| {
            let value = env_set(name)?;
            (!kind.reads(&value)).then(|| format!("{name}={value} (expected {})", kind.expected()))
        })
        .collect();
    for (low, low_default, high, high_default) in ORDERED_PAIRS {
        if env_set(low).is_none() && env_set(high).is_none() {
            continue;
        }
        let read = |name: &str, default: &str| {
            parse_decimal(&env_set(name).unwrap_or_else(|| default.to_string()))
        };
        if let (Some(low_value), Some(high_value)) =
            (read(low, low_default), read(high, high_default))
        {
            if low_value > high_value {
                problems.push(format!(
                    "{low} ({low_value}) is above {high} ({high_value})"
                ));
            }
        }
    }
    problems
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
                max_connections: crate::config::env_count("DATABASE_MAX_CONNECTIONS", 5),
                max_retries: crate::config::env_count("DB_RETRY_MAX_ATTEMPTS", 5),
                retry_base_delay_ms: crate::config::env_count("DB_RETRY_BASE_DELAY_MS", 1000),
                retry_max_delay_ms: crate::config::env_count("DB_RETRY_MAX_DELAY_MS", 30000),
                acquire_timeout_secs: env_count_opt("DB_ACQUIRE_TIMEOUT_SECS"),
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
            (count(1, 300), "300", "0"),
            (count(1, 300), "1", "30.5"),
            (SettingKind::CountOf(&[15, 60, 240]), "60", "30"),
            (above(0.0, 10.0), "1.5", "15"),
            (above(0.0, 10.0), "1e-3", "0"),
            (between(0.0, 1.0), "0", "-0.1"),
            (SettingKind::Choice(&["file", "environment"]), "File", "aws"),
        ] {
            assert!(kind.reads(readable), "{kind:?} {readable}");
            assert!(!kind.reads(unreadable), "{kind:?} {unreadable}");
        }
    }

    #[test]
    fn counts_and_numbers_keep_their_default_outside_their_range() {
        // Unlisted names read any whole number or number.
        std::env::set_var("ENV_TESTS_COUNT", "45");
        assert_eq!(env_count("ENV_TESTS_COUNT", 30_u64), 45);
        std::env::set_var("ENV_TESTS_COUNT", "-5");
        assert_eq!(env_count("ENV_TESTS_COUNT", 30_u64), 30);
        std::env::set_var("ENV_TESTS_COUNT", "1.5");
        assert_eq!(env_count_opt::<i64>("ENV_TESTS_COUNT"), None);
        std::env::set_var("ENV_TESTS_NUMBER", "0.25");
        assert_eq!(env_number("ENV_TESTS_NUMBER", 0.5_f64), 0.25);
        std::env::set_var("ENV_TESTS_NUMBER", "ten");
        assert_eq!(env_number("ENV_TESTS_NUMBER", Decimal::ONE), Decimal::ONE);
        std::env::set_var("ENV_TESTS_CHOICE", "Balanced");
        assert_eq!(env_choice("ENV_TESTS_CHOICE", "stable"), "balanced");
    }

    #[test]
    fn listed_ranges_hold_at_read_time() {
        // A listed name is held to its range when read, too.
        assert_eq!(
            setting_kind("EXIT_POLL_INTERVAL_SECS"),
            Some(count(1, 3_600))
        );
        let kind = setting_kind("QUANT_MAX_HOLD_HOURS").unwrap();
        assert!(!kind.reads("0"));
    }

    /// `.env.example`, copied as is, must start.
    #[test]
    fn the_example_settings_are_all_readable() {
        let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env.example");
        let mut checked = 0;
        for item in dotenvy::from_path_iter(&example).unwrap() {
            let (name, value) = item.unwrap();
            let value = value.trim();
            if let Some(kind) = SETTINGS
                .iter()
                .chain(CHECKED_ELSEWHERE)
                .find(|(listed, _)| *listed == name)
                .map(|(_, kind)| *kind)
            {
                checked += 1;
                assert!(
                    value.is_empty() || kind.reads(value),
                    "{name}={value} (expected {})",
                    kind.expected()
                );
            }
        }
        assert!(checked > 20, "only {checked} example settings checked");
    }

    /// Every setting read through a helper is in `SETTINGS`, as that helper
    /// reads it, so a start checks it.
    #[test]
    fn the_settings_list_matches_the_code() {
        const HELPERS: &[(&str, &str)] = &[
            ("env_flag(", "env_flag"),
            ("env_switch(", "env_switch"),
            // runtime_sync's wrapper for env_switch.
            ("env_enabled(", "env_switch"),
            ("env_decimal(", "env_decimal"),
            ("env_fraction(", "env_fraction"),
            ("env_positive_decimal(", "env_positive_decimal"),
            ("env_share_f64(", "env_share_f64"),
            ("env_count(", "env_count"),
            ("env_count_opt(", "env_count"),
            ("env_count::<", "env_count"),
            ("env_count_opt::<", "env_count"),
            ("env_number(", "env_number"),
            ("env_number::<", "env_number"),
            ("env_choice(", "env_choice"),
        ];

        fn constant(text: &str, name: &str) -> Option<String> {
            let declaration = format!("const {name}: &str = \"");
            let start = text.find(&declaration)? + declaration.len();
            let end = text[start..].find('"')?;
            Some(text[start..start + end].to_string())
        }

        fn scan(text: &str, found: &mut Vec<(String, &'static str)>) {
            for (helper, kind) in HELPERS {
                for (at, _) in text.match_indices(helper) {
                    // Not a longer name, nor this test's own list.
                    let before = text[..at].chars().next_back();
                    if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '"') {
                        continue;
                    }
                    let mut rest = &text[at + helper.len()..];
                    // Past a type argument (`env_count::<u64>(`).
                    if helper.ends_with('<') {
                        let Some(open) = rest.find('(') else {
                            continue;
                        };
                        rest = &rest[open + 1..];
                    }
                    let rest = rest.trim_start();
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

        let listed: std::collections::BTreeMap<&str, &str> = SETTINGS
            .iter()
            .map(|(name, kind)| (*name, kind.helper()))
            .collect();
        assert_eq!(listed.len(), SETTINGS.len(), "a setting is listed twice");
        let mut read: std::collections::BTreeMap<String, &str> = Default::default();
        for (name, helper) in found {
            if let Some(other) = read.insert(name.clone(), helper) {
                assert_eq!(other, helper, "{name} is read by two kinds of helper");
            }
        }
        let read: Vec<(&str, &str)> = read
            .iter()
            .map(|(name, helper)| (name.as_str(), *helper))
            .collect();
        let listed: Vec<(&str, &str)> = listed.into_iter().collect();
        assert_eq!(read, listed);
    }
}
