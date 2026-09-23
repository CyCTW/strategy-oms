use strategy_oms::{journal::MemoryJournal, *};

fn limits() -> Limits {
    Limits {
        max_orders: 256,
        max_requests: 1024,
        max_executions: 1024,
        max_reports: 4096,
        max_order_qty: 1000,
        max_open_qty_per_book: 100_000,
    }
}
fn engine() -> Engine<MemoryJournal> {
    Engine::new(MemoryJournal::new(8192), limits()).unwrap()
}
fn book() -> Book {
    Book {
        strategy: 1,
        account: 2,
        venue: 3,
        instrument: 4,
        side: Side::Buy,
    }
}
fn new(id: u64, request: u64, price: i64, qty: u64) -> Event {
    Event::New(NewOrder {
        order_id: id,
        request_id: request,
        book: book(),
        price,
        total_qty: qty,
    })
}
fn key(id: u64) -> ExecutionKey {
    ExecutionKey {
        venue: 3,
        account: 2,
        trading_day: 20260919,
        execution_id: id,
    }
}
fn report(oms: &mut Engine<MemoryJournal>, id: u64, kind: ReportKind) -> Outcome {
    oms.apply(Event::Report(Report {
        source: 1,
        sequence: oms.last_sequence(1) + 1,
        order_id: id,
        kind,
    }))
    .unwrap()
}
fn accepted(oms: &mut Engine<MemoryJournal>, id: u64, req: u64, price: i64, qty: u64) {
    report(
        oms,
        id,
        ReportKind::Accepted {
            request_id: req,
            exchange_id: id + 1000,
            price,
            total_qty: qty,
        },
    );
}
fn working() -> Engine<MemoryJournal> {
    let mut oms = engine();
    oms.apply(new(1, 1, 100, 10)).unwrap();
    accepted(&mut oms, 1, 1, 100, 10);
    oms
}
fn fill(oms: &mut Engine<MemoryJournal>, exec: u64, qty: u64) {
    report(
        oms,
        1,
        ReportKind::Fill {
            key: key(exec),
            qty,
            price: 100,
        },
    );
}
fn cancel(oms: &mut Engine<MemoryJournal>, req: u64) {
    oms.apply(Event::Cancel {
        order_id: 1,
        request_id: req,
        expected_version: oms.order(1).unwrap().version,
    })
    .unwrap();
}
fn replace(oms: &mut Engine<MemoryJournal>, req: u64, price: i64, qty: u64) {
    oms.apply(Event::Replace {
        order_id: 1,
        request_id: req,
        expected_version: oms.order(1).unwrap().version,
        price,
        total_qty: qty,
    })
    .unwrap();
}

#[test]
fn pending_new_is_queryable_but_not_confirmed() {
    let mut oms = engine();
    let out = oms.apply(new(1, 1, 100, 10)).unwrap();
    assert!(out.outbound.is_some());
    let level = oms.at_price(book(), 100).unwrap();
    assert_eq!(level.totals.pending_new_qty, 10);
    assert_eq!(level.totals.confirmed_leaves, 0);
    assert_eq!(oms.reserved_qty(book()), 10);
    assert_eq!(oms.best_working_price(book()), None);
    accepted(&mut oms, 1, 1, 100, 10);
    assert_eq!(oms.at_price(book(), 100).unwrap().totals.pending_new_qty, 0);
    assert_eq!(oms.best_working_price(book()), Some(100));
}

#[test]
fn fill_before_acceptance_does_not_double_count_or_regress() {
    for qty in [3, 10] {
        let mut oms = engine();
        oms.apply(new(1, 1, 100, 10)).unwrap();
        fill(&mut oms, 1, qty);
        assert_eq!(oms.reserved_qty(book()), 10 - qty);
        accepted(&mut oms, 1, 1, 100, 10);
        let o = oms.order(1).unwrap();
        assert_eq!((o.cum_filled, o.leaves), (qty, 10 - qty));
        assert_eq!(
            o.lifecycle,
            if qty == 10 {
                Lifecycle::Filled
            } else {
                Lifecycle::Working
            }
        );
        assert!(o.pending.is_none());
    }
}

