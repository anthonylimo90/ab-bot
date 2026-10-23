//! Database repository for circuit breaker state persistence.

use anyhow::Result;
use sqlx::{PgPool, Row};
use tracing::{debug, info, warn};

use crate::circuit_breaker::{CircuitBreakerState, TripReason};

/// Repository for circuit breaker state persistence.
/// What [`CircuitBreakerRepository::save`] stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveOutcome {
    /// The state was saved: no operator change was stored since its epoch.
    pub saved: bool,
    /// The closes passed with it are marked recorded (always, when there
    /// were none); false when the state was not saved.
    pub marked: bool,
}

pub struct CircuitBreakerRepository {
    pool: PgPool,
}

impl CircuitBreakerRepository {
    /// Create a new repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Load state from database, with its operator epoch.
    pub async fn load(&self) -> Result<Option<(CircuitBreakerState, i64)>> {
        let row = sqlx::query(
            r#"
            SELECT
                tripped, trip_reason, tripped_at, resume_at,
                daily_pnl, peak_value, drawdown_peak, current_value,
                consecutive_losses, trips_today, last_reset_date,
                portfolio_value_seeded, recovery_state, cash_flows_applied_through,
                operator_epoch
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

            let state = CircuitBreakerState {
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
            };
            (state, r.get::<i64, _>("operator_epoch"))
        });

        if let Some((ref s, _)) = state {
            info!(
                tripped = s.tripped,
                daily_pnl = %s.daily_pnl,
                consecutive_losses = s.consecutive_losses,
                "Loaded circuit breaker state from database"
            );
        }

