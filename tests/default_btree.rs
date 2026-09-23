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
fn default_is_standard_under_every_feature_combination() {
    assert_eq!(IndexBackend::default(), IndexBackend::Standard);
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
fn default_btree_keeps_pending_far_price_out_of_best_and_handles_report_burst() {
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
