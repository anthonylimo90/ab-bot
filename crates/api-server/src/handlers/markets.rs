//! Market data handlers.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use tracing::warn;
use utoipa::{IntoParams, ToSchema};

use crate::error::{ApiError, ApiResult};
use crate::market_catalog::{extract_prices, infer_category};
use crate::state::AppState;
use axum::http::{HeaderMap, HeaderValue};

/// Market response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MarketResponse {
    /// Market identifier.
    pub id: String,
    /// Market question.
    pub question: String,
    /// Market description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Market category.
    pub category: String,
    /// End date; `null` when the source reports none.
    pub end_date: Option<DateTime<Utc>>,
    /// Whether the market is active.
    pub active: bool,
    /// Current yes price; `null` when the source reports none.
    pub yes_price: Option<Decimal>,
    /// Current no price; `null` when the source reports none.
    pub no_price: Option<Decimal>,
    /// 24h volume.
    pub volume_24h: Decimal,
    /// Total liquidity.
    pub liquidity: Decimal,
    /// Created timestamp.
    pub created_at: DateTime<Utc>,
}

/// Orderbook response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct OrderbookResponse {
    /// Market identifier.
    pub market_id: String,
    /// Timestamp of snapshot.
    pub timestamp: DateTime<Utc>,
    /// Yes outcome bids.
    pub yes_bids: Vec<PriceLevel>,
    /// Yes outcome asks.
    pub yes_asks: Vec<PriceLevel>,
    /// No outcome bids.
    pub no_bids: Vec<PriceLevel>,
    /// No outcome asks.
    pub no_asks: Vec<PriceLevel>,
    /// Spread information.
    pub spread: SpreadInfo,
}

/// Price level in orderbook.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PriceLevel {
    /// Price.
    pub price: Decimal,
    /// Quantity at this price level.
    pub quantity: Decimal,
}

/// Spread information.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SpreadInfo {
    /// Yes bid-ask spread.
    pub yes_spread: Decimal,
    /// No bid-ask spread.
    pub no_spread: Decimal,
    /// Arbitrage spread (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arb_spread: Option<Decimal>,
}

/// Query parameters for listing markets.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListMarketsQuery {
    /// Case-insensitive literal question or category search across the catalog.
    pub search: Option<String>,
    /// Filter by category.
    pub category: Option<String>,
    /// Filter by active status.
    pub active: Option<bool>,
    /// Minimum volume.
    pub min_volume: Option<Decimal>,
    /// Maximum results.
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Offset for pagination.
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, FromRow)]
struct MarketRow {
    id: String,
    question: String,
    description: Option<String>,
    category: String,
    end_date: Option<DateTime<Utc>>,
    active: bool,
    yes_price: Option<Decimal>,
    no_price: Option<Decimal>,
    volume_24h: Decimal,
    liquidity: Decimal,
    created_at: DateTime<Utc>,
}

