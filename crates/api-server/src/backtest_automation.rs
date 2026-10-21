//! Automated backtest scheduler.

use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use sqlx::{FromRow, PgPool};
use std::time;
use tracing::{info, warn};
use uuid::Uuid;

use crate::handlers::backtest::{
    enqueue_backtest, reap_stale_running_backtests, RunBacktestRequest, SlippageModel,
    StrategyConfig,
};

/// Default time after which a backtest still marked `running` is considered dead.
pub const DEFAULT_STALE_RUN_TIMEOUT_MINUTES: i64 = 120;

#[derive(Debug, Clone)]
pub struct BacktestAutomationConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub startup_delay_secs: u64,
    pub claim_limit: i64,
    /// A backtest `running` for longer than this is marked `failed` by the reaper.
    pub stale_run_timeout_minutes: i64,
}

impl BacktestAutomationConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("BACKTEST_AUTOMATION_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
            interval_secs: std::env::var("BACKTEST_AUTOMATION_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            startup_delay_secs: std::env::var("BACKTEST_AUTOMATION_STARTUP_DELAY_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(180),
            claim_limit: std::env::var("BACKTEST_AUTOMATION_CLAIM_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4),
            stale_run_timeout_minutes: std::env::var("BACKTEST_STALE_RUN_TIMEOUT_MINUTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|minutes| *minutes > 0)
                .unwrap_or(DEFAULT_STALE_RUN_TIMEOUT_MINUTES),
        }
    }
}

#[derive(Debug, FromRow)]
struct ClaimedScheduleRow {
    id: Uuid,
    name: String,
    strategy: serde_json::Value,
    lookback_days: i32,
    initial_capital: Decimal,
    markets: Option<Vec<String>>,
    slippage_model: serde_json::Value,
    fee_pct: Decimal,
}

pub fn spawn_backtest_automation(config: BacktestAutomationConfig, pool: PgPool) {
    // The reaper protects manual runs as well, so it runs even when scheduling
    // itself is disabled.
    tokio::spawn(run_reaper(config.clone(), pool.clone()));

    if !config.enabled {
        info!("Backtest automation disabled (BACKTEST_AUTOMATION_ENABLED != true)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        startup_delay_secs = config.startup_delay_secs,
        claim_limit = config.claim_limit,
        "Spawning automated backtest scheduler"
    );

    tokio::spawn(run_scheduler(config, pool));
}

/// Periodically fail backtests whose worker died before recording an outcome.
async fn run_reaper(config: BacktestAutomationConfig, pool: PgPool) {
    let max_age = Duration::minutes(config.stale_run_timeout_minutes);
    let interval = time::Duration::from_secs(config.interval_secs.max(30));

    loop {
        match reap_stale_running_backtests(&pool, max_age).await {
            Ok(0) => {}
            Ok(reaped) => warn!(
                reaped,
                timeout_minutes = config.stale_run_timeout_minutes,
                "Marked stale running backtests as failed"
            ),
            Err(error) => warn!(error = %error, "Stale backtest reaper failed"),
        }

        tokio::time::sleep(interval).await;
    }
}

async fn run_scheduler(config: BacktestAutomationConfig, pool: PgPool) {
    tokio::time::sleep(time::Duration::from_secs(config.startup_delay_secs)).await;
    let interval = time::Duration::from_secs(config.interval_secs);

    loop {
        match claim_due_schedules(&pool, config.claim_limit).await {
            Ok(rows) => {
                for row in rows {
                    if let Err(error) = enqueue_claimed_schedule(&pool, &row).await {
                        let _ = mark_enqueue_failure(&pool, row.id, &error.to_string()).await;
                        warn!(error = %error, "Automated backtest enqueue failed");
                    }
                }
            }
            Err(error) => {
                warn!(error = %error, "Automated backtest scheduler cycle failed");
            }
        }

        tokio::time::sleep(interval).await;
    }
}

async fn claim_due_schedules(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<ClaimedScheduleRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        WITH due AS (
            SELECT id
            FROM backtest_schedules
            WHERE enabled = true
              AND next_run_at <= NOW()
            ORDER BY next_run_at ASC, created_at ASC
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE backtest_schedules s
        SET last_run_at = NOW(),
            next_run_at = NOW() + make_interval(hours => s.interval_hours),
            updated_at = NOW()
        FROM due
        WHERE s.id = due.id
        RETURNING s.id, s.name, s.strategy, s.lookback_days, s.initial_capital,
                  s.markets, s.slippage_model, s.fee_pct
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

async fn enqueue_claimed_schedule(
    pool: &PgPool,
    row: &ClaimedScheduleRow,
) -> Result<(), crate::error::ApiError> {
    let strategy: StrategyConfig = serde_json::from_value(row.strategy.clone())
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;
    let slippage_model: SlippageModel = serde_json::from_value(row.slippage_model.clone())
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;

    let end_date = Utc::now();
    let start_date = end_date - Duration::days(row.lookback_days as i64);
    let request = RunBacktestRequest {
        strategy,
        start_date,
        end_date,
        initial_capital: row.initial_capital,
        markets: row.markets.clone(),
        slippage_model,
        fee_pct: row.fee_pct,
    };

    enqueue_backtest(
        pool.clone(),
        request,
        "scheduled",
        Some(row.id),
        Some(row.name.clone()),
    )
    .await?;

    Ok(())
}

async fn mark_enqueue_failure(
    pool: &PgPool,
    schedule_id: Uuid,
    error: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE backtest_schedules
        SET last_status = LEFT($2, 128),
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(schedule_id)
    .bind(format!("enqueue_failed: {error}"))
    .execute(pool)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_from_env_defaults() {
        let config = BacktestAutomationConfig::from_env();
        assert!(config.interval_secs > 0);
        assert!(config.startup_delay_secs > 0);
        assert!(config.claim_limit > 0);
        assert!(config.stale_run_timeout_minutes > 0);
    }
}
