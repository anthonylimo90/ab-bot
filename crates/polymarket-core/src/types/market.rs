//! Market-related types for Polymarket data.

use chrono::{DateTime, Utc};
use rust_decimal::prelude::ToPrimitive;
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
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub fees_enabled: bool,
    #[serde(default)]
    pub fee_type: Option<String>,
}

/// A single outcome (e.g., YES or NO) within a market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub id: String,
    pub name: String,
    pub token_id: String,
    /// Current price from CLOB API (0.0–1.0 range).
    pub price: Option<Decimal>,
    /// Winner flag for resolved markets when upstream data provides it.
    #[serde(default)]
    pub winner: Option<bool>,
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
    /// Restores the best-first invariant: bids by descending price, asks by
    /// ascending price. Polymarket's REST `/book` and WebSocket `book` events
    /// list levels worst-first, so every ingest path must call this.
    pub fn sort_levels(&mut self) {
        sort_bids_best_first(&mut self.bids);
        sort_asks_best_first(&mut self.asks);
    }

    /// Returns the best bid level (highest buy order), regardless of level order.
    pub fn best_bid_level(&self) -> Option<&PriceLevel> {
        self.bids.iter().max_by(|a, b| a.price.cmp(&b.price))
    }

    /// Returns the best ask level (lowest sell order), regardless of level order.
    pub fn best_ask_level(&self) -> Option<&PriceLevel> {
        self.asks.iter().min_by(|a, b| a.price.cmp(&b.price))
    }

    /// Returns the best bid price (highest buy order).
    pub fn best_bid(&self) -> Option<Decimal> {
        self.best_bid_level().map(|l| l.price)
    }

    /// Returns the best ask price (lowest sell order).
    pub fn best_ask(&self) -> Option<Decimal> {
        self.best_ask_level().map(|l| l.price)
    }
}

/// A single price level in the order book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: Decimal,
    pub size: Decimal,
}

impl PriceLevel {
    /// Parses an exchange level, rejecting anything that cannot be a resting
    /// order: unparseable values, prices outside (0, 1) and empty sizes.
    pub fn parse_exchange(price: &str, size: &str) -> Option<Self> {
        let price = price.trim().parse::<Decimal>().ok()?;
        let size = size.trim().parse::<Decimal>().ok()?;
        (price > Decimal::ZERO && price < Decimal::ONE && size > Decimal::ZERO)
            .then_some(Self { price, size })
    }
}

/// Sorts bid levels best-first (highest price first).
pub fn sort_bids_best_first(levels: &mut [PriceLevel]) {
    levels.sort_by_key(|level| std::cmp::Reverse(level.price));
}

