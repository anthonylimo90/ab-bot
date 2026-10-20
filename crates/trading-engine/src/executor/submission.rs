use super::*;
use polymarket_core::api::clob::PostOrderResponse;
use polymarket_core::signing::SignedOrder;

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
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('live-order-submission'))")
            .execute(&mut *tx)
            .await?;
        let retired: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM position_order_links WHERE id=$1 AND settled)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if retired {
            anyhow::bail!("Position order attempt was retired before submission");
        }
        let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution_intents WHERE id = $1 OR (status IN ('submitting', 'unknown') AND LOWER(signed_order->>'maker')=LOWER($2)))")
            .bind(id).bind(&signed.maker).fetch_one(&mut *tx).await?;
        if blocked {
            anyhow::bail!("{SUBMISSION_UNKNOWN}: existing or unresolved live intent; reconcile before trading");
        }
        sqlx::query("INSERT INTO execution_intents (id, market_id, exchange_order_id, signed_order, status) VALUES ($1, $2, $3, $4, 'submitting')")
            .bind(id).bind(market_id).bind(&signed.order_hash).bind(serde_json::to_value(signed)?)
            .execute(&mut *tx).await?;
        tx.commit().await?;

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
                sqlx::query(
                    "UPDATE execution_intents SET status = $2, updated_at = NOW() WHERE id = $1",
                )
                .bind(id)
                .bind(status)
                .execute(pool)
                .await?;
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
        for next_id in [id, Uuid::new_v4()] {
            assert!(make_executor()
                .submit_once(next_id, "market", &signed, || async {
                    count.fetch_add(1, Ordering::SeqCst);
                    unreachable!()
                })
                .await
                .is_err());
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
