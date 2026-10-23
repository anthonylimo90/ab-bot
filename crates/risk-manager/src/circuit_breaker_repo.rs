//! Database repository for circuit breaker state persistence.

use anyhow::Result;
use sqlx::{PgPool, Row};
use tracing::{debug, info, warn};

use crate::circuit_breaker::{CircuitBreakerState, TripReason};

/// What [`CircuitBreakerRepository::save`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveOutcome {
    /// The state was saved, with the closes passed marked recorded.
    pub saved: bool,
    /// Refused because another process has claimed the lead since this one
    /// did: this process must stop saving.
    pub fenced: bool,
}

/// The stored state, with what fences and shares it.
#[derive(Debug, Clone)]
pub struct StoredState {
    pub state: CircuitBreakerState,
    /// Counted up by each operator change (a trip, a reset).
    pub operator_epoch: i64,
    /// Counted up by each process taking over trading.
    pub leader_epoch: i64,
    /// Why the leader holds new entries.
    pub entry_holds: Vec<String>,
}

/// Repository for circuit breaker state persistence.
pub struct CircuitBreakerRepository {
    pool: PgPool,
}

impl CircuitBreakerRepository {
    /// Create a new repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Load the stored state, with its epochs and the leader's holds.
    pub async fn load(&self) -> Result<Option<StoredState>> {
        let row = sqlx::query(
            r#"
            SELECT
                tripped, trip_reason, tripped_at, resume_at,
                daily_pnl, peak_value, drawdown_peak, current_value,
                consecutive_losses, trips_today, last_reset_date,
                portfolio_value_seeded, recovery_state, cash_flows_applied_through,
                operator_epoch, leader_epoch, entry_holds
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
            let entry_holds = serde_json::from_value::<Vec<String>>(r.get("entry_holds"))
                .unwrap_or_else(|error| {
                    // Unreadable holds still hold: entries wait until the
                    // leader stores its own.
                    warn!(%error, "Unreadable stored entry holds");
                    vec!["stored entry holds that cannot be read".to_string()]
                });
            StoredState {
                state,
                operator_epoch: r.get("operator_epoch"),
                leader_epoch: r.get("leader_epoch"),
                entry_holds,
            }
        });

        if let Some(StoredState { state: ref s, .. }) = state {
            info!(
                tripped = s.tripped,
                daily_pnl = %s.daily_pnl,
                consecutive_losses = s.consecutive_losses,
                "Loaded circuit breaker state from database"
            );
        }

