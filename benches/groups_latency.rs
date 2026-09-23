//! Group planner smoke benchmark; per-call timer overhead is included.
use std::{hint::black_box, time::Instant};
use strategy_oms::{gateway::GatewayCommand, journal::MemoryJournal, *};

fn accept(e: &mut Engine<MemoryJournal>, d: Dispatch) {
    let kind = match d.command {
        GatewayCommand::New(n) => ReportKind::Accepted {
            request_id: n.request_id,
            exchange_id: n.order_id,
            price: n.price,
            total_qty: n.total_qty,
        },
        GatewayCommand::Change { request: r, .. } => ReportKind::Replaced {
            request_id: r.id,
            exchange_id: r.order_id,
            price: r.price,
            total_qty: r.total_qty,
        },
    };
    e.on_report(Report {
        source: 1,
        sequence: e.last_sequence(1) + 1,
        order_id: d.order_id,
        kind,
    })
    .unwrap();
}
fn summary(label: &str, samples: &mut [u128]) {
    samples.sort_unstable();
    let p = |n: usize| samples[(samples.len() - 1) * n / 1000];
    println!(
        "{label}: n={} p50={}ns p99={}ns p99.9={}ns max={}ns",
        samples.len(),
        p(500),
        p(990),
        p(999),
        samples.last().unwrap()
    );
}
fn main() {
    let n = 20_000;
    let groups = 100;
    let limits = Limits {
        max_orders: 128,
        max_requests: n + 1024,
        max_executions: 128,
        max_reports: n + 1024,
        max_order_qty: 1000,
        max_open_qty_per_book: 1000,
    };
    let mut e = Engine::new(MemoryJournal::new(n * 3 + 1024), limits).unwrap();
    e.configure_scheduler(SchedulerConfig {
        window_ms: 1000,
        max_actions: 1_000_000,
        cancel_reserve: 0,
    })
    .unwrap();
    let book = Book {
        strategy: 1,
        account: 1,
        venue: 1,
        instrument: 1,
        side: Side::Buy,
    };
    let target = |price| {
        Some(Target {
            price,
            qty: 5,
            quantity_mode: QuantityMode::MaintainLeaves,
            expires_at_ms: None,
        })
    };
    for id in 1..=groups {
        e.create_group(
            id,
            book,
            GroupPolicy {
                max_child_leaves: 5,
                max_active_children: 1,
                max_inflight: 1,
                max_reserved_qty: 5,
                ..GroupPolicy::default()
            },
        )
        .unwrap();
        e.set_target(id, 1, target(100)).unwrap();
        let d = e.dispatch_next().unwrap().unwrap();
        accept(&mut e, d);
    }
    let mut updates = Vec::with_capacity(n);
    let mut views = Vec::with_capacity(n);
    let mut plans = Vec::with_capacity(n);
    let mut acks = Vec::with_capacity(n);
    for i in 0..n {
        let group = (i as u64 % groups) + 1;
        let round = i as u64 / groups;
        let price = 101 - (round % 2) as i64;
        let start = Instant::now();
        black_box(e.set_target(group, round + 2, target(price)).unwrap());
        updates.push(start.elapsed().as_nanos());
        let start = Instant::now();
        black_box(e.group_status(group).unwrap());
        views.push(start.elapsed().as_nanos());
        let start = Instant::now();
        let d = e.dispatch_next().unwrap().unwrap();
        plans.push(start.elapsed().as_nanos());
        let start = Instant::now();
        accept(&mut e, d);
        acks.push(start.elapsed().as_nanos());
    }
    println!(
        "100 groups / 1 active child each / 20,000 reprices / memory journal; no network, disk sync or real-time throttling."
    );
    summary("set_target + memory journal", &mut updates);
    summary("group_status + plan explanation", &mut views);
    summary("dispatch_next across 100 groups + commit", &mut plans);
    summary("normalized replace ACK + group/index update", &mut acks);
}