#[test]
fn fill_during_cancel_retains_remaining_reservation_until_ack() {
    let mut oms = working();
    fill(&mut oms, 1, 3);
    cancel(&mut oms, 2);
    fill(&mut oms, 2, 2);
    let t = oms.at_price(book(), 100).unwrap().totals;
    assert_eq!((t.confirmed_leaves, t.pending_cancel_leaves), (5, 5));
    assert_eq!(oms.reserved_qty(book()), 5);
    report(
        &mut oms,
        1,
        ReportKind::Canceled {
            request_id: Some(2),
        },
    );
    assert_eq!(oms.order(1).unwrap().cum_filled, 5);
    assert_eq!(oms.reserved_qty(book()), 0);
    assert!(oms.at_price(book(), 100).is_none());
}

#[test]
fn cancel_reject_after_full_fill_keeps_filled_state() {
    let mut oms = working();
    cancel(&mut oms, 2);
    fill(&mut oms, 1, 10);
    report(&mut oms, 1, ReportKind::Rejected { request_id: 2 });
    assert_eq!(oms.order(1).unwrap().lifecycle, Lifecycle::Filled);
    assert_eq!(oms.request(2).unwrap().state, RequestState::Rejected);
    assert_eq!(oms.best_working_price(book()), None);
}

#[test]
fn replace_accept_moves_only_current_leaves_after_inflight_fill() {
    let mut oms = working();
    fill(&mut oms, 1, 3);
    replace(&mut oms, 2, 101, 10);
    fill(&mut oms, 2, 2);
    assert_eq!(
        oms.at_price(book(), 100).unwrap().totals.confirmed_leaves,
        5
    );
    assert_eq!(
        oms.at_price(book(), 101).unwrap().totals.pending_replace_in,
        5
    );
    assert_eq!(oms.best_working_price(book()), Some(100));
    assert_eq!(oms.reserved_qty(book()), 5);
    report(
        &mut oms,
        1,
        ReportKind::Replaced {
            request_id: 2,
            exchange_id: 1002,
            price: 101,
            total_qty: 10,
        },
    );
    assert!(oms.at_price(book(), 100).is_none());
    assert_eq!(
        oms.at_price(book(), 101).unwrap().totals.confirmed_leaves,
        5
    );
    assert_eq!(oms.best_working_price(book()), Some(101));
}

#[test]
fn replace_rejection_preserves_fills_and_original_price() {
    let mut oms = working();
    replace(&mut oms, 2, 101, 20);
    assert_eq!(oms.reserved_qty(book()), 20);
    fill(&mut oms, 1, 4);
    assert_eq!(oms.reserved_qty(book()), 16);
    report(&mut oms, 1, ReportKind::Rejected { request_id: 2 });
    let o = oms.order(1).unwrap();
    assert_eq!(
        (o.price, o.total_qty, o.cum_filled, o.leaves),
        (100, 10, 4, 6)
    );
    assert_eq!(oms.reserved_qty(book()), 6);
    assert!(oms.at_price(book(), 101).is_none());
}

#[test]
fn same_price_replace_counts_each_order_once() {
    let mut oms = working();
    replace(&mut oms, 2, 100, 5);
    let l = oms.at_price(book(), 100).unwrap();
    assert_eq!(l.order_ids.len(), 1);
    assert_eq!(
        (l.totals.pending_replace_in, l.totals.pending_replace_out),
        (5, 10)
    );
    assert_eq!(oms.reserved_qty(book()), 10);
    fill(&mut oms, 1, 6);
    // Venue normalizes an amendment below in-flight fills to the filled amount.
    report(
        &mut oms,
        1,
        ReportKind::Replaced {
            request_id: 2,
            exchange_id: 1002,
            price: 100,
            total_qty: 6,
        },
    );
    assert_eq!(oms.order(1).unwrap().lifecycle, Lifecycle::Filled);
    assert_eq!(oms.reserved_qty(book()), 0);
}

