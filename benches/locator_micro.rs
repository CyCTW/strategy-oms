//! Locator-only micro benchmark for the production price locators.
//!
//! Times batches of operations and reports per-operation nanoseconds (batch
//! time / batch size), so timer overhead and single interrupts are amortized:
//! this compares the data structures themselves, not Engine or Index latency.
//! For tail latency use `index_comparison` / `clustered_index`.
pub use strategy_oms::model;
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

use model::Price;
use pool::{Handle, Pool};
use price_deque::{AdaptiveLocator, Entry, FlatDeque};
use price_pages::{PageLocator, PagePool};
use price_slide::SlideLocator;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    hint::black_box,
    time::Instant,
};

/// The operations the Index performs on a locator.
trait Loc: Default {
    fn get(&self, p: Price) -> Option<Handle>;
    fn find_or_insert(&mut self, p: Price, h: Handle, w: bool) -> (Handle, bool);
    fn remove(&mut self, p: Price);
    fn set_working(&mut self, p: Price, w: bool);
    fn best(&self, buy: bool) -> Option<Price>;
    fn range_sum(&self, lo: Price, hi: Price, take: usize) -> usize;
}

#[derive(Default)]
struct Btree {
    levels: BTreeMap<Price, Handle>,
    confirmed: BTreeSet<Price>,
}
impl Loc for Btree {
    fn get(&self, p: Price) -> Option<Handle> {
        self.levels.get(&p).copied()
    }
    fn find_or_insert(&mut self, p: Price, h: Handle, w: bool) -> (Handle, bool) {
        match self.levels.entry(p) {
            std::collections::btree_map::Entry::Occupied(o) => (*o.get(), false),
            std::collections::btree_map::Entry::Vacant(v) => {
                v.insert(h);
                if w {
                    self.confirmed.insert(p);
                }
                (h, true)
            }
        }
    }
    fn remove(&mut self, p: Price) {
        self.levels.remove(&p);
        self.confirmed.remove(&p);
    }
    fn set_working(&mut self, p: Price, w: bool) {
        if w {
            self.confirmed.insert(p);
        } else {
            self.confirmed.remove(&p);
        }
    }
    fn best(&self, buy: bool) -> Option<Price> {
        if buy {
            self.confirmed.last().copied()
        } else {
            self.confirmed.first().copied()
        }
    }
    fn range_sum(&self, lo: Price, hi: Price, take: usize) -> usize {
        self.levels.range(lo..=hi).take(take).count()
    }
}

/// Sparse 64-slot pages (IndexBackend::PooledPages); the pool is per Book here.
#[derive(Default)]
struct Pages {
    pool: PagePool,
    loc: PageLocator,
}
impl Loc for Pages {
    fn get(&self, p: Price) -> Option<Handle> {
        self.loc.get(&self.pool, p)
    }
    fn find_or_insert(&mut self, p: Price, h: Handle, w: bool) -> (Handle, bool) {
        match self.loc.get(&self.pool, p) {
            Some(x) => (x, false),
            None => {
                self.loc.insert(&mut self.pool, p, h, w);
                (h, true)
            }
        }
    }
    fn remove(&mut self, p: Price) {
        self.loc.remove(&mut self.pool, p)
    }
    fn set_working(&mut self, p: Price, w: bool) {
        self.loc.set_working(&mut self.pool, p, w)
    }
    fn best(&self, buy: bool) -> Option<Price> {
        self.loc.best(&self.pool, buy)
    }
    fn range_sum(&self, lo: Price, hi: Price, take: usize) -> usize {
        self.loc.range(&self.pool, lo, hi).take(take).count()
    }
}

macro_rules! adaptive_loc {
    ($t:ty) => {
        impl Loc for $t {
            fn get(&self, p: Price) -> Option<Handle> {
                AdaptiveLocator::get(self, p)
            }
            fn find_or_insert(&mut self, p: Price, h: Handle, w: bool) -> (Handle, bool) {
                AdaptiveLocator::find_or_insert(self, p, || h, w)
            }
            fn remove(&mut self, p: Price) {
                AdaptiveLocator::remove(self, p)
            }
            fn set_working(&mut self, p: Price, w: bool) {
                AdaptiveLocator::set_working(self, p, w)
            }
            fn best(&self, buy: bool) -> Option<Price> {
                AdaptiveLocator::best(self, buy)
            }
            fn range_sum(&self, lo: Price, hi: Price, take: usize) -> usize {
                AdaptiveLocator::range(self, lo, hi).take(take).count()
            }
        }
    };
}
adaptive_loc!(AdaptiveLocator<VecDeque<Entry>>);
adaptive_loc!(AdaptiveLocator<FlatDeque>);

macro_rules! window_loc {
    ($t:ty) => {
        impl Loc for $t {
            fn get(&self, p: Price) -> Option<Handle> {
                <$t>::get(self, p)
            }
            fn find_or_insert(&mut self, p: Price, h: Handle, w: bool) -> (Handle, bool) {
                <$t>::find_or_insert(self, p, || h, w)
            }
            fn remove(&mut self, p: Price) {
                <$t>::remove(self, p)
            }
            fn set_working(&mut self, p: Price, w: bool) {
                <$t>::set_working(self, p, w)
            }
            fn best(&self, buy: bool) -> Option<Price> {
                <$t>::best(self, buy)
            }
            fn range_sum(&self, lo: Price, hi: Price, take: usize) -> usize {
                <$t>::range(self, lo, hi).take(take).count()
            }
        }
    };
}
window_loc!(SlideLocator<u64, 64>);
window_loc!(SlideLocator<u128, 128>);