        Ok(state)
    }

    /// Claim the lead: count the leader epoch up and return it. From here a
    /// save carrying an earlier epoch (a previous leader's, stalled) is
    /// refused. 1 is the first claim (the version before this one never
    /// claimed).
    pub async fn claim_leadership(&self) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO circuit_breaker_state (id, leader_epoch) VALUES (1, 1) \
             ON CONFLICT (id) DO UPDATE SET leader_epoch = circuit_breaker_state.leader_epoch + 1 \
             RETURNING leader_epoch",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    /// The operator epoch stored now, if the row exists.
    pub async fn operator_epoch(&self) -> Result<Option<i64>> {
        Ok(
            sqlx::query_scalar("SELECT operator_epoch FROM circuit_breaker_state WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Save the whole state and the leader's `entry_holds`, provided no
    /// operator change has been stored since `epoch` and no other process
    /// has claimed the lead since `leader_epoch`, and say what it did. The
    /// single row is written whether or not it exists yet: a missing row
    /// must never make a save (a latched trip) a silent no-op. The positions
    /// in `closes` (closes the state now counts) are marked recorded in the
    /// same transaction, all or nothing: a close is marked exactly when the
    /// saved counters hold it, and a mark that fails fails the save.
    pub async fn save(
        &self,
        state: &CircuitBreakerState,
        epoch: i64,
        leader_epoch: i64,
        entry_holds: &[String],
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
                cash_flows_applied_through, operator_epoch, leader_epoch, entry_holds,
                updated_at
            )
            VALUES (
                1, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                NOW()
            )
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
                entry_holds = EXCLUDED.entry_holds,
                updated_at = NOW()
            WHERE circuit_breaker_state.operator_epoch = EXCLUDED.operator_epoch
              AND circuit_breaker_state.leader_epoch = EXCLUDED.leader_epoch
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
        .bind(leader_epoch)
        .bind(serde_json::to_value(entry_holds)?)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;
        if !saved {
            // Refused: an operator change since `epoch`, or another leader.
            let stored_leader: i64 =
                sqlx::query_scalar("SELECT leader_epoch FROM circuit_breaker_state WHERE id = 1")
                    .fetch_one(&mut *tx)
                    .await?;
            return Ok(SaveOutcome {
                saved: false,
                fenced: stored_leader != leader_epoch,
            });
        }
        if !closes.is_empty() {
            // In the same transaction: an error here rolls the counters back
            // too, so the closes stay queued and unmarked, counted nowhere
            // but this process's memory, until a save stores both.
            sqlx::query(
                "UPDATE positions SET breaker_recorded_at = NOW() \
                 WHERE id = ANY($1) AND breaker_recorded_at IS NULL",
            )
            .bind(closes)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        debug!(tripped = state.tripped, "Saved circuit breaker state");
        Ok(SaveOutcome {
            saved: true,
            fenced: false,
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
            include_str!("../../../migrations/099_circuit_breaker_leader_fence.sql"),
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
        let stored_pnl = || async {
            sqlx::query_scalar::<_, rust_decimal::Decimal>(
                "SELECT daily_pnl FROM circuit_breaker_state WHERE id = 1",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        };
        // No positions table: the marks fail, and the counters with them.
        assert!(repo.save(&state, 0, 0, &[], &[close]).await.is_err());
        assert_eq!(stored_pnl().await, rust_decimal::Decimal::ZERO);
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
        // A save refused for an operator change marks nothing, and is not
        // fenced (no other leader).
        assert_eq!(
            repo.save(&state, 7, 0, &[], &[close]).await.unwrap(),
            SaveOutcome {
                saved: false,
                fenced: false
            }
        );
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
            repo.save(&state, 0, 0, &[], &[close]).await.unwrap(),
            SaveOutcome {
                saved: true,
                fenced: false
            }
        );
        assert!(marked().await);
        assert_eq!(stored_pnl().await, rust_decimal::Decimal::from(-5));
        // Another process claims the lead: a save of the old epoch is fenced.
        assert_eq!(repo.claim_leadership().await.unwrap(), 1);
        assert_eq!(
            repo.save(&state, 0, 0, &[], &[]).await.unwrap(),
            SaveOutcome {
                saved: false,
                fenced: true
            }
        );
    }

    #[sqlx::test(migrations = false)]
    async fn a_close_whose_mark_failed_is_marked_by_a_later_save(pool: PgPool) {
        use rust_decimal::Decimal;
        create_schema(&pool).await;
        let breaker = crate::CircuitBreaker::with_persistence(config(), pool.clone());
        breaker.load_state().await.unwrap();
        let close = uuid::Uuid::new_v4();
        // No positions table: the mark fails and the save with it; the close
        // stays queued, counted only in this process (a replay here must
        // not count it again).
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
        assert_eq!(breaker.state().await.daily_pnl, Decimal::from(-5));
        assert_eq!(stored_pnl().await, Decimal::ZERO);
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
        // A flush before stopping (as a sync shortly) saves the counters and
        // marks the close together.
        breaker.flush().await;
        assert_eq!(stored_pnl().await, Decimal::from(-5));
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
    async fn a_switched_off_breaker_marks_a_close_without_counting_it(pool: PgPool) {
        use rust_decimal::Decimal;
        create_schema(&pool).await;
        sqlx::query(
            "CREATE TABLE positions (id UUID PRIMARY KEY, breaker_recorded_at TIMESTAMPTZ)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let close = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO positions (id) VALUES ($1)")
            .bind(close)
            .execute(&pool)
            .await
            .unwrap();
        let breaker = crate::CircuitBreaker::with_persistence(
            crate::CircuitBreakerConfig {
                enabled: false,
                ..config()
            },
            pool.clone(),
        );
        breaker.load_state().await.unwrap();
        breaker
            .record_close(Decimal::from(-5), close)
            .await
            .unwrap();
        assert!(!breaker.counted_unsaved(close));
        assert_eq!(breaker.state().await.daily_pnl, Decimal::ZERO);
        let marked: bool = sqlx::query_scalar(
            "SELECT breaker_recorded_at IS NOT NULL FROM positions WHERE id = $1",
        )
        .bind(close)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(marked);
    }

    #[sqlx::test(migrations = false)]
    async fn a_previous_leaders_save_is_refused_and_it_stops_saving(pool: PgPool) {
        use rust_decimal::Decimal;
        create_schema(&pool).await;
        let old = crate::CircuitBreaker::with_persistence(config(), pool.clone());
        old.lead().await.unwrap();
        assert!(old.first_leader());
        // Another process takes over (the old one's lock went, it has not
        // noticed yet).
        let new = crate::CircuitBreaker::with_persistence(config(), pool.clone());
        new.lead().await.unwrap();
        assert!(!new.first_leader());
        new.record_trade(Decimal::from(-3)).await.unwrap();
        // The old leader's save is refused, and it stops saving.
        old.record_trade(Decimal::from(-40)).await.unwrap();
        assert!(!old.is_leader());
        let stored = CircuitBreakerRepository::new(pool.clone())
            .load()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state.daily_pnl, Decimal::from(-3));
        assert_eq!(stored.leader_epoch, 2);
        // The new leader goes on saving.
        new.record_trade(Decimal::from(-1)).await.unwrap();
        let stored = CircuitBreakerRepository::new(pool.clone())
            .load()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state.daily_pnl, Decimal::from(-4));
    }

    #[sqlx::test(migrations = false)]
    async fn the_leaders_holds_stop_entries_in_every_process(pool: PgPool) {
        create_schema(&pool).await;
        let leader = crate::CircuitBreaker::with_persistence(config(), pool.clone());
        leader.lead().await.unwrap();
        leader.hold_entries("missed_closes", "closes not yet counted");
        leader.flush().await;
        let follower = crate::CircuitBreaker::with_persistence(config(), pool.clone()).following();
        assert!(!follower.can_trade().await);
        assert_eq!(
            follower.entry_holds(),
            vec!["closes not yet counted".to_string()]
        );
        // Lifted, and stored lifted.
        leader.release_entries("missed_closes");
        leader.flush().await;
        let later = crate::CircuitBreaker::with_persistence(config(), pool.clone()).following();
        assert!(later.can_trade().await);
        assert!(later.entry_holds().is_empty());
        // The leader itself answers for its own holds only.
        assert!(leader.entry_holds().is_empty());
        assert!(leader.can_trade().await);
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
        let crate::StoredState {
            state: stored,
            operator_epoch: epoch,
            ..
        } = CircuitBreakerRepository::new(pool.clone())
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
