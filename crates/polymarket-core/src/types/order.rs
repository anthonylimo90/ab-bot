//! Order types for trading execution.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Side of the order (buy or sell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Type of order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderType {
    Market,
    Limit,
    /// Good-til-cancelled limit order.
    GTC,
    /// Fill-or-kill - must be fully filled or cancelled.
    FOK,
}

/// Current status of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    /// Order created but not yet submitted.
    Created,
    /// Order submitted to exchange.
    Pending,
    /// Order partially filled.
    PartiallyFilled,
    /// Order fully filled.
    Filled,
    /// Order cancelled.
    Cancelled,
    /// Order rejected by exchange.
    Rejected,
    /// Order expired (for time-limited orders).
    Expired,
}

/// A market order that executes immediately at best available price.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketOrder {
    pub id: Uuid,
    pub market_id: String,
    pub outcome_id: String,
    pub side: OrderSide,
    pub quantity: Decimal,
    pub created_at: DateTime<Utc>,
    pub status: OrderStatus,
    /// Maximum slippage tolerance (e.g., 0.01 = 1%).
    pub max_slippage: Option<Decimal>,
}

impl MarketOrder {
    pub fn new(
        market_id: String,
        outcome_id: String,
        side: OrderSide,
        quantity: Decimal,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            market_id,
            outcome_id,
            side,
            quantity,
            created_at: Utc::now(),
            status: OrderStatus::Created,
            max_slippage: None,
        }
    }

    pub fn with_slippage(mut self, slippage: Decimal) -> Self {
        self.max_slippage = Some(slippage);
        self
    }
}

/// A limit order with a specific price.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitOrder {
    pub id: Uuid,
    pub market_id: String,
    pub outcome_id: String,
    pub side: OrderSide,
    pub price: Decimal,
    pub quantity: Decimal,
    pub order_type: OrderType,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub status: OrderStatus,
    pub filled_quantity: Decimal,
    pub average_fill_price: Option<Decimal>,
}

impl LimitOrder {
    pub fn new(
        market_id: String,
        outcome_id: String,
        side: OrderSide,
        price: Decimal,
        quantity: Decimal,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            market_id,
            outcome_id,
            side,
            price,
            quantity,
            order_type: OrderType::Limit,
            created_at: Utc::now(),
            expires_at: None,
            status: OrderStatus::Created,
            filled_quantity: Decimal::ZERO,
            average_fill_price: None,
        }
    }

    pub fn gtc(mut self) -> Self {
        self.order_type = OrderType::GTC;
        self
    }

    pub fn fok(mut self) -> Self {
        self.order_type = OrderType::FOK;
        self
    }

    pub fn with_expiry(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    pub fn remaining_quantity(&self) -> Decimal {
        self.quantity - self.filled_quantity
    }

    pub fn is_fully_filled(&self) -> bool {
        self.filled_quantity >= self.quantity
    }
}

/// Report of an executed order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub order_id: Uuid,
    pub exchange_order_id: Option<String>,
    pub market_id: String,
    pub outcome_id: String,
    pub side: OrderSide,
    pub status: OrderStatus,
    pub requested_quantity: Decimal,
    pub filled_quantity: Decimal,
    pub average_price: Decimal,
    pub fees_paid: Decimal,
    pub executed_at: DateTime<Utc>,
    pub transaction_hash: Option<String>,
    pub error_message: Option<String>,
}

impl ExecutionReport {
    pub fn success(
        order_id: Uuid,
        market_id: String,
        outcome_id: String,
        side: OrderSide,
        filled_quantity: Decimal,
        average_price: Decimal,
        fees_paid: Decimal,
    ) -> Self {
        Self {
            order_id,
            exchange_order_id: None,
            market_id,
            outcome_id,
            side,
            status: OrderStatus::Filled,
            requested_quantity: filled_quantity,
            filled_quantity,
            average_price,
            fees_paid,
            executed_at: Utc::now(),
            transaction_hash: None,
            error_message: None,
        }
    }

