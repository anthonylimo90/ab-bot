//! Database repository for stop-loss rules.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use tracing::{debug, info};
use uuid::Uuid;

use crate::stop_loss::{StopLossRule, StopType};

/// Repository for stop-loss rule persistence.
pub struct StopLossRepository {
    pool: PgPool,
}

impl StopLossRepository {
    /// Create a new repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new stop-loss rule.
    pub async fn insert(&self, rule: &StopLossRule) -> Result<()> {
        let (stop_type_id, trigger_price, loss_pct, trailing_offset, peak_price, deadline) =
            Self::decompose_stop_type(&rule.stop_type);

        sqlx::query(
            r#"
            INSERT INTO stop_loss_rules (
                id, position_id, market_id, outcome_id, entry_price, quantity,
                stop_type, trigger_price, loss_percentage, trailing_offset_pct,
                peak_price, deadline, activated, activated_at, executed, executed_at,
                created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
            "#,
        )
        .bind(rule.id)
        .bind(rule.position_id)
        .bind(&rule.market_id)
        .bind(&rule.outcome_id)
        .bind(rule.entry_price)
        .bind(rule.quantity)
        .bind(stop_type_id)
        .bind(trigger_price)
        .bind(loss_pct)
        .bind(trailing_offset)
        .bind(peak_price)
        .bind(deadline)
        .bind(rule.activated)
        .bind(rule.activated_at)
        .bind(rule.executed)
        .bind(rule.executed_at)
        .bind(rule.created_at)
        .execute(&self.pool)
        .await?;

        debug!(rule_id = %rule.id, "Inserted stop-loss rule");
        Ok(())
    }

    /// Update an existing stop-loss rule.
    pub async fn update(&self, rule: &StopLossRule) -> Result<()> {
        let (stop_type_id, trigger_price, loss_pct, trailing_offset, peak_price, deadline) =
            Self::decompose_stop_type(&rule.stop_type);

        sqlx::query(
            r#"
            UPDATE stop_loss_rules SET
                stop_type = $2,
                trigger_price = $3,
                loss_percentage = $4,
                trailing_offset_pct = $5,
                peak_price = $6,
                deadline = $7,
                activated = $8,
                activated_at = $9,
                executed = $10,
                executed_at = $11,
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(rule.id)
        .bind(stop_type_id)
        .bind(trigger_price)
        .bind(loss_pct)
        .bind(trailing_offset)
        .bind(peak_price)
        .bind(deadline)
        .bind(rule.activated)
        .bind(rule.activated_at)
        .bind(rule.executed)
        .bind(rule.executed_at)
        .execute(&self.pool)
        .await?;

        debug!(rule_id = %rule.id, "Updated stop-loss rule");
        Ok(())
    }

    /// Get a rule by ID.
    pub async fn get(&self, id: Uuid) -> Result<Option<StopLossRule>> {
        let row = sqlx::query(
            r#"
            SELECT
                id, position_id, market_id, outcome_id, entry_price, quantity,
                stop_type, trigger_price, loss_percentage, trailing_offset_pct,
                peak_price, deadline, activated, activated_at, executed, executed_at,
                created_at
            FROM stop_loss_rules
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| Self::row_to_rule(&r)))
    }

    /// Get all active (activated but not executed) rules.
    pub async fn get_active(&self) -> Result<Vec<StopLossRule>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, position_id, market_id, outcome_id, entry_price, quantity,
                stop_type, trigger_price, loss_percentage, trailing_offset_pct,
                peak_price, deadline, activated, activated_at, executed, executed_at,
                created_at
            FROM stop_loss_rules
            WHERE activated = TRUE AND executed = FALSE
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        info!(
            count = rows.len(),
            "Loaded active stop-loss rules from database"
        );
        Ok(rows.iter().map(Self::row_to_rule).collect())
    }

