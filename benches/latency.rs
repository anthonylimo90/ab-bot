//! Latency benchmarks for critical trading operations.
//!
//! Run with: `cargo bench --bench latency`

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use chrono::Utc;
use rust_decimal::Decimal;
use uuid::Uuid;

// Re-export types from polymarket-core
use polymarket_core::types::{
    ArbOpportunity, BinaryMarketBook, OrderBook, PriceLevel,
};

/// Generate a synthetic orderbook with the specified depth.
fn generate_orderbook(market_id: &str, outcome_id: &str, depth: usize) -> OrderBook {
    let base_price = Decimal::new(50, 2); // 0.50
    let mut bids = Vec::with_capacity(depth);
    let mut asks = Vec::with_capacity(depth);

    for i in 0..depth {
        let offset = Decimal::new(i as i64, 2);
        bids.push(PriceLevel {
            price: base_price - offset,
            size: Decimal::new(100 + i as i64 * 10, 0),
        });
        asks.push(PriceLevel {
            price: base_price + Decimal::new(1, 2) + offset,
            size: Decimal::new(100 + i as i64 * 10, 0),
        });
    }

    OrderBook {
        market_id: market_id.to_string(),
        outcome_id: outcome_id.to_string(),
        timestamp: Utc::now(),
        bids,
        asks,
    }
}

/// Generate a binary market book with potential arbitrage.
fn generate_arb_market(market_id: &str, yes_ask: i64, no_ask: i64, depth: usize) -> BinaryMarketBook {
    let mut yes_book = generate_orderbook(market_id, "yes", depth);
    let mut no_book = generate_orderbook(market_id, "no", depth);

    // Override best ask prices
    if let Some(ask) = yes_book.asks.first_mut() {
        ask.price = Decimal::new(yes_ask, 2);
    }
    if let Some(ask) = no_book.asks.first_mut() {
        ask.price = Decimal::new(no_ask, 2);
    }

    BinaryMarketBook {
        market_id: market_id.to_string(),
        timestamp: Utc::now(),
        yes_book,
        no_book,
    }
}

/// Benchmark arbitrage detection calculation.
fn bench_arb_detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("arb_detection");

    // Test with different orderbook depths
    for depth in [5, 10, 50, 100].iter() {
        let market = generate_arb_market("test_market", 48, 46, *depth);
        let fee = ArbOpportunity::DEFAULT_FEE;

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(
            BenchmarkId::new("calculate", depth),
            &market,
            |b, market| {
                b.iter(|| {
                    black_box(ArbOpportunity::calculate(black_box(market), black_box(fee)))
                })
            },
        );
    }

    group.finish();
}

/// Benchmark orderbook best price lookups.
fn bench_orderbook_lookups(c: &mut Criterion) {
    let mut group = c.benchmark_group("orderbook_lookups");

    for depth in [5, 10, 50, 100].iter() {
        let book = generate_orderbook("market", "yes", *depth);

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(
            BenchmarkId::new("best_bid", depth),
            &book,
            |b, book| {
                b.iter(|| black_box(book.best_bid()))
            },
        );

        group.bench_with_input(
            BenchmarkId::new("best_ask", depth),
            &book,
            |b, book| {
                b.iter(|| black_box(book.best_ask()))
            },
        );
    }

    group.finish();
}

/// Benchmark entry cost calculation.
fn bench_entry_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("entry_cost");

    for depth in [5, 10, 50, 100].iter() {
        let market = generate_arb_market("test_market", 50, 50, *depth);

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(
            BenchmarkId::new("calculate", depth),
            &market,
            |b, market| {
                b.iter(|| black_box(market.entry_cost()))
            },
        );
    }

    group.finish();
}

/// Benchmark signal serialization (JSON encode).
fn bench_signal_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("signal_serialization");

    let arb = ArbOpportunity {
        market_id: "market_12345".to_string(),
        timestamp: Utc::now(),
        yes_ask: Decimal::new(48, 2),
        no_ask: Decimal::new(46, 2),
        total_cost: Decimal::new(94, 2),
        gross_profit: Decimal::new(6, 2),
        net_profit: Decimal::new(4, 2),
    };

    group.throughput(Throughput::Elements(1));
    group.bench_function("arb_to_json", |b| {
        b.iter(|| black_box(serde_json::to_string(black_box(&arb))))
    });

    let json = serde_json::to_string(&arb).unwrap();
    group.bench_function("json_to_arb", |b| {
        b.iter(|| black_box(serde_json::from_str::<ArbOpportunity>(black_box(&json))))
    });

    group.finish();
}

