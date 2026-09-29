use strategy_oms::{journal::MemoryJournal, *};

fn book() -> Book {
    Book {
        strategy: 7,
        account: 2,
        venue: 3,
        instrument: 4,
        side: Side::Buy,
    }
}
fn limits() -> Limits {
    Limits {
        max_orders: 256,
        max_requests: 512,
        max_reports: 1024,
        max_executions: 256,
        max_order_qty: 1_000,
        max_open_qty_per_book: 100_000,
    }
}
fn accept(e: &mut Engine<MemoryJournal>, id: u64, price: i64) {
    e.on_report(Report {
        source: 1,
        sequence: e.last_sequence(1) + 1,
        order_id: id,
        kind: ReportKind::Accepted {
            request_id: id,
            exchange_id: id,
            price,
            total_qty: 100,
        },
    })
    .unwrap();
}
fn add(e: &mut Engine<MemoryJournal>, id: u64, price: i64) {
    e.apply(Event::New(NewOrder {
        order_id: id,
        request_id: id,
        book: book(),
        price,
        total_qty: 100,
    }))
    .unwrap();
    accept(e, id, price);
}

#[test]
fn default_is_adaptive_under_every_feature_combination() {
    assert_eq!(IndexBackend::default(), IndexBackend::Adaptive);
    let mut e = Engine::new(MemoryJournal::new(1024), limits()).unwrap();
    let h = e.register_book(book());
    add(&mut e, 1, 100);
    let stats = e.index_stats();
    assert_eq!((stats.live_levels, stats.live_members), (1, 1));
    assert_eq!((stats.tree_blocks, stats.page_blocks), (0, 0));
    assert_eq!(e.best_working_price_at(h), Some(100));

    let events = e.journal().events();
    let recovered = Engine::recover(
        MemoryJournal::from_events(events, 1024).unwrap(),
        events,
        limits(),
    )
    .unwrap();
    assert!(recovered.level_summary(h, 100).is_none());
    assert_eq!(recovered.best_working_price(book()), Some(100));
    assert_eq!(
        (
            recovered.index_stats().tree_blocks,
            recovered.index_stats().page_blocks
        ),
        (0, 0)
    );
}

#[test]
fn default_index_keeps_pending_far_price_out_of_best_and_handles_report_burst() {
    let mut e = Engine::new(MemoryJournal::new(2048), limits()).unwrap();
    let h = e.register_book(book());
    add(&mut e, 1, 100);
    add(&mut e, 2, 101);
    assert_eq!(e.best_working_price_at(h), Some(101));

    let replace = e
        .apply(Event::Replace {
            order_id: 2,
            request_id: 3,
            expected_version: e.order(2).unwrap().version,
            price: 1_000_000_000,
            total_qty: 100,
        })
        .unwrap();
    assert!(replace.outbound.is_some());
    assert_eq!(e.best_working_price_at(h), Some(101));
    assert_eq!(
        e.level_summary(h, 1_000_000_000)
            .unwrap()
            .totals
            .confirmed_leaves,
        0
    );

    let queued = e
        .apply(Event::Replace {
            order_id: 2,
            request_id: 4,
            expected_version: e.order(2).unwrap().version,
            price: 102,
            total_qty: 100,
        })
        .unwrap();
    assert!(queued.outbound.is_none());
    assert!(e.level_summary(h, 102).is_none());

    let first = Report {
        source: 1,
        sequence: e.last_sequence(1) + 1,
        order_id: 2,
        kind: ReportKind::Replaced {
            request_id: 3,
            exchange_id: 2,
            price: 1_000_000_000,
            total_qty: 100,
        },
    };
    e.on_report(first).unwrap();
    assert_eq!(e.best_working_price_at(h), Some(1_000_000_000));
    assert!(e.on_report(first).unwrap().duplicate);
    assert_eq!(e.best_working_price_at(h), Some(1_000_000_000));
    assert!(e.dispatch_next_order().unwrap().is_some());
    assert_eq!(e.best_working_price_at(h), Some(1_000_000_000));
    e.on_report(Report {
        source: 1,
        sequence: e.last_sequence(1) + 1,
        order_id: 2,
        kind: ReportKind::Replaced {
            request_id: 4,
            exchange_id: 2,
            price: 102,
            total_qty: 100,
        },
    })
    .unwrap();
    assert!(e.level_summary(h, 1_000_000_000).is_none());
    assert_eq!(e.best_working_price_at(h), Some(102));

    // Repeated reports change the total at a known level without changing
    // price membership. The last fill removes the best level entirely.
    for execution_id in 1..=100 {
        let r = Report {
            source: 1,
            sequence: e.last_sequence(1) + 1,
            order_id: 2,
            kind: ReportKind::Fill {
                key: ExecutionKey {
                    venue: 3,
                    account: 2,
                    trading_day: 20260923,
                    execution_id,
                },
                qty: 1,
                price: 102,
            },
        };
        e.on_report(r).unwrap();
        assert!(e.on_report(r).unwrap().duplicate);
    }
    assert!(e.level_summary(h, 102).is_none());
    assert_eq!(e.best_working_price_at(h), Some(100));
    assert_eq!(e.reserved_qty(book()), 100);
    assert_eq!(
        e.levels_in_range(h, i64::MIN..=i64::MAX)
            .map(|(price, _)| price)
            .collect::<Vec<_>>(),
        vec![100]
    );
}