#[test]
fn duplicate_reports_and_cross_source_fills_are_idempotent() {
    let mut oms = working();
    let r = Report {
        source: 1,
        sequence: 2,
        order_id: 1,
        kind: ReportKind::Fill {
            key: key(1),
            qty: 3,
            price: 100,
        },
    };
    oms.apply(Event::Report(r)).unwrap();
    let before = *oms.order(1).unwrap();
    let count = oms.journal().events().len();
    assert!(oms.apply(Event::Report(r)).unwrap().duplicate);
    assert_eq!(oms.journal().events().len(), count);
    assert!(
        oms.apply(Event::Report(Report {
            source: 2,
            sequence: 1,
            ..r
        }))
        .unwrap()
        .duplicate
    );
    assert_eq!(*oms.order(1).unwrap(), before);
    assert_eq!(oms.last_sequence(2), 1);
    let conflict = Report {
        kind: ReportKind::Fill {
            key: key(1),
            qty: 4,
            price: 100,
        },
        ..r
    };
    assert!(matches!(
        oms.apply(Event::Report(conflict)),
        Err(Error::ConflictingDuplicate)
    ));
}

#[test]
fn sequence_gap_and_invalid_fill_leave_state_and_sequence_unchanged() {
    let mut oms = working();
    let before = *oms.order(1).unwrap();
    let count = oms.journal().events().len();
    let gap = Event::Report(Report {
        source: 1,
        sequence: 3,
        order_id: 1,
        kind: ReportKind::Expired,
    });
    assert!(matches!(
        oms.apply(gap),
        Err(Error::SequenceGap { expected: 2, .. })
    ));
    let overfill = Event::Report(Report {
        source: 1,
        sequence: 2,
        order_id: 1,
        kind: ReportKind::Fill {
            key: key(1),
            qty: 11,
            price: 100,
        },
    });
    assert!(oms.apply(overfill).is_err());
    assert_eq!(*oms.order(1).unwrap(), before);
    assert_eq!(oms.last_sequence(1), 1);
    assert_eq!(oms.journal().events().len(), count);
}

#[test]
fn old_acceptance_on_new_source_does_not_undo_replace() {
    let mut oms = working();
    replace(&mut oms, 2, 101, 10);
    report(
        &mut oms,
        1,
        ReportKind::Replaced {
            request_id: 2,
            exchange_id: 1002,
            price: 101,
            total_qty: 10,
        },
    );
    let before = *oms.order(1).unwrap();
    let result = oms
        .apply(Event::Report(Report {
            source: 2,
            sequence: 1,
            order_id: 1,
            kind: ReportKind::Accepted {
                request_id: 1,
                exchange_id: 1001,
                price: 100,
                total_qty: 10,
            },
        }))
        .unwrap();
    assert!(result.duplicate);
    assert_eq!(*oms.order(1).unwrap(), before);
}

#[test]
fn timeout_keeps_exposure_and_requires_resolution_then_snapshot() {
    let mut oms = working();
    cancel(&mut oms, 2);
    oms.apply(Event::Timeout {
        order_id: 1,
        request_id: 2,
    })
    .unwrap();
    assert_eq!(oms.reserved_qty(book()), 10);
    assert!(matches!(
        oms.apply(new(2, 3, 100, 5)),
        Err(Error::NeedsReconciliation)
    ));
    report(&mut oms, 1, ReportKind::Rejected { request_id: 2 });
    assert!(oms.order(1).unwrap().uncertain);
    report(
        &mut oms,
        1,
        ReportKind::Reconciled {
            price: 100,
            total_qty: 10,
            cum_filled: 0,
            leaves: 10,
            lifecycle: Lifecycle::Working,
        },
    );
    assert!(!oms.order(1).unwrap().uncertain);
    oms.apply(new(2, 3, 100, 5)).unwrap();
}

