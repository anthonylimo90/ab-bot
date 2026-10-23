use super::*;
use polymarket_core::api::clob::PostOrderResponse;
use polymarket_core::signing::SignedOrder;

/// How long a submission waits for another order of the same wallet that is
/// mid-POST, and how often it looks again.
pub(super) const FENCE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
/// How long the second leg of a pair waits at most: as long as a POST may
/// take (the CLOB client's 30-second timeout), and never longer than the
/// executor's own timeout. At the 30-second default, preparation, the wait
/// and the POST together stay within the safety timeout (three times it).
pub(super) const PAIR_FENCE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
const FENCE_POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// Counts a live submission from admission until it is dropped.
struct InFlight<'a>(&'a AtomicUsize);

impl<'a> InFlight<'a> {
    fn enter(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl OrderExecutor {
    pub(super) async fn submit_live_once(
        &self,
        client: &AuthenticatedClobClient,
        id: Uuid,
        market_id: &str,
        signed: SignedOrder,
        kind: OrderType,
        fence_wait: std::time::Duration,
    ) -> Result<PostOrderResponse> {
        self.submit_once(id, market_id, &signed, fence_wait, || {
            client.post_order(signed.clone(), kind)
        })
        .await
    }

    pub async fn submit_wallet_order(
        &self,
        client: &ClobClient,
        id: Uuid,
        signed: SignedOrder,
        credentials: &polymarket_core::api::clob::ApiCredentials,
    ) -> Result<PostOrderResponse> {
        let address = signed.maker.clone();
        self.submit_once(id, &signed.token_id, &signed, FENCE_WAIT, || {
            client.post_signed_order(&address, credentials, signed.clone(), OrderType::Gtd)
        })
        .await
    }

    // The durable fence is committed before executing the HTTP operation. A row
    // left in submitting/unknown blocks further live submissions across replicas.
    async fn submit_once<F, Fut>(
        &self,
        id: Uuid,
        market_id: &str,
        signed: &SignedOrder,
        fence_wait: std::time::Duration,
        operation: F,
    ) -> Result<PostOrderResponse>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = polymarket_core::Result<PostOrderResponse>>,
    {
        let pool = self
            .database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Live execution requires durable intent storage"))?;
        // The order's own hash is how an answer that names no order, or an
        // unclear one, is found again on the exchange.
        if signed.order_hash.as_deref().is_none_or(str::is_empty) {
            anyhow::bail!("The signed order carries no hash; nothing was sent");
        }
        // A shutdown waits for this submission's outcome to be recorded.
        let _in_flight = InFlight::enter(&self.submissions_in_flight);
        // Another order for this wallet that is mid-POST resolves within
        // moments: wait briefly instead of failing every concurrent strategy.
        let wait_until = std::time::Instant::now() + fence_wait;
        loop {
            let mut tx = pool.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock(hashtext('live-order-submission'))")
                .execute(&mut *tx)
                .await?;
            let retired: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM position_order_links WHERE id=$1 AND settled) \
                 OR EXISTS(SELECT 1 FROM orders WHERE id=$1 AND status='rejected')",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            if retired {
                anyhow::bail!("Order attempt was retired before submission");
            }
            let existing: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution_intents WHERE id = $1)")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await?;
            if existing {
                // A previous attempt with this id may have reached the exchange.
                anyhow::bail!(
                    "{SUBMISSION_UNKNOWN}: intent already journaled; reconcile before trading"
                );
            }
            // (unresolved, in flight): `unknown` and stale `submitting` rows need
            // reconciliation; fresh `submitting` rows are being posted now.
            let (unresolved, in_flight): (bool, bool) = sqlx::query_as(
                "SELECT \
                   COALESCE(BOOL_OR(status = 'unknown' OR created_at < NOW() - INTERVAL '2 minutes'), FALSE), \
                   COALESCE(BOOL_OR(status = 'submitting'), FALSE) \
                 FROM execution_intents \
                 WHERE status IN ('submitting', 'unknown') AND LOWER(signed_order->>'maker') = LOWER($1)",
            )
            .bind(&signed.maker)
            .fetch_one(&mut *tx)
            .await?;
            if unresolved {
                anyhow::bail!(
                    "{SUBMISSION_FENCE_BUSY}: an earlier live order for this wallet awaits reconciliation; nothing was sent"
                );
            }
            if in_flight {
                drop(tx);
                if std::time::Instant::now() >= wait_until {
                    anyhow::bail!(
                        "{SUBMISSION_FENCE_BUSY}: another live order for this wallet is still being posted; nothing was sent"
                    );
                }
                tokio::time::sleep(FENCE_POLL).await;
                continue;
            }
            sqlx::query("INSERT INTO execution_intents (id, market_id, exchange_order_id, signed_order, status) VALUES ($1, $2, $3, $4, 'submitting')")
                .bind(id).bind(market_id).bind(&signed.order_hash).bind(serde_json::to_value(signed)?)
                .execute(&mut *tx).await?;
            tx.commit().await?;
            break;
        }

