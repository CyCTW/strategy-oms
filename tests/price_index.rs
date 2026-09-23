use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
use strategy_oms::{journal::MemoryJournal, *};

struct Counter;
thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}
// Test-only allocator instrumentation, isolated per test thread.
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ENABLED.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ENABLED.try_with(Cell::get).unwrap_or(false) {
            let _ = FREES.try_with(|c| c.set(c.get() + 1));
        }
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if ENABLED.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
            let _ = FREES.try_with(|c| c.set(c.get() + 1));
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counter = Counter;
fn count(operation: impl FnOnce()) -> (usize, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ENABLED.with(|c| c.set(false));
        }
    }
    ALLOCATIONS.with(|c| c.set(0));
    FREES.with(|c| c.set(0));
    ENABLED.with(|c| c.set(true));
    let reset = Reset;
    operation();
    drop(reset);
    (ALLOCATIONS.with(Cell::get), FREES.with(Cell::get))
}
fn book() -> Book {
    Book {
        strategy: 1,
        account: 1,
        venue: 1,
        instrument: 1,
        side: Side::Buy,
    }
}
fn limits() -> Limits {
    Limits {
        max_orders: 1024,
        max_requests: 4096,
        max_reports: 8192,
        max_executions: 1024,
        ..Limits::default()
    }
}
fn new_order(e: &mut Engine<MemoryJournal>, id: u64, price: i64) {
    e.apply(Event::New(NewOrder {
        order_id: id,
        request_id: id,
        book: book(),
        price,
        total_qty: 100,
    }))
    .unwrap();
}
fn report(e: &mut Engine<MemoryJournal>, id: u64, kind: ReportKind) {
    e.on_report(Report {
        source: 1,
        sequence: e.last_sequence(1) + 1,
        order_id: id,
        kind,
    })
    .unwrap();
}
fn accept(e: &mut Engine<MemoryJournal>, id: u64, price: i64) {
    report(
        e,
        id,
        ReportKind::Accepted {
            request_id: id,
            exchange_id: id,
            price,
            total_qty: 100,
        },
    );
}
#[test]
fn queries_and_same_price_fills_allocate_nothing_after_initialization() {
    for backend in [
        IndexBackend::Standard,
        IndexBackend::PooledAvl,
        IndexBackend::PooledPages,
    ] {
        let mut e = Engine::new_with_index(MemoryJournal::new(100), limits(), backend).unwrap();
        let h = e.register_book(book());
        new_order(&mut e, 1, 100);
        accept(&mut e, 1, 100);
        assert_eq!(
            count(|| {
                for id in 1..=10 {
                    report(
                        &mut e,
                        1,
                        ReportKind::Fill {
                            key: ExecutionKey {
                                venue: 1,
                                account: 1,
                                trading_day: 1,
                                execution_id: id,
                            },
                            qty: 1,
                            price: 100,
                        },
                    );
                    assert_eq!(
                        e.level_summary(h, 100).unwrap().totals.confirmed_leaves,
                        100 - id
                    );
                    assert_eq!(e.orders_at_price(h, 100).unwrap().sum::<u64>(), 1);
                    assert_eq!(e.best_working_price_at(h), Some(100));
                    assert_eq!(e.levels_in_range(h, i64::MIN..=i64::MAX).count(), 1);
                }
            }),
            (0, 0)
        );
    }
}
#[test]
fn pooled_repricing_reuses_levels_members_and_nodes_without_allocation() {
    let mut e =
        Engine::new_with_index(MemoryJournal::new(100), limits(), IndexBackend::PooledAvl).unwrap();
    new_order(&mut e, 1, 100);
    accept(&mut e, 1, 100);
    assert_eq!(
        count(|| {
            for req in 2..=10 {
                let price = 100 + req as i64;
                e.apply(Event::Replace {
                    order_id: 1,
                    request_id: req,
                    expected_version: e.order(1).unwrap().version,
                    price,
                    total_qty: 100,
                })
                .unwrap();
                report(
                    &mut e,
                    1,
                    ReportKind::Replaced {
                        request_id: req,
                        exchange_id: 1,
                        price,
                        total_qty: 100,
                    },
                );
            }
        }),
        (0, 0)
    );
    assert_eq!(e.index_stats().live_levels, 1);
    assert_eq!(e.index_stats().live_members, 1);
}
#[test]
fn growth_far_apart_prices_and_recovery_preserve_queries_and_invalidate_handles() {
    for backend in [
        IndexBackend::Standard,
        IndexBackend::PooledAvl,
        IndexBackend::PooledPages,
    ] {
        let mut e = Engine::new_with_index(MemoryJournal::new(8192), limits(), backend).unwrap();
        let h = e.register_book(book());
        let prices: Vec<_> = (1..=513)
            .map(|i| match i {
                1 => i64::MIN,
                513 => i64::MAX,
                _ => i * 1_000_000_003,
            })
            .collect();
        for (i, &p) in prices.iter().enumerate() {
            new_order(&mut e, i as u64 + 1, p);
            accept(&mut e, i as u64 + 1, p);
        }
        assert_eq!(e.index_stats().live_levels, 513);
        assert!(e.index_stats().level_blocks > 1);
        assert_eq!(
            e.levels_in_range(h, i64::MIN..=i64::MAX)
                .map(|(p, _)| p)
                .collect::<Vec<_>>(),
            prices
        );
        assert_eq!(e.best_working_price_at(h), Some(i64::MAX));
        assert_eq!(e.level_summary(h, i64::MIN).unwrap().order_count, 1);
        let other = Engine::new_with_index(MemoryJournal::new(8), limits(), backend).unwrap();
        assert!(other.level_summary(h, i64::MIN).is_none());
        assert_eq!(
            e.levels_in_range(h, std::ops::RangeInclusive::new(100, -100))
                .count(),
            0
        );
        let events = e.journal().events();
        let recovered = Engine::recover_with_index(
            MemoryJournal::from_events(events, 8192).unwrap(),
            events,
            limits(),
            backend,
        )
        .unwrap();
        assert!(recovered.level_summary(h, i64::MIN).is_none());
        let new_h = recovered.book_handle(book()).unwrap();
        assert_eq!(
            recovered
                .level_summary(new_h, i64::MIN)
                .unwrap()
                .totals
                .confirmed_leaves,
            100
        );
        assert_eq!(
            recovered
                .level_summary(new_h, i64::MIN)
                .unwrap()
                .totals
                .uncertain_orders,
            1
        );
    }
}
#[test]
fn unchanged_price_keeps_member_links_and_best_ignores_pending_price() {
    for backend in [
        IndexBackend::Standard,
        IndexBackend::PooledAvl,
        IndexBackend::PooledPages,
    ] {
        let mut e = Engine::new_with_index(MemoryJournal::new(100), limits(), backend).unwrap();
        new_order(&mut e, 1, 100);
        accept(&mut e, 1, 100);
        new_order(&mut e, 2, 100);
        accept(&mut e, 2, 100);
        let h = e.book_handle(book()).unwrap();
        let before: Vec<_> = e.orders_at_price(h, 100).unwrap().collect();
        report(
            &mut e,
            1,
            ReportKind::Fill {
                key: ExecutionKey {
                    venue: 1,
                    account: 1,
                    trading_day: 1,
                    execution_id: 1,
                },
                qty: 1,
                price: 100,
            },
        );
        assert_eq!(
            e.orders_at_price(h, 100).unwrap().collect::<Vec<_>>(),
            before
        );
        e.apply(Event::Replace {
            order_id: 1,
            request_id: 3,
            expected_version: e.order(1).unwrap().version,
            price: i64::MAX,
            total_qty: 100,
        })
        .unwrap();
        assert_eq!(e.level_summary(h, 100).unwrap().order_count, 2);
        assert_eq!(e.level_summary(h, i64::MAX).unwrap().order_count, 1);
        assert_eq!(e.best_working_price_at(h), Some(100));
        report(
            &mut e,
            1,
            ReportKind::Replaced {
                request_id: 3,
                exchange_id: 1,
                price: i64::MAX,
                total_qty: 100,
            },
        );
        assert_eq!(e.index_stats().live_members, 2);
        assert_eq!(e.best_working_price_at(h), Some(i64::MAX));
    }
}

