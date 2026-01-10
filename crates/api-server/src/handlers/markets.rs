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

#[derive(Debug, FromRow)]
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
    // Fetch markets from ClobClient
    let clob_markets = state
        .clob_client
        .get_markets()
        .await
        .map_err(|e| {
            warn!(error = %e, "Failed to fetch markets from CLOB");
            ApiError::Internal(format!("Failed to fetch markets: {}", e))
        })?;

    // Filter and transform to response format
    let now = Utc::now();
    let mut markets: Vec<MarketResponse> = clob_markets
        .into_iter()
        .filter(|m| {
            // Filter by active status (not resolved)
            let active = !m.resolved;
            if let Some(q_active) = query.active {
                if active != q_active {
                    return false;
                }
            }

            // Filter by minimum volume
            if let Some(min_vol) = query.min_volume {
                if m.volume < min_vol {
                    return false;
                }
            }

            true
        })
        .map(|m| {
            // Calculate yes/no prices from outcomes if available
            let (yes_price, no_price) = if m.outcomes.len() >= 2 {
                // Best estimate: complement each other to ~1.0
                (Decimal::new(50, 2), Decimal::new(50, 2))
            } else {
                (Decimal::ZERO, Decimal::ZERO)
            };

            // Determine category from question content
            let category = infer_category(&m.question);

            MarketResponse {
                id: m.id,
                question: m.question,
                description: m.description,
                category,
                end_date: m.end_date.unwrap_or(now + chrono::Duration::days(365)),
                active: !m.resolved,
                yes_price,
                no_price,
                volume_24h: m.volume,
                liquidity: m.liquidity,
                created_at: now, // CLOB doesn't provide created_at
            }
        })
        .collect();

    // Apply category filter
    if let Some(ref cat) = query.category {
        markets.retain(|m| m.category.to_lowercase().contains(&cat.to_lowercase()));
    }

    // Apply pagination
    let total = markets.len();
    let offset = query.offset as usize;
    let limit = query.limit as usize;

    if offset < total {
        markets = markets.into_iter().skip(offset).take(limit).collect();
    } else {
        markets = Vec::new();
    }

    Ok(Json(markets))
}

/// Infer category from market question.
fn infer_category(question: &str) -> String {
    let q = question.to_lowercase();

    if q.contains("bitcoin") || q.contains("btc") || q.contains("ethereum") || q.contains("crypto") {
        "crypto".to_string()
    } else if q.contains("president") || q.contains("election") || q.contains("congress") || q.contains("senate") {
        "politics".to_string()
    } else if q.contains("nfl") || q.contains("nba") || q.contains("world cup") || q.contains("super bowl") {
        "sports".to_string()
    } else if q.contains("stock") || q.contains("s&p") || q.contains("nasdaq") || q.contains("fed") {
        "finance".to_string()
    } else if q.contains("ai") || q.contains("openai") || q.contains("google") || q.contains("apple") {
        "tech".to_string()
    } else {
        "other".to_string()
    }
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
        None => Err(ApiError::NotFound(format!("Market {} not found", market_id))),
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
    // First, get the market to find outcome token IDs
    let markets = state
        .clob_client
        .get_markets()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to fetch markets: {}", e)))?;

    let market = markets
        .iter()
        .find(|m| m.id == market_id)
        .ok_or_else(|| ApiError::NotFound(format!("Market {} not found", market_id)))?;

    // Get token IDs for yes/no outcomes
    let (yes_token, no_token) = if market.outcomes.len() >= 2 {
        let yes = market.outcomes.iter().find(|o| o.name.to_lowercase().contains("yes"));
        let no = market.outcomes.iter().find(|o| o.name.to_lowercase().contains("no"));
        (
            yes.map(|o| o.token_id.clone()),
            no.map(|o| o.token_id.clone()),
        )
    } else {
        (None, None)
    };

    let now = Utc::now();

    // Fetch orderbooks for both outcomes
    let (yes_bids, yes_asks) = if let Some(token_id) = yes_token {
        match state.clob_client.get_order_book(&token_id).await {
            Ok(book) => (
                book.bids.into_iter().map(|p| PriceLevel { price: p.price, size: p.size }).collect(),
                book.asks.into_iter().map(|p| PriceLevel { price: p.price, size: p.size }).collect(),
            ),
            Err(e) => {
                warn!(error = %e, token_id = %token_id, "Failed to fetch yes orderbook");
                (Vec::new(), Vec::new())
            }
        }
    } else {
        (Vec::new(), Vec::new())
    };

    let (no_bids, no_asks) = if let Some(token_id) = no_token {
        match state.clob_client.get_order_book(&token_id).await {
            Ok(book) => (
                book.bids.into_iter().map(|p| PriceLevel { price: p.price, size: p.size }).collect(),
                book.asks.into_iter().map(|p| PriceLevel { price: p.price, size: p.size }).collect(),
            ),
            Err(e) => {
                warn!(error = %e, token_id = %token_id, "Failed to fetch no orderbook");
                (Vec::new(), Vec::new())
            }
        }
    } else {
        (Vec::new(), Vec::new())
    };

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
