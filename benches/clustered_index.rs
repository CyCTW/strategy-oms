//! Actual shared Index + Engine paths, synthetic moving-cluster workloads.
//! Raw price unit = one tick in this benchmark only; no venue tick conversion.
pub use strategy_oms::model;
#[allow(dead_code, unused_imports)]
#[path = "../src/index.rs"]
mod index;
#[path = "support/cluster_measure.rs"]
mod measure;
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
use measure::*;
use model::*;
use std::{hint::black_box, sync::atomic::Ordering};

fn book(i: usize) -> Book {
    Book {
        strategy: i as u64,
        account: 1,
        venue: 1,
        instrument: 1,
        side: Side::Buy,
    }
}
fn order(id: usize, b: Book, price: i64) -> Order {
    Order {
        id: id as u64 + 1,
        book: b,
        exchange_id: Some(id as u64 + 1),
        price,
        total_qty: 1_000_000,
        cum_filled: 0,
        leaves: 1_000_000,
        lifecycle: Lifecycle::Working,
        pending: None,
        uncertain: false,
        version: 1,
    }
}
fn backend(name: &str) -> IndexBackend {
    match name {
        "standard" => IndexBackend::Standard,
        "pages" => IndexBackend::PooledPages,
        "adaptive" => IndexBackend::Adaptive,
        _ => unreachable!(),
    }
}
fn engine_backend(name: &str) -> strategy_oms::IndexBackend {
    match name {
        "standard" => strategy_oms::IndexBackend::Standard,
        "pages" => strategy_oms::IndexBackend::PooledPages,
        "adaptive" => strategy_oms::IndexBackend::Adaptive,
        _ => unreachable!(),
    }
}
// A subset retains the exact measured samples; subset classification is outside
// the timing region and never subtracts growth from the overall distribution.
fn subset(dst: &mut Samples, src: &Samples, a: u64, b: u64) {
    dst.times.push(*src.times.last().unwrap());
    dst.allocs += src.allocs - a;
    dst.bytes += src.bytes - b;
}

fn index_run(name: &str, width: usize, mode: &str, n: usize, round: usize, count: bool) {
    let far = mode == "outliers";
    let mut orders: Vec<_> = (0..width)
        .map(|i| order(i, book(0), 62 + i as i64))
        .collect();
    if far {
        orders.push(order(width, book(0), -1_000_000_000));
        orders.push(order(width + 1, book(0), 1_000_000_000));
    }
    let mut members: Vec<_> = (0..orders.len()).map(|_| Memberships::default()).collect();
    let mut index = Index::new(backend(name));
    for i in 0..orders.len() {
        index.update(None, &orders[i], &mut members[i]);
    }
    let h = index.book_handle(book(0)).unwrap();
    let mut query = Samples::new(n);
    let mut range = Samples::new(n);
    let mut submit = Samples::new(n);
    let mut ack = Samples::new(n);
    let mut cross = Samples::new(n);
    let mut new_page = Samples::new(n);
    let mut growth = Samples::new(n);
    let mut far_query = Samples::new(n);
    let mut jump = Samples::new(n);
    // All orders in a batch remain pending until the batch's ACK phase.
    // Consequently old/new clusters coexist; no second pending request per order.
    let mut step = 0;
    while step < n {
        let batch = step / width;
        let center = match mode {
            "boundary" => {
                if batch.is_multiple_of(2) {
                    63
                } else {
                    62
                }
            }
            "jump" => {
                (if (batch / 8).is_multiple_of(2) {
                    0
                } else {
                    1_000_000
                }) + 62
                    + (batch % 8) as i64
            }
            _ => 63 + batch as i64,
        };
        let size = width.min(n - step);
        for i in 0..size {
            let old = orders[i];
            let mut new = old;
            let target = center + i as i64;
            new.pending = Some(Request {
                id: step as u64 + i as u64 + 1,
                order_id: old.id,
                kind: RequestKind::Replace,
                price: target,
                total_qty: old.total_qty,
                state: RequestState::Pending,
            });
            let stats = index.stats();
            let a = submit.allocs;
            let b = submit.bytes;
            submit.measure(count, || {
                index.update(Some(&old), black_box(&new), &mut members[i])
            });
            let after = index.stats();
            if old.price.div_euclid(64) != target.div_euclid(64) {
                subset(&mut cross, &submit, a, b);
            }
            if after.live_pages > stats.live_pages {
                subset(&mut new_page, &submit, a, b);
            }
            if after.page_blocks > stats.page_blocks {
                subset(&mut growth, &submit, a, b);
            }
            if old.price.abs_diff(target) > 10_000 {
                subset(&mut jump, &submit, a, b);
            }
            query.measure(count, || index.summary(black_box(h), black_box(target)));
            range.measure(count, || {
                index
                    .range(Some(black_box(h)), center..=center + width as i64 - 1)
                    .take(8)
                    .map(|(_, p)| p.totals.confirmed_leaves)
                    .sum::<u64>()
            });
            if far {
                far_query.measure(count, || {
                    index.summary(black_box(h), black_box(1_000_000_000))
                });
            }
            orders[i] = new;
        }
        for i in 0..size {
            let old = orders[i];
            let mut new = old;
            new.price = old.pending.unwrap().price;
            new.pending = None;
            ack.measure(count, || {
                index.update(Some(&old), black_box(&new), &mut members[i])
            });
            orders[i] = new;
        }
        step += size;
    }
    let scenario = format!("{mode}_{width}");
    for (s, m) in [
        (query, "price_query"),
        (range, "near_range8"),
        (submit, "index_submit"),
        (ack, "index_ack"),
        (cross, "cross_page_submit_subset"),
        (new_page, "page_creation_subset"),
        (growth, "page_block_growth_subset"),
        (jump, "jump_submit_subset"),
        (far_query, "far_query"),
    ] {
        s.print(count, round, name, &scenario, m);
    }
}

