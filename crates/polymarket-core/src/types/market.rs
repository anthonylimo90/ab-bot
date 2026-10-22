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
    /// Taker fee terms published for the market, when known.
    #[serde(default)]
    pub fee_schedule: Option<FeeSchedule>,
}

/// The indices of the outcomes treated as YES and NO in a binary market.
///
/// Every position, order and price in the system uses this one mapping:
/// the first outcome is YES when its name says so, otherwise the second is.
/// For named outcomes ("Lakers"/"Celtics") the choice is a convention, but it
/// must never differ between the tokens a position holds and the prices
/// used to value it.
pub fn yes_no_indices(outcomes: &[Outcome]) -> Option<(usize, usize)> {
    (outcomes.len() == 2).then(|| {
        if outcomes[0].name.to_lowercase().contains("yes") {
            (0, 1)
        } else {
            (1, 0)
        }
    })
}

impl Market {
    /// The YES and NO token ids of a binary market, under [`yes_no_indices`].
    pub fn yes_no_token_ids(&self) -> Option<(String, String)> {
        yes_no_indices(&self.outcomes).map(|(yes, no)| {
            (
                self.outcomes[yes].token_id.clone(),
                self.outcomes[no].token_id.clone(),
            )
        })
    }

    /// The taker fee terms to price trades with. A market that reports fees
    /// without publishing their terms is priced at the highest known rate.
    pub fn effective_fee_schedule(&self) -> FeeSchedule {
        match (self.fee_schedule, self.fees_enabled) {
            (Some(schedule), _) => schedule,
            (None, true) => FeeSchedule::CONSERVATIVE,
            (None, false) => FeeSchedule::FREE,
        }
    }
}

/// Taker fee terms of a market. A taker buying or selling `C` shares at price
/// `p` pays `C × rate × (p × (1 − p))^exponent` USDC; makers pay nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeSchedule {
    pub rate: Decimal,
    pub exponent: Decimal,
}

impl FeeSchedule {
    /// No trading fee.
    pub const FREE: Self = Self {
        rate: Decimal::ZERO,
        exponent: Decimal::ONE,
    };

    /// The highest published category rate (crypto, 0.07), for markets that
    /// report fees without their terms.
    pub const CONSERVATIVE: Self = Self {
        rate: Decimal::from_parts(7, 0, 0, false, 2),
        exponent: Decimal::ONE,
    };