    /// Get all rules for a position.
    pub async fn get_by_position(&self, position_id: Uuid) -> Result<Vec<StopLossRule>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, position_id, market_id, outcome_id, entry_price, quantity,
                stop_type, trigger_price, loss_percentage, trailing_offset_pct,
                peak_price, deadline, activated, activated_at, executed, executed_at,
                created_at
            FROM stop_loss_rules
            WHERE position_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(position_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(Self::row_to_rule).collect())
    }

    /// Delete a rule by ID.
    pub async fn delete(&self, id: Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM stop_loss_rules WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Delete all rules for a position.
    pub async fn delete_by_position(&self, position_id: Uuid) -> Result<u64> {
        let result = sqlx::query("DELETE FROM stop_loss_rules WHERE position_id = $1")
            .bind(position_id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }

    /// Convert database row to StopLossRule.
    fn row_to_rule(r: &sqlx::postgres::PgRow) -> StopLossRule {
        let stop_type_id: i16 = r.get("stop_type");
        let trigger_price: Option<Decimal> = r.get("trigger_price");
        let loss_pct: Option<Decimal> = r.get("loss_percentage");
        let trailing_offset: Option<Decimal> = r.get("trailing_offset_pct");
        let peak_price: Option<Decimal> = r.get("peak_price");
        let deadline: Option<DateTime<Utc>> = r.get("deadline");

        let stop_type = match stop_type_id {
            0 => StopType::Fixed {
                trigger_price: trigger_price.unwrap_or_default(),
            },
            1 => StopType::Percentage {
                loss_pct: loss_pct.unwrap_or_default(),
            },
            2 => StopType::Trailing {
                offset_pct: trailing_offset.unwrap_or_default(),
                peak_price: peak_price.unwrap_or_default(),
            },
            3 => StopType::TimeBased {
                deadline: deadline.unwrap_or_else(Utc::now),
            },
            _ => StopType::Fixed {
                trigger_price: Decimal::ZERO,
            },
        };

        StopLossRule {
            id: r.get("id"),
            position_id: r.get("position_id"),
            market_id: r.get("market_id"),
            outcome_id: r.get("outcome_id"),
            entry_price: r.get("entry_price"),
            quantity: r.get("quantity"),
            stop_type,
            activated: r.get("activated"),
            activated_at: r.get("activated_at"),
            executed: r.get("executed"),
            executed_at: r.get("executed_at"),
            created_at: r.get("created_at"),
        }
    }

    /// Decompose StopType into database columns.
    fn decompose_stop_type(
        stop_type: &StopType,
    ) -> (
        i16,
        Option<Decimal>,
        Option<Decimal>,
        Option<Decimal>,
        Option<Decimal>,
        Option<DateTime<Utc>>,
    ) {
        match stop_type {
            StopType::Fixed { trigger_price } => (0, Some(*trigger_price), None, None, None, None),
            StopType::Percentage { loss_pct } => (1, None, Some(*loss_pct), None, None, None),
            StopType::Trailing {
                offset_pct,
                peak_price,
            } => (2, None, None, Some(*offset_pct), Some(*peak_price), None),
            StopType::TimeBased { deadline } => (3, None, None, None, None, Some(*deadline)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decompose_fixed_stop() {
        let stop = StopType::Fixed {
            trigger_price: Decimal::new(50, 2),
        };
        let (id, trigger, loss, trailing, peak, deadline) =
            StopLossRepository::decompose_stop_type(&stop);

        assert_eq!(id, 0);
        assert_eq!(trigger, Some(Decimal::new(50, 2)));
        assert!(loss.is_none());
        assert!(trailing.is_none());
        assert!(peak.is_none());
        assert!(deadline.is_none());
    }

    #[test]
    fn test_decompose_percentage_stop() {
        let stop = StopType::Percentage {
            loss_pct: Decimal::new(10, 2),
        };
        let (id, trigger, loss, trailing, peak, deadline) =
            StopLossRepository::decompose_stop_type(&stop);

        assert_eq!(id, 1);
        assert!(trigger.is_none());
        assert_eq!(loss, Some(Decimal::new(10, 2)));
        assert!(trailing.is_none());
        assert!(peak.is_none());
        assert!(deadline.is_none());
    }

    #[test]
    fn test_decompose_trailing_stop() {
        let stop = StopType::Trailing {
            offset_pct: Decimal::new(5, 2),
            peak_price: Decimal::new(100, 2),
        };
        let (id, trigger, loss, trailing, peak, deadline) =
            StopLossRepository::decompose_stop_type(&stop);

        assert_eq!(id, 2);
        assert!(trigger.is_none());
        assert!(loss.is_none());
        assert_eq!(trailing, Some(Decimal::new(5, 2)));
        assert_eq!(peak, Some(Decimal::new(100, 2)));
        assert!(deadline.is_none());
    }
}