fn footprint(name: &str, books: usize, width: usize, far: bool) {
    let per = width + if far { 2 } else { 0 };
    let orders: Vec<_> = (0..books * per)
        .map(|i| {
            let j = i % per;
            let p = if j < width {
                62 + j as i64
            } else if j == width {
                -1_000_000_000
            } else {
                1_000_000_000
            };
            order(i, book(i / per), p)
        })
        .collect();
    let mut members: Vec<_> = (0..orders.len()).map(|_| Memberships::default()).collect();
    let a = ALLOCS.load(Ordering::Relaxed);
    let b = BYTES.load(Ordering::Relaxed);
    let f = FREED.load(Ordering::Relaxed);
    TRACK.store(true, Ordering::Relaxed);
    let mut index = Index::new(backend(name));
    for i in 0..orders.len() {
        index.update(None, &orders[i], &mut members[i]);
    }
    TRACK.store(false, Ordering::Relaxed);
    let live = BYTES.load(Ordering::Relaxed) - b - (FREED.load(Ordering::Relaxed) - f);
    println!(
        "memory,0,{name},{books}books_{width}near_{}far,resident_index_bytes,1,0,0,0,0,{},{}",
        if far { 2 } else { 0 },
        ALLOCS.load(Ordering::Relaxed) - a,
        live + std::mem::size_of::<Index>() as u64
    );
    let stats = index.stats();
    println!(
        "capacity,0,{name},{books}books_{width}near_{}far,pages_and_blocks,1,0,0,0,0,{},{}",
        if far { 2 } else { 0 },
        stats.live_pages,
        stats.page_blocks
    );
    black_box(index);
}

fn many_books(name: &str, n: usize, round: usize, count: bool) {
    let mut index = Index::new(backend(name));
    let mut members: Vec<_> = (0..4096 * 6).map(|_| Memberships::default()).collect();
    for b in 0..4096 {
        for j in 0..6 {
            let id = b * 6 + j;
            let p = if j < 4 {
                62 + j as i64
            } else if j == 4 {
                -1_000_000_000
            } else {
                1_000_000_000
            };
            index.update(None, &order(id, book(b), p), &mut members[id]);
        }
    }
    let handles: Vec<_> = (0..4096)
        .map(|b| index.book_handle(book(b)).unwrap())
        .collect();
    let mut q = Samples::new(n);
    let mut seed = 11u64;
    for _ in 0..n {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        q.measure(count, || {
            index.summary(
                black_box(handles[(seed >> 32) as usize % handles.len()]),
                black_box(62 + (seed % 4) as i64),
            )
        });
    }
    q.print(count, round, name, "4096books_4near_2far", "price_query");
}

