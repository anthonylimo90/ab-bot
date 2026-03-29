//! Polymarket Gamma API client.
//!
//! The Gamma API (`gamma-api.polymarket.com`) provides market metadata
//! absent from CLOB: categories, tags, end dates, resolution criteria.

use crate::types::{Market, Outcome};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use reqwest::header::RETRY_AFTER;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::time::Duration as StdDuration;
use tracing::{debug, warn};

/// Gamma API base URL.
const DEFAULT_BASE_URL: &str = "https://gamma-api.polymarket.com";
const DEFAULT_MAX_PAGE_SIZE: u32 = 100;
const DEFAULT_PAGE_DELAY_MS: u64 = 125;

/// Market metadata from the Gamma API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GammaMarket {
    /// Polymarket condition ID (primary key for markets).
    #[serde(alias = "conditionId", alias = "condition_id")]
    pub condition_id: String,
    /// Market question text.
    pub question: String,
    /// Market category (e.g., "Politics", "Sports", "Crypto").
    #[serde(default)]
    pub category: Option<String>,
    /// Market tags for finer classification.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Whether the market currently has trading fees enabled.
    #[serde(default, alias = "feesEnabled")]
    pub fees_enabled: bool,
    /// Optional fee type label from Gamma.
    #[serde(default, alias = "feeType")]
    pub fee_type: Option<String>,
    /// When the market is expected to resolve.
    #[serde(alias = "endDate", alias = "end_date")]
    pub end_date: Option<String>,
    /// Total traded volume in USD.
    #[serde(default)]
    pub volume: Option<String>,
    /// Current liquidity in USD.
    #[serde(default)]
    pub liquidity: Option<String>,
    /// Whether the market is currently active.
    #[serde(default = "default_true")]
    pub active: bool,
    /// Whether the market is closed/resolved.
    #[serde(default)]
    pub closed: bool,
    /// Whether the market is archived.
    #[serde(default)]
    pub archived: bool,
    /// Whether the market accepts orders right now.
    #[serde(default, alias = "acceptingOrders")]
    pub accepting_orders: bool,
    /// Whether the market has an order book enabled.
    #[serde(default, alias = "enableOrderBook")]
    pub enable_order_book: bool,
    /// Market description / resolution criteria.
    #[serde(default)]
    pub description: Option<String>,
    /// Market slug for URL construction.
    #[serde(default)]
    pub slug: Option<String>,
    /// JSON-encoded list of outcome labels.
    #[serde(default)]
    pub outcomes: Option<String>,
    /// JSON-encoded list of CLOB token ids aligned with outcomes.
    #[serde(default, alias = "clobTokenIds")]
    pub clob_token_ids: Option<String>,
    /// JSON-encoded list of current outcome prices aligned with outcomes.
    #[serde(default, alias = "outcomePrices")]
    pub outcome_prices: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Parsed market metadata ready for storage.
#[derive(Debug, Clone)]
pub struct ParsedGammaMarket {
    pub condition_id: String,
    pub question: String,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub end_date: Option<DateTime<Utc>>,
    pub volume: Decimal,
    pub liquidity: Decimal,
    pub active: bool,
}

impl From<GammaMarket> for ParsedGammaMarket {
    fn from(m: GammaMarket) -> Self {
        Self {
            condition_id: m.condition_id,
            question: m.question,
            category: m.category,
            tags: m.tags,
            end_date: m.end_date.and_then(|s| s.parse().ok()),
            volume: m
                .volume
                .and_then(|v| v.parse().ok())
                .unwrap_or(Decimal::ZERO),
            liquidity: m
                .liquidity
                .and_then(|v| v.parse().ok())
                .unwrap_or(Decimal::ZERO),
            active: m.active,
        }
    }
}

impl GammaMarket {
    /// Returns whether the market is currently tradable on the order book.
    pub fn is_tradable(&self) -> bool {
        self.active
            && !self.closed
            && !self.archived
            && self.accepting_orders
            && self.enable_order_book
    }

    fn parse_string_array(raw: Option<&str>) -> Option<Vec<String>> {
        let raw = raw?;
        serde_json::from_str::<Vec<String>>(raw).ok()
    }