#[test]
fn stale_version_is_rejected_but_cancel_during_replace_is_saved() {
    let mut oms = working();
    assert!(matches!(
        oms.apply(Event::Cancel {
            order_id: 1,
            request_id: 2,
            expected_version: 1
        }),
        Err(Error::StaleVersion { .. })
    ));
    replace(&mut oms, 2, 101, 10);
    let out = oms
        .apply(Event::Cancel {
            order_id: 1,
            request_id: 3,
            expected_version: oms.order(1).unwrap().version,
        })
        .unwrap();
    assert!(out.outbound.is_none());
    assert_eq!(oms.request(3).unwrap().state, RequestState::Queued);
    assert_eq!(oms.order(1).unwrap().pending.unwrap().id, 2);
}

#[test]
fn correction_of_filled_order_does_not_assume_reinstatement() {
    let mut oms = working();
    fill(&mut oms, 1, 10);
    report(
        &mut oms,
        1,
        ReportKind::Corrected {
            key: key(1),
            revision: 1,
            new_qty: 7,
            new_price: 99,
        },
    );
    let o = oms.order(1).unwrap();
    assert_eq!(
        (o.cum_filled, o.leaves, o.lifecycle, o.uncertain),
        (7, 0, Lifecycle::Filled, true)
    );
    assert_eq!(oms.reserved_qty(book()), 3);
    assert_eq!(oms.best_working_price(book()), None);
    // Delayed duplicate of the ORIGINAL trade, following the correction.
    assert!(
        report(
            &mut oms,
            1,
            ReportKind::Fill {
                key: key(1),
                qty: 10,
                price: 100
            }
        )
        .duplicate
    );
    assert_eq!(oms.order(1).unwrap().cum_filled, 7);
    report(
        &mut oms,
        1,
        ReportKind::Reconciled {
            price: 100,
            total_qty: 10,
            cum_filled: 7,
            leaves: 0,
            lifecycle: Lifecycle::Canceled,
        },
    );
    assert_eq!(oms.reserved_qty(book()), 0);
    assert_eq!(oms.uncertain_orders(book()), 0);
}

#[test]
fn correction_revision_gap_rejected_and_bust_can_reinstate_via_snapshot() {
    let mut oms = working();
    fill(&mut oms, 1, 10);
    let bad = Event::Report(Report {
        source: 1,
        sequence: 3,
        order_id: 1,
        kind: ReportKind::Corrected {
            key: key(1),
            revision: 2,
            new_qty: 0,
            new_price: 100,
        },
    });
    assert!(oms.apply(bad).is_err());
    report(
        &mut oms,
        1,
        ReportKind::Corrected {
            key: key(1),
            revision: 1,
            new_qty: 0,
            new_price: 100,
        },
    );
    report(
        &mut oms,
        1,
        ReportKind::Reconciled {
            price: 100,
            total_qty: 10,
            cum_filled: 0,
            leaves: 10,
            lifecycle: Lifecycle::Working,
        },
    );
    assert_eq!(
        oms.at_price(book(), 100).unwrap().totals.confirmed_leaves,
        10
    );
    assert_eq!(oms.best_working_price(book()), Some(100));
}

#[test]
fn unsolicited_cancel_and_expiry_supersede_pending_requests() {
    for kind in [
        ReportKind::Canceled { request_id: None },
        ReportKind::Expired,
    ] {
        let mut oms = working();
        replace(&mut oms, 2, 101, 20);
        report(&mut oms, 1, kind);
        assert_eq!(oms.request(2).unwrap().state, RequestState::Superseded);
        assert!(oms.order(1).unwrap().pending.is_none());
        assert_eq!(oms.reserved_qty(book()), 0);
        assert_eq!(oms.price_range(book(), 0..=1000).count(), 0);
    }
}