/// Benchmark UUID generation (used for order IDs).
fn bench_uuid_generation(c: &mut Criterion) {
    c.bench_function("uuid_v4", |b| {
        b.iter(|| black_box(Uuid::new_v4()))
    });
}

/// Benchmark Decimal arithmetic (critical for price calculations).
fn bench_decimal_arithmetic(c: &mut Criterion) {
    let mut group = c.benchmark_group("decimal_arithmetic");

    let val_a = Decimal::new(12345, 4); // 1.2345
    let val_b = Decimal::new(67890, 4); // 6.7890

    group.bench_function("addition", |bencher| {
        bencher.iter(|| black_box(black_box(val_a) + black_box(val_b)))
    });

    group.bench_function("multiplication", |bencher| {
        bencher.iter(|| black_box(black_box(val_a) * black_box(val_b)))
    });

    group.bench_function("division", |bencher| {
        bencher.iter(|| black_box(black_box(val_a) / black_box(val_b)))
    });

    group.bench_function("comparison", |bencher| {
        bencher.iter(|| black_box(black_box(val_a) > black_box(val_b)))
    });

    group.finish();
}

/// Benchmark stop-loss trigger check.
fn bench_stop_loss_check(c: &mut Criterion) {
    use risk_manager::{StopLossRule, StopType};

    let mut group = c.benchmark_group("stop_loss");

    // Fixed stop
    let mut fixed_rule = StopLossRule::new(
        Uuid::new_v4(),
        "market".to_string(),
        "yes".to_string(),
        Decimal::new(50, 2),
        Decimal::new(100, 0),
        StopType::fixed(Decimal::new(40, 2)),
    );
    fixed_rule.activate();

    group.bench_function("fixed_trigger_check", |b| {
        b.iter(|| {
            black_box(fixed_rule.is_triggered(black_box(Decimal::new(38, 2))))
        })
    });

    // Percentage stop
    let mut pct_rule = StopLossRule::new(
        Uuid::new_v4(),
        "market".to_string(),
        "yes".to_string(),
        Decimal::new(50, 2),
        Decimal::new(100, 0),
        StopType::percentage(Decimal::new(20, 2)),
    );
    pct_rule.activate();

    group.bench_function("percentage_trigger_check", |b| {
        b.iter(|| {
            black_box(pct_rule.is_triggered(black_box(Decimal::new(38, 2))))
        })
    });

    // Trailing stop
    let mut trailing_rule = StopLossRule::new(
        Uuid::new_v4(),
        "market".to_string(),
        "yes".to_string(),
        Decimal::new(50, 2),
        Decimal::new(100, 0),
        StopType::trailing(Decimal::new(10, 2)),
    );
    trailing_rule.activate();
    trailing_rule.update_peak(Decimal::new(60, 2));

    group.bench_function("trailing_trigger_check", |b| {
        b.iter(|| {
            black_box(trailing_rule.is_triggered(black_box(Decimal::new(52, 2))))
        })
    });

    group.bench_function("trailing_peak_update", |b| {
        let mut rule = trailing_rule.clone();
        b.iter(|| {
            rule.update_peak(black_box(Decimal::new(65, 2)));
        })
    });

    group.finish();
}

/// Benchmark concurrent DashMap operations (used for order/position tracking).
fn bench_dashmap_operations(c: &mut Criterion) {
    use dashmap::DashMap;

    let mut group = c.benchmark_group("dashmap");

    let map: DashMap<Uuid, String> = DashMap::new();

    // Pre-populate
    for _ in 0..1000 {
        let id = Uuid::new_v4();
        map.insert(id, format!("value_{}", id));
    }

    // Get a known key for lookup tests
    let known_key = *map.iter().next().unwrap().key();

    group.bench_function("insert", |b| {
        b.iter(|| {
            let id = Uuid::new_v4();
            map.insert(id, black_box(format!("value_{}", id)));
        })
    });

    group.bench_function("get", |b| {
        b.iter(|| {
            black_box(map.get(&known_key))
        })
    });

    group.bench_function("contains", |b| {
        b.iter(|| {
            black_box(map.contains_key(&known_key))
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_arb_detection,
    bench_orderbook_lookups,
    bench_entry_cost,
    bench_signal_serialization,
    bench_uuid_generation,
    bench_decimal_arithmetic,
    bench_stop_loss_check,
    bench_dashmap_operations,
);

criterion_main!(benches);
