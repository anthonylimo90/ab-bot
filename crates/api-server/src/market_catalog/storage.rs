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
                .push_bind(market.end_date)
                .push_bind(!market.resolved)
                .push_bind(yes)
                .push_bind(no)
                .push_bind(market.volume)
                .push_bind(market.liquidity)
                .push_bind(observed_at);
        });
        // Rewrite only rows whose content changed: a refresh covers the whole
        // universe, and most markets are unchanged between refreshes.
        query.push(" ON CONFLICT (id) DO UPDATE SET question=EXCLUDED.question,description=EXCLUDED.description,category=EXCLUDED.category,end_date=EXCLUDED.end_date,active=EXCLUDED.active,yes_price=EXCLUDED.yes_price,no_price=EXCLUDED.no_price,volume_24h=EXCLUDED.volume_24h,liquidity=EXCLUDED.liquidity,updated_at=EXCLUDED.updated_at \
            WHERE (markets.question,markets.description,markets.category,markets.end_date,markets.active,markets.yes_price,markets.no_price,markets.volume_24h,markets.liquidity) \
            IS DISTINCT FROM (EXCLUDED.question,EXCLUDED.description,EXCLUDED.category,EXCLUDED.end_date,EXCLUDED.active,EXCLUDED.yes_price,EXCLUDED.no_price,EXCLUDED.volume_24h,EXCLUDED.liquidity)");
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
        pool.execute(include_str!(
            "../../../../migrations/084_market_prices_nullable.sql"
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
        let rows: Vec<(String, bool, Option<Decimal>)> =
            sqlx::query_as("SELECT id,active,yes_price FROM markets ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                ("a".into(), true, Some(Decimal::new(4, 1))),
                ("b".into(), false, Some(Decimal::new(4, 1)))
            ]
        );
        assert!(catalog_observed_at(&pool).await.unwrap().unwrap() >= first);
    }

    #[sqlx::test(migrations = false)]
    async fn prices_follow_the_token_mapping_and_unknown_prices_stay_null(pool: PgPool) {
        let schema = include_str!("../../../../migrations/005_api_server_tables.sql");
        pool.execute(
            schema
                .split("CREATE TABLE IF NOT EXISTS orders")
                .next()
                .unwrap(),
        )
        .await
        .unwrap();
        for migration in [
            include_str!("../../../../migrations/073_market_catalog_sync.sql"),
            include_str!("../../../../migrations/084_market_prices_nullable.sql"),
        ] {
            pool.execute(migration).await.unwrap();
        }
        let market = |id: &str, outcomes: serde_json::Value| -> Market {
            serde_json::from_value(serde_json::json!({
                "id": id, "question": "Who wins?", "description": null, "outcomes": outcomes,
                "volume": "1", "liquidity": "1", "end_date": null, "resolved": false,
                "resolution": null
            }))
            .unwrap()
        };
        let named = market(
            "named",
            serde_json::json!([
                {"id":"a","token_id":"lakers","name":"Lakers","price":"0.3"},
                {"id":"b","token_id":"celtics","name":"Celtics","price":"0.7"}
            ]),
        );
        let unpriced = market(
            "unpriced",
            serde_json::json!([
                {"id":"y","token_id":"y","name":"Yes","price":null},
                {"id":"n","token_id":"n","name":"No","price":"0.6"}
            ]),
        );
        // The YES token of a named-outcome market is its second outcome.
        assert_eq!(yes_no_indices(&named.outcomes), Some((1, 0)));
        persist_markets(&pool, &[named.clone(), unpriced], true)
            .await
            .unwrap();
        let rows: Vec<(String, Option<Decimal>, Option<Decimal>)> =
            sqlx::query_as("SELECT id,yes_price,no_price FROM markets ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                (
                    "named".into(),
                    Some(Decimal::new(7, 1)),
                    Some(Decimal::new(3, 1))
                ),
                ("unpriced".into(), None, Some(Decimal::new(6, 1))),
            ]
        );

        // An unchanged refresh rewrites nothing.
        let stamp = |pool: PgPool| async move {
            sqlx::query_scalar::<_, chrono::DateTime<Utc>>(
                "SELECT updated_at FROM markets WHERE id='named'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        };
        let before = stamp(pool.clone()).await;
        persist_markets(&pool, &[named], false).await.unwrap();
        assert_eq!(stamp(pool.clone()).await, before);
    }
}
