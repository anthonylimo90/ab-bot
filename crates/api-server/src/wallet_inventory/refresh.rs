use super::*;
use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Serialize};
use std::future::Future;
use tokio::sync::Mutex;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct RefreshStatus {
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    result: Option<serde_json::Value>,
}

pub(crate) async fn status(
    pool: &sqlx::PgPool,
    wallet: &str,
) -> anyhow::Result<Option<RefreshStatus>> {
    Ok(sqlx::query_as("SELECT started_at,finished_at,completed_at,last_error,result FROM wallet_inventory_refresh_status WHERE wallet_address=LOWER($1)")
        .bind(wallet).fetch_optional(pool).await?)
}

/// After a failed scan, how long a scan that may reuse an older result
/// (`max_age` above zero) reports that failure instead of scanning again.
const FAILURE_BACKOFF: chrono::Duration = chrono::Duration::seconds(60);

/// Local serialization prevents pool exhaustion; the transaction lock coordinates
/// replicas. Requests already waiting reuse the completed result of the same scan.
pub(super) async fn coordinate<T, F, Fut>(
    pool: &sqlx::PgPool,
    local: &Mutex<()>,
    wallet: &str,
    max_age: time::Duration,
    operation: F,
) -> anyhow::Result<T>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let requested_at = Utc::now();
    let _local = local.lock().await;
    let mut fence = pool.begin().await?;
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtext('wallet-inventory-refresh'),hashtext(LOWER($1)))",
    )
    .bind(wallet)
    .execute(&mut *fence)
    .await?;
    if let Some(previous) = status(pool, wallet).await? {
        if let Some(finished) = previous.finished_at {
            // The scan this request waited for failed or, unless a fresh
            // scan was demanded, one failed moments ago: report it rather
            // than hammer the sources again.
            let recent_failure = if max_age.is_zero() {
                requested_at
            } else {
                requested_at - FAILURE_BACKOFF
            };
            if finished >= recent_failure {
                if let Some(error) = previous.last_error.as_deref() {
                    anyhow::bail!("{error}");
                }
            }
            if previous.last_error.is_none()
                && previous.completed_at.is_some_and(|at| {
                    at >= requested_at - chrono::Duration::from_std(max_age).unwrap_or_default()
                })
            {
                if let Some(value) = previous.result {
                    return Ok(serde_json::from_value(value)?);
                }
            }
        }
    }
    sqlx::query("INSERT INTO wallet_inventory_refresh_status(wallet_address,started_at) VALUES(LOWER($1),NOW()) ON CONFLICT(wallet_address) DO UPDATE SET started_at=EXCLUDED.started_at,finished_at=NULL,last_error=NULL")
        .bind(wallet).execute(pool).await?;
    let result = operation().await;
    match &result {
        Ok(value) => {
            sqlx::query("UPDATE wallet_inventory_refresh_status SET completed_at=NOW(),finished_at=NOW(),last_error=NULL,result=$2 WHERE wallet_address=LOWER($1)")
                .bind(wallet).bind(serde_json::to_value(value)?).execute(pool).await?;
        }
        Err(error) => {
            sqlx::query("UPDATE wallet_inventory_refresh_status SET finished_at=NOW(),last_error=$2 WHERE wallet_address=LOWER($1)")
                .bind(wallet).bind(error.to_string()).execute(pool).await?;
        }
    }
    fence.commit().await?;
    result
}

pub(super) async fn balances<F, Fut>(
    tokens: Vec<String>,
    read: F,
) -> anyhow::Result<HashMap<String, Decimal>>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = anyhow::Result<Decimal>>,
{
    use futures_util::{stream, StreamExt, TryStreamExt};
    stream::iter(tokens)
        .map(|token| {
            let result = read(token.clone());
            async move { result.await.map(|quantity| (token, quantity)) }
        })
        .buffer_unordered(4)
        .try_collect()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Executor;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[sqlx::test(migrations = false)]
    async fn shares_inflight_scan_across_replicas_and_preserves_last_success_on_error(
        pool: sqlx::PgPool,
    ) {
        pool.execute(include_str!(
            "../../../../migrations/074_inventory_refresh_status.sql"
        ))
        .await
        .unwrap();
        // Separate local mutexes simulate independent API replicas.
        let a = Mutex::new(());
        let b = Mutex::new(());
        let count = AtomicUsize::new(0);
        let operation = || async {
            count.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(time::Duration::from_millis(50)).await;
            Ok(7_i32)
        };
        let (x, y) = tokio::join!(
            coordinate(&pool, &a, "WALLET", time::Duration::ZERO, operation),
            coordinate(&pool, &b, "wallet", time::Duration::ZERO, operation)
        );
        assert_eq!((x.unwrap(), y.unwrap()), (7, 7));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let completed = status(&pool, "wallet").await.unwrap().unwrap().completed_at;
        assert!(completed.is_some());
        // What the withdrawal preflight reports as the inventory's freshness.
        assert_eq!(
            crate::wallet_inventory::last_completed_scan(&pool, "WALLET")
                .await
                .unwrap(),
            completed
        );
        assert_eq!(
            crate::wallet_inventory::last_completed_scan(&pool, "other")
                .await
                .unwrap(),
            None
        );
        let failure: anyhow::Result<i32> =
            coordinate(&pool, &a, "wallet", time::Duration::ZERO, || async {
                anyhow::bail!("balance unavailable")
            })
            .await;
        assert!(failure.is_err());
        let failed = status(&pool, "wallet").await.unwrap().unwrap();
        assert_eq!(failed.completed_at, completed);
        assert_eq!(failed.result, Some(serde_json::json!(7)));
        assert_eq!(failed.last_error.as_deref(), Some("balance unavailable"));
        // A caller content with an older result does not rescan right after
        // a failure; it gets the failure.
        let backed_off: anyhow::Result<i32> = coordinate(
            &pool,
            &a,
            "wallet",
            time::Duration::from_secs(300),
            || async {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(8_i32)
            },
        )
        .await;
        assert_eq!(backed_off.unwrap_err().to_string(), "balance unavailable");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // A demanded refresh always scans.
        assert_eq!(
            coordinate(&pool, &a, "wallet", time::Duration::ZERO, || async {
                Ok(9_i32)
            })
            .await
            .unwrap(),
            9
        );
    }

    #[tokio::test]
    async fn balance_reads_are_bounded_and_failures_are_not_zero() {
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let quantities = balances((0..20).map(|v| v.to_string()).collect(), |_| async {
            let n = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(n, Ordering::SeqCst);
            tokio::task::yield_now().await;
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(Decimal::ONE)
        })
        .await
        .unwrap();
        assert_eq!(quantities.len(), 20);
        assert!(peak.load(Ordering::SeqCst) <= 4);
        assert!(balances(vec!["bad".into()], |_| async {
            anyhow::bail!("unavailable")
        })
        .await
        .is_err());
    }
}