    pub fn rejected(order_id: Uuid, market_id: String, outcome_id: String, side: OrderSide, error: String) -> Self {
        Self {
            order_id,
            exchange_order_id: None,
            market_id,
            outcome_id,
            side,
            status: OrderStatus::Rejected,
            requested_quantity: Decimal::ZERO,
            filled_quantity: Decimal::ZERO,
            average_price: Decimal::ZERO,
            fees_paid: Decimal::ZERO,
            executed_at: Utc::now(),
            transaction_hash: None,
            error_message: Some(error),
        }
    }

    pub fn with_tx_hash(mut self, hash: String) -> Self {
        self.transaction_hash = Some(hash);
        self
    }

    pub fn with_exchange_id(mut self, id: String) -> Self {
        self.exchange_order_id = Some(id);
        self
    }

    pub fn total_value(&self) -> Decimal {
        self.filled_quantity * self.average_price
    }

    pub fn is_success(&self) -> bool {
        self.status == OrderStatus::Filled || self.status == OrderStatus::PartiallyFilled
    }
}

/// Aggregated order for arbitrage (buying both YES and NO).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbOrder {
    pub id: Uuid,
    pub market_id: String,
    pub yes_order: MarketOrder,
    pub no_order: MarketOrder,
    pub total_quantity: Decimal,
    pub expected_cost: Decimal,
    pub expected_profit: Decimal,
    pub created_at: DateTime<Utc>,
}

impl ArbOrder {
    pub fn new(
        market_id: String,
        yes_outcome_id: String,
        no_outcome_id: String,
        quantity: Decimal,
        expected_yes_price: Decimal,
        expected_no_price: Decimal,
    ) -> Self {
        let yes_order = MarketOrder::new(
            market_id.clone(),
            yes_outcome_id,
            OrderSide::Buy,
            quantity,
        );
        let no_order = MarketOrder::new(
            market_id.clone(),
            no_outcome_id,
            OrderSide::Buy,
            quantity,
        );
        let expected_cost = (expected_yes_price + expected_no_price) * quantity;
        let expected_profit = quantity - expected_cost;

        Self {
            id: Uuid::new_v4(),
            market_id,
            yes_order,
            no_order,
            total_quantity: quantity,
            expected_cost,
            expected_profit,
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_market_order_creation() {
        let order = MarketOrder::new(
            "market123".to_string(),
            "yes_token".to_string(),
            OrderSide::Buy,
            Decimal::new(100, 0),
        )
        .with_slippage(Decimal::new(1, 2));

        assert_eq!(order.status, OrderStatus::Created);
        assert_eq!(order.quantity, Decimal::new(100, 0));
        assert_eq!(order.max_slippage, Some(Decimal::new(1, 2)));
    }

    #[test]
    fn test_limit_order_fill_tracking() {
        let mut order = LimitOrder::new(
            "market123".to_string(),
            "yes_token".to_string(),
            OrderSide::Buy,
            Decimal::new(50, 2), // 0.50
            Decimal::new(100, 0),
        )
        .gtc();

        assert_eq!(order.order_type, OrderType::GTC);
        assert_eq!(order.remaining_quantity(), Decimal::new(100, 0));
        assert!(!order.is_fully_filled());

        order.filled_quantity = Decimal::new(50, 0);
        assert_eq!(order.remaining_quantity(), Decimal::new(50, 0));

        order.filled_quantity = Decimal::new(100, 0);
        assert!(order.is_fully_filled());
    }

    #[test]
    fn test_arb_order_profit_calculation() {
        let arb = ArbOrder::new(
            "market123".to_string(),
            "yes".to_string(),
            "no".to_string(),
            Decimal::new(100, 0),
            Decimal::new(48, 2), // 0.48
            Decimal::new(46, 2), // 0.46
        );

        // Cost: (0.48 + 0.46) * 100 = 94
        assert_eq!(arb.expected_cost, Decimal::new(94, 0));
        // Profit: 100 - 94 = 6
        assert_eq!(arb.expected_profit, Decimal::new(6, 0));
    }
}
