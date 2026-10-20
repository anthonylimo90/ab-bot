use super::*;
use chrono::{DateTime, Utc};

/// Publish the API projection atomically; missing active markets become inactive.
pub(super) async fn persist_markets(
    pool: &PgPool,
    markets: &[Market],
    complete: bool,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let observed_at = Utc::now();
    for chunk in markets.chunks(250) {
        let mut query = sqlx::QueryBuilder::new(
            "INSERT INTO markets (id,question,description,category,end_date,active,yes_price,no_price,volume_24h,liquidity,updated_at) "
        );
        query.push_values(chunk, |mut row, market| {
            let (yes, no) = extract_prices(&market.outcomes);
            row.push_bind(&market.id)
                .push_bind(&market.question)
                .push_bind(&market.description)
                .push_bind(
                    market
                        .category
                        .clone()
                        .unwrap_or_else(|| infer_category(&market.question)),
                )
                .push_bind(
                    market
                        .end_date
                        .unwrap_or(observed_at + chrono::Duration::days(365)),
                )
                .push_bind(!market.resolved)
                .push_bind(yes)
                .push_bind(no)
                .push_bind(market.volume)
                .push_bind(market.liquidity)
                .push_bind(observed_at);
        });
        query.push(" ON CONFLICT (id) DO UPDATE SET question=EXCLUDED.question,description=EXCLUDED.description,category=EXCLUDED.category,end_date=EXCLUDED.end_date,active=EXCLUDED.active,yes_price=EXCLUDED.yes_price,no_price=EXCLUDED.no_price,volume_24h=EXCLUDED.volume_24h,liquidity=EXCLUDED.liquidity,updated_at=EXCLUDED.updated_at");
        query.build().execute(&mut *tx).await?;
    }
    if complete {
        let ids: Vec<&str> = markets.iter().map(|market| market.id.as_str()).collect();
        sqlx::query(
            "UPDATE markets SET active=FALSE,updated_at=$2 WHERE active=TRUE AND NOT (id=ANY($1))",
        )
        .bind(ids)
        .bind(observed_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO market_catalog_sync(singleton,refreshed_at) VALUES(TRUE,$1) ON CONFLICT(singleton) DO UPDATE SET refreshed_at=EXCLUDED.refreshed_at")
            .bind(observed_at).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn catalog_observed_at(
    pool: &PgPool,
) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    sqlx::query_scalar("SELECT refreshed_at FROM market_catalog_sync WHERE singleton=TRUE")
        .fetch_optional(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Executor;

    #[sqlx::test(migrations = false)]
    async fn publishes_complete_projection_and_retains_inactive_history(pool: PgPool) {
        let schema = include_str!("../../../../migrations/005_api_server_tables.sql");
        pool.execute(
            schema
                .split("CREATE TABLE IF NOT EXISTS orders")
                .next()
                .unwrap(),
        )
        .await
        .unwrap();
        pool.execute(include_str!(
            "../../../../migrations/073_market_catalog_sync.sql"
        ))
        .await
        .unwrap();
        assert!(catalog_observed_at(&pool).await.unwrap().is_none());
        let mut market: Market = serde_json::from_value(serde_json::json!({
            "id":"b","question":"Bitcoin?","description":null,
            "outcomes":[{"id":"y","token_id":"y","name":"Yes","price":"0.4"},
                        {"id":"n","token_id":"n","name":"No","price":"0.6"}],
            "volume":"100","liquidity":"50","end_date":null,"resolved":false,"resolution":null
        }))
        .unwrap();
        persist_markets(&pool, &[market.clone()], true)
            .await
            .unwrap();
        let first = catalog_observed_at(&pool).await.unwrap().unwrap();
        market.id = "a".into();
        persist_markets(&pool, &[market], true).await.unwrap();
        let rows: Vec<(String, bool, Decimal)> =
            sqlx::query_as("SELECT id,active,yes_price FROM markets ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                ("a".into(), true, Decimal::new(4, 1)),
                ("b".into(), false, Decimal::new(4, 1))
            ]
        );
        assert!(catalog_observed_at(&pool).await.unwrap().unwrap() >= first);
    }
}