fn rng(s: &mut u64) -> u64 {
    *s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
    *s >> 33
}

const BATCH: usize = 256;
const BATCHES: usize = 200;

/// Per-op ns for each batch; returns (median, p90).
fn measure(mut op: impl FnMut(usize)) -> (f64, f64) {
    let mut per_op = Vec::with_capacity(BATCHES);
    let mut i = 0;
    for _ in 0..BATCHES {
        let start = Instant::now();
        for _ in 0..BATCH {
            op(i);
            i += 1;
        }
        per_op.push(start.elapsed().as_nanos() as f64 / BATCH as f64);
    }
    per_op.sort_by(f64::total_cmp);
    (per_op[BATCHES / 2], per_op[BATCHES * 9 / 10])
}

struct Ctx {
    handles: Vec<Handle>,
}

/// Book with `w` consecutive ticks starting at 1_000_000, all working, plus two
/// far prices (a near-market cluster with outliers).
fn book<L: Loc>(ctx: &Ctx, w: usize) -> (L, Vec<Price>) {
    let mut l = L::default();
    let mut prices: Vec<Price> = (0..w as Price).map(|i| 1_000_000 + i).collect();
    prices.push(1);
    prices.push(2_000_000_000);
    for (i, &p) in prices.iter().enumerate() {
        l.find_or_insert(p, ctx.handles[i % ctx.handles.len()], true);
    }
    (l, prices)
}

fn run<L: Loc>(name: &str, round: usize, ctx: &Ctx) {
    for w in [4usize, 16, 32, 128, 512] {
        let scenario = format!("w{w}");
        let emit = |metric: &str, (p50, p90): (f64, f64)| {
            println!("{round},{name},{scenario},{metric},{p50:.2},{p90:.2}");
        };

        // Query: random existing price (80%) or a miss (20%).
        let (l, prices) = book::<L>(ctx, w);
        let mut s = 7;
        let targets: Vec<Price> = (0..BATCH * BATCHES)
            .map(|_| {
                let r = rng(&mut s);
                if r.is_multiple_of(5) {
                    -5
                } else {
                    prices[r as usize % prices.len()]
                }
            })
            .collect();
        emit(
            "get",
            measure(|i| {
                black_box(l.get(black_box(targets[i])));
            }),
        );

        // Best-price refresh (cache invalidated), bid then ask.
        emit(
            "best",
            measure(|i| {
                black_box(l.best(i % 2 == 0));
            }),
        );

        // First 8 levels of the near cluster.
        emit(
            "range8",
            measure(|_| {
                black_box(l.range_sum(1_000_000, 1_000_000 + w as Price, 8));
            }),
        );

        // Toggle working on a random existing price.
        let (mut l, prices) = book::<L>(ctx, w);
        let mut s = 11;
        let targets: Vec<Price> = (0..BATCH * BATCHES)
            .map(|_| prices[rng(&mut s) as usize % prices.len()])
            .collect();
        emit(
            "set_working",
            measure(|i| {
                l.set_working(targets[i], i % 2 == 0);
            }),
        );

        // Reprice: an order leaves a random live level (interior delete) for a
        // new price just above the cluster (insert at the top end).
        let (mut l, _) = book::<L>(ctx, w);
        let mut live: Vec<Price> = (0..w as Price).map(|i| 1_000_000 + i).collect();
        let mut s = 13;
        let mut next = 1_000_000 + w as Price;
        emit(
            "reprice_to_top",
            measure(|i| {
                let k = rng(&mut s) as usize % live.len();
                l.remove(live[k]);
                black_box(l.find_or_insert(next, ctx.handles[i % ctx.handles.len()], true));
                live[k] = next;
                next += 1;
            }),
        );

        // Rolling: best (top) level fills away, a new level is added one tick
        // below the worst (bottom): delete at one end, insert at the other.
        let (mut l, _) = book::<L>(ctx, w);
        let mut top = 1_000_000 + w as Price - 1;
        let mut bottom = 1_000_000;
        emit(
            "roll",
            measure(|i| {
                l.remove(top);
                top -= 1;
                bottom -= 1;
                black_box(l.find_or_insert(bottom, ctx.handles[i % ctx.handles.len()], true));
            }),
        );

        // Aggressive refill: a new best above the top, then it fills away.
        let (mut l, _) = book::<L>(ctx, w);
        emit(
            "aggressive_in_out",
            measure(|i| {
                let p = 1_500_000 + (i % 7) as Price;
                black_box(l.find_or_insert(p, ctx.handles[i % ctx.handles.len()], true));
                l.remove(p);
            }),
        );
    }
}

fn main() {
    let rounds: usize = std::env::var("OMS_BENCH_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    let mut pool = Pool::default();
    let ctx = Ctx {
        handles: (0..64).map(|i| pool.insert(i)).collect(),
    };
    println!("round,backend,scenario,metric,per_op_p50_ns,per_op_p90_ns");
    for round in 0..rounds {
        const B: usize = 6;
        let mut order: [usize; B] = std::array::from_fn(|i| i);
        order.rotate_left(round % B);
        if (round / B) % 2 == 1 {
            order.reverse();
        }
        for b in order {
            match b {
                0 => run::<Btree>("btree", round, &ctx),
                1 => run::<Pages>("pages", round, &ctx),
                2 => run::<AdaptiveLocator<VecDeque<Entry>>>("vecdeque", round, &ctx),
                3 => run::<AdaptiveLocator<FlatDeque>>("flat", round, &ctx),
                4 => run::<SlideLocator<u64, 64>>("window64", round, &ctx),
                _ => run::<SlideLocator<u128, 128>>("window128", round, &ctx),
            }
        }
    }
}
