//! Reproducible CPU service-time comparison, NOT an end-to-end production SLO.
//! Timing and allocation-count passes are separate. Block growth is included.
use model::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};
pub use strategy_oms::model;
#[allow(dead_code, unused_imports)]
#[path = "../src/index.rs"]
mod index;
#[allow(dead_code, unused_imports)]
#[path = "support/legacy_index.rs"]
mod legacy;
#[allow(dead_code, unused_imports)]
#[path = "../src/pool.rs"]
mod pool;
#[allow(dead_code, unused_imports)]
#[path = "../src/price_deque.rs"]
mod price_deque;
#[allow(dead_code, unused_imports)]
#[path = "../src/price_pages.rs"]
mod price_pages;
#[allow(dead_code, unused_imports)]
#[path = "../src/price_tree.rs"]
mod price_tree;
use index::{Index, IndexBackend, Memberships};

struct CountingAllocator;
static TRACK: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static FREES: AtomicU64 = AtomicU64::new(0);
// Instrumentation only. Production pool/index implementations contain no unsafe.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACK.load(Ordering::Relaxed) {
            FREES.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            FREES.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct Samples {
    values: Vec<u64>,
    allocations: u64,
    frees: u64,
}
impl Samples {
    fn new(n: usize) -> Self {
        Self {
            values: Vec::with_capacity(n),
            allocations: 0,
            frees: 0,
        }
    }
    fn measure<T>(&mut self, counting: bool, operation: impl FnOnce() -> T) -> T {
        if counting {
            let a = ALLOCS.load(Ordering::Relaxed);
            let f = FREES.load(Ordering::Relaxed);
            TRACK.store(true, Ordering::Relaxed);
            let result = black_box(operation());
            TRACK.store(false, Ordering::Relaxed);
            self.allocations += ALLOCS.load(Ordering::Relaxed) - a;
            self.frees += FREES.load(Ordering::Relaxed) - f;
            self.values.push(0);
            result
        } else {
            let start = Instant::now();
            let result = black_box(operation());
            self.values.push(start.elapsed().as_nanos() as u64);
            result
        }
    }
    fn print(mut self, round: usize, backend: &str, scenario: &str, metric: &str, counting: bool) {
        if self.values.is_empty() {
            return;
        }
        self.values.sort_unstable();
        let q = |p: usize| self.values[(self.values.len() - 1) * p / 1000];
        println!(
            "{},{round},{backend},{scenario},{metric},{},{},{},{},{},{},{}",
            if counting { "alloc" } else { "time" },
            self.values.len(),
            q(500),
            q(990),
            q(999),
            self.values.last().unwrap(),
            self.allocations,
            self.frees
        );
    }
}
fn book(i: usize) -> Book {
    Book {
        strategy: i as u64,
        account: 1,
        venue: 1,
        instrument: 1,
        side: Side::Buy,
    }
}
fn order(i: usize, books: usize, levels: usize) -> Order {
    Order {
        id: i as u64 + 1,
        book: book(i % books),
        exchange_id: Some(i as u64 + 1),
        price: ((i / books) % levels) as i64 * 1_000_003 - 1_000_000_000,
        total_qty: 1_000_000,
        cum_filled: 0,
        leaves: 1_000_000,
        lifecycle: Lifecycle::Working,
        pending: None,
        uncertain: false,
        version: 1,
    }
}

