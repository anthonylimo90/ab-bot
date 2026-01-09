//! Market data handlers.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

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
    /// End date.
    pub end_date: DateTime<Utc>,
    /// Whether the market is active.
    pub active: bool,
    /// Current yes price.
    pub yes_price: Decimal,
    /// Current no price.
    pub no_price: Decimal,
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
    /// Size at this price.
    pub size: Decimal,
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
) -> ApiResult<Json<Vec<MarketResponse>>> {
    let mut sql = String::from(
        "SELECT id, question, description, category, end_date, active,
                yes_price, no_price, volume_24h, liquidity, created_at
         FROM markets WHERE 1=1"
    );

    if query.category.is_some() {
        sql.push_str(" AND category = $1");
    }
    if query.active.is_some() {
        sql.push_str(" AND active = $2");
    }
    if query.min_volume.is_some() {
        sql.push_str(" AND volume_24h >= $3");
    }

    sql.push_str(" ORDER BY volume_24h DESC LIMIT $4 OFFSET $5");

    // For now, return mock data since we don't have the full schema
    // In production, this would query the actual database
    let markets = vec![
        MarketResponse {
            id: "market_1".to_string(),
            question: "Will BTC reach $100k by end of 2026?".to_string(),
            description: Some("Bitcoin price prediction market".to_string()),
            category: "crypto".to_string(),
            end_date: Utc::now() + chrono::Duration::days(365),
            active: true,
            yes_price: Decimal::new(65, 2),
            no_price: Decimal::new(35, 2),
            volume_24h: Decimal::new(1000000, 2),
            liquidity: Decimal::new(5000000, 2),
            created_at: Utc::now(),
        },
    ];

    Ok(Json(markets))
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
    let result = sqlx::query_as!(
        MarketRow,
        r#"
        SELECT id, question, description, category, end_date, active,
               yes_price, no_price, volume_24h, liquidity, created_at
        FROM markets
        WHERE id = $1
        "#,
        market_id
    )
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
        None => Err(ApiError::NotFound(format!("Market {} not found", market_id))),
    }
}

#[derive(Debug)]
struct MarketRow {
    id: String,
    question: String,
    description: Option<String>,
    category: String,
    end_date: DateTime<Utc>,
    active: bool,
    yes_price: Decimal,
    no_price: Decimal,
    volume_24h: Decimal,
    liquidity: Decimal,
    created_at: DateTime<Utc>,
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
    // In production, this would fetch from the orderbook cache or API
    // For now, return mock data
    let orderbook = OrderbookResponse {
        market_id: market_id.clone(),
        timestamp: Utc::now(),
        yes_bids: vec![
            PriceLevel { price: Decimal::new(64, 2), size: Decimal::new(1000, 0) },
            PriceLevel { price: Decimal::new(63, 2), size: Decimal::new(2000, 0) },
        ],
        yes_asks: vec![
            PriceLevel { price: Decimal::new(65, 2), size: Decimal::new(1500, 0) },
            PriceLevel { price: Decimal::new(66, 2), size: Decimal::new(2500, 0) },
        ],
        no_bids: vec![
            PriceLevel { price: Decimal::new(34, 2), size: Decimal::new(1000, 0) },
            PriceLevel { price: Decimal::new(33, 2), size: Decimal::new(2000, 0) },
        ],
        no_asks: vec![
            PriceLevel { price: Decimal::new(35, 2), size: Decimal::new(1500, 0) },
            PriceLevel { price: Decimal::new(36, 2), size: Decimal::new(2500, 0) },
        ],
        spread: SpreadInfo {
            yes_spread: Decimal::new(1, 2),
            no_spread: Decimal::new(1, 2),
            arb_spread: None,
        },
    };

    Ok(Json(orderbook))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_market_response_serialization() {
        let market = MarketResponse {
            id: "test".to_string(),
            question: "Test question?".to_string(),
            description: None,
            category: "test".to_string(),
            end_date: Utc::now(),
            active: true,
            yes_price: Decimal::new(50, 2),
            no_price: Decimal::new(50, 2),
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
            yes_bids: vec![PriceLevel { price: Decimal::new(50, 2), size: Decimal::new(100, 0) }],
            yes_asks: vec![PriceLevel { price: Decimal::new(51, 2), size: Decimal::new(100, 0) }],
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