    /// Taker fee in USDC for one share traded at `price`.
    pub fn fee_per_share(&self, price: Decimal) -> Decimal {
        if self.rate <= Decimal::ZERO {
            return Decimal::ZERO;
        }
        let p = price.max(Decimal::ZERO).min(Decimal::ONE);
        let base = p * (Decimal::ONE - p);
        let curve = if self.exponent == Decimal::ONE {
            base
        } else if self.exponent.fract().is_zero() && self.exponent >= Decimal::ZERO {
            // p(1 − p) ≤ 1/4, so 64 factors are already below what a Decimal
            // holds: a larger published exponent must not spin the loop.
            let steps = self.exponent.to_u32().unwrap_or(u32::MAX).min(64);
            (0..steps).fold(Decimal::ONE, |acc, _| acc * base)
        } else {
            base.to_f64()
                .zip(self.exponent.to_f64())
                .and_then(|(base, exponent)| Decimal::from_f64_retain(base.powf(exponent)))
                // An unrepresentable curve is priced at exponent 1, the
                // larger value for any p(1 − p) ≤ 1/4.
                .unwrap_or(base)
        };
        self.rate * curve
    }
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
    /// Fee terms the opportunity was priced with, so it can be re-priced on
    /// fresh books before execution.
    #[serde(default)]
    pub fee_schedule: Option<FeeSchedule>,
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
            fee_drag: total_cost * fee,
            worst_case_payout: Decimal::ONE - (total_cost * fee),
            yes_fee_shares: Decimal::ZERO,
            no_fee_shares: Decimal::ZERO,
            fee_schedule: None,
        })
    }

    /// Price buying one YES and one NO share at the best asks under the
    /// market's taker fees. Fees are charged in USDC on each leg, so the pair
    /// still pays $1 at resolution and the fees reduce the net edge.
    pub fn calculate_with_fee_schedule(
        book: &BinaryMarketBook,
        schedule: FeeSchedule,
    ) -> Option<Self> {
        let (yes_ask, no_ask, total_cost) = book.entry_cost()?;
        if total_cost <= Decimal::ZERO {
            return None;
        }

        let gross_profit = Decimal::ONE - total_cost;
        let fee_drag = schedule.fee_per_share(yes_ask) + schedule.fee_per_share(no_ask);
        let net_profit = gross_profit - fee_drag;

        Some(Self {
            market_id: book.market_id.clone(),
            timestamp: book.timestamp,
            yes_ask,
            no_ask,
            total_cost,
            gross_profit,
            net_profit,
            fee_drag,
            worst_case_payout: (Decimal::ONE - fee_drag).max(Decimal::ZERO),
            yes_fee_shares: Decimal::ZERO,
            no_fee_shares: Decimal::ZERO,
            fee_schedule: Some(schedule),
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

    fn level(price: i64, size: i64) -> PriceLevel {
        PriceLevel {
            price: Decimal::new(price, 2),
            size: Decimal::new(size, 0),
        }
    }

    #[test]
    fn every_binary_market_maps_its_tokens_to_yes_and_no_one_way() {
        let outcome = |name: &str, token: &str| Outcome {
            id: token.into(),
            name: name.into(),
            token_id: token.into(),
            price: None,
            winner: None,
        };
        let market = |outcomes| Market {
            id: "m".into(),
            question: "q".into(),
            description: None,
            outcomes,
            volume: Decimal::ZERO,
            liquidity: Decimal::ZERO,
            end_date: None,
            resolved: false,
            resolution: None,
            category: None,
            tags: vec![],
            fees_enabled: false,
            fee_type: None,
            fee_schedule: None,
        };
        let pair = |a: &str, b: &str| Some((a.to_string(), b.to_string()));
        assert_eq!(
            market(vec![outcome("Yes", "y"), outcome("No", "n")]).yes_no_token_ids(),
            pair("y", "n")
        );
        assert_eq!(
            market(vec![outcome("No", "n"), outcome("Yes", "y")]).yes_no_token_ids(),
            pair("y", "n")
        );
        // Named outcomes: by convention the second is YES.
        assert_eq!(
            market(vec![outcome("Lakers", "l"), outcome("Celtics", "c")]).yes_no_token_ids(),
            pair("c", "l")
        );
        assert_eq!(market(vec![outcome("A", "a")]).yes_no_token_ids(), None);
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

        let arb = ArbOpportunity::calculate_with_fee_schedule(&book, FeeSchedule::FREE).unwrap();
        assert_eq!(arb.fee_drag, Decimal::ZERO);
        assert_eq!(arb.worst_case_payout, Decimal::ONE);
        assert_eq!(arb.net_profit, Decimal::new(6, 2));
        assert_eq!(arb.fee_schedule, Some(FeeSchedule::FREE));
    }

    #[test]
    fn test_fee_enabled_market_charges_usdc_taker_fees_on_both_legs() {
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
                    price: Decimal::new(50, 2),
                    size: Decimal::new(100, 0),
                }],
            },
        };

        // sports_fees_v3: fee = C × 0.05 × p × (1 − p) on each leg.
        let schedule = FeeSchedule {
            rate: Decimal::new(5, 2),
            exponent: Decimal::ONE,
        };
        let arb = ArbOpportunity::calculate_with_fee_schedule(&book, schedule).unwrap();
        // 0.05 × 0.48 × 0.52 + 0.05 × 0.50 × 0.50 = 0.01248 + 0.0125
        assert_eq!(arb.fee_drag, "0.02498".parse::<Decimal>().unwrap());
        assert_eq!(arb.gross_profit, Decimal::new(2, 2));
        // A 2% gross edge is a loss once both taker fees are paid.
        assert_eq!(arb.net_profit, "-0.00498".parse::<Decimal>().unwrap());
        assert!(!arb.is_profitable());
        assert_eq!(arb.worst_case_payout, Decimal::ONE - arb.fee_drag);
        assert_eq!(arb.yes_fee_shares, Decimal::ZERO);
    }

    #[test]
    fn effective_fee_schedule_never_assumes_free_when_fees_are_reported() {
        let mut market = Market {
            id: "m".into(),
            question: "q".into(),
            description: None,
            outcomes: vec![],
            volume: Decimal::ZERO,
            liquidity: Decimal::ZERO,
            end_date: None,
            resolved: false,
            resolution: None,
            category: None,
            tags: vec![],
            fees_enabled: false,
            fee_type: None,
            fee_schedule: None,
        };
        assert_eq!(market.effective_fee_schedule(), FeeSchedule::FREE);
        market.fees_enabled = true;
        assert_eq!(market.effective_fee_schedule(), FeeSchedule::CONSERVATIVE);
        let zero = FeeSchedule {
            rate: Decimal::ZERO,
            exponent: Decimal::ONE,
        };
        market.fee_schedule = Some(zero);
        assert_eq!(market.effective_fee_schedule(), zero);
    }

    #[test]
    fn a_huge_exponent_is_priced_at_once() {
        let schedule = FeeSchedule {
            rate: Decimal::new(7, 2),
            exponent: Decimal::from(1_000_000_000u64),
        };
        let started = std::time::Instant::now();
        assert!(schedule.fee_per_share(Decimal::new(5, 1)) < Decimal::new(1, 20));
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
    }

    #[test]
    fn fee_curve_honours_integer_exponents() {
        let squared = FeeSchedule {
            rate: Decimal::ONE,
            exponent: Decimal::TWO,
        };
        assert_eq!(
            squared.fee_per_share(Decimal::new(50, 2)),
            Decimal::new(625, 4)
        );
        assert_eq!(
            FeeSchedule::FREE.fee_per_share(Decimal::new(50, 2)),
            Decimal::ZERO
        );
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