// Keep both candidates inline so the harness adds no backend-specific box.
#[allow(clippy::large_enum_variant)]
enum Harness {
    Legacy(legacy::Index),
    New(Index),
}
impl Harness {
    fn new(name: &str) -> Self {
        match name {
            "legacy" => Self::Legacy(legacy::Index::default()),
            "standard" => Self::New(Index::new(IndexBackend::Standard)),
            "pooled" => Self::New(Index::new(IndexBackend::PooledAvl)),
            "adaptive" => Self::New(Index::new(IndexBackend::Adaptive)),
            _ => unreachable!(),
        }
    }
    fn update(&mut self, old: Option<&Order>, new: &Order, membership: &mut Memberships) {
        match self {
            Self::Legacy(i) => i.update(old, new),
            Self::New(i) => i.update(old, new, membership),
        }
    }
    fn query(&self, b: Book, p: Price) -> Option<u64> {
        match self {
            Self::Legacy(i) => Some(i.books.get(&b)?.levels.get(&p)?.totals.confirmed_leaves),
            Self::New(i) => Some(i.summary(i.book_handle(b)?, p)?.totals.confirmed_leaves),
        }
    }
    fn best(&self, b: Book) -> Option<Price> {
        match self {
            Self::Legacy(i) => i.books.get(&b)?.confirmed_prices.last().copied(),
            Self::New(i) => i.best(i.book_handle(b)?),
        }
    }
    fn blocks(&self) -> usize {
        match self {
            Self::Legacy(_) => 0,
            Self::New(i) => {
                let s = i.stats();
                s.level_blocks + s.member_blocks + s.tree_blocks + s.book_blocks + s.page_blocks
            }
        }
    }
    fn range(&self, b: Book) -> u64 {
        match self {
            Self::Legacy(i) => i.books[&b]
                .levels
                .range(i64::MIN..=i64::MAX)
                .take(8)
                .map(|(_, l)| l.totals.confirmed_leaves)
                .sum(),
            Self::New(i) => i
                .range(i.book_handle(b), i64::MIN..=i64::MAX)
                .take(8)
                .map(|(_, l)| l.totals.confirmed_leaves)
                .sum(),
        }
    }
}
fn index_run(round: usize, name: &str, scenario: &str, n: usize, counting: bool) {
    let (orders, books, levels) = match scenario {
        "small" => (4, 1, 4),
        "crowded" => (4096, 1, 1),
        "many_prices" => (4096, 1, 4096),
        "many_books" => (1024, 128, 8),
        "churn" => (128, 1, 128),
        "growth" => (n, 1, n),
        _ => unreachable!(),
    };
    let mut h = Harness::new(name);
    let mut states: Vec<_> = (0..orders).map(|i| order(i, books, levels)).collect();
    let mut memberships: Vec<_> = (0..orders).map(|_| Memberships::default()).collect();
    let growing = scenario == "growth";
    if !growing {
        for i in 0..orders {
            h.update(None, &states[i], &mut memberships[i]);
        }
    }
    let handles: Vec<_> = match &h {
        Harness::New(i) if !growing => (0..books)
            .map(|b| i.book_handle(book(b)).unwrap())
            .collect(),
        _ => Vec::new(),
    };
    if !growing {
        for i in 0..4096 {
            black_box(h.query(states[i % orders].book, states[i % orders].price));
        }
    }
    let mut query = Samples::new(n);
    let mut miss = Samples::new(n);
    let mut best = Samples::new(n);
    let mut range = Samples::new(n);
    let mut handle_query = Samples::new(n);
    let mut updates = Samples::new(n);
    let mut growth = Samples::new(n);
    let mut seed = 77u64;
    for step in 0..n {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = if growing {
            step
        } else {
            (seed >> 32) as usize % orders
        };
        let old = states[j];
        let mut new = old;
        if !growing {
            query.measure(counting, || {
                h.query(black_box(old.book), black_box(old.price))
            });
            miss.measure(counting, || {
                h.query(black_box(old.book), black_box(i64::MAX))
            });
            best.measure(counting, || h.best(black_box(old.book)));
            range.measure(counting, || h.range(black_box(old.book)));
            if let Harness::New(i) = &h {
                handle_query.measure(counting, || {
                    i.summary(black_box(handles[j % books]), black_box(old.price))
                });
            }
            if scenario == "churn" {
                new.price = (step as i64 + 10_000) * 1_000_003;
            } else {
                new.cum_filled += 1;
                new.leaves -= 1;
            }
        }
        let blocks = h.blocks();
        let a = updates.allocations;
        let f = updates.frees;
        updates.measure(counting, || {
            h.update(
                if growing { None } else { Some(&old) },
                black_box(&new),
                &mut memberships[j],
            )
        });
        if growing && h.blocks() > blocks {
            growth.values.push(*updates.values.last().unwrap());
            growth.allocations += updates.allocations - a;
            growth.frees += updates.frees - f;
        }
        states[j] = new;
    }
    query.print(round, name, scenario, "price_hit", counting);
    miss.print(round, name, scenario, "price_miss", counting);
    best.print(round, name, scenario, "best", counting);
    range.print(round, name, scenario, "range_first8", counting);
    handle_query.print(round, name, scenario, "handle_hit", counting);
    updates.print(round, name, scenario, "index_update", counting);
    growth.print(round, name, scenario, "block_growth_subset", counting);
}

