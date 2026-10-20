use super::*;
use crate::data_store::TimeResolution;
use sqlx::Executor;

struct BuyAndHold;
#[async_trait::async_trait]
impl Strategy for BuyAndHold {
    fn name(&self) -> &str {
        "review-buy-and-hold"
    }
    async fn on_data(&mut self, _: &StrategyContext) -> Result<Vec<Signal>> {
        Ok(vec![Signal::buy("market", "yes", Decimal::new(1, 1))])
    }
}

#[tokio::test]
async fn review_terminal_equity_includes_forced_close_fees() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1/unused")
        .unwrap();
    let simulator = BacktestSimulator::new(
        HistoricalDataStore::new(pool),
        SimulatorConfig {
            initial_capital: Decimal::from(1000),
            slippage_model: SlippageModel::None,
            fee_model: FeeModel::Fixed(Decimal::new(2, 2)),
            ..Default::default()
        },
    );
    let start = Utc::now();
    let prepared = PreparedBacktest::new(vec![MarketSnapshot::new(
        "market",
        start,
        Decimal::new(5, 1),
        Decimal::new(5, 1),
        Decimal::new(5, 1),
        Decimal::new(5, 1),
    )])
    .unwrap();
    let result = simulator
        .run_prepared(
            &mut BuyAndHold,
            &DataQuery::range(start, start + chrono::Duration::days(1)),
            &prepared,
        )
        .await
        .unwrap();
    assert!((result.max_drawdown - 0.004).abs() < 1e-10);
    assert_eq!(result.end_time, result.equity_curve.last().unwrap().0);
    assert_eq!(
        result.equity_curve.last().unwrap().1,
        result.final_value,
        "terminal equity omits forced-close costs"
    );
}

#[sqlx::test(migrations = false)]
#[ignore = "requires an isolated TimescaleDB test service"]
async fn review_aggregated_prices_are_not_timestamped_before_observation(pool: sqlx::PgPool) {
    pool.execute("CREATE EXTENSION IF NOT EXISTS timescaledb")
        .await
        .unwrap();
    pool.execute(
        "CREATE TABLE orderbook_snapshots (
        market_id TEXT, timestamp TIMESTAMPTZ, yes_bid NUMERIC, yes_ask NUMERIC,
        no_bid NUMERIC DEFAULT 0.5,no_ask NUMERIC DEFAULT 0.5,
        yes_bid_depth NUMERIC DEFAULT 100,yes_ask_depth NUMERIC DEFAULT 100,
        no_bid_depth NUMERIC DEFAULT 100,no_ask_depth NUMERIC DEFAULT 100,
        yes_mid NUMERIC DEFAULT 0.5,no_mid NUMERIC DEFAULT 0.5,
        yes_spread NUMERIC DEFAULT 0,no_spread NUMERIC DEFAULT 0,
        volume_24h NUMERIC DEFAULT 100);
        INSERT INTO orderbook_snapshots(market_id,timestamp,yes_bid,yes_ask)
        VALUES ('market','2026-10-19T12:00:00Z',0.1,0.1),
               ('market','2026-10-19T12:04:00Z',0.9,0.9)",
    )
    .await
    .unwrap();
    let start = "2026-10-19T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let store = HistoricalDataStore::new(pool);
    let incomplete = store
        .query_snapshots(
            &DataQuery::range(start, start + chrono::Duration::minutes(4))
                .resolution(TimeResolution::Minute5),
        )
        .await
        .unwrap();
    assert!(incomplete.is_empty());
    let rows = store
        .query_snapshots(
            &DataQuery::range(start, start + chrono::Duration::minutes(5))
                .resolution(TimeResolution::Minute5),
        )
        .await
        .unwrap();
    assert_eq!(rows[0].yes_bid, Decimal::new(9, 1));
    assert!(
        rows[0].timestamp >= start + chrono::Duration::minutes(4),
        "12:04 price was exposed to the strategy at {}",
        rows[0].timestamp
    );
}