    fn parse_decimal_array(raw: Option<&str>) -> Option<Vec<Decimal>> {
        let raw = raw?;
        let values = serde_json::from_str::<Vec<String>>(raw).ok()?;
        Some(
            values
                .into_iter()
                .map(|value| value.parse().unwrap_or(Decimal::ZERO))
                .collect(),
        )
    }

    /// Convert a tradable Gamma market into the shared `Market` type.
    pub fn into_market(self) -> Option<Market> {
        let outcome_names = Self::parse_string_array(self.outcomes.as_deref())?;
        let token_ids = Self::parse_string_array(self.clob_token_ids.as_deref())?;
        let prices = Self::parse_decimal_array(self.outcome_prices.as_deref()).unwrap_or_default();

        if outcome_names.len() != 2
            || token_ids.len() != 2
            || outcome_names.len() != token_ids.len()
        {
            return None;
        }

        let outcomes = outcome_names
            .into_iter()
            .zip(token_ids)
            .enumerate()
            .map(|(idx, (name, token_id))| Outcome {
                id: token_id.clone(),
                name,
                token_id,
                price: prices.get(idx).copied(),
                winner: None,
            })
            .collect();

        Some(Market {
            id: self.condition_id,
            question: self.question,
            description: self.description,
            outcomes,
            volume: self
                .volume
                .and_then(|value| value.parse().ok())
                .unwrap_or(Decimal::ZERO),
            liquidity: self
                .liquidity
                .and_then(|value| value.parse().ok())
                .unwrap_or(Decimal::ZERO),
            end_date: self.end_date.and_then(|value| value.parse().ok()),
            resolved: self.closed || self.archived,
            resolution: None,
            category: self.category,
            tags: self.tags,
            fees_enabled: self.fees_enabled,
            fee_type: self.fee_type,
        })
    }
}

/// Client for the Polymarket Gamma API.
///
/// Provides market metadata not available from the CLOB API:
/// categories, tags, end dates, and resolution criteria.
pub struct GammaClient {
    base_url: String,
    http_client: reqwest::Client,
}

impl GammaClient {
    /// Maximum retry attempts for API calls.
    const MAX_RETRIES: u32 = 3;

    /// Create a new Gamma API client.
    pub fn new(base_url: Option<String>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(StdDuration::from_secs(30))
            .connect_timeout(StdDuration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            http_client,
        }
    }

    fn retry_after_delay(response: &reqwest::Response) -> Option<StdDuration> {
        response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(StdDuration::from_secs)
    }

    fn capped_page_size(page_size: u32) -> u32 {
        page_size.max(1).min(
            std::env::var("GAMMA_MAX_PAGE_SIZE")
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(DEFAULT_MAX_PAGE_SIZE)
                .max(1),
        )
    }