        match operation().await {
            Ok(response) => {
                // An answer without an id keeps the order's own hash as its
                // identity. One the exchange refused outright is rejected; an
                // answer that is neither a refusal nor an acceptance leaves
                // the order's existence unknown, for the reconciler to read
                // from the exchange under the order's own hash. Only a
                // submission still `submitting` takes the answer: the
                // reconciler may have settled it from the exchange already.
                let status = if response.refused() {
                    "rejected"
                } else if response.unclear() {
                    "unknown"
                } else {
                    "acknowledged"
                };
                sqlx::query("UPDATE execution_intents SET status = $4, exchange_order_id = CASE WHEN $4 = 'unknown' THEN COALESCE(exchange_order_id, NULLIF($2, '')) ELSE COALESCE(NULLIF($2, ''), exchange_order_id) END, response = $3, updated_at = NOW() WHERE id = $1 AND status = 'submitting'")
                    .bind(id).bind(&response.order_id).bind(serde_json::to_value(&response)?).bind(status)
                    .execute(pool).await.map_err(|e| anyhow::anyhow!("{SUBMISSION_UNKNOWN}: acknowledgement persistence failed: {e}"))?;
                if status == "unknown" {
                    anyhow::bail!(
                        "{SUBMISSION_UNKNOWN}: the exchange's answer does not say whether it took the order \
                         (success {:?}, status {:?}, order {:?}): {}",
                        response.success,
                        response.status,
                        response.order_id,
                        response.error_msg.as_deref().unwrap_or_default()
                    );
                }
                Ok(response)
            }
            Err(error) => {
                // An explicit 4xx rejection confirms no order was accepted, and
                // a connection that never opened carried no request. All other
                // transport, decode, gateway and server failures are
                // ambiguous: the order may have reached the exchange.
                let unsent =
                    matches!(&error, polymarket_core::Error::Http(http) if http.is_connect());
                let rejected = unsent
                    || matches!(&error, polymarket_core::Error::Api { status: Some(code), .. } if (400..500).contains(code) && *code != 408);
                let status = if rejected { "rejected" } else { "unknown" };
                if let Err(db_error) = sqlx::query(
                    "UPDATE execution_intents SET status = $2, updated_at = NOW() \
                     WHERE id = $1 AND status = 'submitting'",
                )
                .bind(id)
                .bind(status)
                .execute(pool)
                .await
                {
                    // The journal still says `submitting`: keep the fence.
                    anyhow::bail!(
                        "{SUBMISSION_UNKNOWN}: {error}; outcome persistence failed: {db_error}"
                    );
                }
                if unsent {
                    Err(anyhow::anyhow!(
                        "{error}: the connection to the exchange failed, so nothing was sent"
                    ))
                } else if rejected {
                    Err(anyhow::anyhow!(error.to_string()))
                } else {
                    Err(anyhow::anyhow!("{SUBMISSION_UNKNOWN}: {error}"))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Executor;
    use std::sync::atomic::AtomicUsize;

    #[sqlx::test(migrations = false)]
    async fn ambiguous_submission_is_fenced_across_restart_and_new_intents(pool: sqlx::PgPool) {
        pool.execute(include_str!(
            "../../../../migrations/069_execution_intents.sql"
        ))
        .await
        .unwrap();
        pool.execute("CREATE TABLE position_order_links(id UUID,settled BOOLEAN)")
            .await
            .unwrap();
        pool.execute("CREATE TABLE orders(id UUID,status TEXT)")
            .await
            .unwrap();
        let make_executor = || {
            OrderExecutor::new(
                Arc::new(ClobClient::new(None, None)),
                ExecutorConfig::default(),
            )
            .with_database(pool.clone())
        };
        let signed = OrderSigner::new(
            TradingWallet::from_private_key(&format!("{:064x}", 1))
                .unwrap()
                .signer()
                .clone(),
        );
        let order = signed
            .order_builder()
            .token_id_str("1")
            .side(SigningOrderSide::Buy)
            .price(Decimal::new(5, 1))
            .size(Decimal::ONE)
            .expires_at(0)
            .build()
            .unwrap();
        let signed = signed.sign_order(&order).await.unwrap();
        let count = AtomicUsize::new(0);
        let id = Uuid::new_v4();
        let error = make_executor()
            .submit_once(id, "market", &signed, FENCE_WAIT, || async {
                count.fetch_add(1, Ordering::SeqCst);
                Err(polymarket_core::Error::Api {
                    message: "gateway lost accepted response".into(),
                    status: Some(504),
                })
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains(SUBMISSION_UNKNOWN));
        // Replaying the journaled id stays ambiguous; any other order for the
        // wallet is definitively not sent while the first awaits reconciliation.
        for (next_id, marker) in [
            (id, SUBMISSION_UNKNOWN),
            (Uuid::new_v4(), SUBMISSION_FENCE_BUSY),
        ] {
            let error = make_executor()
                .submit_once(next_id, "market", &signed, FENCE_WAIT, || async {
                    count.fetch_add(1, Ordering::SeqCst);
                    unreachable!()
                })
                .await
                .unwrap_err();
            assert!(error.to_string().contains(marker), "{error}");
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let row: (String, String) =
            sqlx::query_as("SELECT status, exchange_order_id FROM execution_intents WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "unknown");
        assert_eq!(Some(row.1), signed.order_hash);
    }

    #[sqlx::test(migrations = false)]
    async fn a_post_whose_connection_never_opened_is_not_sent(pool: sqlx::PgPool) {
        pool.execute(include_str!(
            "../../../../migrations/069_execution_intents.sql"
        ))
        .await
        .unwrap();
        pool.execute("CREATE TABLE position_order_links(id UUID,settled BOOLEAN)")
            .await
            .unwrap();
        pool.execute("CREATE TABLE orders(id UUID,status TEXT)")
            .await
            .unwrap();
        let executor = OrderExecutor::new(
            Arc::new(ClobClient::new(None, None)),
            ExecutorConfig::default(),
        )
        .with_database(pool.clone());
        let signer = OrderSigner::new(
            TradingWallet::from_private_key(&format!("{:064x}", 1))
                .unwrap()
                .signer()
                .clone(),
        );
        let order = signer
            .order_builder()
            .token_id_str("1")
            .side(SigningOrderSide::Buy)
            .price(Decimal::new(5, 1))
            .size(Decimal::ONE)
            .expires_at(0)
            .build()
            .unwrap();
        let signed = signer.sign_order(&order).await.unwrap();
        // Nothing listens on port 1: the connection is refused.
        let refused = reqwest::Client::new()
            .post("http://127.0.0.1:1/order")
            .send()
            .await
            .unwrap_err();
        assert!(refused.is_connect());
        let id = Uuid::new_v4();
        let error = executor
            .submit_once(id, "market", &signed, FENCE_WAIT, || async {
                Err(polymarket_core::Error::from(refused))
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("nothing was sent"), "{error}");
        assert!(!error.contains(SUBMISSION_UNKNOWN), "{error}");
        let status: String = sqlx::query_scalar("SELECT status FROM execution_intents WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        // Rejected, not unknown: the wallet's next order is not fenced.
        assert_eq!(status, "rejected");
    }

    #[sqlx::test(migrations = false)]
    async fn an_answer_without_an_id_keeps_the_order_hash_and_a_refusal_is_rejected(
        pool: sqlx::PgPool,
    ) {
        pool.execute(include_str!(
            "../../../../migrations/069_execution_intents.sql"
        ))
        .await
        .unwrap();
        pool.execute("CREATE TABLE position_order_links(id UUID,settled BOOLEAN)")
            .await
            .unwrap();
        pool.execute("CREATE TABLE orders(id UUID,status TEXT)")
            .await
            .unwrap();
        let executor = OrderExecutor::new(
            Arc::new(ClobClient::new(None, None)),
            ExecutorConfig::default(),
        )
        .with_database(pool.clone());
        let signer = OrderSigner::new(
            TradingWallet::from_private_key(&format!("{:064x}", 1))
                .unwrap()
                .signer()
                .clone(),
        );
        let answer = |json: serde_json::Value| {
            serde_json::from_value::<polymarket_core::api::clob::PostOrderResponse>(json).unwrap()
        };
        for (json, status) in [
            // Taken, but the answer names no order: still found by its hash.
            (
                serde_json::json!({"success": true, "status": "live"}),
                "acknowledged",
            ),
            // Refused outright: nothing to reconcile, nothing reserved.
            (
                serde_json::json!({"success": false, "errorMsg": "not enough balance"}),
                "rejected",
            ),
        ] {
            let order = signer
                .order_builder()
                .token_id_str("1")
                .side(SigningOrderSide::Buy)
                .price(Decimal::new(5, 1))
                .size(Decimal::ONE)
                .expires_at(0)
                .build()
                .unwrap();
            let signed = signer.sign_order(&order).await.unwrap();
            let id = Uuid::new_v4();
            let response = answer(json);
            executor
                .submit_once(id, "market", &signed, FENCE_WAIT, || async { Ok(response) })
                .await
                .unwrap();
            let row: (String, String) = sqlx::query_as(
                "SELECT status, exchange_order_id FROM execution_intents WHERE id=$1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(row.0, status);
            assert_eq!(Some(row.1), signed.order_hash);
        }

        // An answer that neither refuses nor accepts the order may still have
        // placed it: the outcome is unknown, reconciled under the order's own
        // hash, and fences the wallet.
        let sign = |token: &'static str| {
            let order = signer
                .order_builder()
                .token_id_str(token)
                .side(SigningOrderSide::Buy)
                .price(Decimal::new(5, 1))
                .size(Decimal::ONE)
                .expires_at(0)
                .build()
                .unwrap();
            let signer = &signer;
            async move { signer.sign_order(&order).await.unwrap() }
        };
        for (token, json) in [
            (
                "2",
                serde_json::json!({"success": false, "errorMsg": "timeout", "orderID": "0xnamed"}),
            ),
            (
                "3",
                serde_json::json!({"success": true, "orderID": "0xnamed", "status": "unmatched"}),
            ),
        ] {
            let signed = sign(token).await;
            let id = Uuid::new_v4();
            let response = answer(json);
            let error = executor
                .submit_once(id, "market", &signed, FENCE_WAIT, || async { Ok(response) })
                .await
                .unwrap_err();
            assert!(error.to_string().contains(SUBMISSION_UNKNOWN));
            let row: (String, String, serde_json::Value) = sqlx::query_as(
                "SELECT status, exchange_order_id, response FROM execution_intents WHERE id=$1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(row.0, "unknown");
            assert_eq!(Some(row.1), signed.order_hash);
            assert_eq!(row.2["orderID"], "0xnamed");
            let posts = AtomicUsize::new(0);
            let fenced = executor
                .submit_once(
                    Uuid::new_v4(),
                    "market",
                    &sign("4").await,
                    FENCE_WAIT,
                    || async {
                        posts.fetch_add(1, Ordering::SeqCst);
                        Ok(answer(
                            serde_json::json!({"success": true, "status": "live"}),
                        ))
                    },
                )
                .await
                .unwrap_err();
            assert!(fenced.to_string().contains(SUBMISSION_FENCE_BUSY));
            assert_eq!(posts.load(Ordering::SeqCst), 0);
            // Reconciled as never placed, the fence lifts for the next case.
            sqlx::query("UPDATE execution_intents SET status='rejected' WHERE id=$1")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
    }

    #[sqlx::test(migrations = false)]
    async fn an_order_waits_out_another_mid_post_only_as_long_as_it_may(pool: sqlx::PgPool) {
        pool.execute(include_str!(
            "../../../../migrations/069_execution_intents.sql"
        ))
        .await
        .unwrap();
        pool.execute("CREATE TABLE position_order_links(id UUID,settled BOOLEAN)")
            .await
            .unwrap();
        pool.execute("CREATE TABLE orders(id UUID,status TEXT)")
            .await
            .unwrap();
        let executor = OrderExecutor::new(
            Arc::new(ClobClient::new(None, None)),
            ExecutorConfig::default(),
        )
        .with_database(pool.clone());
        let signer = OrderSigner::new(
            TradingWallet::from_private_key(&format!("{:064x}", 3))
                .unwrap()
                .signer()
                .clone(),
        );
        let sign = |token: &'static str| {
            let order = signer
                .order_builder()
                .token_id_str(token)
                .side(SigningOrderSide::Buy)
                .price(Decimal::new(5, 1))
                .size(Decimal::ONE)
                .expires_at(0)
                .build()
                .unwrap();
            let signer = &signer;
            async move { signer.sign_order(&order).await.unwrap() }
        };
        // Another order of the wallet is being posted right now.
        let posting = sign("1").await;
        let posting_id = Uuid::new_v4();
        sqlx::query("INSERT INTO execution_intents (id, market_id, exchange_order_id, signed_order, status) VALUES ($1, 'market', $2, $3, 'submitting')")
            .bind(posting_id)
            .bind(&posting.order_hash)
            .bind(serde_json::to_value(&posting).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        let answer = || {
            serde_json::from_value::<polymarket_core::api::clob::PostOrderResponse>(
                serde_json::json!({"success": true, "status": "matched"}),
            )
            .unwrap()
        };
        // An order that may not wait is refused, unsent.
        let refused = executor
            .submit_once(
                Uuid::new_v4(),
                "market",
                &sign("2").await,
                std::time::Duration::ZERO,
                || async { Ok(answer()) },
            )
            .await
            .unwrap_err();
        assert!(refused.to_string().contains(SUBMISSION_FENCE_BUSY));
        // One that may is sent once the other's answer is recorded.
        let answered = {
            let pool = pool.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                sqlx::query("UPDATE execution_intents SET status='acknowledged' WHERE id=$1")
                    .bind(posting_id)
                    .execute(&pool)
                    .await
                    .unwrap();
            })
        };
        executor
            .submit_once(
                Uuid::new_v4(),
                "market",
                &sign("3").await,
                std::time::Duration::from_secs(5),
                || async { Ok(answer()) },
            )
            .await
            .unwrap();
        answered.await.unwrap();
        assert!(PAIR_FENCE_WAIT > FENCE_WAIT);
    }

    #[sqlx::test(migrations = false)]
    async fn an_order_without_its_hash_is_never_sent(pool: sqlx::PgPool) {
        pool.execute(include_str!(
            "../../../../migrations/069_execution_intents.sql"
        ))
        .await
        .unwrap();
        let executor = OrderExecutor::new(
            Arc::new(ClobClient::new(None, None)),
            ExecutorConfig::default(),
        )
        .with_database(pool.clone());
        let signer = OrderSigner::new(
            TradingWallet::from_private_key(&format!("{:064x}", 1))
                .unwrap()
                .signer()
                .clone(),
        );
        let order = signer
            .order_builder()
            .token_id_str("1")
            .side(SigningOrderSide::Buy)
            .price(Decimal::new(5, 1))
            .size(Decimal::ONE)
            .expires_at(0)
            .build()
            .unwrap();
        let mut signed = signer.sign_order(&order).await.unwrap();
        signed.order_hash = None;
        let posts = AtomicUsize::new(0);
        let error = executor
            .submit_once(Uuid::new_v4(), "market", &signed, FENCE_WAIT, || async {
                posts.fetch_add(1, Ordering::SeqCst);
                Err(polymarket_core::Error::Api {
                    message: "unreachable".into(),
                    status: None,
                })
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("nothing was sent"));
        assert_eq!(posts.load(Ordering::SeqCst), 0);
        let journaled: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_intents")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(journaled, 0);
    }

    #[sqlx::test(migrations = false)]
    async fn a_drain_refuses_new_operations_and_waits_for_started_ones(pool: sqlx::PgPool) {
        pool.execute(include_str!(
            "../../../../migrations/069_execution_intents.sql"
        ))
        .await
        .unwrap();
        pool.execute("CREATE TABLE position_order_links(id UUID,settled BOOLEAN)")
            .await
            .unwrap();
        pool.execute("CREATE TABLE orders(id UUID,status TEXT)")
            .await
            .unwrap();
        let executor = Arc::new(
            OrderExecutor::new(
                Arc::new(ClobClient::new(None, None)),
                ExecutorConfig::default(),
            )
            .with_database(pool.clone()),
        );
        let signer = OrderSigner::new(
            TradingWallet::from_private_key(&format!("{:064x}", 2))
                .unwrap()
                .signer()
                .clone(),
        );
        let order = signer
            .order_builder()
            .token_id_str("1")
            .side(SigningOrderSide::Buy)
            .price(Decimal::new(5, 1))
            .size(Decimal::ONE)
            .expires_at(0)
            .build()
            .unwrap();
        let signed = signer.sign_order(&order).await.unwrap();

        // An operation (say an arb) is between its two legs when shutdown begins.
        let operation = executor
            .begin_operation()
            .expect("admitted before shutdown");
        executor.begin_draining();
        assert!(
            executor.begin_operation().is_none(),
            "no new operation starts"
        );
        // One that only takes risk away (a cancel) starts and is waited for.
        let cancel = executor.track_operation();
        drop(operation);
        assert!(
            !executor.drain(std::time::Duration::from_millis(100)).await,
            "a tracked operation is waited for"
        );
        drop(cancel);
        // The arb, still between its legs, for what follows.
        let operation = executor.track_operation();
        assert!(
            !executor.drain(std::time::Duration::from_millis(100)).await,
            "the started operation is waited for"
        );
        // Its second leg is still sent while draining.
        let (posting_tx, posting_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let second_leg = {
            let executor = executor.clone();
            tokio::spawn(async move {
                let result = executor
                    .submit_once(
                        Uuid::new_v4(),
                        "market",
                        &signed,
                        FENCE_WAIT,
                        || async move {
                            posting_tx.send(()).unwrap();
                            release_rx.await.unwrap();
                            Err(polymarket_core::Error::Api {
                                message: "rejected".into(),
                                status: Some(400),
                            })
                        },
                    )
                    .await;
                drop(operation);
                result
            })
        };
        posting_rx.await.unwrap();
        assert!(!executor.drain(std::time::Duration::from_millis(100)).await);
        release_tx.send(()).unwrap();
        second_leg.await.unwrap().unwrap_err();
        assert!(executor.drain(std::time::Duration::from_secs(5)).await);
    }
}
