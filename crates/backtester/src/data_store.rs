//! Historical data storage using TimescaleDB hypertables.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use tracing::{debug, info};

/// A point-in-time snapshot of market orderbook state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSnapshot {
    /// Market identifier.
    pub market_id: String,
    /// Timestamp of the snapshot.
    pub timestamp: DateTime<Utc>,
    /// Best yes bid price.
    pub yes_bid: Decimal,
    /// Best yes ask price.
    pub yes_ask: Decimal,
    /// Best no bid price.
    pub no_bid: Decimal,
    /// Best no ask price.
    pub no_ask: Decimal,
    /// Yes bid depth (quantity available).
    pub yes_bid_depth: Decimal,
    /// Yes ask depth (quantity available).
    pub yes_ask_depth: Decimal,
    /// No bid depth.
    pub no_bid_depth: Decimal,
    /// No ask depth.
    pub no_ask_depth: Decimal,
    /// Mid price for yes outcome.
    pub yes_mid: Decimal,
    /// Mid price for no outcome.
    pub no_mid: Decimal,
    /// Spread for yes outcome.
    pub yes_spread: Decimal,
    /// Spread for no outcome.
    pub no_spread: Decimal,
    /// 24h volume.
    pub volume_24h: Decimal,
}

impl MarketSnapshot {
    /// Create a new market snapshot.
    pub fn new(
        market_id: &str,
        timestamp: DateTime<Utc>,
        yes_bid: Decimal,
        yes_ask: Decimal,
        no_bid: Decimal,
        no_ask: Decimal,
    ) -> Self {
        let yes_mid = (yes_bid + yes_ask) / Decimal::TWO;
        let no_mid = (no_bid + no_ask) / Decimal::TWO;

        Self {
            market_id: market_id.to_string(),
            timestamp,
            yes_bid,
            yes_ask,
            no_bid,
            no_ask,
            yes_bid_depth: Decimal::ZERO,
            yes_ask_depth: Decimal::ZERO,
            no_bid_depth: Decimal::ZERO,
            no_ask_depth: Decimal::ZERO,
            yes_mid,
            no_mid,
            yes_spread: yes_ask - yes_bid,
            no_spread: no_ask - no_bid,
            volume_24h: Decimal::ZERO,
        }
    }

    /// Set depth values.
    pub fn with_depth(
        mut self,
        yes_bid_depth: Decimal,
        yes_ask_depth: Decimal,
        no_bid_depth: Decimal,
        no_ask_depth: Decimal,
    ) -> Self {
        self.yes_bid_depth = yes_bid_depth;
        self.yes_ask_depth = yes_ask_depth;
        self.no_bid_depth = no_bid_depth;
        self.no_ask_depth = no_ask_depth;
        self
    }

    /// Set 24h volume.
    pub fn with_volume(mut self, volume: Decimal) -> Self {
        self.volume_24h = volume;
        self
    }

    /// Check if there's an arbitrage opportunity.
    pub fn has_arbitrage(&self, min_spread: Decimal) -> bool {
        let total_ask = self.yes_ask + self.no_ask;
        (Decimal::ONE - total_ask) >= min_spread
    }

    /// Calculate arbitrage spread.
    pub fn arbitrage_spread(&self) -> Decimal {
        Decimal::ONE - (self.yes_ask + self.no_ask)
    }
}

/// A historical trade record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalTrade {
    /// Trade identifier.
    pub id: uuid::Uuid,
    /// Market identifier.
    pub market_id: String,
    /// Outcome traded (yes/no).
    pub outcome_id: String,
    /// Trade timestamp.
    pub timestamp: DateTime<Utc>,
    /// Trade price.
    pub price: Decimal,
    /// Trade quantity.
    pub quantity: Decimal,
    /// Trade side (buy/sell).
    pub side: TradeSide,
    /// Maker/taker fee paid.
    pub fee: Decimal,
}

