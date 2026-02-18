//! Database repository for circuit breaker state persistence.

use anyhow::Result;
use chrono::NaiveDate;
use sqlx::{PgPool, Row};
use tracing::{debug, info};

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
                daily_pnl, peak_value, current_value,
                consecutive_losses, trips_today, last_reset_date
            FROM circuit_breaker_state
            WHERE id = 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        let state = row.map(|r| {
            let trip_reason_str: Option<String> = r.get("trip_reason");
            let trip_reason = trip_reason_str.and_then(|s| Self::parse_trip_reason(&s));

            CircuitBreakerState {
                tripped: r.get("tripped"),
                trip_reason,
                tripped_at: r.get("tripped_at"),
                resume_at: r.get("resume_at"),
                daily_pnl: r.get("daily_pnl"),
                peak_value: r.get("peak_value"),
                current_value: r.get("current_value"),
                consecutive_losses: r.get::<i32, _>("consecutive_losses") as u32,
                trips_today: r.get::<i32, _>("trips_today") as u32,
                // Recovery state is transient and not persisted to database
                // It will be re-initialized when recovery mode starts
                recovery_state: None,
                // Use today's date; actual reset check happens in load_state()
                last_reset_date: chrono::Utc::now().date_naive(),
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
        .bind(state.consecutive_losses as i32)
        .bind(state.trips_today as i32)
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
        ];

        for reason in reasons {
            let s = CircuitBreakerRepository::trip_reason_to_string(&reason);
            let parsed = CircuitBreakerRepository::parse_trip_reason(&s);
            assert_eq!(parsed, Some(reason));
        }
    }
}