fn engine_backend(name: &str) -> strategy_oms::IndexBackend {
    match name {
        "pooled" => strategy_oms::IndexBackend::PooledAvl,
        "adaptive" => strategy_oms::IndexBackend::Adaptive,
        _ => strategy_oms::IndexBackend::Standard,
    }
}

fn engine_run(round: usize, name: &str, n: usize, counting: bool) {
    use strategy_oms::{Engine, Limits, journal::MemoryJournal};
    let backend = engine_backend(name);
    let limits = Limits {
        max_orders: 16,
        max_requests: 2 * n + 16,
        max_reports: 3 * n + 16,
        max_executions: n + 16,
        ..Limits::default()
    };
    let mut e = Engine::new_with_index(MemoryJournal::new(n * 6 + 16), limits, backend).unwrap();
    e.apply(Event::New(NewOrder {
        order_id: 1,
        request_id: 1,
        book: book(0),
        price: 100,
        total_qty: 1_000_000,
    }))
    .unwrap();
    e.on_report(Report {
        source: 1,
        sequence: 1,
        order_id: 1,
        kind: ReportKind::Accepted {
            request_id: 1,
            exchange_id: 1,
            price: 100,
            total_qty: 1_000_000,
        },
    })
    .unwrap();
    let mut immediate = Samples::new(n);
    let mut deferred = Samples::new(n);
    let mut ready = Samples::new(n);
    let mut reports = Samples::new(n);
    let mut seq = 1;
    for step in 0..n {
        let req = 2 + step as u64 * 2;
        let price = 101 + (step % 2) as i64;
        let action = Event::Replace {
            order_id: 1,
            request_id: req,
            expected_version: e.order(1).unwrap().version,
            price,
            total_qty: 1_000_000,
        };
        assert!(
            immediate
                .measure(counting, || e.apply(action).unwrap())
                .outbound
                .is_some()
        );
        let action = Event::Replace {
            order_id: 1,
            request_id: req + 1,
            expected_version: e.order(1).unwrap().version,
            price: price + 10,
            total_qty: 1_000_000,
        };
        assert!(
            deferred
                .measure(counting, || e.apply(action).unwrap())
                .outbound
                .is_none()
        );
        seq += 1;
        reports.measure(counting, || {
            e.on_report(Report {
                source: 1,
                sequence: seq,
                order_id: 1,
                kind: ReportKind::Replaced {
                    request_id: req,
                    exchange_id: 1,
                    price,
                    total_qty: 1_000_000,
                },
            })
            .unwrap()
        });
        assert!(
            ready
                .measure(counting, || e.dispatch_next_order().unwrap())
                .is_some()
        );
        seq += 1;
        e.on_report(Report {
            source: 1,
            sequence: seq,
            order_id: 1,
            kind: ReportKind::Replaced {
                request_id: req + 1,
                exchange_id: 1,
                price: price + 10,
                total_qty: 1_000_000,
            },
        })
        .unwrap();
    }
    immediate.print(round, name, "engine", "intent_immediate", counting);
    deferred.print(round, name, "engine", "intent_queued", counting);
    ready.print(round, name, "engine", "ready_to_command", counting);
    reports.print(round, name, "engine", "replace_report", counting);
}

fn engine_growth(round: usize, name: &str, n: usize, counting: bool) {
    use strategy_oms::{Engine, Limits, journal::MemoryJournal};
    let backend = engine_backend(name);
    let limits = Limits {
        max_orders: n + 1,
        max_requests: n + 1,
        max_reports: 1,
        max_executions: 1,
        ..Limits::default()
    };
    let mut e = Engine::new_with_index(MemoryJournal::new(n + 1), limits, backend).unwrap();
    let mut samples = Samples::new(n);
    for i in 0..n {
        let action = Event::New(NewOrder {
            order_id: i as u64 + 1,
            request_id: i as u64 + 1,
            book: book(0),
            price: (i as i64) * 1_000_003,
            total_qty: 1,
        });
        assert!(
            samples
                .measure(counting, || e.apply(action).unwrap())
                .outbound
                .is_some()
        );
    }
    samples.print(round, name, "engine_growth", "new_to_command", counting);
}