/// Sorts ask levels best-first (lowest price first).
pub fn sort_asks_best_first(levels: &mut [PriceLevel]) {
    levels.sort_by_key(|level| level.price);
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

    /// Calculate entry cost with liquidity depth validation.
    /// Returns None if either side has less than `min_depth_usd` at best ask.
    /// Returns (yes_ask, no_ask, total_cost, yes_size, no_size).
    pub fn entry_cost_with_depth(
        &self,
        min_depth_usd: Decimal,
    ) -> Option<(Decimal, Decimal, Decimal, Decimal, Decimal)> {
        let yes_ask_level = self.yes_book.best_ask_level()?;
        let no_ask_level = self.no_book.best_ask_level()?;

        let yes_notional = yes_ask_level.price * yes_ask_level.size;
        let no_notional = no_ask_level.price * no_ask_level.size;

        // Both sides must have sufficient depth
        if yes_notional < min_depth_usd || no_notional < min_depth_usd {
            return None;
        }

        Some((
            yes_ask_level.price,
            no_ask_level.price,
            yes_ask_level.price + no_ask_level.price,
            yes_ask_level.size,
            no_ask_level.size,
        ))
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
    #[serde(default)]
    pub fee_drag: Decimal,
    #[serde(default)]
    pub worst_case_payout: Decimal,
    #[serde(default)]
    pub yes_fee_shares: Decimal,
    #[serde(default)]
    pub no_fee_shares: Decimal,
}

impl ArbOpportunity {
    /// Default fee percentage (2%).
    pub const DEFAULT_FEE: Decimal = Decimal::from_parts(2, 0, 0, false, 2); // 0.02
    /// Current fee-enabled market base rate from Polymarket docs.
    pub const FEE_ENABLED_BASE_RATE: Decimal = Decimal::from_parts(25, 0, 0, false, 2); // 0.25

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
            fee_drag: total_cost * fee,
            worst_case_payout: Decimal::ONE - (total_cost * fee),
            yes_fee_shares: Decimal::ZERO,
            no_fee_shares: Decimal::ZERO,
        })
    }

    /// Calculate arbitrage profitability using Polymarket's fee-enabled market model.
    ///
    /// When fees are enabled, Polymarket currently charges buy-side fees in shares.
    /// For a YES+NO pair held to resolution, the guaranteed payout is reduced by the
    /// larger of the two entry-share fees.
    pub fn calculate_with_fees_enabled(
        book: &BinaryMarketBook,
        fees_enabled: bool,
    ) -> Option<Self> {
        let (yes_ask, no_ask, total_cost) = book.entry_cost()?;
        if total_cost <= Decimal::ZERO {
            return None;
        }

        let gross_profit = Decimal::ONE - total_cost;
        let yes_fee_shares = Self::estimate_buy_fee_shares(yes_ask, fees_enabled);
        let no_fee_shares = Self::estimate_buy_fee_shares(no_ask, fees_enabled);
        let fee_drag = yes_fee_shares.max(no_fee_shares);
        let worst_case_payout = (Decimal::ONE - fee_drag).max(Decimal::ZERO);
        let net_profit = worst_case_payout - total_cost;

        Some(Self {
            market_id: book.market_id.clone(),
            timestamp: book.timestamp,
            yes_ask,
            no_ask,
            total_cost,
            gross_profit,
            net_profit,
            fee_drag,
            worst_case_payout,
            yes_fee_shares,
            no_fee_shares,
        })
    }

    /// Returns true if this is a profitable arbitrage opportunity.
    pub fn is_profitable(&self) -> bool {
        self.net_profit > Decimal::ZERO
    }

    fn estimate_buy_fee_shares(price: Decimal, fees_enabled: bool) -> Decimal {
        if !fees_enabled {
            return Decimal::ZERO;
        }

        let bounded_price = price.max(Decimal::ZERO).min(Decimal::ONE);
        let curve_input = bounded_price * (Decimal::ONE - bounded_price);
        if curve_input <= Decimal::ZERO {
            return Decimal::ZERO;
        }

        let base_rate = Self::FEE_ENABLED_BASE_RATE.to_f64().unwrap_or(0.25);
        let curve_value = curve_input.to_f64().unwrap_or(0.0);
        let fee_shares = base_rate * curve_value.powi(2);
        Decimal::from_f64_retain(fee_shares).unwrap_or(Decimal::ZERO)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(price: i64, size: i64) -> PriceLevel {
        PriceLevel {
            price: Decimal::new(price, 2),
            size: Decimal::new(size, 0),
        }
    }

    #[test]
    fn best_levels_do_not_depend_on_level_order() {
        let mut book = OrderBook {
            market_id: "m".to_string(),
            outcome_id: "o".to_string(),
            timestamp: Utc::now(),
            bids: vec![level(1, 1000), level(45, 10), level(47, 5)],
            asks: vec![level(99, 1000), level(52, 7), level(49, 3)],
        };

        assert_eq!(book.best_bid(), Some(Decimal::new(47, 2)));
        assert_eq!(book.best_ask(), Some(Decimal::new(49, 2)));
        assert_eq!(book.best_ask_level().unwrap().size, Decimal::new(3, 0));

        book.sort_levels();
        assert_eq!(book.bids[0].price, Decimal::new(47, 2));
        assert_eq!(book.asks[0].price, Decimal::new(49, 2));
        assert_eq!(book.asks[2].price, Decimal::new(99, 2));

        let paired = BinaryMarketBook {
            market_id: "m".to_string(),
            timestamp: Utc::now(),
            yes_book: OrderBook {
                asks: vec![level(99, 1000), level(49, 300)],
                ..book.clone()
            },
            no_book: OrderBook {
                asks: vec![level(98, 1000), level(48, 300)],
                ..book
            },
        };
        let (yes, no, total, _, _) = paired.entry_cost_with_depth(Decimal::ONE).unwrap();
        assert_eq!(
            (yes, no, total),
            (
                Decimal::new(49, 2),
                Decimal::new(48, 2),
                Decimal::new(97, 2)
            )
        );
    }

    #[test]
    fn parse_exchange_level_rejects_impossible_levels() {
        assert!(PriceLevel::parse_exchange("0.5", "10").is_some());
        assert!(PriceLevel::parse_exchange("0", "10").is_none());
        assert!(PriceLevel::parse_exchange("1", "10").is_none());
        assert!(PriceLevel::parse_exchange("0.5", "0").is_none());
        assert!(PriceLevel::parse_exchange("x", "10").is_none());
    }

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
    fn test_fee_free_market_uses_zero_fee_drag() {
        let book = BinaryMarketBook {
            market_id: "test".to_string(),
            timestamp: Utc::now(),
            yes_book: OrderBook {
                market_id: "test".to_string(),
                outcome_id: "yes".to_string(),
                timestamp: Utc::now(),
                bids: vec![],
                asks: vec![PriceLevel {
                    price: Decimal::new(48, 2),
                    size: Decimal::new(100, 0),
                }],
            },
            no_book: OrderBook {
                market_id: "test".to_string(),
                outcome_id: "no".to_string(),
                timestamp: Utc::now(),
                bids: vec![],
                asks: vec![PriceLevel {
                    price: Decimal::new(46, 2),
                    size: Decimal::new(100, 0),
                }],
            },
        };

        let arb = ArbOpportunity::calculate_with_fees_enabled(&book, false).unwrap();
        assert_eq!(arb.fee_drag, Decimal::ZERO);
        assert_eq!(arb.worst_case_payout, Decimal::ONE);
        assert_eq!(arb.net_profit, Decimal::new(6, 2));
    }

    #[test]
    fn test_fee_enabled_market_uses_share_fee_drag() {
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
                    price: Decimal::new(40, 2),
                    size: Decimal::new(100, 0),
                }],
            },
        };

        let arb = ArbOpportunity::calculate_with_fees_enabled(&book, true).unwrap();
        assert!(arb.fee_drag > Decimal::ZERO);
        assert_eq!(arb.fee_drag, arb.yes_fee_shares.max(arb.no_fee_shares));
        assert_eq!(arb.worst_case_payout, Decimal::ONE - arb.fee_drag);
        assert!(arb.net_profit < arb.gross_profit);
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