fn page_growth(name: &str, n: usize, round: usize, count: bool) {
    let mut index = Index::new(backend(name));
    let mut members: Vec<_> = (0..n).map(|_| Memberships::default()).collect();
    let mut s = Samples::new(n);
    let mut growth = Samples::new(n);
    // Retain one pending far-price order per new Book: shared pool must grow.
    for (i, m) in members.iter_mut().enumerate() {
        let o = order(i, book(i), 1_000_000_000);
        let before = index.stats().page_blocks;
        let a = s.allocs;
        let b = s.bytes;
        s.measure(count, || index.update(None, black_box(&o), m));
        if index.stats().page_blocks > before {
            subset(&mut growth, &s, a, b);
        }
    }
    s.print(count, round, name, "new_book_growth", "index_insert");
    growth.print(
        count,
        round,
        name,
        "new_book_growth",
        "page_block_growth_subset",
    );
}

fn engine_run(name: &str, width: usize, mode: &str, n: usize, round: usize, count: bool) {
    use strategy_oms::{Engine, Limits, journal::MemoryJournal};
    let limits = Limits {
        max_orders: width + 4,
        max_requests: 2 * n + width + 16,
        max_reports: 2 * n + width + 16,
        max_executions: 1,
        ..Limits::default()
    };
    let mut e = Engine::new_with_index(
        MemoryJournal::new(6 * n + width * 2 + 16),
        limits,
        engine_backend(name),
    )
    .unwrap();
    let mut seq = 0;
    for i in 0..width + 2 {
        let p = if i < width {
            62 + i as i64
        } else if i == width {
            -1_000_000_000
        } else {
            1_000_000_000
        };
        let id = i as u64 + 1;
        e.apply(Event::New(NewOrder {
            order_id: id,
            request_id: id,
            book: book(0),
            price: p,
            total_qty: 100,
        }))
        .unwrap();
        seq += 1;
        e.on_report(Report {
            source: 1,
            sequence: seq,
            order_id: id,
            kind: ReportKind::Accepted {
                request_id: id,
                exchange_id: id,
                price: p,
                total_qty: 100,
            },
        })
        .unwrap();
    }
    let h = e.book_handle(book(0)).unwrap();
    let mut q = Samples::new(n);
    let mut immediate = Samples::new(n);
    let mut queued = Samples::new(n);
    let mut reports = Samples::new(n);
    let mut ready = Samples::new(n);
    let mut jumps = Samples::new(n);
    for step in 0..n {
        let id = (step % width) as u64 + 1;
        let generation = step / width;
        let center = if mode == "jump" && !(generation / 8).is_multiple_of(2) {
            1_000_000
        } else {
            0
        };
        let target = center + 66 + (generation as i64) * 4 + (step % width) as i64;
        let old = *e.order(id).unwrap();
        let req = (width + 3 + step * 2) as u64;
        q.measure(count, || {
            e.level_summary(black_box(h), black_box(old.price))
        });
        let action = Event::Replace {
            order_id: id,
            request_id: req,
            expected_version: old.version,
            price: target,
            total_qty: 100,
        };
        let a = immediate.allocs;
        let b = immediate.bytes;
        assert!(
            immediate
                .measure(count, || e.apply(black_box(action)).unwrap())
                .outbound
                .is_some()
        );
        if old.price.abs_diff(target) > 10_000 {
            subset(&mut jumps, &immediate, a, b);
        }
        let action = Event::Replace {
            order_id: id,
            request_id: req + 1,
            expected_version: e.order(id).unwrap().version,
            price: target + 1,
            total_qty: 100,
        };
        assert!(
            queued
                .measure(count, || e.apply(black_box(action)).unwrap())
                .outbound
                .is_none()
        );
        seq += 1;
        let r = Report {
            source: 1,
            sequence: seq,
            order_id: id,
            kind: ReportKind::Replaced {
                request_id: req,
                exchange_id: id,
                price: target,
                total_qty: 100,
            },
        };
        reports.measure(count, || e.on_report(black_box(r)).unwrap());
        assert!(
            ready
                .measure(count, || e.dispatch_next_order().unwrap())
                .is_some()
        );
        seq += 1;
        e.on_report(Report {
            source: 1,
            sequence: seq,
            order_id: id,
            kind: ReportKind::Replaced {
                request_id: req + 1,
                exchange_id: id,
                price: target + 1,
                total_qty: 100,
            },
        })
        .unwrap();
    }
    let scenario = format!("engine_{mode}_{width}_2far");
    for (s, m) in [
        (q, "price_query"),
        (immediate, "intent_to_command"),
        (queued, "intent_queued"),
        (reports, "replace_report"),
        (ready, "ready_to_command"),
        (jumps, "jump_intent_subset"),
    ] {
        s.print(count, round, name, &scenario, m);
    }
}

