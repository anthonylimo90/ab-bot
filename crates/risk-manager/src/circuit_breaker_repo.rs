//! Database repository for circuit breaker state persistence.

use anyhow::Result;
use chrono::NaiveDate;
use sqlx::{PgPool, Row};
use tracing::{debug, info, warn};

use crate::circuit_breaker::{CircuitBreakerState, TripReason};

/// Repository for circuit breaker state persistence.
pub struct CircuitBreakerRepository {
    pool: PgPool,
}

impl CircuitBreakerRepository {
    /// Create a new repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Load state from database.
    pub async fn load(&self) -> Result<Option<CircuitBreakerState>> {
        let row = sqlx::query(
            r#"
            SELECT
                tripped, trip_reason, tripped_at, resume_at,
                daily_pnl, peak_value, drawdown_peak, current_value,
                consecutive_losses, trips_today, last_reset_date,
                portfolio_value_seeded, recovery_state, cash_flows_applied_through
            FROM circuit_breaker_state
            WHERE id = 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        let state = row.map(|r| {
            let trip_reason_str: Option<String> = r.get("trip_reason");
            let tripped: bool = r.get("tripped");
            let trip_reason = match trip_reason_str.as_deref().map(Self::parse_trip_reason) {
                Some(Some(reason)) => Some(reason),
                // An unrecognized reason must not weaken an active trip.
                Some(None) if tripped => Some(TripReason::Manual),
                _ => None,
            };
            let recovery_state = r
                .get::<Option<serde_json::Value>, _>("recovery_state")
                .and_then(|value| match serde_json::from_value(value) {
                    Ok(recovery) => Some(recovery),
                    Err(error) => {
                        warn!(%error, "Discarding unreadable circuit breaker recovery state");
                        None
                    }
                });

            CircuitBreakerState {
                tripped,
                trip_reason,
                tripped_at: r.get("tripped_at"),
                resume_at: r.get("resume_at"),
                daily_pnl: r.get("daily_pnl"),
                peak_value: r.get("peak_value"),
                drawdown_peak: r.get("drawdown_peak"),
                current_value: r.get("current_value"),
                consecutive_losses: r.get::<i32, _>("consecutive_losses").max(0) as u32,
                trips_today: r.get::<i32, _>("trips_today").max(0) as u32,
                recovery_state,
                last_reset_date: r.get("last_reset_date"),
                portfolio_value_seeded: r.get("portfolio_value_seeded"),
                cash_flows_applied_through: r.get("cash_flows_applied_through"),
            }
        });

        if let Some(ref s) = state {
            info!(
                tripped = s.tripped,
                daily_pnl = %s.daily_pnl,
                consecutive_losses = s.consecutive_losses,
                "Loaded circuit breaker state from database"
            );
        }

        Ok(state)
    }

    /// Save state to database.
    pub async fn save(&self, state: &CircuitBreakerState) -> Result<()> {
        let trip_reason_str = state.trip_reason.as_ref().map(Self::trip_reason_to_string);

        sqlx::query(
            r#"
            UPDATE circuit_breaker_state SET
                tripped = $1,
                trip_reason = $2,
                tripped_at = $3,
                resume_at = $4,
                daily_pnl = $5,
                peak_value = $6,
                current_value = $7,
                consecutive_losses = $8,
                trips_today = $9,
                last_reset_date = $10,
                drawdown_peak = $11,
                portfolio_value_seeded = $12,
                recovery_state = $13,
                cash_flows_applied_through = $14,
                updated_at = NOW()
            WHERE id = 1
            "#,
        )
        .bind(state.tripped)
        .bind(&trip_reason_str)
        .bind(state.tripped_at)
        .bind(state.resume_at)
        .bind(state.daily_pnl)
        .bind(state.peak_value)
        .bind(state.current_value)
        .bind(i32::try_from(state.consecutive_losses).unwrap_or(i32::MAX))
        .bind(i32::try_from(state.trips_today).unwrap_or(i32::MAX))
        .bind(state.last_reset_date)
        .bind(state.drawdown_peak)
        .bind(state.portfolio_value_seeded)
        .bind(
            state
                .recovery_state
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?,
        )
        .bind(state.cash_flows_applied_through)
        .execute(&self.pool)
        .await?;

        debug!(tripped = state.tripped, "Saved circuit breaker state");
        Ok(())
    }

    /// Check if daily reset is needed and get last reset date.
    pub async fn get_last_reset_date(&self) -> Result<Option<NaiveDate>> {
        let row = sqlx::query("SELECT last_reset_date FROM circuit_breaker_state WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|r| r.get("last_reset_date")))
    }

    /// Update last reset date after daily reset.
    pub async fn update_last_reset_date(&self, date: NaiveDate) -> Result<()> {
        sqlx::query(
            "UPDATE circuit_breaker_state SET last_reset_date = $1, updated_at = NOW() WHERE id = 1",
        )
        .bind(date)
        .execute(&self.pool)
        .await?;

        info!(date = %date, "Updated circuit breaker last reset date");
        Ok(())
    }

    /// Parse trip reason from string.
    fn parse_trip_reason(s: &str) -> Option<TripReason> {
        match s {
            "daily_loss_limit" => Some(TripReason::DailyLossLimit),
            "max_drawdown" => Some(TripReason::MaxDrawdown),
            "consecutive_losses" => Some(TripReason::ConsecutiveLosses),
            "manual" => Some(TripReason::Manual),
            "connectivity" => Some(TripReason::Connectivity),
            "market_conditions" => Some(TripReason::MarketConditions),
            "hard_kill_switch" => Some(TripReason::HardKillSwitch),
            _ => None,
        }
    }

    /// Convert trip reason to string.
    fn trip_reason_to_string(reason: &TripReason) -> String {
        match reason {
            TripReason::DailyLossLimit => "daily_loss_limit".to_string(),
            TripReason::MaxDrawdown => "max_drawdown".to_string(),
            TripReason::ConsecutiveLosses => "consecutive_losses".to_string(),
            TripReason::Manual => "manual".to_string(),
            TripReason::Connectivity => "connectivity".to_string(),
            TripReason::MarketConditions => "market_conditions".to_string(),
            TripReason::HardKillSwitch => "hard_kill_switch".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trip_reason_roundtrip() {
        let reasons = vec![
            TripReason::DailyLossLimit,
            TripReason::MaxDrawdown,
            TripReason::ConsecutiveLosses,
            TripReason::Manual,
            TripReason::Connectivity,
            TripReason::MarketConditions,
            TripReason::HardKillSwitch,
        ];

        for reason in reasons {
            let s = CircuitBreakerRepository::trip_reason_to_string(&reason);
            let parsed = CircuitBreakerRepository::parse_trip_reason(&s);
            assert_eq!(parsed, Some(reason));
        }
    }

    async fn create_schema(pool: &PgPool) {
        sqlx::query(
            r#"
            CREATE TABLE circuit_breaker_state (
                id INTEGER PRIMARY KEY DEFAULT 1,
                tripped BOOLEAN NOT NULL DEFAULT FALSE,
                trip_reason VARCHAR(50),
                tripped_at TIMESTAMPTZ,
                resume_at TIMESTAMPTZ,
                daily_pnl DECIMAL(18, 8) NOT NULL DEFAULT 0,
                peak_value DECIMAL(18, 8) NOT NULL DEFAULT 0,
                current_value DECIMAL(18, 8) NOT NULL DEFAULT 0,
                consecutive_losses INTEGER NOT NULL DEFAULT 0,
                trips_today INTEGER NOT NULL DEFAULT 0,
                last_reset_date DATE NOT NULL DEFAULT CURRENT_DATE,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                CONSTRAINT single_row CHECK (id = 1)
            )
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO circuit_breaker_state (id) VALUES (1)")
            .execute(pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/079_circuit_breaker_full_state.sql"
        ))
        .execute(pool)
        .await
        .unwrap();
    }

    fn config() -> crate::CircuitBreakerConfig {
        crate::CircuitBreakerConfig {
            max_consecutive_losses: 3,
            max_daily_loss: rust_decimal::Decimal::new(100000, 0),
            cooldown_minutes: 30,
            ..Default::default()
        }
    }

    #[sqlx::test(migrations = false)]
    async fn restart_on_a_new_day_keeps_latch_and_references(pool: PgPool) {
        use crate::CircuitBreaker;
        use rust_decimal::Decimal;

        create_schema(&pool).await;
        let breaker = CircuitBreaker::with_persistence(config(), pool.clone());
        breaker
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();
        breaker
            .record_trade(Decimal::new(-10, 0), false)
            .await
            .unwrap();
        breaker
            .update_portfolio_value(Decimal::new(500, 0))
            .await
            .unwrap();
        breaker
            .update_portfolio_value(Decimal::new(500, 0))
            .await
            .unwrap();
        assert_eq!(
            breaker.state().await.trip_reason,
            Some(TripReason::HardKillSwitch)
        );

        sqlx::query(
            "UPDATE circuit_breaker_state SET last_reset_date = CURRENT_DATE - 1 WHERE id = 1",
        )
        .execute(&pool)
        .await
        .unwrap();

        let restarted = CircuitBreaker::with_persistence(config(), pool.clone());
        assert!(restarted.load_state().await.unwrap());
        let state = restarted.state().await;
        assert!(state.tripped);
        assert_eq!(state.trip_reason, Some(TripReason::HardKillSwitch));
        assert_eq!(state.resume_at, None);
        assert_eq!(state.peak_value, Decimal::new(1000, 0));
        assert_eq!(state.current_value, Decimal::new(500, 0));
        assert!(state.portfolio_value_seeded);
        // Only the per-day counters roll over.
        assert_eq!(state.daily_pnl, Decimal::ZERO);
        assert_eq!(state.consecutive_losses, 0);
        assert_eq!(state.last_reset_date, chrono::Utc::now().date_naive());
        assert!(!restarted.can_trade().await);

        let persisted: NaiveDate =
            sqlx::query_scalar("SELECT last_reset_date FROM circuit_breaker_state WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(persisted, chrono::Utc::now().date_naive());
    }

    #[sqlx::test(migrations = false)]
    async fn recovery_and_cash_flow_cursor_survive_restart(pool: PgPool) {
        use crate::{CircuitBreaker, CircuitBreakerConfig};
        use rust_decimal::Decimal;

        create_schema(&pool).await;
        let config = CircuitBreakerConfig {
            cooldown_minutes: 0,
            recovery_stage_minutes: 60,
            ..config()
        };
        let breaker = CircuitBreaker::with_persistence(config.clone(), pool.clone());
        let through = chrono::Utc::now();
        breaker
            .update_portfolio_value_with_flows(Decimal::new(1000, 0), Decimal::ZERO, Some(through))
            .await
            .unwrap();
        for _ in 0..3 {
            breaker
                .record_trade(Decimal::NEGATIVE_ONE, false)
                .await
                .unwrap();
        }
        assert!(breaker.can_trade().await);
        assert!(breaker.is_in_recovery().await);

        let restarted = CircuitBreaker::with_persistence(config, pool.clone());
        restarted.load_state().await.unwrap();
        let recovery = restarted
            .recovery_state()
            .await
            .expect("recovery persisted");
        assert_eq!(recovery.current_stage, 1);
        assert!(restarted.trading_capacity().await < Decimal::ONE);
        let applied = restarted.cash_flows_applied_through().await.unwrap();
        assert!((applied - through).num_milliseconds().abs() < 1);
    }
}