/// Engine-level check that a default Book converts to the B-tree past 1,024
/// prices and back below 256, keeping best price, summaries and recovery
/// correct on both sides of each conversion.
#[test]
fn default_index_crosses_both_conversion_thresholds() {
    const N: u64 = 1_100;
    let limits = Limits {
        max_orders: 2 * N as usize,
        max_requests: 4 * N as usize,
        max_reports: 4 * N as usize,
        max_executions: 16,
        max_order_qty: 1_000,
        max_open_qty_per_book: 1_000_000,
    };
    let mut e = Engine::new(MemoryJournal::new(16 * N as usize), limits).unwrap();
    let h = e.register_book(book());
    // Sparse prices on both sides of 0, so far and near prices interleave.
    let price = |id: u64| (id as i64 - N as i64 / 2) * 1_000_003;
    for id in 1..=N {
        add(&mut e, id, price(id));
    }
    // Pending-only (unacknowledged) prices above the best bid: never working,
    // so best must skip them in both modes and across both conversions.
    let pending = [N + 1, N + 2, N + 3];
    for id in pending {
        e.apply(Event::New(NewOrder {
            order_id: 10 * N + id,
            request_id: 10 * N + id,
            book: book(),
            price: price(id),
            total_qty: 100,
        }))
        .unwrap();
    }
    assert_eq!(e.index_stats().live_levels, N as usize + 3);
    assert_eq!(e.best_working_price_at(h), Some(price(N)));
    for id in [1, N / 2, N] {
        assert_eq!(
            e.level_summary(h, price(id))
                .unwrap()
                .totals
                .confirmed_leaves,
            100
        );
    }
    assert!(e.level_summary(h, price(1) + 1).is_none());
    let lo = price(10);
    let hi = price(20);
    let got: Vec<_> = e.price_range(book(), lo..=hi).map(|(p, _)| p).collect();
    assert_eq!(got, (10..=20).map(price).collect::<Vec<_>>());

    // Cancel from the top (best bid) down to 200 prices: crosses 256.
    for id in (201..=N).rev() {
        let version = e.order(id).unwrap().version;
        e.apply(Event::Cancel {
            order_id: id,
            request_id: N + id,
            expected_version: version,
        })
        .unwrap();
        e.on_report(Report {
            source: 1,
            sequence: e.last_sequence(1) + 1,
            order_id: id,
            kind: ReportKind::Canceled {
                request_id: Some(N + id),
            },
        })
        .unwrap();
    }
    assert_eq!(e.index_stats().live_levels, 203);
    assert_eq!(e.best_working_price_at(h), Some(price(200)));
    assert_eq!(
        e.level_summary(h, price(N + 1))
            .unwrap()
            .totals
            .pending_new_qty,
        100
    );
    assert!(e.level_summary(h, price(201)).is_none());
    assert_eq!(
        e.level_summary(h, price(1))
            .unwrap()
            .totals
            .confirmed_leaves,
        100
    );

    let events = e.journal().events();
    let recovered = Engine::recover(
        MemoryJournal::from_events(events, 16 * N as usize).unwrap(),
        events,
        limits,
    )
    .unwrap();
    assert_eq!(recovered.best_working_price(book()), Some(price(200)));
    assert_eq!(recovered.index_stats().live_levels, 203);
}