/// Trade side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeSide {
    Buy,
    Sell,
}

/// Time resolution for data aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeResolution {
    /// 1 second
    Second,
    /// 1 minute
    Minute,
    /// 5 minutes
    Minute5,
    /// 15 minutes
    Minute15,
    /// 1 hour
    Hour,
    /// 1 day
    Day,
}

impl TimeResolution {
    /// Get the interval as a chrono Duration.
    pub fn to_duration(&self) -> Duration {
        match self {
            TimeResolution::Second => Duration::seconds(1),
            TimeResolution::Minute => Duration::minutes(1),
            TimeResolution::Minute5 => Duration::minutes(5),
            TimeResolution::Minute15 => Duration::minutes(15),
            TimeResolution::Hour => Duration::hours(1),
            TimeResolution::Day => Duration::days(1),
        }
    }

    /// Get the PostgreSQL interval string.
    pub fn to_pg_interval(&self) -> &str {
        match self {
            TimeResolution::Second => "1 second",
            TimeResolution::Minute => "1 minute",
            TimeResolution::Minute5 => "5 minutes",
            TimeResolution::Minute15 => "15 minutes",
            TimeResolution::Hour => "1 hour",
            TimeResolution::Day => "1 day",
        }
    }
}

/// Query parameters for fetching historical data.
#[derive(Debug, Clone)]
pub struct DataQuery {
    /// Market IDs to fetch (empty = all).
    pub market_ids: Vec<String>,
    /// Start time.
    pub start_time: DateTime<Utc>,
    /// End time.
    pub end_time: DateTime<Utc>,
    /// Time resolution for aggregation.
    pub resolution: TimeResolution,
    /// Maximum number of records.
    pub limit: Option<usize>,
}

impl DataQuery {
    /// Create a new query for the last N days.
    pub fn last_days(days: i64) -> Self {
        Self {
            market_ids: vec![],
            start_time: Utc::now() - Duration::days(days),
            end_time: Utc::now(),
            resolution: TimeResolution::Minute5,
            limit: None,
        }
    }

    /// Create a query for a specific time range.
    pub fn range(start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self {
            market_ids: vec![],
            start_time: start,
            end_time: end,
            resolution: TimeResolution::Minute5,
            limit: None,
        }
    }

    /// Filter by specific markets.
    pub fn markets(mut self, market_ids: Vec<String>) -> Self {
        self.market_ids = market_ids;
        self
    }

    /// Set time resolution.
    pub fn resolution(mut self, resolution: TimeResolution) -> Self {
        self.resolution = resolution;
        self
    }

    /// Set maximum records.
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// Historical data store backed by TimescaleDB.
pub struct HistoricalDataStore {
    pool: PgPool,
}

impl HistoricalDataStore {
    /// Create a new historical data store.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a market snapshot.
    pub async fn insert_snapshot(&self, snapshot: &MarketSnapshot) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO orderbook_snapshots (
                market_id, timestamp, yes_bid, yes_ask, no_bid, no_ask,
                yes_bid_depth, yes_ask_depth, no_bid_depth, no_ask_depth,
                yes_mid, no_mid, yes_spread, no_spread, volume_24h
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            ON CONFLICT (market_id, timestamp) DO UPDATE SET
                yes_bid = EXCLUDED.yes_bid,
                yes_ask = EXCLUDED.yes_ask,
                no_bid = EXCLUDED.no_bid,
                no_ask = EXCLUDED.no_ask,
                yes_bid_depth = EXCLUDED.yes_bid_depth,
                yes_ask_depth = EXCLUDED.yes_ask_depth,
                no_bid_depth = EXCLUDED.no_bid_depth,
                no_ask_depth = EXCLUDED.no_ask_depth,
                yes_mid = EXCLUDED.yes_mid,
                no_mid = EXCLUDED.no_mid,
                yes_spread = EXCLUDED.yes_spread,
                no_spread = EXCLUDED.no_spread,
                volume_24h = EXCLUDED.volume_24h
            "#,
        )
        .bind(&snapshot.market_id)
        .bind(&snapshot.timestamp)
        .bind(&snapshot.yes_bid)
        .bind(&snapshot.yes_ask)
        .bind(&snapshot.no_bid)
        .bind(&snapshot.no_ask)
        .bind(&snapshot.yes_bid_depth)
        .bind(&snapshot.yes_ask_depth)
        .bind(&snapshot.no_bid_depth)
        .bind(&snapshot.no_ask_depth)
        .bind(&snapshot.yes_mid)
        .bind(&snapshot.no_mid)
        .bind(&snapshot.yes_spread)
        .bind(&snapshot.no_spread)
        .bind(&snapshot.volume_24h)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Insert multiple snapshots in batch.
    pub async fn insert_snapshots_batch(&self, snapshots: &[MarketSnapshot]) -> Result<usize> {
        if snapshots.is_empty() {
            return Ok(0);
        }

        let mut inserted = 0;
        for snapshot in snapshots {
            if self.insert_snapshot(snapshot).await.is_ok() {
                inserted += 1;
            }
        }

        info!(count = inserted, "Inserted orderbook snapshots");
        Ok(inserted)
    }