// One event-loop replay with fixed, preassigned arrival times. This accounts for
// backlog relative to the schedule, but does not model a transport or queue cost.
fn burst_run(round: usize, name: &str, n: usize) {
    use strategy_oms::{Engine, Limits, journal::MemoryJournal};
    let backend = engine_backend(name);
    let limits = Limits {
        max_orders: 1,
        max_requests: 1,
        max_reports: n + 8,
        max_executions: n + 8,
        ..Limits::default()
    };
    let mut e = Engine::new_with_index(MemoryJournal::new(n + 8), limits, backend).unwrap();
    e.apply(Event::New(NewOrder {
        order_id: 1,
        request_id: 1,
        book: book(0),
        price: 100,
        total_qty: (n + 1) as u64,
    }))
    .unwrap();
    e.on_report(Report {
        source: 1,
        sequence: 1,
        order_id: 1,
        kind: ReportKind::Accepted {
            request_id: 1,
            exchange_id: 1,
            price: 100,
            total_qty: (n + 1) as u64,
        },
    })
    .unwrap();
    let reports: Vec<_> = (0..n)
        .map(|i| Report {
            source: 1,
            sequence: i as u64 + 2,
            order_id: 1,
            kind: ReportKind::Fill {
                key: ExecutionKey {
                    venue: 1,
                    account: 1,
                    trading_day: 1,
                    execution_id: i as u64 + 1,
                },
                qty: 1,
                price: 100,
            },
        })
        .collect();
    let size: usize = std::env::var("OMS_BURST_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    let period: u64 = std::env::var("OMS_BURST_PERIOD_NS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);
    assert!(size > 0 && period > 0);
    let scenario = format!("burst{size}_period{period}ns");
    let mut service = Samples::new(n);
    let mut completion = Samples::new(n);
    let epoch = Instant::now();
    for (i, r) in reports.into_iter().enumerate() {
        let arrival = (i / size) as u128 * period as u128;
        while epoch.elapsed().as_nanos() < arrival {
            std::hint::spin_loop();
        }
        service.measure(false, || e.on_report(r).unwrap());
        completion
            .values
            .push(epoch.elapsed().as_nanos().saturating_sub(arrival) as u64);
    }
    service.print(round, name, &scenario, "fill_service", false);
    completion.print(
        round,
        name,
        &scenario,
        "scheduled_arrival_to_visible",
        false,
    );
}

fn main() {
    let n = std::env::var("OMS_BENCH_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20_000);
    let rounds = std::env::var("OMS_BENCH_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    assert!(n > 0 && rounds > 0);
    println!("pass,round,backend,scenario,metric,n,p50_ns,p99_ns,p999_ns,max_ns,allocations,frees");
    for round in 0..rounds {
        let names = if round % 2 == 0 {
            ["legacy", "standard", "pooled", "adaptive"]
        } else {
            ["adaptive", "pooled", "standard", "legacy"]
        };
        for name in names {
            for scenario in [
                "small",
                "crowded",
                "many_prices",
                "many_books",
                "churn",
                "growth",
            ] {
                index_run(round, name, scenario, n, false);
            }
            if name != "legacy" {
                engine_run(round, name, n, false);
                engine_growth(round, name, n, false);
                burst_run(round, name, n);
            }
        }
    }
    for name in ["legacy", "standard", "pooled", "adaptive"] {
        for scenario in [
            "small",
            "crowded",
            "many_prices",
            "many_books",
            "churn",
            "growth",
        ] {
            index_run(0, name, scenario, n, true);
        }
        if name != "legacy" {
            engine_run(0, name, n, true);
            engine_growth(0, name, n, true);
        }
    }
    let mut timer = Samples::new(n);
    for _ in 0..n {
        timer.measure(false, || black_box(()));
    }
    timer.print(0, "timer", "baseline", "timer", false);
}
