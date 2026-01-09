//! Throughput benchmarks for bulk operations.
//!
//! Run with: `cargo bench --bench throughput`

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use chrono::Utc;
use rand::Rng;
use rust_decimal::Decimal;
use std::collections::HashMap;
use uuid::Uuid;

use polymarket_core::types::{
    ArbOpportunity, BinaryMarketBook, OrderBook, PriceLevel,
};

/// Generate a random orderbook with specified depth.
fn generate_random_orderbook(
    rng: &mut impl Rng,
    market_id: &str,
    outcome_id: &str,
    depth: usize,
) -> OrderBook {
    let base_price = Decimal::new(rng.gen_range(30..70), 2);
    let mut bids = Vec::with_capacity(depth);
    let mut asks = Vec::with_capacity(depth);

    for i in 0..depth {
        let offset = Decimal::new(i as i64 + 1, 2);
        bids.push(PriceLevel {
            price: base_price - offset,
            size: Decimal::new(rng.gen_range(50..500), 0),
        });
        asks.push(PriceLevel {
            price: base_price + offset,
            size: Decimal::new(rng.gen_range(50..500), 0),
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

/// Generate a batch of binary market books.
fn generate_market_batch(count: usize, depth: usize) -> Vec<BinaryMarketBook> {
    let mut rng = rand::thread_rng();
    let mut markets = Vec::with_capacity(count);

    for i in 0..count {
        let market_id = format!("market_{}", i);
        let yes_book = generate_random_orderbook(&mut rng, &market_id, "yes", depth);
        let no_book = generate_random_orderbook(&mut rng, &market_id, "no", depth);

        markets.push(BinaryMarketBook {
            market_id,
            timestamp: Utc::now(),
            yes_book,
            no_book,
        });
    }

    markets
}

/// Benchmark scanning multiple markets for arbitrage.
fn bench_arb_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("arb_scan");
    let fee = ArbOpportunity::DEFAULT_FEE;

    for market_count in [10, 50, 100, 500, 1000].iter() {
        let markets = generate_market_batch(*market_count, 10);

        group.throughput(Throughput::Elements(*market_count as u64));
        group.bench_with_input(
            BenchmarkId::new("scan_markets", market_count),
            &markets,
            |b, markets| {
                b.iter(|| {
                    let mut opportunities = Vec::new();
                    for market in markets {
                        if let Some(arb) = ArbOpportunity::calculate(market, fee) {
                            if arb.is_profitable() {
                                opportunities.push(arb);
                            }
                        }
                    }
                    black_box(opportunities)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark parallel arbitrage scanning using rayon.
fn bench_parallel_arb_scan(c: &mut Criterion) {
    use rayon::prelude::*;

    let mut group = c.benchmark_group("parallel_arb_scan");
    let fee = ArbOpportunity::DEFAULT_FEE;

    for market_count in [100, 500, 1000, 5000].iter() {
        let markets = generate_market_batch(*market_count, 10);

        group.throughput(Throughput::Elements(*market_count as u64));
        group.bench_with_input(
            BenchmarkId::new("parallel_scan", market_count),
            &markets,
            |b, markets| {
                b.iter(|| {
                    let opportunities: Vec<_> = markets
                        .par_iter()
                        .filter_map(|market| {
                            ArbOpportunity::calculate(market, fee)
                                .filter(|arb| arb.is_profitable())
                        })
                        .collect();
                    black_box(opportunities)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark bulk stop-loss trigger checking.
fn bench_bulk_stop_check(c: &mut Criterion) {
    use risk_manager::{StopLossRule, StopType};

    let mut group = c.benchmark_group("bulk_stop_check");

    for rule_count in [10, 50, 100, 500, 1000].iter() {
        // Create rules
        let mut rules: Vec<StopLossRule> = Vec::with_capacity(*rule_count);
        let mut rng = rand::thread_rng();

        for _ in 0..*rule_count {
            let entry_price = Decimal::new(rng.gen_range(30..70), 2);
            let stop_type = match rng.gen_range(0..3) {
                0 => StopType::fixed(entry_price - Decimal::new(10, 2)),
                1 => StopType::percentage(Decimal::new(rng.gen_range(10..30), 2)),
                _ => {
                    let mut stop = StopType::trailing(Decimal::new(rng.gen_range(5..15), 2));
                    if let StopType::Trailing { ref mut peak_price, .. } = stop {
                        *peak_price = entry_price + Decimal::new(10, 2);
                    }
                    stop
                }
            };

            let mut rule = StopLossRule::new(
                Uuid::new_v4(),
                format!("market_{}", rng.gen_range(0..100)),
                "yes".to_string(),
                entry_price,
                Decimal::new(100, 0),
                stop_type,
            );
            rule.activate();
            rules.push(rule);
        }

        // Generate test prices
        let test_price = Decimal::new(35, 2);

        group.throughput(Throughput::Elements(*rule_count as u64));
        group.bench_with_input(
            BenchmarkId::new("check_all", rule_count),
            &rules,
            |b, rules| {
                b.iter(|| {
                    let triggered: Vec<_> = rules
                        .iter()
                        .filter(|rule| rule.is_triggered(test_price))
                        .collect();
                    black_box(triggered)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark orderbook update processing.
fn bench_orderbook_updates(c: &mut Criterion) {
    let mut group = c.benchmark_group("orderbook_updates");

    // Simulate processing orderbook updates into a cache
    for update_count in [100, 500, 1000, 5000].iter() {
        let mut rng = rand::thread_rng();

        // Generate updates
        let updates: Vec<(String, OrderBook)> = (0..*update_count)
            .map(|_| {
                let market_id = format!("market_{}", rng.gen_range(0..100));
                let book = generate_random_orderbook(&mut rng, &market_id, "yes", 10);
                (market_id, book)
            })
            .collect();

        group.throughput(Throughput::Elements(*update_count as u64));
        group.bench_with_input(
            BenchmarkId::new("process_updates", update_count),
            &updates,
            |b, updates| {
                b.iter(|| {
                    let mut cache: HashMap<String, OrderBook> = HashMap::new();
                    for (market_id, book) in updates {
                        cache.insert(market_id.clone(), book.clone());
                    }
                    black_box(cache)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark DashMap concurrent updates (more realistic for multi-threaded).
fn bench_dashmap_bulk_updates(c: &mut Criterion) {
    use dashmap::DashMap;

    let mut group = c.benchmark_group("dashmap_bulk");

    for update_count in [100, 500, 1000, 5000].iter() {
        let mut rng = rand::thread_rng();

        // Generate updates
        let updates: Vec<(String, OrderBook)> = (0..*update_count)
            .map(|_| {
                let market_id = format!("market_{}", rng.gen_range(0..100));
                let book = generate_random_orderbook(&mut rng, &market_id, "yes", 10);
                (market_id, book)
            })
            .collect();

        group.throughput(Throughput::Elements(*update_count as u64));
        group.bench_with_input(
            BenchmarkId::new("process_updates", update_count),
            &updates,
            |b, updates| {
                b.iter(|| {
                    let cache: DashMap<String, OrderBook> = DashMap::new();
                    for (market_id, book) in updates {
                        cache.insert(market_id.clone(), book.clone());
                    }
                    black_box(cache)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark JSON serialization throughput for signals.
fn bench_signal_batch_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("signal_batch_serialization");

    for count in [10, 50, 100, 500].iter() {
        let mut rng = rand::thread_rng();

        let arbs: Vec<ArbOpportunity> = (0..*count)
            .map(|i| {
                let yes_ask = Decimal::new(rng.gen_range(40..55), 2);
                let no_ask = Decimal::new(rng.gen_range(40..55), 2);
                ArbOpportunity {
                    market_id: format!("market_{}", i),
                    timestamp: Utc::now(),
                    yes_ask,
                    no_ask,
                    total_cost: yes_ask + no_ask,
                    gross_profit: Decimal::ONE - (yes_ask + no_ask),
                    net_profit: Decimal::ONE - (yes_ask + no_ask) - Decimal::new(2, 2),
                }
            })
            .collect();

        group.throughput(Throughput::Elements(*count as u64));
        group.bench_with_input(
            BenchmarkId::new("serialize_batch", count),
            &arbs,
            |b, arbs| {
                b.iter(|| {
                    let serialized: Vec<_> = arbs
                        .iter()
                        .map(|arb| serde_json::to_string(arb).unwrap())
                        .collect();
                    black_box(serialized)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark position P&L calculations.
fn bench_pnl_calculations(c: &mut Criterion) {
    let mut group = c.benchmark_group("pnl_calculations");

    for position_count in [10, 50, 100, 500, 1000].iter() {
        let mut rng = rand::thread_rng();

        // Generate positions with entry prices
        let positions: Vec<(Decimal, Decimal, Decimal)> = (0..*position_count)
            .map(|_| {
                let entry = Decimal::new(rng.gen_range(30..70), 2);
                let current = Decimal::new(rng.gen_range(25..75), 2);
                let quantity = Decimal::new(rng.gen_range(10..1000), 0);
                (entry, current, quantity)
            })
            .collect();

        group.throughput(Throughput::Elements(*position_count as u64));
        group.bench_with_input(
            BenchmarkId::new("calculate_all", position_count),
            &positions,
            |b, positions| {
                b.iter(|| {
                    let pnls: Vec<Decimal> = positions
                        .iter()
                        .map(|(entry, current, qty)| (current - entry) * qty)
                        .collect();
                    black_box(pnls)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark market filtering (common operation for finding tradeable markets).
fn bench_market_filtering(c: &mut Criterion) {
    let mut group = c.benchmark_group("market_filtering");

    for market_count in [100, 500, 1000, 5000].iter() {
        let markets = generate_market_batch(*market_count, 10);
        let fee = ArbOpportunity::DEFAULT_FEE;

        // Filter: profitable arb, minimum profit threshold
        let min_profit = Decimal::new(1, 2); // 0.01

        group.throughput(Throughput::Elements(*market_count as u64));
        group.bench_with_input(
            BenchmarkId::new("filter_profitable", market_count),
            &markets,
            |b, markets| {
                b.iter(|| {
                    let filtered: Vec<_> = markets
                        .iter()
                        .filter_map(|m| ArbOpportunity::calculate(m, fee))
                        .filter(|arb| arb.net_profit >= min_profit)
                        .collect();
                    black_box(filtered)
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_arb_scan,
    bench_parallel_arb_scan,
    bench_bulk_stop_check,
    bench_orderbook_updates,
    bench_dashmap_bulk_updates,
    bench_signal_batch_serialization,
    bench_pnl_calculations,
    bench_market_filtering,
);

criterion_main!(benches);