    /// Insert a historical trade.
    pub async fn insert_trade(&self, trade: &HistoricalTrade) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO historical_trades (
                id, market_id, outcome_id, timestamp, price, quantity, side, fee
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(&trade.id)
        .bind(&trade.market_id)
        .bind(&trade.outcome_id)
        .bind(&trade.timestamp)
        .bind(&trade.price)
        .bind(&trade.quantity)
        .bind(match trade.side {
            TradeSide::Buy => 0i16,
            TradeSide::Sell => 1i16,
        })
        .bind(&trade.fee)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Query orderbook snapshots.
    pub async fn query_snapshots(&self, query: &DataQuery) -> Result<Vec<MarketSnapshot>> {
        let sql = if query.market_ids.is_empty() {
            format!(
                r#"
                SELECT
                    market_id,
                    time_bucket('{}', timestamp) AS bucket,
                    last(yes_bid, timestamp) as yes_bid,
                    last(yes_ask, timestamp) as yes_ask,
                    last(no_bid, timestamp) as no_bid,
                    last(no_ask, timestamp) as no_ask,
                    last(yes_bid_depth, timestamp) as yes_bid_depth,
                    last(yes_ask_depth, timestamp) as yes_ask_depth,
                    last(no_bid_depth, timestamp) as no_bid_depth,
                    last(no_ask_depth, timestamp) as no_ask_depth,
                    last(yes_mid, timestamp) as yes_mid,
                    last(no_mid, timestamp) as no_mid,
                    last(yes_spread, timestamp) as yes_spread,
                    last(no_spread, timestamp) as no_spread,
                    max(volume_24h) as volume_24h
                FROM orderbook_snapshots
                WHERE timestamp >= $1 AND timestamp <= $2
                GROUP BY market_id, bucket
                ORDER BY bucket
                {}
                "#,
                query.resolution.to_pg_interval(),
                query.limit.map(|l| format!("LIMIT {}", l)).unwrap_or_default()
            )
        } else {
            format!(
                r#"
                SELECT
                    market_id,
                    time_bucket('{}', timestamp) AS bucket,
                    last(yes_bid, timestamp) as yes_bid,
                    last(yes_ask, timestamp) as yes_ask,
                    last(no_bid, timestamp) as no_bid,
                    last(no_ask, timestamp) as no_ask,
                    last(yes_bid_depth, timestamp) as yes_bid_depth,
                    last(yes_ask_depth, timestamp) as yes_ask_depth,
                    last(no_bid_depth, timestamp) as no_bid_depth,
                    last(no_ask_depth, timestamp) as no_ask_depth,
                    last(yes_mid, timestamp) as yes_mid,
                    last(no_mid, timestamp) as no_mid,
                    last(yes_spread, timestamp) as yes_spread,
                    last(no_spread, timestamp) as no_spread,
                    max(volume_24h) as volume_24h
                FROM orderbook_snapshots
                WHERE market_id = ANY($3)
                  AND timestamp >= $1 AND timestamp <= $2
                GROUP BY market_id, bucket
                ORDER BY bucket
                {}
                "#,
                query.resolution.to_pg_interval(),
                query.limit.map(|l| format!("LIMIT {}", l)).unwrap_or_default()
            )
        };

        let rows = if query.market_ids.is_empty() {
            sqlx::query(&sql)
                .bind(&query.start_time)
                .bind(&query.end_time)
                .fetch_all(&self.pool)
                .await?
        } else {
            sqlx::query(&sql)
                .bind(&query.start_time)
                .bind(&query.end_time)
                .bind(&query.market_ids)
                .fetch_all(&self.pool)
                .await?
        };

        let snapshots: Vec<MarketSnapshot> = rows
            .iter()
            .map(|row| {
                use sqlx::Row;
                MarketSnapshot {
                    market_id: row.get("market_id"),
                    timestamp: row.get("bucket"),
                    yes_bid: row.get("yes_bid"),
                    yes_ask: row.get("yes_ask"),
                    no_bid: row.get("no_bid"),
                    no_ask: row.get("no_ask"),
                    yes_bid_depth: row.get("yes_bid_depth"),
                    yes_ask_depth: row.get("yes_ask_depth"),
                    no_bid_depth: row.get("no_bid_depth"),
                    no_ask_depth: row.get("no_ask_depth"),
                    yes_mid: row.get("yes_mid"),
                    no_mid: row.get("no_mid"),
                    yes_spread: row.get("yes_spread"),
                    no_spread: row.get("no_spread"),
                    volume_24h: row.get("volume_24h"),
                }
            })
            .collect();

        debug!(count = snapshots.len(), "Fetched orderbook snapshots");
        Ok(snapshots)
    }

    /// Query historical trades.
    pub async fn query_trades(
        &self,
        market_id: &str,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> Result<Vec<HistoricalTrade>> {
        let rows = sqlx::query(
            r#"
            SELECT id, market_id, outcome_id, timestamp, price, quantity, side, fee
            FROM historical_trades
            WHERE market_id = $1 AND timestamp >= $2 AND timestamp <= $3
            ORDER BY timestamp
            "#,
        )
        .bind(market_id)
        .bind(&start_time)
        .bind(&end_time)
        .fetch_all(&self.pool)
        .await?;

        let trades: Vec<HistoricalTrade> = rows
            .iter()
            .map(|row| {
                use sqlx::Row;
                HistoricalTrade {
                    id: row.get("id"),
                    market_id: row.get("market_id"),
                    outcome_id: row.get("outcome_id"),
                    timestamp: row.get("timestamp"),
                    price: row.get("price"),
                    quantity: row.get("quantity"),
                    side: if row.get::<i16, _>("side") == 0 {
                        TradeSide::Buy
                    } else {
                        TradeSide::Sell
                    },
                    fee: row.get("fee"),
                }
            })
            .collect();

        Ok(trades)
    }

    /// Get available markets in the data store.
    pub async fn get_available_markets(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT market_id
            FROM orderbook_snapshots
            ORDER BY market_id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let markets: Vec<String> = rows
            .iter()
            .map(|row| {
                use sqlx::Row;
                row.get("market_id")
            })
            .collect();

        Ok(markets)
    }

    /// Get data range for a market.
    pub async fn get_data_range(&self, market_id: &str) -> Result<Option<(DateTime<Utc>, DateTime<Utc>)>> {
        let row = sqlx::query(
            r#"
            SELECT MIN(timestamp) as min_ts, MAX(timestamp) as max_ts
            FROM orderbook_snapshots
            WHERE market_id = $1
            "#,
        )
        .bind(market_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.and_then(|r| {
            use sqlx::Row;
            let min_ts: Option<DateTime<Utc>> = r.get("min_ts");
            let max_ts: Option<DateTime<Utc>> = r.get("max_ts");
            min_ts.zip(max_ts)
        }))
    }

    /// Get snapshot count for a market.
    pub async fn get_snapshot_count(&self, market_id: &str) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*) as count
            FROM orderbook_snapshots
            WHERE market_id = $1
            "#,
        )
        .bind(market_id)
        .fetch_one(&self.pool)
        .await?;