#[test]
fn rejected_new_releases_pending_quantity_and_request_ids_are_unique() {
    let mut oms = engine();
    oms.apply(new(1, 1, 100, 10)).unwrap();
    report(&mut oms, 1, ReportKind::Rejected { request_id: 1 });
    assert_eq!(oms.reserved_qty(book()), 0);
    assert!(oms.at_price(book(), 100).is_none());
    assert!(matches!(
        oms.apply(new(2, 1, 100, 10)),
        Err(Error::DuplicateId)
    ));
}

#[test]
fn pending_quantity_and_atomic_replace_are_in_risk_limit() {
    let mut l = limits();
    l.max_open_qty_per_book = 15;
    let mut oms = Engine::new(MemoryJournal::new(100), l).unwrap();
    oms.apply(new(1, 1, 100, 10)).unwrap();
    assert!(matches!(
        oms.apply(new(2, 2, 100, 6)),
        Err(Error::RiskLimit)
    ));
    accepted(&mut oms, 1, 1, 100, 10);
    assert!(matches!(
        oms.apply(Event::Replace {
            order_id: 1,
            request_id: 2,
            expected_version: 2,
            price: 101,
            total_qty: 16
        }),
        Err(Error::RiskLimit)
    ));
    replace(&mut oms, 2, 101, 15);
    assert_eq!(oms.reserved_qty(book()), 15); // not 10 + 15 for an atomic replace
}

#[test]
fn venue_terms_over_limit_are_recorded_and_cancel_remains_possible() {
    let mut l = limits();
    l.max_open_qty_per_book = 10;
    let mut oms = Engine::new(MemoryJournal::new(100), l).unwrap();
    oms.apply(new(1, 1, 100, 10)).unwrap();
    accepted(&mut oms, 1, 1, 100, 12);
    assert_eq!(oms.reserved_qty(book()), 12);
    cancel(&mut oms, 2);
}

#[test]
fn capacity_failures_are_explicit_and_do_not_create_orders() {
    let mut l = limits();
    l.max_orders = 1;
    let mut oms = Engine::new(MemoryJournal::new(100), l).unwrap();
    oms.apply(new(1, 1, 100, 10)).unwrap();
    assert!(matches!(
        oms.apply(new(2, 2, 100, 10)),
        Err(Error::Capacity("orders"))
    ));
    assert!(oms.order(2).is_none());
    assert_eq!(oms.reserved_qty(book()), 10);
}

#[test]
fn price_queries_are_scoped_and_best_ignores_pending_prices() {
    let mut oms = working();
    oms.apply(new(2, 2, 200, 10)).unwrap();
    assert_eq!(oms.best_working_price(book()), Some(100));
    accepted(&mut oms, 2, 2, 200, 10);
    assert_eq!(oms.best_working_price(book()), Some(200));
    let sell = Book {
        side: Side::Sell,
        ..book()
    };
    let other = Book {
        strategy: 99,
        ..book()
    };
    for (id, b, price) in [(3, sell, 90), (4, sell, 110), (5, other, 999)] {
        oms.apply(Event::New(NewOrder {
            order_id: id,
            request_id: id,
            book: b,
            price,
            total_qty: 10,
        }))
        .unwrap();
        accepted(&mut oms, id, id, price, 10);
    }
    assert_eq!(oms.best_working_price(sell), Some(90));
    assert_eq!(oms.best_working_price(other), Some(999));
    assert_eq!(
        oms.price_range(book(), 100..=199)
            .map(|(p, _)| p)
            .collect::<Vec<_>>(),
        vec![100]
    );
    let reversed = RangeForTest::reversed();
    assert_eq!(oms.price_range(book(), reversed).count(), 0);
}
// Construct an intentionally reversed range without clippy's literal warning.
struct RangeForTest;
impl RangeForTest {
    fn reversed() -> std::ops::RangeInclusive<i64> {
        let hi = 100;
        hi..=0
    }
}

