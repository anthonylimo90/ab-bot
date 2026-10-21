//! Realized trade history for the bot-score win-rate feature.
//!
//! On-chain ERC-20 transfers say nothing about whether trades were profitable,
//! so the `win_rate` feature is derived from the wallet's BUY/SELL fills in
//! `wallet_trades` (collected from the Polymarket Data API), replayed through
//! the shared FIFO engine. When a wallet has no (or too little) history the
//! feature stays `None`; it is never estimated.

use anyhow::Result;
use polymarket_core::feature_extractor::fifo::Fill;
use polymarket_core::feature_extractor::realized_win_rate;
use polymarket_core::types::WalletFeatures;
use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use tracing::warn;

/// Newest `wallet_trades` rows replayed per wallet.
const WALLET_TRADES_LIMIT: i64 = 2000;

/// Load a wallet's newest fills from `wallet_trades`, oldest first.
pub async fn load_fills(pool: &PgPool, address: &str) -> Result<Vec<Fill>> {
    let rows = sqlx::query(
        r#"
        SELECT asset_id, side, price, quantity, timestamp
        FROM wallet_trades
        WHERE LOWER(wallet_address) = LOWER($1)
        ORDER BY timestamp DESC
        LIMIT $2
        "#,
    )
    .bind(address)
    .bind(WALLET_TRADES_LIMIT)
    .fetch_all(pool)
    .await?;

    let mut fills: Vec<Fill> = rows
        .iter()
        .map(|row| {
            let side: String = row.get("side");
            Fill {
                timestamp: row.get("timestamp"),
                asset_id: row.get("asset_id"),
                is_buy: side.eq_ignore_ascii_case("BUY"),
                price: row.get::<Decimal, _>("price"),
                quantity: row.get::<Decimal, _>("quantity"),
            }
        })
        .collect();
    fills.sort_by_key(|f| f.timestamp);
    Ok(fills)
}

/// Realized win rate of `address`, or `None` if it has too few closed sales.
pub async fn win_rate_for(pool: &PgPool, address: &str) -> Result<Option<f64>> {
    let fills = load_fills(pool, address).await?;
    Ok(realized_win_rate(&fills))
}

/// Fill `features.win_rate` from realized trade history.
///
/// A database failure is logged and leaves the feature `None`: an unavailable
/// signal must not abort scoring.
pub async fn attach_win_rate(pool: &PgPool, features: &mut WalletFeatures) {
    match win_rate_for(pool, &features.address).await {
        Ok(win_rate) => features.win_rate = win_rate,
        Err(e) => warn!(
            address = %features.address,
            error = %e,
            "Could not load trade history for win-rate feature"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Duration, TimeZone, Utc};

    async fn create_table(pool: &PgPool) {
        sqlx::raw_sql(
            r#"
            CREATE TABLE wallet_trades (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                transaction_hash VARCHAR NOT NULL UNIQUE,
                wallet_address VARCHAR NOT NULL,
                asset_id VARCHAR NOT NULL,
                condition_id VARCHAR,
                side VARCHAR NOT NULL CHECK (side IN ('BUY', 'SELL')),
                price DECIMAL(20, 10) NOT NULL,
                quantity DECIMAL(20, 10) NOT NULL,
                value DECIMAL(20, 10) NOT NULL,
                timestamp TIMESTAMPTZ NOT NULL,
                outcome TEXT
            )
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 3, 1, 0, 0, 0).unwrap()
    }

    async fn insert(
        pool: &PgPool,
        wallet: &str,
        n: usize,
        asset: &str,
        side: &str,
        price: &str,
        minute: i64,
    ) {
        sqlx::query(
            "INSERT INTO wallet_trades \
             (transaction_hash, wallet_address, asset_id, side, price, quantity, value, timestamp) \
             VALUES ($1, $2, $3, $4, $5, 10, $5 * 10, $6)",
        )
        .bind(format!("0xtx{wallet}{n}"))
        .bind(wallet)
        .bind(asset)
        .bind(side)
        .bind(price.parse::<Decimal>().unwrap())
        .bind(t0() + Duration::minutes(minute))
        .execute(pool)
        .await
        .unwrap();
    }

    /// `wins` + `losses` closed round trips, one asset each.
    async fn insert_round_trips(pool: &PgPool, wallet: &str, wins: usize, losses: usize) {
        for i in 0..wins + losses {
            let asset = format!("tok{i}");
            let sell = if i < wins { "0.60" } else { "0.40" };
            insert(pool, wallet, 2 * i, &asset, "BUY", "0.50", 2 * i as i64).await;
            insert(
                pool,
                wallet,
                2 * i + 1,
                &asset,
                "SELL",
                sell,
                2 * i as i64 + 1,
            )
            .await;
        }
    }

    #[sqlx::test(migrations = false)]
    async fn win_rate_comes_from_realized_sales(pool: PgPool) {
        create_table(&pool).await;
        insert_round_trips(&pool, "0xBot", 24, 1).await;
        // An open position and a basis-less sale must not move the rate.
        insert(&pool, "0xBot", 900, "held", "BUY", "0.30", 5000).await;
        insert(&pool, "0xBot", 901, "orphan", "SELL", "0.99", 5001).await;

        let rate = win_rate_for(&pool, "0xbot").await.unwrap().unwrap();
        assert!((rate - 24.0 / 25.0).abs() < 1e-12, "got {rate}");
    }

    #[sqlx::test(migrations = false)]
    async fn too_little_history_leaves_win_rate_unset(pool: PgPool) {
        create_table(&pool).await;
        insert_round_trips(&pool, "0xnew", 5, 0).await;
        assert_eq!(win_rate_for(&pool, "0xnew").await.unwrap(), None);
        // A wallet with no rows at all.
        assert_eq!(win_rate_for(&pool, "0xnobody").await.unwrap(), None);
    }

    #[sqlx::test(migrations = false)]
    async fn other_wallets_do_not_leak_in(pool: PgPool) {
        create_table(&pool).await;
        insert_round_trips(&pool, "0xa", 25, 0).await;
        insert_round_trips(&pool, "0xb", 0, 25).await;
        assert_eq!(win_rate_for(&pool, "0xa").await.unwrap(), Some(1.0));
        assert_eq!(win_rate_for(&pool, "0xb").await.unwrap(), Some(0.0));
    }

    #[sqlx::test(migrations = false)]
    async fn attach_win_rate_sets_the_feature_and_tolerates_failures(pool: PgPool) {
        create_table(&pool).await;
        insert_round_trips(&pool, "0xbot", 23, 2).await;

        let mut features = WalletFeatures {
            address: "0xbot".to_string(),
            ..Default::default()
        };
        attach_win_rate(&pool, &mut features).await;
        assert!((features.win_rate.unwrap() - 0.92).abs() < 1e-12);

        // Missing table: the error is swallowed and the feature stays unset.
        sqlx::raw_sql("DROP TABLE wallet_trades")
            .execute(&pool)
            .await
            .unwrap();
        let mut features = WalletFeatures {
            address: "0xbot".to_string(),
            ..Default::default()
        };
        attach_win_rate(&pool, &mut features).await;
        assert_eq!(features.win_rate, None);
    }
}