        use sqlx::Row;
        Ok(row.get("count"))
    }

    /// Delete old data (for maintenance).
    pub async fn delete_old_data(&self, older_than: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM orderbook_snapshots
            WHERE timestamp < $1
            "#,
        )
        .bind(&older_than)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Group snapshots by market for backtest processing.
    pub fn group_by_market(snapshots: &[MarketSnapshot]) -> HashMap<String, Vec<MarketSnapshot>> {
        let mut grouped: HashMap<String, Vec<MarketSnapshot>> = HashMap::new();

        for snapshot in snapshots {
            grouped
                .entry(snapshot.market_id.clone())
                .or_default()
                .push(snapshot.clone());
        }

        // Sort each group by timestamp
        for snapshots in grouped.values_mut() {
            snapshots.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        }

        grouped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_market_snapshot_creation() {
        let snapshot = MarketSnapshot::new(
            "market1",
            Utc::now(),
            Decimal::new(48, 2),
            Decimal::new(50, 2),
            Decimal::new(48, 2),
            Decimal::new(50, 2),
        );

        assert_eq!(snapshot.market_id, "market1");
        assert_eq!(snapshot.yes_mid, Decimal::new(49, 2));
        assert_eq!(snapshot.yes_spread, Decimal::new(2, 2));
    }

    #[test]
    fn test_arbitrage_detection() {
        // Total ask = 0.50 + 0.48 = 0.98, spread = 0.02
        let snapshot = MarketSnapshot::new(
            "market1",
            Utc::now(),
            Decimal::new(48, 2),
            Decimal::new(50, 2),
            Decimal::new(46, 2),
            Decimal::new(48, 2),
        );

        assert!(snapshot.has_arbitrage(Decimal::new(2, 2)));
        assert!(!snapshot.has_arbitrage(Decimal::new(3, 2)));
        assert_eq!(snapshot.arbitrage_spread(), Decimal::new(2, 2));
    }

    #[test]
    fn test_data_query_builder() {
        let query = DataQuery::last_days(7)
            .markets(vec!["market1".to_string()])
            .resolution(TimeResolution::Hour)
            .limit(1000);

        assert_eq!(query.market_ids, vec!["market1"]);
        assert_eq!(query.resolution, TimeResolution::Hour);
        assert_eq!(query.limit, Some(1000));
    }

    #[test]
    fn test_time_resolution() {
        assert_eq!(TimeResolution::Minute.to_pg_interval(), "1 minute");
        assert_eq!(TimeResolution::Hour.to_duration(), Duration::hours(1));
    }

    #[test]
    fn test_group_by_market() {
        let snapshots = vec![
            MarketSnapshot::new("market1", Utc::now(), Decimal::ONE, Decimal::ONE, Decimal::ONE, Decimal::ONE),
            MarketSnapshot::new("market2", Utc::now(), Decimal::ONE, Decimal::ONE, Decimal::ONE, Decimal::ONE),
            MarketSnapshot::new("market1", Utc::now(), Decimal::ONE, Decimal::ONE, Decimal::ONE, Decimal::ONE),
        ];

        let grouped = HistoricalDataStore::group_by_market(&snapshots);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped.get("market1").map(|v| v.len()), Some(2));
        assert_eq!(grouped.get("market2").map(|v| v.len()), Some(1));
    }
}
