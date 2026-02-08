//! Database operations for wallet features and bot scores.

use crate::types::{BotScore, WalletClassification, WalletFeatures};
use crate::Result;
use sqlx::{PgPool, Row};

/// Repository for wallet analysis data.
pub struct WalletRepository {
    pool: PgPool,
}

impl WalletRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert or update wallet features.
    pub async fn upsert_features(&self, features: &WalletFeatures) -> Result<()> {
        let hourly_dist: Vec<i64> = features
            .hourly_distribution
            .iter()
            .map(|&x| x as i64)
            .collect();

        sqlx::query(
            r#"
            INSERT INTO wallet_features (
                address, total_trades, interval_cv, win_rate, avg_latency_ms,
                markets_traded, has_opposing_positions, opposing_position_count,
                hourly_distribution, activity_spread, total_volume,
                first_trade, last_trade
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (address) DO UPDATE SET
                total_trades = EXCLUDED.total_trades,
                interval_cv = EXCLUDED.interval_cv,
                win_rate = EXCLUDED.win_rate,
                avg_latency_ms = EXCLUDED.avg_latency_ms,
                markets_traded = EXCLUDED.markets_traded,
                has_opposing_positions = EXCLUDED.has_opposing_positions,
                opposing_position_count = EXCLUDED.opposing_position_count,
                hourly_distribution = EXCLUDED.hourly_distribution,
                activity_spread = EXCLUDED.activity_spread,
                total_volume = EXCLUDED.total_volume,
                first_trade = EXCLUDED.first_trade,
                last_trade = EXCLUDED.last_trade,
                updated_at = NOW()
            "#,
        )
        .bind(&features.address)
        .bind(features.total_trades as i64)
        .bind(features.interval_cv)
        .bind(features.win_rate)
        .bind(features.avg_latency_ms)
        .bind(features.markets_traded as i64)
        .bind(features.has_opposing_positions)
        .bind(features.opposing_position_count as i64)
        .bind(&hourly_dist)
        .bind(features.activity_spread)
        .bind(features.total_volume)
        .bind(features.first_trade)
        .bind(features.last_trade)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Accumulate wallet features from CLOB trade data.
    ///
    /// Unlike `upsert_features` which replaces all fields, this method
    /// *adds* the incoming trade count and volume to existing values and
    /// widens the first_trade / last_trade window.
    pub async fn accumulate_features(
        &self,
        address: &str,
        trade_count: i64,
        total_volume: rust_decimal::Decimal,
        first_trade: chrono::DateTime<chrono::Utc>,
        last_trade: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO wallet_features (address, total_trades, total_volume, first_trade, last_trade)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (address) DO UPDATE SET
                total_trades = wallet_features.total_trades + EXCLUDED.total_trades,
                total_volume = wallet_features.total_volume + EXCLUDED.total_volume,
                first_trade = LEAST(wallet_features.first_trade, EXCLUDED.first_trade),
                last_trade = GREATEST(wallet_features.last_trade, EXCLUDED.last_trade),
                updated_at = NOW()
            "#,
        )
        .bind(address)
        .bind(trade_count)
        .bind(total_volume)
        .bind(first_trade)
        .bind(last_trade)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Insert a bot score.
    pub async fn insert_score(&self, score: &BotScore) -> Result<()> {
        let signals_json = serde_json::to_value(&score.signals)?;

        sqlx::query(
            r#"
            INSERT INTO bot_scores (
                address, total_score, signals, classification, computed_at
            )
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(&score.address)
        .bind(score.total_score as i32)
        .bind(&signals_json)
        .bind(score.classification as i16)
        .bind(score.computed_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get wallets flagged as likely bots.
    pub async fn get_flagged_wallets(&self, limit: i64) -> Result<Vec<BotScore>> {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT ON (address)
                address, total_score, signals, classification, computed_at
            FROM bot_scores
            WHERE classification = 2
            ORDER BY address, computed_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let signals_json: serde_json::Value = r.get("signals");
                Some(BotScore {
                    address: r.get("address"),
                    total_score: r.get::<i32, _>("total_score") as u32,
                    signals: serde_json::from_value(signals_json).ok()?,
                    classification: match r.get::<i16, _>("classification") {
                        0 => WalletClassification::LikelyHuman,
                        1 => WalletClassification::Suspicious,
                        _ => WalletClassification::LikelyBot,
                    },
                    computed_at: r.get("computed_at"),
                })
            })
            .collect())
    }

    /// Get the latest score for a wallet.
    pub async fn get_latest_score(&self, address: &str) -> Result<Option<BotScore>> {
        let row = sqlx::query(
            r#"
            SELECT address, total_score, signals, classification, computed_at
            FROM bot_scores
            WHERE address = $1
            ORDER BY computed_at DESC
            LIMIT 1
            "#,
        )
        .bind(address)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.and_then(|r| {
            let signals_json: serde_json::Value = r.get("signals");
            Some(BotScore {
                address: r.get("address"),
                total_score: r.get::<i32, _>("total_score") as u32,
                signals: serde_json::from_value(signals_json).ok()?,
                classification: match r.get::<i16, _>("classification") {
                    0 => WalletClassification::LikelyHuman,
                    1 => WalletClassification::Suspicious,
                    _ => WalletClassification::LikelyBot,
                },
                computed_at: r.get("computed_at"),
            })
        }))
    }
}
