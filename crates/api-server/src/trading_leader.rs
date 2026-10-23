//! One API process trades at a time: the trading leader. It holds a Postgres
//! session advisory lock on a connection of its own for as long as it runs.
//! Another process (the next version, while a deploy overlaps them) serves
//! requests and waits for the lock before it starts the trading tasks, so
//! the two never run executors, exits or the circuit breaker's bookkeeping
//! side by side.

use std::sync::Arc;
use std::time::Duration;

use sqlx::{PgConnection, PgPool};
use tokio::sync::Notify;
use tracing::{error, info, warn};

/// How often a waiting process tries for the lock.
const RETRY: Duration = Duration::from_secs(2);
/// How often the leader checks that the lock's connection is alive.
const CHECK: Duration = Duration::from_secs(5);
/// Waiting attempts between "still waiting" logs (about a minute).
const LOG_EVERY: u64 = 30;

/// The held lock. Its connection is out of the pool; dropping it ends the
/// session and so releases the lock.
pub struct Leadership {
    conn: PgConnection,
}

/// Wait until this process is the trading leader.
pub async fn acquire(pool: &PgPool) -> Leadership {
    let mut attempts = 0u64;
    loop {
        match try_acquire(pool).await {
            Ok(Some(leadership)) => {
                info!("This process is now the trading leader");
                return leadership;
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
    let held: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext('trading-leader'))")
        .fetch_one(&mut *conn)
        .await?;
    // Without the lock the connection goes back to the pool holding nothing.
    Ok(held.then(|| Leadership {
        conn: conn.detach(),
    }))
}

impl Leadership {
    /// Keep the lock's connection checked; `lost` is notified once it fails,
    /// when another process may take the lock.
    pub fn watch(mut self, lost: Arc<Notify>) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(CHECK).await;
                let alive =
                    tokio::time::timeout(CHECK, sqlx::query("SELECT 1").execute(&mut self.conn))
                        .await;
                if !matches!(alive, Ok(Ok(_))) {
                    error!("The trading leadership's database connection failed; stopping");
                    lost.notify_one();
                    return;
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
    }
}