#[test]
fn replay_rebuilds_closed_orders_requests_executions_and_indexes() {
    let mut oms = working();
    fill(&mut oms, 1, 3);
    replace(&mut oms, 2, 101, 10);
    fill(&mut oms, 2, 2);
    report(
        &mut oms,
        1,
        ReportKind::Replaced {
            request_id: 2,
            exchange_id: 1002,
            price: 101,
            total_qty: 10,
        },
    );
    cancel(&mut oms, 3);
    report(
        &mut oms,
        1,
        ReportKind::Canceled {
            request_id: Some(3),
        },
    );
    let events = oms.journal().events();
    let restored = Engine::recover(
        MemoryJournal::from_events(events, 8192).unwrap(),
        events,
        limits(),
    )
    .unwrap();
    assert_eq!(restored.order(1), oms.order(1));
    for id in 1..=3 {
        assert_eq!(restored.request(id), oms.request(id));
    }
    for id in 1..=2 {
        assert_eq!(restored.execution(&key(id)), oms.execution(&key(id)));
    }
    assert_eq!(restored.reserved_qty(book()), 0);
    assert_eq!(restored.journal().events(), events);
}

#[test]
fn repeated_restarts_preserve_recovery_boundaries_and_command_versions() {
    let oms = working();
    let events = oms.journal().events();
    let mut restored = Engine::recover(
        MemoryJournal::from_events(events, 8192).unwrap(),
        events,
        limits(),
    )
    .unwrap();
    assert!(restored.order(1).unwrap().uncertain);
    assert!(matches!(
        restored.apply(new(2, 2, 100, 5)),
        Err(Error::NeedsReconciliation)
    ));
    assert!(matches!(
        restored.journal().events().last(),
        Some(Event::MarkUncertain { order_id: 1 })
    ));
    report(
        &mut restored,
        1,
        ReportKind::Reconciled {
            price: 100,
            total_qty: 10,
            cum_filled: 0,
            leaves: 10,
            lifecycle: Lifecycle::Working,
        },
    );
    cancel(&mut restored, 2);
    let events = restored.journal().events();
    let mut twice = Engine::recover(
        MemoryJournal::from_events(events, 8192).unwrap(),
        events,
        limits(),
    )
    .unwrap();
    assert_eq!(twice.order(1).unwrap().pending.unwrap().id, 2);
    assert!(twice.order(1).unwrap().uncertain);
    report(
        &mut twice,
        1,
        ReportKind::Canceled {
            request_id: Some(2),
        },
    );
    report(
        &mut twice,
        1,
        ReportKind::Reconciled {
            price: 100,
            total_qty: 10,
            cum_filled: 0,
            leaves: 0,
            lifecycle: Lifecycle::Canceled,
        },
    );
    let events = twice.journal().events();
    let third = Engine::recover(
        MemoryJournal::from_events(events, 8192).unwrap(),
        events,
        limits(),
    )
    .unwrap();
    assert_eq!(third.order(1), twice.order(1));
}

#[test]
fn snapshots_cannot_invent_executions_or_resolve_pending_requests() {
    let mut oms = working();
    let bad = Event::Report(Report {
        source: 1,
        sequence: 2,
        order_id: 1,
        kind: ReportKind::Reconciled {
            price: 100,
            total_qty: 10,
            cum_filled: 5,
            leaves: 5,
            lifecycle: Lifecycle::Working,
        },
    });
    assert!(oms.apply(bad).is_err());
    cancel(&mut oms, 2);
    let unresolved = Event::Report(Report {
        source: 1,
        sequence: 2,
        order_id: 1,
        kind: ReportKind::Reconciled {
            price: 100,
            total_qty: 10,
            cum_filled: 0,
            leaves: 0,
            lifecycle: Lifecycle::Canceled,
        },
    });
    assert!(matches!(oms.apply(unresolved), Err(Error::PendingRequest)));
}