    fn page_delay() -> StdDuration {
        StdDuration::from_millis(
            std::env::var("GAMMA_PAGE_DELAY_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(DEFAULT_PAGE_DELAY_MS),
        )
    }

    /// Execute an HTTP GET with retry and exponential backoff.
    ///
    /// Retries on 5xx server errors and 429 rate-limit responses.
    /// All other 4xx errors fail immediately.
    async fn get_with_retry(&self, url: &str) -> Result<reqwest::Response> {
        let mut last_error = None;

        for attempt in 0..Self::MAX_RETRIES {
            match self.http_client.get(url).send().await {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response)
                    if response.status().as_u16() == 429 || response.status().is_server_error() =>
                {
                    let status = response.status();
                    let is_rate_limited = status.as_u16() == 429;
                    warn!(
                        attempt = attempt + 1,
                        status = %status,
                        url = url,
                        "Gamma API error, retrying"
                    );
                    let retry_after = Self::retry_after_delay(&response);
                    let backoff = if is_rate_limited {
                        StdDuration::from_millis(2000 * 2u64.pow(attempt))
                    } else {
                        StdDuration::from_millis(500 * 2u64.pow(attempt))
                    };
                    let backoff = retry_after
                        .map(|delay| delay.max(backoff))
                        .unwrap_or(backoff);
                    tokio::time::sleep(backoff).await;
                    last_error = Some(Error::Api {
                        message: format!("Gamma API returned {}", status),
                        status: Some(status.as_u16()),
                    });
                }
                Ok(response) => {
                    let status = response.status();
                    return Err(Error::Api {
                        message: format!("Gamma API error: {}", status),
                        status: Some(status.as_u16()),
                    });
                }
                Err(e) => {
                    warn!(
                        attempt = attempt + 1,
                        error = %e,
                        url = url,
                        "Gamma API request failed, retrying"
                    );
                    let backoff = StdDuration::from_millis(500 * 2u64.pow(attempt));
                    tokio::time::sleep(backoff).await;
                    last_error = Some(Error::Http(Box::new(e)));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| Error::Api {
            message: "Gamma API: max retries exceeded".to_string(),
            status: None,
        }))
    }

    /// Fetch a paginated list of markets from the Gamma API.
    ///
    /// Returns raw `GammaMarket` structs. Use `ParsedGammaMarket::from()` to convert.
    pub async fn get_markets(&self, limit: u32, offset: u32) -> Result<Vec<GammaMarket>> {
        let url = format!(
            "{}/markets?limit={}&offset={}&active=true&closed=false",
            self.base_url, limit, offset
        );
        debug!(url = %url, "Fetching Gamma markets");

        let response = self.get_with_retry(&url).await?;
        let markets: Vec<GammaMarket> = response.json().await.map_err(|e| Error::Api {
            message: format!("Failed to parse Gamma markets response: {}", e),
            status: None,
        })?;

        debug!(count = markets.len(), "Fetched Gamma markets");
        Ok(markets)
    }

    /// Fetch all active markets by paginating through the Gamma API.
    ///
    /// Stops when a page returns fewer results than the page size.
    pub async fn get_all_markets(&self, page_size: u32) -> Result<Vec<GammaMarket>> {
        let capped_page_size = Self::capped_page_size(page_size);
        if capped_page_size != page_size {
            warn!(
                requested = page_size,
                capped = capped_page_size,
                "Clamped Gamma page size to reduce rate-limit pressure"
            );
        }

        let mut all_markets = Vec::new();
        let mut offset = 0u32;
        let page_delay = Self::page_delay();

        loop {
            let page = self.get_markets(capped_page_size, offset).await?;
            let page_len = page.len() as u32;
            all_markets.extend(page);

            if page_len < capped_page_size {
                break;
            }
            offset += capped_page_size;

            if !page_delay.is_zero() {
                tokio::time::sleep(page_delay).await;
            }

            // Yield to prevent blocking the executor during large fetches
            tokio::task::yield_now().await;
        }

        debug!(total = all_markets.len(), "Fetched all Gamma markets");
        Ok(all_markets)
    }

    /// Fetch all currently tradable binary markets from the Gamma API.
    pub async fn get_all_tradable_markets(&self, page_size: u32) -> Result<Vec<Market>> {
        let markets = self.get_all_markets(page_size).await?;
        Ok(markets
            .into_iter()
            .filter(GammaMarket::is_tradable)
            .filter_map(GammaMarket::into_market)
            .collect())
    }

    /// Fetch a single market by condition ID.
    pub async fn get_market(&self, condition_id: &str) -> Result<GammaMarket> {
        let url = format!("{}/markets/{}", self.base_url, condition_id);
        debug!(condition_id = condition_id, "Fetching Gamma market");

        let response = self.get_with_retry(&url).await?;
        let market: GammaMarket = response.json().await.map_err(|e| Error::Api {
            message: format!("Failed to parse Gamma market response: {}", e),
            status: None,
        })?;

        Ok(market)
    }

    /// Search markets by query string.
    pub async fn search_markets(&self, query: &str, limit: u32) -> Result<Vec<GammaMarket>> {
        let encoded_query: String = query
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                    c.to_string()
                } else {
                    format!("%{:02X}", c as u32)
                }
            })
            .collect();
        let url = format!(
            "{}/markets?limit={}&text_query={}",
            self.base_url, limit, encoded_query
        );
        debug!(query = query, "Searching Gamma markets");

        let response = self.get_with_retry(&url).await?;
        let markets: Vec<GammaMarket> = response.json().await.map_err(|e| Error::Api {
            message: format!("Failed to parse Gamma search response: {}", e),
            status: None,
        })?;

        debug!(count = markets.len(), query = query, "Gamma search results");
        Ok(markets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gamma_market_deserialization() {
        let json = r#"{
            "conditionId": "0x1234",
            "question": "Will BTC hit $100k?",
            "category": "Crypto",
            "tags": ["bitcoin", "price"],
            "endDate": "2026-06-01T00:00:00Z",
            "volume": "50000.5",
            "liquidity": "12000.0",
            "active": true,
            "description": "Resolves YES if...",
            "slug": "will-btc-hit-100k"
        }"#;

        let market: GammaMarket = serde_json::from_str(json).unwrap();
        assert_eq!(market.condition_id, "0x1234");
        assert_eq!(market.question, "Will BTC hit $100k?");
        assert_eq!(market.category, Some("Crypto".to_string()));
        assert_eq!(market.tags, vec!["bitcoin", "price"]);
        assert!(market.active);
    }

    #[test]
    fn test_gamma_market_missing_fields() {
        let json = r#"{
            "conditionId": "0xabcd",
            "question": "Test market"
        }"#;

        let market: GammaMarket = serde_json::from_str(json).unwrap();
        assert_eq!(market.condition_id, "0xabcd");
        assert!(market.category.is_none());
        assert!(market.tags.is_empty());
        assert!(market.end_date.is_none());
        assert!(market.volume.is_none());
        assert!(market.active); // defaults to true
    }

    #[test]
    fn test_parsed_gamma_market_conversion() {
        let gamma = GammaMarket {
            condition_id: "0x1234".to_string(),
            question: "Test?".to_string(),
            category: Some("Politics".to_string()),
            tags: vec!["election".to_string()],
            fees_enabled: false,
            fee_type: None,
            end_date: Some("2026-06-01T00:00:00Z".to_string()),
            volume: Some("50000.50".to_string()),
            liquidity: Some("12000".to_string()),
            active: true,
            closed: false,
            archived: false,
            accepting_orders: true,
            enable_order_book: true,
            description: None,
            slug: None,
            outcomes: None,
            clob_token_ids: None,
            outcome_prices: None,
        };

        let parsed = ParsedGammaMarket::from(gamma);
        assert_eq!(parsed.condition_id, "0x1234");
        assert_eq!(parsed.volume, Decimal::new(5000050, 2));
        assert!(parsed.end_date.is_some());
    }

    #[test]
    fn test_gamma_market_into_market() {
        let gamma = GammaMarket {
            condition_id: "0x1234".to_string(),
            question: "Will BTC hit $100k?".to_string(),
            category: Some("Crypto".to_string()),
            tags: vec!["bitcoin".to_string()],
            fees_enabled: true,
            fee_type: Some("curve".to_string()),
            end_date: Some("2026-06-01T00:00:00Z".to_string()),
            volume: Some("50000.50".to_string()),
            liquidity: Some("12000".to_string()),
            active: true,
            closed: false,
            archived: false,
            accepting_orders: true,
            enable_order_book: true,
            description: Some("Resolves YES if...".to_string()),
            slug: Some("will-btc-hit-100k".to_string()),
            outcomes: Some("[\"Yes\",\"No\"]".to_string()),
            clob_token_ids: Some("[\"1\",\"2\"]".to_string()),
            outcome_prices: Some("[\"0.42\",\"0.58\"]".to_string()),
        };

        assert!(gamma.is_tradable());
        let market = gamma
            .into_market()
            .expect("tradable gamma market should convert");
        assert_eq!(market.id, "0x1234");
        assert_eq!(market.outcomes.len(), 2);
        assert_eq!(market.outcomes[0].token_id, "1");
        assert_eq!(market.outcomes[1].name, "No");
        assert_eq!(market.outcomes[0].price, Some(Decimal::new(42, 2)));
        assert!(!market.resolved);
    }
}
