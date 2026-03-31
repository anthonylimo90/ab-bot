//! Maps CEX symbols to Polymarket short-duration crypto price contracts.

use chrono::{DateTime, Utc};
use polymarket_core::api::gamma::GammaClient;
use std::collections::HashMap;
use std::sync::Arc;
use std::time;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::price_tracker::CexSymbol;

/// A Polymarket market that is mappable to a CEX price signal.
#[derive(Debug, Clone)]
pub struct MappedMarket {
    /// Polymarket condition ID.
    pub condition_id: String,
    /// Human-readable market question.
    pub question: String,
    /// The strike/threshold price parsed from the title (e.g. 70000.0 for "BTC above $70,000").
    pub threshold_price: f64,
    /// Which CEX symbol this maps to.
    pub cex_symbol: CexSymbol,
    /// Whether the contract is "above" (YES = price > threshold).
    pub is_above: bool,
    /// When the market resolves.
    pub end_date: DateTime<Utc>,
    /// Current YES price on Polymarket (refreshed periodically).
    pub yes_price: f64,
    /// Last time the yes_price was refreshed.
    pub price_updated_at: time::Instant,
}

/// Manages the mapping between CEX symbols and Polymarket markets.
pub struct MarketMapper {
    gamma_client: Arc<GammaClient>,
    /// Mapped markets keyed by condition_id.
    markets: Arc<RwLock<HashMap<String, MappedMarket>>>,
    /// How often to refresh the market list.
    refresh_interval_secs: u64,
}

impl MarketMapper {
    pub fn new(gamma_client: Arc<GammaClient>, refresh_interval_secs: u64) -> Self {
        Self {
            gamma_client,
            markets: Arc::new(RwLock::new(HashMap::new())),
            refresh_interval_secs,
        }
    }

    /// Get all mapped markets for a given CEX symbol.
    pub async fn get_markets_for_symbol(&self, symbol: CexSymbol) -> Vec<MappedMarket> {
        let markets = self.markets.read().await;
        markets
            .values()
            .filter(|m| m.cex_symbol == symbol)
            .cloned()
            .collect()
    }

    /// Spawn a background task that periodically refreshes the market mappings.
    pub fn spawn_refresh_loop(self: Arc<Self>) {
        let interval = time::Duration::from_secs(self.refresh_interval_secs);

        tokio::spawn(async move {
            if let Err(e) = self.refresh_markets().await {
                warn!(error = %e, "Initial market mapper refresh failed");
            }

            loop {
                tokio::time::sleep(interval).await;
                if let Err(e) = self.refresh_markets().await {
                    warn!(error = %e, "Market mapper refresh failed");
                }
            }
        });
    }

    /// Refresh the list of mapped markets from Polymarket Gamma API.
    async fn refresh_markets(&self) -> anyhow::Result<()> {
        let all_markets = self.gamma_client.get_all_tradable_markets(500).await?;
        let now = Utc::now();
        let mut mapped = HashMap::new();

        for market in &all_markets {
            let question_lower = market.question.to_lowercase();

            // Filter to crypto price prediction markets
            let (cex_symbol, is_crypto) =
                if question_lower.contains("bitcoin") || question_lower.contains("btc") {
                    (CexSymbol::BtcUsdt, true)
                } else if question_lower.contains("ethereum") || question_lower.contains("eth") {
                    (CexSymbol::EthUsdt, true)
                } else {
                    (CexSymbol::BtcUsdt, false)
                };

            if !is_crypto {
                continue;
            }

            // Must resolve within 24 hours
            let Some(end_date) = market.end_date else {
                continue;
            };
            let hours_until_resolution = (end_date - now).num_hours();
            if hours_until_resolution <= 0 || hours_until_resolution > 24 {
                continue;
            }

            // Determine direction
            let is_above = question_lower.contains("above")
                || question_lower.contains("higher")
                || question_lower.contains("over");
            let is_below = question_lower.contains("below")
                || question_lower.contains("lower")
                || question_lower.contains("under");

            if !is_above && !is_below {
                continue;
            }

            // Parse threshold price from question
            let Some(threshold) = parse_price_from_question(&market.question) else {
                continue;
            };

            // Get YES price from outcomes
            let yes_price = market
                .outcomes
                .iter()
                .find(|o| o.name.to_lowercase() == "yes")
                .and_then(|o| o.price)
                .and_then(|p| p.to_string().parse::<f64>().ok())
                .unwrap_or(0.5);

            mapped.insert(
                market.id.clone(),
                MappedMarket {
                    condition_id: market.id.clone(),
                    question: market.question.clone(),
                    threshold_price: threshold,
                    cex_symbol,
                    is_above,
                    end_date,
                    yes_price,
                    price_updated_at: time::Instant::now(),
                },
            );
        }

        info!(
            mapped_count = mapped.len(),
            "Refreshed CEX->Polymarket market mappings"
        );

        let mut markets = self.markets.write().await;
        *markets = mapped;
        Ok(())
    }

    /// Update the YES price for a specific market.
    pub async fn update_yes_price(&self, condition_id: &str, yes_price: f64) {
        let mut markets = self.markets.write().await;
        if let Some(market) = markets.get_mut(condition_id) {
            market.yes_price = yes_price;
            market.price_updated_at = time::Instant::now();
        }
    }
}

/// Parse a dollar price from a market question like:
/// "Will BTC be above $70,000 on Jan 31?"
/// "Bitcoin price higher than $68,500?"
fn parse_price_from_question(question: &str) -> Option<f64> {
    let mut chars = question.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            let mut num_str = String::new();
            while let Some(&next) = chars.peek() {
                if next.is_ascii_digit() || next == '.' {
                    num_str.push(next);
                    chars.next();
                } else if next == ',' {
                    chars.next(); // Skip commas in numbers like $70,000
                } else {
                    break;
                }
            }
            if !num_str.is_empty() {
                if let Ok(price) = num_str.parse::<f64>() {
                    if price > 0.0 {
                        return Some(price);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_price_standard() {
        assert_eq!(
            parse_price_from_question("Will BTC be above $70,000?"),
            Some(70000.0)
        );
    }

    #[test]
    fn test_parse_price_with_decimals() {
        assert_eq!(
            parse_price_from_question("ETH price higher than $3,500.50?"),
            Some(3500.50)
        );
    }

    #[test]
    fn test_parse_price_no_commas() {
        assert_eq!(
            parse_price_from_question("Bitcoin above $95000 by Friday?"),
            Some(95000.0)
        );
    }

    #[test]
    fn test_parse_price_none() {
        assert_eq!(parse_price_from_question("Will it rain tomorrow?"), None);
    }
}