        Ok(state)
    }

    /// The operator epoch stored now, if the row exists.
    pub async fn operator_epoch(&self) -> Result<Option<i64>> {
        Ok(
            sqlx::query_scalar("SELECT operator_epoch FROM circuit_breaker_state WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Save the whole state, provided no operator change has been stored
    /// since `epoch`, and says what it stored. The single row is written
    /// whether or not it exists yet: a missing row must never make a save (a
    /// latched trip) a silent no-op. The positions in `closes` (closes the
    /// state now counts) are marked recorded in the same transaction, so a
    /// close is marked exactly when the saved counters hold it; a mark that
    /// fails leaves the counters saved and says so ([`SaveOutcome::marked`]),
    /// for the caller to mark them with its next save.
    pub async fn save(
        &self,
        state: &CircuitBreakerState,
        epoch: i64,
        closes: &[uuid::Uuid],
    ) -> Result<SaveOutcome> {
        let trip_reason_str = state.trip_reason.as_ref().map(Self::trip_reason_to_string);

        let mut tx = self.pool.begin().await?;
        let saved = sqlx::query(
            r#"
            INSERT INTO circuit_breaker_state (
                id, tripped, trip_reason, tripped_at, resume_at, daily_pnl, peak_value,
                current_value, consecutive_losses, trips_today, last_reset_date,
                drawdown_peak, portfolio_value_seeded, recovery_state,
                cash_flows_applied_through, operator_epoch, updated_at
            )
            VALUES (1, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, NOW())
            ON CONFLICT (id) DO UPDATE SET
                tripped = EXCLUDED.tripped,
                trip_reason = EXCLUDED.trip_reason,
                tripped_at = EXCLUDED.tripped_at,
                resume_at = EXCLUDED.resume_at,
                daily_pnl = EXCLUDED.daily_pnl,
                peak_value = EXCLUDED.peak_value,
                current_value = EXCLUDED.current_value,
                consecutive_losses = EXCLUDED.consecutive_losses,
                trips_today = EXCLUDED.trips_today,
                last_reset_date = EXCLUDED.last_reset_date,
                drawdown_peak = EXCLUDED.drawdown_peak,
                portfolio_value_seeded = EXCLUDED.portfolio_value_seeded,
                recovery_state = EXCLUDED.recovery_state,
                cash_flows_applied_through = EXCLUDED.cash_flows_applied_through,
                updated_at = NOW()
            WHERE circuit_breaker_state.operator_epoch = EXCLUDED.operator_epoch
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
        .bind(epoch)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;
        let mut marked = true;
        if saved && !closes.is_empty() {
            // A schema without the mark (or a lock timeout) must not stop
            // the counters saving: the marks are left out and reported.
            sqlx::query("SAVEPOINT close_marks")
                .execute(&mut *tx)
                .await?;
            if let Err(error) = sqlx::query(
                "UPDATE positions SET breaker_recorded_at = NOW() \
                 WHERE id = ANY($1) AND breaker_recorded_at IS NULL",
            )
            .bind(closes)
            .execute(&mut *tx)
            .await
            {
                warn!(%error, "Could not mark closes recorded with the circuit breaker");
                sqlx::query("ROLLBACK TO SAVEPOINT close_marks")
                    .execute(&mut *tx)
                    .await?;
                marked = false;
            }
        }
        tx.commit().await?;

        debug!(
            tripped = state.tripped,
            saved, "Saved circuit breaker state"
        );
        Ok(SaveOutcome {
            saved,
            marked: saved && marked,
        })
    }

    /// Mark closes as seen without counting them (the breaker is switched
    /// off): a new leader must not count them either.
    pub async fn mark_closes(&self, closes: &[uuid::Uuid]) -> Result<()> {
        sqlx::query(
            "UPDATE positions SET breaker_recorded_at = NOW() \
             WHERE id = ANY($1) AND breaker_recorded_at IS NULL",
        )
        .bind(closes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Store an operator's change (a manual trip or a reset): the trip, the
    /// recovery stage, the drawdown references and the day's trip count,
    /// leaving the counters the trading process keeps. Counts the operator
    /// epoch up and returns it.
    pub async fn save_operator_change(&self, state: &CircuitBreakerState) -> Result<i64> {
        let trip_reason_str = state.trip_reason.as_ref().map(Self::trip_reason_to_string);

        let epoch = sqlx::query_scalar(
            r#"
            INSERT INTO circuit_breaker_state (
                id, tripped, trip_reason, tripped_at, resume_at, daily_pnl, peak_value,
                current_value, consecutive_losses, trips_today, last_reset_date,
                drawdown_peak, portfolio_value_seeded, recovery_state,
                cash_flows_applied_through, operator_epoch, updated_at
            )
            VALUES (1, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, 1, NOW())
            ON CONFLICT (id) DO UPDATE SET
                tripped = EXCLUDED.tripped,
                trip_reason = EXCLUDED.trip_reason,
                tripped_at = EXCLUDED.tripped_at,
                resume_at = EXCLUDED.resume_at,
                peak_value = EXCLUDED.peak_value,
                drawdown_peak = EXCLUDED.drawdown_peak,
                trips_today = EXCLUDED.trips_today,
                recovery_state = EXCLUDED.recovery_state,
                operator_epoch = circuit_breaker_state.operator_epoch + 1,
                updated_at = NOW()
            RETURNING operator_epoch
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
        .fetch_one(&self.pool)
        .await?;

        info!(
            tripped = state.tripped,
            epoch, "Saved an operator change to the circuit breaker"
        );
        Ok(epoch)
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
        for migration in [
            include_str!("../../../migrations/079_circuit_breaker_full_state.sql"),
            include_str!("../../../migrations/094_circuit_breaker_operator_epoch.sql"),
        ] {
            sqlx::raw_sql(migration).execute(pool).await.unwrap();
        }
    }

    #[sqlx::test(migrations = false)]
    async fn closes_are_marked_only_with_the_save_that_stores_them(pool: PgPool) {
        create_schema(&pool).await;
        let repo = CircuitBreakerRepository::new(pool.clone());
        let close = uuid::Uuid::new_v4();
        let state = CircuitBreakerState {
            daily_pnl: rust_decimal::Decimal::from(-5),
            ..CircuitBreakerState::default()
        };
        // No positions table: the counters still save, the mark is left out
        // and reported, for the next save to make.
        assert_eq!(
            repo.save(&state, 0, &[close]).await.unwrap(),
            SaveOutcome {
                saved: true,
                marked: false
            }
        );
        let stored_pnl: rust_decimal::Decimal =
            sqlx::query_scalar("SELECT daily_pnl FROM circuit_breaker_state WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored_pnl, rust_decimal::Decimal::from(-5));
        sqlx::query(
            "CREATE TABLE positions (id UUID PRIMARY KEY, breaker_recorded_at TIMESTAMPTZ)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO positions (id) VALUES ($1)")
            .bind(close)
            .execute(&pool)
            .await
            .unwrap();
        // A save refused for an operator change marks nothing.
        assert!(!repo.save(&state, 7, &[close]).await.unwrap().saved);
        let marked = || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT breaker_recorded_at IS NOT NULL FROM positions WHERE id = $1",
            )
            .bind(close)
            .fetch_one(&pool)
            .await
            .unwrap()
        };
        assert!(!marked().await);
        assert_eq!(
            repo.save(&state, 0, &[close]).await.unwrap(),
            SaveOutcome {
                saved: true,
                marked: true
            }
        );
        assert!(marked().await);
    }

    #[sqlx::test(migrations = false)]
    async fn a_close_whose_mark_failed_is_marked_by_a_later_save(pool: PgPool) {
        use rust_decimal::Decimal;
        create_schema(&pool).await;
        let breaker = crate::CircuitBreaker::with_persistence(config(), pool.clone());
        breaker.load_state().await.unwrap();
        let close = uuid::Uuid::new_v4();
        // No positions table: the counters save, the mark fails, and the
        // close stays owed (a replay must not count it again).
        breaker
            .record_close(Decimal::from(-5), close)
            .await
            .unwrap();
        assert!(breaker.counted_unsaved(close));
        let stored_pnl = || async {
            sqlx::query_scalar::<_, Decimal>(
                "SELECT daily_pnl FROM circuit_breaker_state WHERE id = 1",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        };
        assert_eq!(stored_pnl().await, Decimal::from(-5));
        sqlx::query(
            "CREATE TABLE positions (id UUID PRIMARY KEY, breaker_recorded_at TIMESTAMPTZ)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO positions (id) VALUES ($1)")
            .bind(close)
            .execute(&pool)
            .await
            .unwrap();
        // The next save, for another result, marks the close it kept.
        breaker.record_trade(Decimal::from(-1)).await.unwrap();
        assert_eq!(stored_pnl().await, Decimal::from(-6));
        let marked: bool = sqlx::query_scalar(
            "SELECT breaker_recorded_at IS NOT NULL FROM positions WHERE id = $1",
        )
        .bind(close)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(marked);
        assert!(!breaker.counted_unsaved(close));
    }

    #[sqlx::test(migrations = false)]
    async fn a_latch_saved_without_its_row_survives_restart(pool: PgPool) {
        create_schema(&pool).await;
        sqlx::query("DELETE FROM circuit_breaker_state")
            .execute(&pool)
            .await
            .unwrap();
        let breaker = crate::CircuitBreaker::with_persistence(config(), pool.clone());
        breaker.manual_trip(Some("operator".into())).await.unwrap();
        let restarted = crate::CircuitBreaker::with_persistence(config(), pool);
        assert!(restarted.load_state().await.unwrap());
        assert!(restarted.is_tripped());
        assert!(!restarted.can_trade().await);
    }

    #[sqlx::test(migrations = false)]
    async fn a_trip_made_through_a_follower_is_adopted_not_overwritten(pool: PgPool) {
        use crate::CircuitBreaker;
        use rust_decimal::Decimal;

        create_schema(&pool).await;
        let leader = CircuitBreaker::with_persistence(config(), pool.clone());
        leader.load_state().await.unwrap();
        leader
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();
        let follower = CircuitBreaker::with_persistence(config(), pool.clone()).following();
        follower.load_state().await.unwrap();

        // The follower counts nothing into the stored state...
        follower.record_trade(Decimal::new(-50, 0)).await.unwrap();
        // ...but its operator trip is stored at once.
        follower.manual_trip(Some("operator".into())).await.unwrap();

        // The leader's next save finds the trip, takes it, and keeps the
        // loss it counted itself.
        leader.record_trade(Decimal::new(-5, 0)).await.unwrap();
        assert!(leader.is_tripped());
        assert!(!leader.can_trade().await);
        let (stored, epoch) = CircuitBreakerRepository::new(pool.clone())
            .load()
            .await
            .unwrap()
            .unwrap();
        assert!(stored.tripped);
        assert_eq!(stored.trip_reason, Some(TripReason::Manual));
        assert_eq!(stored.daily_pnl, Decimal::new(-5, 0));
        assert_eq!(stored.peak_value, Decimal::new(1000, 0));
        assert_eq!(epoch, 1);

        // A reset through the follower reaches the leader without a save.
        follower.reset().await.unwrap();
        let reader = CircuitBreaker::with_persistence(config(), pool).following();
        assert!(reader.can_trade().await);
        assert!(leader.is_tripped());
        tokio::time::sleep(crate::circuit_breaker::LEADER_SYNC).await;
        assert!(leader.can_trade().await);
        assert!(!leader.is_tripped());
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
        breaker.record_trade(Decimal::new(-10, 0)).await.unwrap();
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

        let persisted: chrono::NaiveDate =
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
            breaker.record_trade(Decimal::NEGATIVE_ONE).await.unwrap();
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
