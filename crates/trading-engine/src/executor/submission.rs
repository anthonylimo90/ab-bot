use super::*;
use polymarket_core::api::clob::PostOrderResponse;
use polymarket_core::signing::SignedOrder;

/// How long a submission waits for another order of the same wallet that is
/// mid-POST, and how often it looks again.
const FENCE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
const FENCE_POLL: std::time::Duration = std::time::Duration::from_millis(200);

impl OrderExecutor {
    pub(super) async fn submit_live_once(
        &self,
        client: &AuthenticatedClobClient,
        id: Uuid,
        market_id: &str,
        signed: SignedOrder,
        kind: OrderType,
    ) -> Result<PostOrderResponse> {
        self.submit_once(id, market_id, &signed, || {
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
        self.submit_once(id, &signed.token_id, &signed, || {
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
        // Another order for this wallet that is mid-POST resolves within
        // moments: wait briefly instead of failing every concurrent strategy.
        let wait_until = std::time::Instant::now() + FENCE_WAIT;
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
                sqlx::query("UPDATE execution_intents SET status = 'acknowledged', exchange_order_id = $2, response = $3, updated_at = NOW() WHERE id = $1")
                    .bind(id).bind(&response.order_id).bind(serde_json::to_value(&response)?)
                    .execute(pool).await.map_err(|e| anyhow::anyhow!("{SUBMISSION_UNKNOWN}: acknowledgement persistence failed: {e}"))?;
                Ok(response)
            }
            Err(error) => {
                // An explicit 4xx rejection confirms no order was accepted. All
                // transport, decode, gateway and server failures are ambiguous.
                let rejected = matches!(&error, polymarket_core::Error::Api { status: Some(code), .. } if (400..500).contains(code) && *code != 408);
                let status = if rejected { "rejected" } else { "unknown" };
                if let Err(db_error) = sqlx::query(
                    "UPDATE execution_intents SET status = $2, updated_at = NOW() WHERE id = $1",
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
                if rejected {
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
            .submit_once(id, "market", &signed, || async {
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
                .submit_once(next_id, "market", &signed, || async {
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
}
