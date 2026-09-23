use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};
struct CountingAllocator;
pub static TRACK: AtomicBool = AtomicBool::new(false);
pub static ALLOCS: AtomicU64 = AtomicU64::new(0);
pub static BYTES: AtomicU64 = AtomicU64::new(0);
pub static FREED: AtomicU64 = AtomicU64::new(0);
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

pub struct Samples {
    pub times: Vec<u64>,
    pub allocs: u64,
    pub bytes: u64,
}
impl Samples {
    pub fn new(n: usize) -> Self {
        Self {
            times: Vec::with_capacity(n),
            allocs: 0,
            bytes: 0,
        }
    }
    pub fn measure<R>(&mut self, count: bool, f: impl FnOnce() -> R) -> R {
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
    pub fn print(mut self, count: bool, round: usize, name: &str, scenario: &str, metric: &str) {
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
