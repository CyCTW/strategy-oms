//! Locator-only screening. Excludes Book/Level/member pools, reducer and WAL.
//! These numbers must NOT be presented as Engine latency or production p99.
#[path = "support/locator_candidates.rs"]
mod candidates;
use candidates::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};

struct CountingAllocator;
static TRACK: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static FREED: AtomicU64 = AtomicU64::new(0);
// Instrumentation only; the candidates themselves are safe Rust.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(l.size() as u64, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if TRACK.load(Ordering::Relaxed) {
            FREED.fetch_add(l.size() as u64, Ordering::Relaxed);
        }
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(n as u64, Ordering::Relaxed);
            FREED.fetch_add(l.size() as u64, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct Samples {
    times: Vec<u64>,
    allocs: u64,
    bytes: u64,
}
impl Samples {
    fn new(n: usize) -> Self {
        Self {
            times: Vec::with_capacity(n),
            allocs: 0,
            bytes: 0,
        }
    }
    fn measure<R>(&mut self, count: bool, f: impl FnOnce() -> R) -> R {
        if count {
            let a = ALLOCS.load(Ordering::Relaxed);
            let b = BYTES.load(Ordering::Relaxed);
            TRACK.store(true, Ordering::Relaxed);
            let result = black_box(f());
            TRACK.store(false, Ordering::Relaxed);
            self.allocs += ALLOCS.load(Ordering::Relaxed) - a;
            self.bytes += BYTES.load(Ordering::Relaxed) - b;
            self.times.push(0);
            result
        } else {
            let start = Instant::now();
            let result = black_box(f());
            self.times.push(start.elapsed().as_nanos() as u64);
            result
        }
    }
    fn print(mut self, count: bool, round: usize, name: &str, scenario: &str, metric: &str) {
        if self.times.is_empty() {
            return;
        }
        self.times.sort_unstable();
        let q = |p| self.times[(self.times.len() - 1) * p / 1000];
        println!(
            "{},{round},{name},{scenario},{metric},{},{},{},{},{},{},{}",
            if count { "alloc" } else { "time" },
            self.times.len(),
            q(500),
            q(990),
            q(999),
            self.times.last().unwrap(),
            self.allocs,
            self.bytes
        );
    }
}
fn value(i: usize) -> Value {
    Value {
        slot: i,
        generation: 1,
    }
}
fn price(i: usize, dense: bool) -> i64 {
    (i as i64) * if dense { 1 } else { 1_000_003 } - 1_000_000_000
}
fn build<T: Locator>(levels: usize, dense: bool) -> T {
    let mut t = T::default();
    for i in 0..levels {
        t.insert(price(i, dense), value(i), true);
    }
    t
}
fn footprint<T: Locator>(name: &str, levels: usize, dense: bool) {
    let a = ALLOCS.load(Ordering::Relaxed);
    let b = BYTES.load(Ordering::Relaxed);
    let f = FREED.load(Ordering::Relaxed);
    TRACK.store(true, Ordering::Relaxed);
    let t = black_box(build::<T>(levels, dense));
    TRACK.store(false, Ordering::Relaxed);
    let live = BYTES.load(Ordering::Relaxed) - b - (FREED.load(Ordering::Relaxed) - f);
    // footprint rows: allocations = setup allocation count; allocated_bytes =
    // final live requested heap bytes + inline object bytes (NOT RSS).
    println!(
        "memory,0,{name},{}_n{levels},resident_requested,1,0,0,0,0,{},{}",
        if dense { "dense" } else { "sparse" },
        ALLOCS.load(Ordering::Relaxed) - a,
        live + std::mem::size_of::<T>() as u64
    );
    drop(t);
}
fn range8(t: &impl Locator, p: i64) -> usize {
    let mut sum = 0usize;
    let mut n = 0;
    t.visit(p, i64::MAX, |_, v| {
        sum = sum.wrapping_add(v.slot);
        n += 1;
        n < 8
    });
    sum
}
fn steady<T: Locator>(name: &str, levels: usize, dense: bool, n: usize, round: usize, count: bool) {
    let mut t = build::<T>(levels, dense);
    let mut hit = Samples::new(n);
    let mut miss = Samples::new(n);
    let mut range = Samples::new(n);
    let mut toggle = Samples::new(n);
    let mut churn = Samples::new(n);
    let mut seed = 77u64;
    for i in 0..4096 {
        black_box(t.get(black_box(price(i % levels, dense))));
    }
    for _ in 0..n {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let i = (seed >> 32) as usize % levels;
        let p = price(i, dense);
        hit.measure(count, || t.get(black_box(p)));
        miss.measure(count, || {
            t.get(black_box(if dense {
                price(levels, dense) + i as i64
            } else {
                p + 1
            }))
        });
        range.measure(count, || range8(&t, black_box(p)));
        toggle.measure(count, || {
            t.set_working(p, false);
            black_box(t.best(true));
            t.set_working(p, true);
        });
    }
    // Churn is a separate phase: randomly chosen old level -> fresh price,
    // preserving live cardinality. Flat moves O(P) elements here.
    let mut prices: Vec<_> = (0..levels).map(|i| price(i, dense)).collect();
    for step in 0..n {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let i = (seed >> 32) as usize % levels;
        let old = prices[i];
        let new = price(levels + step, dense);
        churn.measure(count, || {
            t.remove(black_box(old));
            t.insert(black_box(new), value(i), true);
        });
        prices[i] = new;
    }
    let scenario = format!("{}_n{levels}", if dense { "dense" } else { "sparse" });
    for (s, m) in [
        (hit, "hit"),
        (miss, "miss"),
        (range, "range8"),
        (toggle, "working_off_best_on"),
        (churn, "reprice"),
    ] {
        s.print(count, round, name, &scenario, m);
    }
}
fn many_books<T: Locator>(name: &str, n: usize, round: usize, count: bool) {
    let books: Vec<T> = (0..4096).map(|_| build(4, false)).collect();
    let mut samples = Samples::new(n);
    let mut seed = 41u64;
    for _ in 0..n {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let b = (seed >> 32) as usize % books.len();
        let p = price((seed & 3) as usize, false);
        samples.measure(count, || books[black_box(b)].get(black_box(p)));
    }
    samples.print(count, round, name, "4096books_4prices", "hit");
}
fn edges<T: Locator>(name: &str, n: usize, round: usize, count: bool) {
    for size in [4, 8, 16] {
        let mut promotion = Samples::new(n);
        for _ in 0..n {
            let mut t = build::<T>(size, false);
            promotion.measure(count, || {
                t.insert(black_box(price(size, false)), value(size), true)
            });
        }
        promotion.print(count, round, name, &format!("insert_after{size}"), "insert");
    }
    let mut t = T::default();
    let mut growth = Samples::new(n);
    // Multiplication by an odd number permutes all u64 keys; inserting these
    // exercises random positions, unlike an append-only favourable Flat test.
    for i in 0..n {
        let p = (i as u64).wrapping_mul(0x9e3779b97f4a7c15) as i64;
        growth.measure(count, || t.insert(black_box(p), value(i), true));
    }
    growth.print(count, round, name, "random_growth", "insert");
    let mut t = build::<T>(4096, false);
    for i in 1..4096 {
        t.set_working(price(i, false), false);
    }
    let mut best = Samples::new(n);
    for _ in 0..n {
        best.measure(count, || t.best(black_box(true)));
    }
    best.print(count, round, name, "4095_pending_only", "refresh_best");
}
fn run<T: Locator>(name: &str, n: usize, round: usize, count: bool) {
    for size in [1, 4, 8, 16, 64, 4096] {
        steady::<T>(name, size, false, n, round, count);
    }
    steady::<T>(name, 4096, true, n, round, count);
    many_books::<T>(name, n, round, count);
    edges::<T>(name, n, round, count);
    if count {
        for (size, dense) in [
            (4, false),
            (8, false),
            (16, false),
            (4096, false),
            (4096, true),
        ] {
            footprint::<T>(name, size, dense);
        }
    }
}
fn dispatch(name: &str, n: usize, round: usize, count: bool) {
    match name {
        "standard" => run::<Standard>(name, n, round, count),
        "flat" => run::<Flat>(name, n, round, count),
        "adaptive4" => run::<Adaptive<4>>(name, n, round, count),
        "adaptive8" => run::<Adaptive<8>>(name, n, round, count),
        "adaptive16" => run::<Adaptive<16>>(name, n, round, count),
        "hash_ordered" => run::<HashOrdered>(name, n, round, count),
        "paged64" => run::<Paged>(name, n, round, count),
        _ => unreachable!(),
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
    let names = [
        "standard",
        "flat",
        "adaptive4",
        "adaptive8",
        "adaptive16",
        "hash_ordered",
        "paged64",
    ];
    // Rotate starting candidate and alternate direction to reduce order bias.
    for round in 0..rounds {
        for j in 0..names.len() {
            let offset = if round % 2 == 0 {
                j
            } else {
                names.len() - 1 - j
            };
            dispatch(names[(round + offset) % names.len()], n, round, false);
        }
    }
    for name in names {
        dispatch(name, n, 0, true);
    }
    let mut timer = Samples::new(n);
    for _ in 0..n {
        timer.measure(false, || black_box(()));
    }
    timer.print(false, 0, "timer", "baseline", "timer");
}
