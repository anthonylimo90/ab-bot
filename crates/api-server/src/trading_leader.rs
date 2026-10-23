//! One API process trades at a time: the trading leader. It holds a Postgres
//! session advisory lock on a connection of its own for as long as it runs.
//! Another process (the next version, while a deploy overlaps them) serves
//! requests and waits for the lock before it starts the trading tasks, so
//! the two never run executors, exits or the circuit breaker's bookkeeping
//! side by side.

use std::sync::Arc;
use std::time::Duration;

use sqlx::{Connection, PgConnection, PgPool};
use tokio::sync::Notify;
use tracing::{error, info, warn};

/// How often a waiting process tries for the lock.
const RETRY: Duration = Duration::from_secs(2);
/// How often the leader checks that the lock's connection is alive: well
/// inside the time another process needs to take the lock after it fails.
const CHECK: Duration = Duration::from_secs(1);
/// How long that check may take before it counts as slow.
const CHECK_TIMEOUT: Duration = Duration::from_secs(3);
/// Slow checks in a row that count as a lost connection. A failed check
/// counts at once: the session, and so the lock, may be gone.
const SLOW_CHECKS: u32 = 3;
/// Waiting attempts between "still waiting" logs (about a minute).
const LOG_EVERY: u64 = 30;
/// Waiting attempts after which the wait is reported as an error (about
/// ten minutes): a holder that never lets go (a dead process whose session
/// the server has not ended) leaves nobody trading.
const WAIT_ALARM: u64 = 300;

/// The held lock. Its connection is out of the pool; dropping it ends the
/// session and so releases the lock.
pub struct Leadership {
    conn: PgConnection,
}

/// Wait until this process is the trading leader, or `None` once `stopping`
/// says the process is shutting down.
pub async fn acquire(pool: &PgPool, stopping: impl Fn() -> bool) -> Option<Leadership> {
    let mut attempts = 0u64;
    loop {
        if stopping() {
            return None;
        }
        match try_acquire(pool).await {
            Ok(Some(leadership)) => {
                info!("This process is now the trading leader");
                return Some(leadership);
            }
            Ok(None) if attempts >= WAIT_ALARM && attempts.is_multiple_of(LOG_EVERY) => {
                error!(
                    waited_secs = attempts * RETRY.as_secs(),
                    "Still waiting for the trading leadership: another session holds it"
                );
            }
            Ok(None) if attempts.is_multiple_of(LOG_EVERY) => {
                info!("Another process is trading; waiting to take over");
            }
            Ok(None) => {}
            Err(error) => warn!(%error, "Could not try for the trading leadership"),
        }
        attempts += 1;
        tokio::time::sleep(RETRY).await;
    }
}

async fn try_acquire(pool: &PgPool) -> Result<Option<Leadership>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    let held =
        sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock(hashtext('trading-leader'))")
            .fetch_one(&mut *conn)
            .await;
    match held {
        Ok(true) => {
            let mut conn = conn.detach();
            // The server ends a session whose client has gone silent (a
            // crashed host, a dropped NAT entry) within about half a minute,
            // and with it the lock.
            if let Err(error) = sqlx::query(
                "SELECT set_config('tcp_keepalives_idle', '10', false), \
                        set_config('tcp_keepalives_interval', '5', false), \
                        set_config('tcp_keepalives_count', '3', false)",
            )
            .execute(&mut conn)
            .await
            {
                warn!(%error, "Could not set keepalives on the trading leadership's session");
            }
            // And one whose host died with data unacknowledged, which
            // keepalives do not cover (PostgreSQL 12 and later).
            if let Err(error) = sqlx::query("SELECT set_config('tcp_user_timeout', '30000', false)")
                .execute(&mut conn)
                .await
            {
                warn!(%error, "Could not set a TCP user timeout on the trading leadership's session");
            }
            Ok(Some(Leadership { conn }))
        }
        // Without the lock the connection goes back to the pool holding nothing.
        Ok(false) => Ok(None),
        Err(error) => {
            // The lock may have been taken before the error: end the session
            // rather than return a connection that may hold it to the pool.
            let _ = conn.detach().close().await;
            Err(error)
        }
    }
}

impl Leadership {
    /// Keep the lock's connection checked; `lost` is notified once it fails,
    /// when another process may take the lock.
    /// `on_lost` runs first, whatever the rest of the process is doing (a
    /// shutdown's drain included): it stops this process acting as leader
    /// before another can take over.
    pub fn watch(mut self, lost: Arc<Notify>, on_lost: impl FnOnce() + Send + 'static) {
        tokio::spawn(async move {
            let mut slow = 0;
            loop {
                tokio::time::sleep(CHECK).await;
                let check = tokio::time::timeout(
                    CHECK_TIMEOUT,
                    sqlx::query("SELECT 1").execute(&mut self.conn),
                )
                .await;
                match check {
                    Ok(Ok(_)) => slow = 0,
                    Ok(Err(error)) => {
                        error!(%error, "The trading leadership's database connection failed; stopping");
                        on_lost();
                        lost.notify_one();
                        return;
                    }
                    Err(_) => {
                        slow += 1;
                        warn!(slow, "The trading leadership's database connection is slow");
                        if slow >= SLOW_CHECKS {
                            error!(
                                "The trading leadership's database connection stopped answering; \
                                 stopping"
                            );
                            on_lost();
                            lost.notify_one();
                            return;
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = false)]
    async fn only_one_process_leads_until_it_lets_go(pool: PgPool) {
        let first = try_acquire(&pool).await.unwrap().expect("free lock");
        // A second process (here another session) is refused while it is held.
        assert!(try_acquire(&pool).await.unwrap().is_none());
        drop(first);
        // Ending the leader's session hands the lock on.
        let mut next = None;
        for _ in 0..50 {
            next = try_acquire(&pool).await.unwrap();
            if next.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(next.is_some());
        // A process that is stopping does not take it.
        drop(next);
        assert!(acquire(&pool, || true).await.is_none());
    }
}
