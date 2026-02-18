//! Market-related types for Polymarket data.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Represents a Polymarket market (prediction market).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    pub id: String,
    pub question: String,
    pub description: Option<String>,
    pub outcomes: Vec<Outcome>,
    pub volume: Decimal,
    pub liquidity: Decimal,
    pub end_date: Option<DateTime<Utc>>,
    pub resolved: bool,
    pub resolution: Option<String>,
}

/// A single outcome (e.g., YES or NO) within a market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub id: String,
    pub name: String,
    pub token_id: String,
}

/// Real-time order book data for a market outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBook {
    pub market_id: String,
    pub outcome_id: String,
    pub timestamp: DateTime<Utc>,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}

impl OrderBook {
    /// Returns the best bid price (highest buy order).
    pub fn best_bid(&self) -> Option<Decimal> {
        self.bids.first().map(|l| l.price)
    }

    /// Returns the best ask price (lowest sell order).
    pub fn best_ask(&self) -> Option<Decimal> {
        self.asks.first().map(|l| l.price)
    }
}

/// A single price level in the order book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: Decimal,
    pub size: Decimal,
}

/// Paired order book data for both outcomes of a binary market.
#[derive(Debug, Clone)]
pub struct BinaryMarketBook {
    pub market_id: String,
    pub timestamp: DateTime<Utc>,
    pub yes_book: OrderBook,
    pub no_book: OrderBook,
}

impl BinaryMarketBook {
    /// Calculate the total cost to buy both YES and NO outcomes.
    /// Returns (yes_ask, no_ask, total_cost).
    pub fn entry_cost(&self) -> Option<(Decimal, Decimal, Decimal)> {
        let yes_ask = self.yes_book.best_ask()?;
        let no_ask = self.no_book.best_ask()?;
        Some((yes_ask, no_ask, yes_ask + no_ask))
    }

    /// Calculate the total value if selling both positions now.
    /// Returns (yes_bid, no_bid, total_value).
    pub fn exit_value(&self) -> Option<(Decimal, Decimal, Decimal)> {
        let yes_bid = self.yes_book.best_bid()?;
        let no_bid = self.no_book.best_bid()?;
        Some((yes_bid, no_bid, yes_bid + no_bid))
    }
}

/// Arbitrage opportunity detected in a market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbOpportunity {
    pub market_id: String,
    pub timestamp: DateTime<Utc>,
    pub yes_ask: Decimal,
    pub no_ask: Decimal,
    pub total_cost: Decimal,
    pub gross_profit: Decimal,
    pub net_profit: Decimal,
}

impl ArbOpportunity {
    /// Default fee percentage (2%).
    pub const DEFAULT_FEE: Decimal = Decimal::from_parts(2, 0, 0, false, 2); // 0.02

    /// Calculate arbitrage opportunity from a binary market book.
    ///
    /// Returns `None` if the order book has no valid asks, or if total cost is
    /// zero/negative (which would indicate an empty or corrupted order book).
    pub fn calculate(book: &BinaryMarketBook, fee: Decimal) -> Option<Self> {
        let (yes_ask, no_ask, total_cost) = book.entry_cost()?;

        // Guard: total_cost must be positive and within valid range (0, 1].
        // A zero or negative cost means the order book is empty or corrupted.
        if total_cost <= Decimal::ZERO {
            return None;
        }

        let gross_profit = Decimal::ONE - total_cost;
        // Fee is a rate (e.g. 0.02 = 2%) applied to the notional cost of both legs
        let net_profit = gross_profit - (total_cost * fee);

        Some(Self {
            market_id: book.market_id.clone(),
            timestamp: book.timestamp,
            yes_ask,
            no_ask,
            total_cost,
            gross_profit,
            net_profit,
        })
    }

    /// Returns true if this is a profitable arbitrage opportunity.
    pub fn is_profitable(&self) -> bool {
        self.net_profit > Decimal::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arb_detection() {
        let book = BinaryMarketBook {
            market_id: "test".to_string(),
            timestamp: Utc::now(),
            yes_book: OrderBook {
                market_id: "test".to_string(),
                outcome_id: "yes".to_string(),
                timestamp: Utc::now(),
                bids: vec![PriceLevel {
                    price: Decimal::new(45, 2),
                    size: Decimal::new(100, 0),
                }],
                asks: vec![PriceLevel {
                    price: Decimal::new(48, 2),
                    size: Decimal::new(100, 0),
                }],
            },
            no_book: OrderBook {
                market_id: "test".to_string(),
                outcome_id: "no".to_string(),
                timestamp: Utc::now(),
                bids: vec![PriceLevel {
                    price: Decimal::new(44, 2),
                    size: Decimal::new(100, 0),
                }],
                asks: vec![PriceLevel {
                    price: Decimal::new(46, 2),
                    size: Decimal::new(100, 0),
                }],
            },
        };

        // Total cost: 0.48 + 0.46 = 0.94
        // Gross profit: 1.00 - 0.94 = 0.06
        // Fees: 0.94 * 0.02 = 0.0188
        // Net profit: 0.06 - 0.0188 = 0.0412
        let arb = ArbOpportunity::calculate(&book, ArbOpportunity::DEFAULT_FEE).unwrap();

        assert_eq!(arb.total_cost, Decimal::new(94, 2));
        assert_eq!(arb.gross_profit, Decimal::new(6, 2));
        assert_eq!(arb.net_profit, Decimal::new(412, 4)); // 0.0412
        assert!(arb.is_profitable());
    }

    #[test]
    fn test_no_arb_opportunity() {
        let book = BinaryMarketBook {
            market_id: "test".to_string(),
            timestamp: Utc::now(),
            yes_book: OrderBook {
                market_id: "test".to_string(),
                outcome_id: "yes".to_string(),
                timestamp: Utc::now(),
                bids: vec![],
                asks: vec![PriceLevel {
                    price: Decimal::new(55, 2),
                    size: Decimal::new(100, 0),
                }],
            },
            no_book: OrderBook {
                market_id: "test".to_string(),
                outcome_id: "no".to_string(),
                timestamp: Utc::now(),
                bids: vec![],
                asks: vec![PriceLevel {
                    price: Decimal::new(50, 2),
                    size: Decimal::new(100, 0),
                }],
            },
        };

        // Total cost: 0.55 + 0.50 = 1.05 (no profit possible)
        let arb = ArbOpportunity::calculate(&book, ArbOpportunity::DEFAULT_FEE).unwrap();
        assert!(!arb.is_profitable());
    }
}
