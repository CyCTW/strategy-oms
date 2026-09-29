//! Index-level batch benchmark: the real `Index::update` / `summary` paths
//! (pools, memberships, totals, best cache, locator), timed per batch of 256
//! operations and reported as per-operation nanoseconds. Batching amortizes
//! timer overhead and single interrupts, so differences of a few ns between
//! locators are visible; tail latency stays with `clustered_index`.
pub use strategy_oms::model;
#[allow(dead_code, unused_imports)]
#[path = "../src/index.rs"]
mod index;
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
#[path = "../src/price_slide.rs"]
mod price_slide;
#[allow(dead_code, unused_imports)]
#[path = "../src/price_tree.rs"]
mod price_tree;
use index::{Index, IndexBackend, Memberships};
use model::*;
use std::{hint::black_box, time::Instant};

const BATCH: usize = 256;
const BATCHES: usize = 160;

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

/// Runs `op` BATCHES * BATCH times; per-op ns of each batch -> (median, p90).
fn measure(mut op: impl FnMut()) -> (f64, f64) {
    let mut per_op = Vec::with_capacity(BATCHES);
    for _ in 0..BATCHES {
        let start = Instant::now();
        for _ in 0..BATCH {
            op();
        }
        per_op.push(start.elapsed().as_nanos() as f64 / BATCH as f64);
    }
    per_op.sort_by(f64::total_cmp);
    (per_op[BATCHES / 2], per_op[BATCHES * 9 / 10])
}

/// Moving near-market cluster, as in clustered_index: every order replaces
/// to `center + i` (pending), then ACKs; the center moves per mode.
fn cluster(backend: IndexBackend, width: usize, mode: &str) -> [(f64, f64); 2] {
    let far = mode == "outliers";
    let mut orders: Vec<_> = (0..width)
        .map(|i| order(i, book(0), 62 + i as i64))
        .collect();
    if far {
        orders.push(order(width, book(0), -1_000_000_000));
        orders.push(order(width + 1, book(0), 1_000_000_000));
    }
    let mut members: Vec<_> = (0..orders.len()).map(|_| Memberships::default()).collect();
    let mut index = Index::new(backend);
    for i in 0..orders.len() {
        index.update(None, &orders[i], &mut members[i]);
    }
    let h = index.book_handle(book(0)).unwrap();
    // Update stream: submit then ACK per order, center advancing per sweep.
    let mut step = 0usize;
    let updates = measure(|| {
        let sweep = step / (2 * width);
        let i = (step / 2) % width;
        let center = match mode {
            "boundary" => 62 + (sweep % 2) as i64,
            "jump" => {
                (if (sweep / 8).is_multiple_of(2) {
                    0
                } else {
                    1_000_000
                }) + 62
                    + (sweep % 8) as i64
            }
            _ => 63 + sweep as i64,
        };
        let old = orders[i];
        let mut new = old;
        if step.is_multiple_of(2) {
            new.pending = Some(Request {
                id: step as u64 + 1,
                order_id: old.id,
                kind: RequestKind::Replace,
                price: center + i as i64,
                total_qty: old.total_qty,
                state: RequestState::Pending,
            });
        } else {
            new.price = old.pending.unwrap().price;
            new.pending = None;
        }
        index.update(Some(&old), black_box(&new), &mut members[i]);
        orders[i] = new;
        step += 1;
    });
    let mut s = 7u64;
    let queries = measure(|| {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        let p = orders[(s >> 33) as usize % width].price;
        black_box(index.summary(black_box(h), p));
    });
    [updates, queries]
}

/// 4096 Books with 4 near + 2 far prices each: random price queries, and a
/// same-book reprice (old level leaves, new level created) per operation.
fn many_books(backend: IndexBackend) -> [(f64, f64); 2] {
    let mut index = Index::new(backend);
    let mut orders = Vec::with_capacity(4096 * 6);
    let mut members: Vec<_> = (0..4096 * 6).map(|_| Memberships::default()).collect();
    for b in 0..4096 {
        for j in 0..6 {
            let id = b * 6 + j;
            let p = match j {
                0..4 => 62 + j as i64,
                4 => -1_000_000_000,
                _ => 1_000_000_000,
            };
            let o = order(id, book(b), p);
            index.update(None, &o, &mut members[id]);
            orders.push(o);
        }
    }
    let handles: Vec<_> = (0..4096)
        .map(|b| index.book_handle(book(b)).unwrap())
        .collect();
    let mut s = 11u64;
    let queries = measure(|| {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        black_box(index.summary(
            black_box(handles[(s >> 32) as usize % 4096]),
            62 + (s % 4) as i64,
        ));
    });
    let updates = measure(|| {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        let id = ((s >> 32) as usize % 4096) * 6 + (s % 4) as usize;
        let old = orders[id];
        let mut new = old;
        // Move within the near band: 62..=69 keeps prices dense.
        new.price = 62 + ((old.price - 62 + 1 + (s % 3) as i64) % 8);
        index.update(Some(&old), black_box(&new), &mut members[id]);
        orders[id] = new;
    });
    [queries, updates]
}

/// A new Book per operation with its first order (Book registration and the
/// locator's first allocation included).
fn new_books(backend: IndexBackend) -> (f64, f64) {
    let mut index = Index::new(backend);
    let mut members: Vec<_> = (0..BATCH * BATCHES)
        .map(|_| Memberships::default())
        .collect();
    let mut i = 0;
    measure(|| {
        index.update(None, &order(i, book(i), 62), &mut members[i]);
        i += 1;
    })
}

fn main() {
    let rounds: usize = std::env::var("OMS_BENCH_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    let backends: Vec<(&str, IndexBackend)> = vec![
        ("standard", IndexBackend::Standard),
        ("pages", IndexBackend::PooledPages),
        ("adaptive", IndexBackend::Adaptive),
        ("window", IndexBackend::SlidingWindow),
    ];
    println!("round,backend,scenario,metric,per_op_p50_ns,per_op_p90_ns");
    for round in 0..rounds {
        let mut order: Vec<usize> = (0..backends.len()).collect();
        order.rotate_left(round % backends.len());
        if (round / backends.len()) % 2 == 1 {
            order.reverse();
        }
        for b in order {
            let (name, backend) = backends[b];
            let emit = |scenario: &str, metric: &str, (p50, p90): (f64, f64)| {
                println!("{round},{name},{scenario},{metric},{p50:.2},{p90:.2}");
            };
            for width in [4, 16, 32, 128] {
                for mode in ["rolling", "outliers", "jump"] {
                    let [u, q] = cluster(backend, width, mode);
                    emit(&format!("{mode}_{width}"), "update", u);
                    emit(&format!("{mode}_{width}"), "query", q);
                }
            }
            let [q, u] = many_books(backend);
            emit("4096books", "query", q);
            emit("4096books", "update", u);
            emit("new_books", "first_insert", new_books(backend));
        }
    }
}