fn cancel_rehang(name: &str, n: usize, round: usize, count: bool) {
    use strategy_oms::{Engine, Limits, journal::MemoryJournal};
    let limits = Limits {
        max_orders: n + 8,
        max_requests: 2 * n + 16,
        max_reports: 2 * n + 16,
        max_executions: 1,
        ..Limits::default()
    };
    let mut e =
        Engine::new_with_index(MemoryJournal::new(4 * n + 32), limits, engine_backend(name))
            .unwrap();
    let mut seq = 0;
    for id in 1..=5 {
        let p = 62 + id as i64;
        e.apply(Event::New(NewOrder {
            order_id: id,
            request_id: id,
            book: book(0),
            price: p,
            total_qty: 100,
        }))
        .unwrap();
        seq += 1;
        e.on_report(Report {
            source: 1,
            sequence: seq,
            order_id: id,
            kind: ReportKind::Accepted {
                request_id: id,
                exchange_id: id,
                price: p,
                total_qty: 100,
            },
        })
        .unwrap();
    }
    let mut id = 5;
    let mut cancel = Samples::new(n);
    let mut new = Samples::new(n);
    let mut accepted = Samples::new(n);
    let mut canceled = Samples::new(n);
    let h = e.book_handle(book(0)).unwrap();
    for step in 0..n {
        let req = 6 + step as u64 * 2;
        let next = 6 + step as u64;
        let price = if step.is_multiple_of(2) {
            -1_000_000_000
        } else {
            68 + (step % 32) as i64
        };
        let action = Event::Cancel {
            order_id: id,
            request_id: req,
            expected_version: e.order(id).unwrap().version,
        };
        assert!(
            cancel
                .measure(count, || e.apply(black_box(action)).unwrap())
                .outbound
                .is_some()
        );
        let action = Event::New(NewOrder {
            order_id: next,
            request_id: req + 1,
            book: book(0),
            price,
            total_qty: 100,
        });
        assert!(
            new.measure(count, || e.apply(black_box(action)).unwrap())
                .outbound
                .is_some()
        );
        // New ACK precedes old Cancel ACK: distinct physical orders coexist.
        // Risk reservation accounts for both; no pending state is silently erased.
        seq += 1;
        let r = Report {
            source: 1,
            sequence: seq,
            order_id: next,
            kind: ReportKind::Accepted {
                request_id: req + 1,
                exchange_id: next,
                price,
                total_qty: 100,
            },
        };
        accepted.measure(count, || e.on_report(black_box(r)).unwrap());
        assert_eq!(e.reserved_qty(book(0)), 600);
        seq += 1;
        let r = Report {
            source: 1,
            sequence: seq,
            order_id: id,
            kind: ReportKind::Canceled {
                request_id: Some(req),
            },
        };
        canceled.measure(count, || e.on_report(black_box(r)).unwrap());
        assert_eq!(e.reserved_qty(book(0)), 500);
        black_box(e.level_summary(h, price).unwrap());
        id = next;
    }
    for (s, m) in [
        (cancel, "cancel_to_command"),
        (new, "new_to_command"),
        (accepted, "accepted_report"),
        (canceled, "canceled_report"),
    ] {
        s.print(count, round, name, "engine_cancel_rehang_4near", m);
    }
}

fn run(name: &str, n: usize, round: usize, count: bool) {
    for width in [4, 32, 128] {
        for mode in ["rolling", "outliers", "boundary", "jump"] {
            index_run(name, width, mode, n, round, count);
        }
    }
    many_books(name, n, round, count);
    page_growth(name, n, round, count);
    cancel_rehang(name, n, round, count);
    for width in [4, 32] {
        for mode in ["rolling", "jump"] {
            engine_run(name, width, mode, n, round, count);
        }
    }
    if count {
        for books in [1, 128] {
            for width in [4, 32, 128] {
                for far in [false, true] {
                    footprint(name, books, width, far);
                }
            }
        }
    }
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
    println!(
        "pass,round,backend,scenario,metric,n,p50_ns,p99_ns,p999_ns,max_ns,allocations,allocated_bytes"
    );
    for round in 0..rounds {
        for name in if round % 2 == 0 {
            ["standard", "pages", "adaptive"]
        } else {
            ["adaptive", "pages", "standard"]
        } {
            run(name, n, round, false);
        }
    }
    for name in ["standard", "pages", "adaptive"] {
        run(name, n, 0, true);
    }
    let mut timer = Samples::new(n);
    for _ in 0..n {
        timer.measure(false, || black_box(()));
    }
    timer.print(false, 0, "timer", "baseline", "timer");
}