#[test]
fn cancel_near_and_rehang_far_preserves_inflight_prices_and_recycles_pages() {
    let mut e =
        Engine::new_with_index(MemoryJournal::new(100), limits(), IndexBackend::PooledPages)
            .unwrap();
    new_order(&mut e, 1, 63);
    accept(&mut e, 1, 63);
    new_order(&mut e, 2, 64);
    accept(&mut e, 2, 64);
    let h = e.book_handle(book()).unwrap();
    assert_eq!(e.index_stats().live_pages, 2);
    e.apply(Event::Cancel {
        order_id: 1,
        request_id: 3,
        expected_version: e.order(1).unwrap().version,
    })
    .unwrap();
    e.apply(Event::New(NewOrder {
        order_id: 3,
        request_id: 4,
        book: book(),
        price: 1_000_000_000,
        total_qty: 100,
    }))
    .unwrap();
    assert_eq!(e.index_stats().live_pages, 3);
    assert_eq!(
        e.level_summary(h, 63).unwrap().totals.pending_cancel_leaves,
        100
    );
    assert_eq!(
        e.level_summary(h, 1_000_000_000)
            .unwrap()
            .totals
            .pending_new_qty,
        100
    );
    assert_eq!(e.best_working_price_at(h), Some(64));
    assert_eq!(e.reserved_qty(book()), 300);
    report(
        &mut e,
        3,
        ReportKind::Accepted {
            request_id: 4,
            exchange_id: 3,
            price: 1_000_000_000,
            total_qty: 100,
        },
    );
    assert_eq!(e.best_working_price_at(h), Some(1_000_000_000));
    report(
        &mut e,
        1,
        ReportKind::Canceled {
            request_id: Some(3),
        },
    );
    assert!(e.level_summary(h, 63).is_none());
    assert_eq!(e.index_stats().live_pages, 2);
    e.apply(Event::Cancel {
        order_id: 3,
        request_id: 5,
        expected_version: e.order(3).unwrap().version,
    })
    .unwrap();
    report(
        &mut e,
        3,
        ReportKind::Canceled {
            request_id: Some(5),
        },
    );
    assert_eq!(e.index_stats().live_pages, 1);
    assert_eq!(e.best_working_price_at(h), Some(64));
    let blocks = e.index_stats().page_blocks;
    e.apply(Event::New(NewOrder {
        order_id: 4,
        request_id: 6,
        book: book(),
        price: i64::MAX,
        total_qty: 100,
    }))
    .unwrap();
    assert_eq!(e.index_stats().page_blocks, blocks);
    assert_eq!(e.index_stats().live_pages, 2);
    assert_eq!(e.levels_in_range(h, 64..=64).count(), 1);
}