/// List markets.
#[utoipa::path(
    get,
    path = "/api/v1/markets",
    tag = "markets",
    params(ListMarketsQuery),
    responses(
        (status = 200, description = "List of markets", body = Vec<MarketResponse>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_markets(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListMarketsQuery>,
) -> ApiResult<(HeaderMap, Json<Vec<MarketResponse>>)> {
    let observed_at = crate::market_catalog::catalog_observed_at(&state.pool)
        .await?
        .ok_or_else(|| {
            ApiError::ServiceUnavailable("Market catalog is warming up; retry shortly".into())
        })?;
    let markets = load_market_page(&state.pool, &query).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-catalog-observed-at",
        HeaderValue::from_str(&observed_at.to_rfc3339()).unwrap(),
    );
    headers.insert(
        "x-catalog-stale",
        HeaderValue::from_static(
            if Utc::now().signed_duration_since(observed_at).num_seconds() > 600 {
                "true"
            } else {
                "false"
            },
        ),
    );
    Ok((headers, Json(markets)))
}

async fn load_market_page(
    pool: &sqlx::PgPool,
    query: &ListMarketsQuery,
) -> ApiResult<Vec<MarketResponse>> {
    if query.offset < 0 || !(1..=500).contains(&query.limit) {
        return Err(ApiError::BadRequest(
            "limit must be 1–500 and offset nonnegative".into(),
        ));
    }
    let rows: Vec<MarketRow> = sqlx::query_as(
        "SELECT id,question,description,category,end_date,active,yes_price,no_price,volume_24h,liquidity,created_at
         FROM markets WHERE ($1::boolean IS NULL OR active=$1)
         AND ($2::numeric IS NULL OR volume_24h >= $2)
         AND ($3::text IS NULL OR strpos(lower(category),lower($3)) > 0)
         AND ($6::text IS NULL OR strpos(lower(question),lower($6))>0 OR strpos(lower(category),lower($6))>0)
         ORDER BY id ASC LIMIT $4 OFFSET $5"
    ).bind(query.active).bind(query.min_volume).bind(&query.category)
        .bind(query.limit).bind(query.offset).bind(query.search.as_deref().map(str::trim).filter(|s| !s.is_empty())).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|row| MarketResponse {
            id: row.id,
            question: row.question,
            description: row.description,
            category: row.category,
            end_date: row.end_date,
            active: row.active,
            yes_price: row.yes_price,
            no_price: row.no_price,
            volume_24h: row.volume_24h,
            liquidity: row.liquidity,
            created_at: row.created_at,
        })
        .collect())
}

/// Get a specific market.
#[utoipa::path(
    get,
    path = "/api/v1/markets/{market_id}",
    tag = "markets",
    params(
        ("market_id" = String, Path, description = "Market identifier")
    ),
    responses(
        (status = 200, description = "Market details", body = MarketResponse),
        (status = 404, description = "Market not found")
    )
)]
pub async fn get_market(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<String>,
) -> ApiResult<Json<MarketResponse>> {
    // Query the database for the market
    let result: Option<MarketRow> = sqlx::query_as(
        r#"
        SELECT id, question, description, category, end_date, active,
               yes_price, no_price, volume_24h, liquidity, created_at
        FROM markets
        WHERE id = $1
        "#,
    )
    .bind(&market_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    match result {
        Some(row) => Ok(Json(MarketResponse {
            id: row.id,
            question: row.question,
            description: row.description,
            category: row.category,
            end_date: row.end_date,
            active: row.active,
            yes_price: row.yes_price,
            no_price: row.no_price,
            volume_24h: row.volume_24h,
            liquidity: row.liquidity,
            created_at: row.created_at,
        })),
        None => {
            // Fallback: fetch from CLOB API by condition_id
            let m = state
                .clob_client
                .get_market_by_id(&market_id)
                .await
                .map_err(|e| {
                    warn!(error = %e, market_id = %market_id, "CLOB fallback failed");
                    ApiError::NotFound(format!("Market {} not found", market_id))
                })?;

            let now = Utc::now();
            let (yes_price, no_price) = extract_prices(&m.outcomes);
            let category = infer_category(&m.question);

            Ok(Json(MarketResponse {
                id: m.id,
                question: m.question,
                description: m.description,
                category,
                end_date: m.end_date,
                active: !m.resolved,
                yes_price,
                no_price,
                volume_24h: m.volume,
                liquidity: m.liquidity,
                created_at: now,
            }))
        }
    }
}

/// Get market orderbook.
#[utoipa::path(
    get,
    path = "/api/v1/markets/{market_id}/orderbook",
    tag = "markets",
    params(
        ("market_id" = String, Path, description = "Market identifier")
    ),
    responses(
        (status = 200, description = "Market orderbook", body = OrderbookResponse),
        (status = 404, description = "Market not found")
    )
)]
pub async fn get_market_orderbook(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<String>,
) -> ApiResult<Json<OrderbookResponse>> {
    let (yes_token, no_token) = state
        .market_catalog
        .hydrate_market(&market_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("Market {market_id} not found")))?;
    let now = Utc::now();
    let (yes, no) = fetch_orderbook_pair(state.clob_client.as_ref(), &yes_token, &no_token).await;
    let (yes_bids, yes_asks) = yes;
    let (no_bids, no_asks) = no;

    // Calculate spreads
    let yes_spread = calculate_spread(&yes_bids, &yes_asks);
    let no_spread = calculate_spread(&no_bids, &no_asks);
    let arb_spread = calculate_arb_spread(&yes_asks, &no_asks);

    let orderbook = OrderbookResponse {
        market_id,
        timestamp: now,
        yes_bids,
        yes_asks,
        no_bids,
        no_asks,
        spread: SpreadInfo {
            yes_spread,
            no_spread,
            arb_spread,
        },
    };

    Ok(Json(orderbook))
}

