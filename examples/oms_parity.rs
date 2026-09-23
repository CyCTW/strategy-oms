#[path = "parity/codec.rs"]
mod codec;
use codec::*;
use std::{hint::black_box, time::Instant};
use strategy_oms::{journal::MemoryJournal, *};
fn trace(path: &str, backend: IndexBackend) {
    let mut limits = Limits::default();
    let mut capacity = 10000;
    let mut e = Engine::new_with_index(MemoryJournal::new(capacity), limits, backend).unwrap();
    let mut known = Known::default();
    for (step, line) in std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .enumerate()
    {
        let t: Vec<_> = line.split_whitespace().collect();
        let mut result = Hash::new();
        let out: Result<Option<Outcome>, Error> = (|| match t[0] {
            "RESET" => {
                let u = |i: usize| t[i].parse::<usize>().unwrap();
                limits = Limits {
                    max_orders: u(1),
                    max_requests: u(2),
                    max_executions: u(3),
                    max_reports: u(4),
                    max_order_qty: u(5) as u64,
                    max_open_qty_per_book: u(6) as u64,
                };
                capacity = u(7);
                e = Engine::new_with_index(MemoryJournal::new(capacity), limits, backend)?;
                known = Known::default();
                Ok(None)
            }
            "DISPATCH" => e.dispatch_next_order(),
            "RECOVER" => {
                let events = e.journal().events().to_vec();
                let journal =
                    MemoryJournal::from_events(&events, capacity).map_err(Error::Journal)?;
                e = Engine::recover_with_index(journal, &events, limits, backend)?;
                Ok(None)
            }
            "HOLD" => {
                e.hold_for_recovery()?;
                Ok(None)
            }
            _ => {
                let ev = parse(&t, &e, &mut known);
                if t[0] == "REPORT" {
                    if let Event::Report(r) = ev {
                        e.on_report(r).map(Some)
                    } else {
                        unreachable!()
                    }
                } else {
                    e.apply(ev).map(Some)
                }
            }
        })();
        let status = match out {
            Ok(out) => {
                result.add(out.is_some() as u64);
                if let Some(o) = out {
                    outcome(&mut result, o);
                }
                "OK"
            }
            Err(ref error) => error_name(error),
        };
        let mut journal = Hash::new();
        for &v in e.journal().events() {
            event(&mut journal, v);
        }
        println!(
            "{step} {status} {} {} {} {}",
            result.0,
            snapshot(&e, &known),
            journal.0,
            e.journal().events().len()
        );
    }
}
struct Samples(Vec<u64>);
impl Samples {
    fn new(n: usize) -> Self {
        Self(Vec::with_capacity(n))
    }
    fn measure<T>(&mut self, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let v = black_box(f());
        self.0.push(start.elapsed().as_nanos() as u64);
        v
    }
    fn print(mut self, scenario: &str, metric: &str) {
        self.0.sort_unstable();
        let q = |p| self.0[(self.0.len() - 1) * p / 1000];
        println!(
            "time,0,rust,{scenario},{metric},{},{},{},{},{},0,0",
            self.0.len(),
            q(500),
            q(990),
            q(999),
            self.0.last().unwrap()
        );
    }
}
fn limits(n: usize, width: usize) -> Limits {
    Limits {
        max_orders: width.max(32),
        max_requests: 4 * n + width + 64,
        max_executions: n + 64,
        max_reports: 4 * n + width + 64,
        max_order_qty: 1000000,
        max_open_qty_per_book: 1000000000,
    }
}
fn bench(n: usize, width: usize, backend: IndexBackend, name: &str) {
    let mut e = Engine::new_with_index(
        MemoryJournal::new(6 * n + 4 * width + 64),
        limits(n, width),
        backend,
    )
    .unwrap();
    let mut known = Known::default();
    known.books.insert((1, true));
    let mut seq = 0;
    for id in 1..=width as u64 {
        let p = 100 + id as i64 * 4;
        e.apply(Event::New(NewOrder {
            order_id: id,
            request_id: id,
            book: book(1, true),
            price: p,
            total_qty: 10000,
        }))
        .unwrap();
        seq += 1;
        e.on_report(Report {
            source: 1,
            sequence: seq,
            order_id: id,
            kind: ReportKind::Accepted {
                request_id: id,
                exchange_id: 100 + id,
                price: p,
                total_qty: 10000,
            },
        })
        .unwrap();
        known.orders.insert(id);
        known.requests.insert(id);
    }
    let bh = e.book_handle(book(1, true)).unwrap();
    let mut query = Samples::new(n);
    let mut immediate = Samples::new(n);
    let mut queued = Samples::new(n);
    let mut coalesced = Samples::new(n);
    let mut ack = Samples::new(n);
    let mut duplicate = Samples::new(n);
    let mut dispatch = Samples::new(n);
    for step in 0..n {
        let id = (step % width + 1) as u64;
        let req = (width + step * 3 + 1) as u64;
        let o = *e.order(id).unwrap();
        query.measure(|| e.level_summary(bh, black_box(o.price)));
        let p = if (step / width).is_multiple_of(2) {
            1000 + id as i64 * 4
        } else {
            100 + id as i64 * 4
        };
        let change = Event::Replace {
            order_id: id,
            request_id: req,
            expected_version: o.version,
            price: p,
            total_qty: 10000,
        };
        assert!(
            immediate
                .measure(|| e.apply(change))
                .unwrap()
                .outbound
                .is_some()
        );
        let q = Event::Replace {
            order_id: id,
            request_id: req + 1,
            expected_version: e.order(id).unwrap().version,
            price: p + 1,
            total_qty: 10000,
        };
        queued.measure(|| e.apply(q)).unwrap();
        let q2 = Event::Replace {
            order_id: id,
            request_id: req + 2,
            expected_version: e.order(id).unwrap().version,
            price: p + 2,
            total_qty: 10000,
        };
        coalesced.measure(|| e.apply(q2)).unwrap();
        seq += 1;
        let r = Report {
            source: 1,
            sequence: seq,
            order_id: id,
            kind: ReportKind::Replaced {
                request_id: req,
                exchange_id: 100 + id,
                price: p,
                total_qty: 10000,
            },
        };
        ack.measure(|| e.on_report(r)).unwrap();
        duplicate.measure(|| e.on_report(r)).unwrap();
        assert!(
            dispatch
                .measure(|| e.dispatch_next_order())
                .unwrap()
                .unwrap()
                .outbound
                .is_some()
        );
        seq += 1;
        e.on_report(Report {
            source: 1,
            sequence: seq,
            order_id: id,
            kind: ReportKind::Replaced {
                request_id: req + 2,
                exchange_id: 100 + id,
                price: p + 2,
                total_qty: 10000,
            },
        })
        .unwrap();
    }
    known
        .requests
        .extend((width + 1) as u64..=(width + 3 * n) as u64);
    let scenario = format!("{name}_single_{width}");
    let targets: [i64; 64] = std::array::from_fn(|j| {
        if j % 8 == 0 {
            -999999
        } else {
            e.order((j % width + 1) as u64).unwrap().price
        }
    });
    let mut batches = Samples::new(2000);
    for _ in 0..2000 {
        batches.measure(|| {
            let mut sum = 0;
            for p in targets {
                sum += e
                    .level_summary(bh, black_box(p))
                    .map_or(0, |v| v.totals.confirmed_leaves);
            }
            sum
        });
    }
    batches.print(&scenario, "price_query_batch64");
    for (s, label) in [
        (query, "price_query"),
        (immediate, "intent_to_command"),
        (queued, "queued_intent"),
        (coalesced, "coalesced_intent"),
        (ack, "guarded_report_visible"),
        (duplicate, "report_transport_duplicate"),
        (dispatch, "ready_to_command"),
    ] {
        s.print(&scenario, label);
    }
    let mut j = Hash::new();
    for &v in e.journal().events() {
        event(&mut j, v);
    }
    eprintln!(
        "DIGEST single_{width} {} {} {}",
        snapshot(&e, &known),
        j.0,
        e.journal().events().len()
    );
}
fn fills(n: usize, backend: IndexBackend, name: &str) {
    let mut e =
        Engine::new_with_index(MemoryJournal::new(3 * n + 64), limits(n, 1), backend).unwrap();
    let mut known = Known::default();
    known.orders.insert(1);
    known.requests.insert(1);
    known.books.insert((1, true));
    let total = n as u64 + 1;
    e.apply(Event::New(NewOrder {
        order_id: 1,
        request_id: 1,
        book: book(1, true),
        price: 100,
        total_qty: total,
    }))
    .unwrap();
    e.on_report(Report {
        source: 1,
        sequence: 1,
        order_id: 1,
        kind: ReportKind::Accepted {
            request_id: 1,
            exchange_id: 101,
            price: 100,
            total_qty: total,
        },
    })
    .unwrap();
    let mut fill = Samples::new(n);
    let mut business = Samples::new(n);
    let mut duplicate = Samples::new(n);
    for i in 1..=n as u64 {
        let r = Report {
            source: 1,
            sequence: i + 1,
            order_id: 1,
            kind: ReportKind::Fill {
                key: ExecutionKey {
                    venue: 1,
                    account: 1,
                    trading_day: 20260922,
                    execution_id: i,
                },
                qty: 1,
                price: 100,
            },
        };
        fill.measure(|| e.on_report(r)).unwrap();
        let b = Report {
            source: 2,
            sequence: i,
            ..r
        };
        business.measure(|| e.on_report(b)).unwrap();
        duplicate.measure(|| e.on_report(b)).unwrap();
    }
    for (s, label) in [
        (fill, "guarded_fill"),
        (business, "fill_business_duplicate"),
        (duplicate, "fill_transport_duplicate"),
    ] {
        s.print(&format!("{name}_fills"), label);
    }
    known.executions.extend(1..=n as u64);
    let mut j = Hash::new();
    for &v in e.journal().events() {
        event(&mut j, v);
    }
    eprintln!(
        "DIGEST fills {} {} {}",
        snapshot(&e, &known),
        j.0,
        e.journal().events().len()
    );
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 2 && args[1] == "sizes" {
        println!(
            "Order {} Request {} Report {} Event {}",
            size_of::<Order>(),
            size_of::<Request>(),
            size_of::<Report>(),
            size_of::<Event>()
        );
        return;
    }
    let backend = match args[2].as_str() {
        "standard" => IndexBackend::Standard,
        "pages" => IndexBackend::PooledPages,
        _ => panic!("backend"),
    };
    if args[1] == "trace" {
        trace(&args[3], backend);
        return;
    }
    let n = args[3].parse::<usize>().unwrap();
    assert!(n > 0 && n <= 200000);
    println!(
        "pass,round,backend,scenario,metric,n,p50_ns,p99_ns,p999_ns,max_ns,allocations,allocated_bytes"
    );
    for width in [4, 32, 128, 1024] {
        bench(n, width, backend, &args[2]);
    }
    fills(n, backend, &args[2]);
    let mut timer = Samples::new(n);
    for _ in 0..n {
        timer.measure(|| black_box(()));
    }
    timer.print("baseline", "timer");
}
