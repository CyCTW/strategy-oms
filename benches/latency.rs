//! Per-operation wall-clock samples, including timer overhead. This is a local
//! smoke benchmark, not a production SLO or a coordinated-omission-safe load test.
use std::{hint::black_box, time::Instant};
use strategy_oms::{journal::MemoryJournal, *};

fn summary(name: &str, samples: &mut [u128]) {
    samples.sort_unstable();
    let p = |n: usize| samples[(samples.len() - 1) * n / 1000];
    println!(
        "{name}: n={} p50={}ns p99={}ns p99.9={}ns max={}ns",
        samples.len(),
        p(500),
        p(990),
        p(999),
        samples.last().unwrap()
    );
}

fn main() {
    let n = 20_000usize;
    let limits = Limits {
        max_orders: 1024,
        max_requests: 1024,
        max_executions: n + 1024,
        max_reports: n + 2048,
        max_order_qty: 1_000_000,
        max_open_qty_per_book: 100_000_000,
    };
    let mut oms = Engine::new(MemoryJournal::new(n + 4096), limits).unwrap();
    let book = Book {
        strategy: 1,
        account: 1,
        venue: 1,
        instrument: 1,
        side: Side::Buy,
    };
    for i in 1..=1000 {
        oms.apply(Event::New(NewOrder {
            order_id: i,
            request_id: i,
            book,
            price: 100 + (i % 100) as i64,
            total_qty: 100_000,
        }))
        .unwrap();
        oms.apply(Event::Report(Report {
            source: 1,
            sequence: i,
            order_id: i,
            kind: ReportKind::Accepted {
                request_id: i,
                exchange_id: i,
                price: 100 + (i % 100) as i64,
                total_qty: 100_000,
            },
        }))
        .unwrap();
    }
    // Warm query paths before individual samples; timestamps remain in samples.
    for i in 0..10_000 {
        black_box(oms.at_price(book, 100 + i % 100));
    }
    let mut timer = Vec::with_capacity(n);
    let mut query = Vec::with_capacity(n);
    let mut best = Vec::with_capacity(n);
    let mut report = Vec::with_capacity(n);
    for i in 0..n {
        let start = Instant::now();
        black_box(());
        timer.push(start.elapsed().as_nanos());
        let start = Instant::now();
        black_box(oms.at_price(black_box(book), black_box(100 + (i % 100) as i64)));
        query.push(start.elapsed().as_nanos());
        let start = Instant::now();
        black_box(oms.best_working_price(black_box(book)));
        best.push(start.elapsed().as_nanos());
        let event = Event::Report(Report {
            source: 1,
            sequence: 1001 + i as u64,
            order_id: 1 + (i % 1000) as u64,
            kind: ReportKind::Fill {
                key: ExecutionKey {
                    venue: 1,
                    account: 1,
                    trading_day: 20260919,
                    execution_id: i as u64 + 1,
                },
                qty: 1,
                price: 100,
            },
        });
        let start = Instant::now();
        black_box(oms.apply(black_box(event)).unwrap());
        report.push(start.elapsed().as_nanos());
    }
    println!(
        "1,000 active orders / 100 price levels / one book; MemoryJournal; no network, disk sync, or concurrent producers."
    );
    summary("timer baseline", &mut timer);
    summary("at_price", &mut query);
    summary("best_working_price", &mut best);
    summary("fill apply + index + memory journal", &mut report);
}