async fn fetch_orderbook_pair(
    client: &polymarket_core::api::ClobClient,
    yes: &str,
    no: &str,
) -> (
    (Vec<PriceLevel>, Vec<PriceLevel>),
    (Vec<PriceLevel>, Vec<PriceLevel>),
) {
    let (yes_book, no_book) = tokio::join!(client.get_order_book(yes), client.get_order_book(no));
    let levels = |result: polymarket_core::Result<polymarket_core::types::OrderBook>,
                  token_id: &str| {
        match result {
            Ok(book) => (
                book.bids
                    .into_iter()
                    .map(|p| PriceLevel {
                        price: p.price,
                        quantity: p.size,
                    })
                    .collect(),
                book.asks
                    .into_iter()
                    .map(|p| PriceLevel {
                        price: p.price,
                        quantity: p.size,
                    })
                    .collect(),
            ),
            Err(error) => {
                warn!(%error,token_id,"Failed to fetch outcome orderbook");
                (Vec::new(), Vec::new())
            }
        }
    };
    (levels(yes_book, yes), levels(no_book, no))
}

/// Calculate bid-ask spread.
fn calculate_spread(bids: &[PriceLevel], asks: &[PriceLevel]) -> Decimal {
    let best_bid = bids.first().map(|p| p.price).unwrap_or(Decimal::ZERO);
    let best_ask = asks.first().map(|p| p.price).unwrap_or(Decimal::ZERO);

    if best_bid > Decimal::ZERO && best_ask > Decimal::ZERO {
        best_ask - best_bid
    } else {
        Decimal::ZERO
    }
}

/// Calculate arbitrage spread (if buying both outcomes costs less than $1).
fn calculate_arb_spread(yes_asks: &[PriceLevel], no_asks: &[PriceLevel]) -> Option<Decimal> {
    let yes_ask = yes_asks.first().map(|p| p.price)?;
    let no_ask = no_asks.first().map(|p| p.price)?;

    let total_cost = yes_ask + no_ask;

    // If total cost < 1.0, there's profit potential
    if total_cost < Decimal::ONE {
        Some(Decimal::ONE - total_cost)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn orderbook_pair_fetches_concurrently_and_keeps_healthy_side() {
        use axum::{response::IntoResponse, routing::get, Router};
        use std::sync::atomic::{AtomicBool, Ordering};
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let failing = Arc::new(AtomicBool::new(false));
        let fail = failing.clone();
        let app=Router::new().route("/book",get(move |Query(params):Query<std::collections::HashMap<String,String>>| {
            let barrier=barrier.clone();let fail=fail.clone();async move {
                // A sequential implementation cannot get past this barrier.
                barrier.wait().await;
                let token=&params["token_id"];
                if token=="no" && fail.load(Ordering::SeqCst) {
                    return axum::http::StatusCode::BAD_REQUEST.into_response();
                }
                Json(serde_json::json!({"market":"market","asset_id":token,"timestamp":"2026-10-19T00:00:00Z",
                    "bids":[{"price":"0.4","size":"10"}],"asks":[{"price":"0.5","size":"20"}]})).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = polymarket_core::api::ClobClient::new(
            Some(format!("http://{}", listener.local_addr().unwrap())),
            None,
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (yes, no) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            fetch_orderbook_pair(&client, "yes", "no"),
        )
        .await
        .unwrap();
        assert_eq!((yes.0.len(), no.0.len()), (1, 1));
        failing.store(true, Ordering::SeqCst);
        let (yes, no) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            fetch_orderbook_pair(&client, "yes", "no"),
        )
        .await
        .unwrap();
        assert_eq!(yes.0.len(), 1);
        assert!(no.0.is_empty());
        server.abort();
    }

    #[sqlx::test(migrations = false)]
    async fn catalog_pages_filter_before_limit_and_include_inactive_history(pool: sqlx::PgPool) {
        use sqlx::Executor;
        let schema = include_str!("../../../../migrations/005_api_server_tables.sql");
        pool.execute(
            schema
                .split("CREATE TABLE IF NOT EXISTS orders")
                .next()
                .unwrap(),
        )
        .await
        .unwrap();
        pool.execute(
            "INSERT INTO markets(id,question,category,end_date,active,volume_24h) VALUES
            ('c','Third','crypto',NOW(),TRUE,30),('a','First','crypto',NOW(),FALSE,10),
            ('b','Second','sports',NOW(),TRUE,20)",
        )
        .await
        .unwrap();
        let mut query = ListMarketsQuery {
            search: None,
            category: None,
            active: None,
            min_volume: None,
            limit: 1,
            offset: 0,
        };
        assert_eq!(load_market_page(&pool, &query).await.unwrap()[0].id, "a");
        query.offset = 1;
        assert_eq!(load_market_page(&pool, &query).await.unwrap()[0].id, "b");
        query.active = Some(true);
        query.category = Some("CRYP".into());
        query.offset = 0;
        assert_eq!(load_market_page(&pool, &query).await.unwrap()[0].id, "c");
        query.min_volume = Some(Decimal::from(31));
        assert!(load_market_page(&pool, &query).await.unwrap().is_empty());
        pool.execute("INSERT INTO markets(id,question,category,end_date,active,volume_24h) SELECT 'prefix-'||n,'Unrelated','sports',NOW(),TRUE,10 FROM generate_series(1,150) n; INSERT INTO markets(id,question,category,end_date,active,volume_24h) VALUES ('zz-target','Rare needle question','science',NOW(),TRUE,50)").await.unwrap();
        query.category = None;
        query.min_volume = None;
        query.search = Some("NEEDLE".into());
        assert_eq!(
            load_market_page(&pool, &query).await.unwrap()[0].id,
            "zz-target"
        );
        query.offset = -1;
        assert!(load_market_page(&pool, &query).await.is_err());
    }

    #[test]
    fn test_market_response_serialization() {
        let market = MarketResponse {
            id: "test".to_string(),
            question: "Test question?".to_string(),
            description: None,
            category: "test".to_string(),
            end_date: Some(Utc::now()),
            active: true,
            yes_price: Some(Decimal::new(50, 2)),
            no_price: None,
            volume_24h: Decimal::new(1000, 0),
            liquidity: Decimal::new(5000, 0),
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&market).unwrap();
        assert!(json.contains("test"));
        assert!(json.contains("Test question?"));
    }

    #[test]
    fn test_orderbook_response() {
        let orderbook = OrderbookResponse {
            market_id: "market1".to_string(),
            timestamp: Utc::now(),
            yes_bids: vec![PriceLevel {
                price: Decimal::new(50, 2),
                quantity: Decimal::new(100, 0),
            }],
            yes_asks: vec![PriceLevel {
                price: Decimal::new(51, 2),
                quantity: Decimal::new(100, 0),
            }],
            no_bids: vec![],
            no_asks: vec![],
            spread: SpreadInfo {
                yes_spread: Decimal::new(1, 2),
                no_spread: Decimal::ZERO,
                arb_spread: None,
            },
        };

        let json = serde_json::to_string(&orderbook).unwrap();
        assert!(json.contains("market1"));
    }
}
