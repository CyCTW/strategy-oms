use strategy_oms::{
    gateway::GatewayCommand,
    journal::{Journal, MemoryJournal},
    *,
};

fn limits() -> Limits {
    Limits {
        max_orders: 32,
        max_requests: 256,
        max_reports: 512,
        max_executions: 128,
        max_order_qty: 100,
        max_open_qty_per_book: 100,
    }
}
fn engine() -> Engine<MemoryJournal> {
    Engine::new(MemoryJournal::new(1024), limits()).unwrap()
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
fn new<J: Journal>(e: &mut Engine<J>, id: u64, req: u64, qty: u64) -> Outcome {
    e.apply(Event::New(NewOrder {
        order_id: id,
        request_id: req,
        book: book(),
        price: 100,
        total_qty: qty,
    }))
    .unwrap()
}
fn report<J: Journal>(e: &mut Engine<J>, id: u64, kind: ReportKind) -> Outcome {
    e.on_report(Report {
        source: 1,
        sequence: e.last_sequence(1) + 1,
        order_id: id,
        kind,
    })
    .unwrap()
}
fn ack<J: Journal>(e: &mut Engine<J>, id: u64, req: u64, qty: u64) {
    report(
        e,
        id,
        ReportKind::Accepted {
            request_id: req,
            exchange_id: id + 100,
            price: 100,
            total_qty: qty,
        },
    );
}
fn replaced<J: Journal>(e: &mut Engine<J>, req: u64, price: i64, qty: u64) {
    report(
        e,
        1,
        ReportKind::Replaced {
            request_id: req,
            exchange_id: 101,
            price,
            total_qty: qty,
        },
    );
}
fn modify_event<J: Journal>(e: &Engine<J>, req: u64, price: i64, qty: u64) -> Event {
    Event::Replace {
        order_id: 1,
        request_id: req,
        expected_version: e.order(1).unwrap().version,
        price,
        total_qty: qty,
    }
}
fn modify<J: Journal>(e: &mut Engine<J>, req: u64, price: i64, qty: u64) -> Outcome {
    e.apply(modify_event(e, req, price, qty)).unwrap()
}
fn cancel<J: Journal>(e: &mut Engine<J>, id: u64, req: u64) -> Outcome {
    e.apply(Event::Cancel {
        order_id: id,
        request_id: req,
        expected_version: e.order(id).unwrap().version,
    })
    .unwrap()
}
fn fill<J: Journal>(e: &mut Engine<J>, qty: u64) {
    let key = ExecutionKey {
        venue: 1,
        account: 1,
        trading_day: 20260920,
        execution_id: e.last_sequence(1) + 1,
    };
    report(
        e,
        1,
        ReportKind::Fill {
            key,
            qty,
            price: 100,
        },
    );
}
fn change(out: Outcome) -> Request {
    match out.outbound.unwrap() {
        GatewayCommand::Change { request, .. } => request,
        _ => panic!("expected a change"),
    }
}
fn working() -> Engine<MemoryJournal> {
    let mut e = engine();
    new(&mut e, 1, 1, 10);
    ack(&mut e, 1, 1, 10);
    e
}
fn snapshot<J: Journal>(e: &mut Engine<J>) {
    let o = *e.order(1).unwrap();
    report(
        e,
        1,
        ReportKind::Reconciled {
            price: o.price,
            total_qty: o.total_qty,
            cum_filled: o.cum_filled,
            leaves: o.leaves,
            lifecycle: o.lifecycle,
        },
    );
}

#[test]
fn coalesces_updates_during_new_and_replace_without_changing_confirmed_terms() {
    let mut e = engine();
    assert_eq!(new(&mut e, 1, 1, 10).intent_revision, Some(1));
    assert!(modify(&mut e, 2, 101, 12).outbound.is_none());
    assert!(modify(&mut e, 3, 102, 15).outbound.is_none());
    assert_eq!(e.request(2).unwrap().state, RequestState::Superseded);
    assert_eq!(e.order(1).unwrap().price, 100);
    assert_eq!(e.reserved_qty(book()), 10);
    assert!(e.at_price(book(), 102).is_none());
    assert!(e.dispatch_next_order().unwrap().is_none());
    ack(&mut e, 1, 1, 10);
    let r = change(e.dispatch_next_order().unwrap().unwrap());
    assert_eq!((r.id, r.price, r.total_qty), (3, 102, 15));
    assert_eq!(e.reserved_qty(book()), 15);
    modify(&mut e, 4, 103, 15);
    modify(&mut e, 5, 104, 15);
    replaced(&mut e, 3, 102, 15);
    let r = change(e.dispatch_next_order().unwrap().unwrap());
    assert_eq!((r.id, r.price), (5, 104));
    assert_eq!(e.request(4).unwrap().state, RequestState::Superseded);
    replaced(&mut e, 5, 104, 15);
    assert_eq!(
        e.order_intent(1).unwrap().status,
        OrderIntentStatus::Resolved
    );
    assert!(e.dispatch_next_order().unwrap().is_none());
}

#[test]
fn cancel_supersedes_unsent_modify_and_cannot_be_undone() {
    let mut e = engine();
    new(&mut e, 1, 1, 10);
    modify(&mut e, 2, 101, 10);
    assert!(cancel(&mut e, 1, 3).outbound.is_none());
    let count = e.journal().events().len();
    assert!(matches!(
        e.apply(modify_event(&e, 4, 102, 10)),
        Err(Error::CancelRequested)
    ));
    assert_eq!(e.journal().events().len(), count);
    assert!(e.request(4).is_none());
    ack(&mut e, 1, 1, 10);
    let r = change(e.dispatch_next_order().unwrap().unwrap());
    assert_eq!((r.id, r.kind), (3, RequestKind::Cancel));
    report(
        &mut e,
        1,
        ReportKind::Canceled {
            request_id: Some(3),
        },
    );
    assert_eq!(
        e.order_intent(1).unwrap().status,
        OrderIntentStatus::Closed(Lifecycle::Canceled)
    );
    assert_eq!(e.request(2).unwrap().state, RequestState::Superseded);
    assert!(e.dispatch_next_order().unwrap().is_none());
}

#[test]
fn cancellation_waits_for_replace_and_preserves_fills() {
    let mut e = working();
    modify(&mut e, 2, 101, 10);
    cancel(&mut e, 1, 3);
    fill(&mut e, 3);
    assert!(e.dispatch_next_order().unwrap().is_none());
    replaced(&mut e, 2, 101, 10);
    let r = change(e.dispatch_next_order().unwrap().unwrap());
    assert_eq!((r.kind, r.id), (RequestKind::Cancel, 3));
    assert_eq!(e.reserved_qty(book()), 7);
    assert_eq!(e.order(1).unwrap().cum_filled, 3);
}

#[test]
fn latest_modify_can_be_superseded_before_dispatch_even_after_ack() {
    let mut e = engine();
    new(&mut e, 1, 1, 10);
    modify(&mut e, 2, 101, 10);
    ack(&mut e, 1, 1, 10);
    let out = modify(&mut e, 3, 102, 10);
    assert_eq!(change(out).id, 3);
    assert_eq!(e.request(2).unwrap().state, RequestState::Superseded);
    assert!(e.dispatch_next_order().unwrap().is_none());
}

#[test]
fn identical_confirmed_terms_settle_without_a_wire_request() {
    let mut e = working();
    assert!(modify(&mut e, 2, 100, 10).outbound.is_none());
    assert_eq!(e.request(2).unwrap().state, RequestState::NotNeeded);
    modify(&mut e, 3, 101, 10);
    modify(&mut e, 4, 101, 10);
    replaced(&mut e, 3, 101, 10);
    assert_eq!(e.request(4).unwrap().state, RequestState::NotNeeded);
    assert!(e.dispatch_next_order().unwrap().is_none());
}

#[test]
fn fully_filled_or_rejected_new_does_not_create_another_order() {
    for canceled in [false, true] {
        for rejected in [false, true] {
            let mut e = engine();
            new(&mut e, 1, 1, 10);
            if canceled {
                cancel(&mut e, 1, 2);
            } else {
                modify(&mut e, 2, 101, 20);
            }
            if rejected {
                report(&mut e, 1, ReportKind::Rejected { request_id: 1 });
            } else {
                fill(&mut e, 10);
                ack(&mut e, 1, 1, 10);
            }
            assert!(e.dispatch_next_order().unwrap().is_none());
            assert_eq!(e.orders().count(), 1);
            assert_eq!(
                e.request(2).unwrap().state,
                if canceled {
                    RequestState::NotNeeded
                } else {
                    RequestState::Unexecutable
                }
            );
        }
    }
}

#[test]
fn partial_fills_keep_total_quantity_semantics_or_make_intent_unexecutable() {
    // Independent expectation: changing total to 5 must leave 5 - fills,
    // and must never dispatch when that total is already exhausted.
    for filled in 1..10 {
        for ack_first in [false, true] {
            let mut e = engine();
            new(&mut e, 1, 1, 10);
            modify(&mut e, 2, 101, 5);
            if ack_first {
                ack(&mut e, 1, 1, 10);
            }
            fill(&mut e, filled);
            if !ack_first {
                ack(&mut e, 1, 1, 10);
            }
            if filled >= 5 {
                assert_eq!(
                    e.order_intent(1).unwrap().status,
                    OrderIntentStatus::Unexecutable
                );
                assert!(e.dispatch_next_order().unwrap().is_none());
                assert_eq!(e.order(1).unwrap().leaves, 10 - filled);
            } else {
                assert_eq!(
                    change(e.dispatch_next_order().unwrap().unwrap()).total_qty,
                    5
                );
                replaced(&mut e, 2, 101, 5);
                assert_eq!(e.order(1).unwrap().leaves, 5 - filled);
            }
        }
    }
}

#[test]
fn same_intent_rejection_does_not_spin_but_newer_intent_can_proceed() {
    let mut e = working();
    modify(&mut e, 2, 101, 10);
    report(&mut e, 1, ReportKind::Rejected { request_id: 2 });
    assert_eq!(
        e.order_intent(1).unwrap().status,
        OrderIntentStatus::Rejected
    );
    assert!(e.dispatch_next_order().unwrap().is_none());
    modify(&mut e, 3, 102, 10);
    modify(&mut e, 4, 103, 10);
    report(
        &mut e,
        1,
        ReportKind::RejectedWithReason {
            request_id: 3,
            reason: RejectReason::Permanent,
        },
    );
    assert_eq!(change(e.dispatch_next_order().unwrap().unwrap()).id, 4);
}

#[test]
fn cancel_rejection_remains_latched_and_needs_explicit_cancel_retry() {
    let mut e = working();
    cancel(&mut e, 1, 2);
    report(&mut e, 1, ReportKind::Rejected { request_id: 2 });
    assert!(e.dispatch_next_order().unwrap().is_none());
    assert!(matches!(
        e.apply(modify_event(&e, 3, 101, 10)),
        Err(Error::CancelRequested)
    ));
    assert_eq!(change(cancel(&mut e, 1, 3)).kind, RequestKind::Cancel);
}

#[test]
fn timeout_accepts_intent_but_requires_resolution_and_snapshot_before_dispatch() {
    let mut e = working();
    modify(&mut e, 2, 101, 10);
    e.apply(Event::Timeout {
        order_id: 1,
        request_id: 2,
    })
    .unwrap();
    assert!(cancel(&mut e, 1, 3).outbound.is_none());
    replaced(&mut e, 2, 101, 10);
    assert_eq!(
        e.order_intent(1).unwrap().status,
        OrderIntentStatus::NeedsReconciliation
    );
    assert!(e.dispatch_next_order().unwrap().is_none());
    snapshot(&mut e);
    assert_eq!(
        change(e.dispatch_next_order().unwrap().unwrap()).kind,
        RequestKind::Cancel
    );
}

#[test]
fn deferred_increase_rechecks_shared_book_risk_and_other_orders_can_progress() {
    let mut e = engine();
    new(&mut e, 1, 1, 10);
    modify(&mut e, 2, 101, 20);
    new(&mut e, 2, 3, 90);
    cancel(&mut e, 2, 4);
    ack(&mut e, 1, 1, 10);
    ack(&mut e, 2, 3, 90);
    assert_eq!(
        e.order_intent(1).unwrap().status,
        OrderIntentStatus::RiskBlocked
    );
    let out = e.dispatch_next_order().unwrap().unwrap();
    assert_eq!(out.order_id, 2);
    report(
        &mut e,
        2,
        ReportKind::Canceled {
            request_id: Some(4),
        },
    );
    assert_eq!(change(e.dispatch_next_order().unwrap().unwrap()).id, 2);
    assert_eq!(e.reserved_qty(book()), 20);
}

#[test]
fn pending_intent_ids_are_reserved_against_reuse_and_group_allocation() {
    let mut e = engine();
    new(&mut e, 1, 1, 10);
    modify(&mut e, 50, 101, 10);
    let count = e.journal().events().len();
    assert!(matches!(
        e.apply(modify_event(&e, 50, 102, 10)),
        Err(Error::DuplicateId)
    ));
    assert_eq!(e.journal().events().len(), count);
    e.create_group(1, book(), GroupPolicy::default()).unwrap();
    e.set_target(
        1,
        1,
        Some(Target {
            price: 100,
            qty: 5,
            quantity_mode: QuantityMode::MaintainLeaves,
            expires_at_ms: None,
        }),
    )
    .unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    assert!(d.request_id > 50);
    assert!(e.order_intent(d.order_id).is_err());
}

#[test]
fn unsent_request_cannot_be_acknowledged_and_on_report_quarantines_singles() {
    let mut e = engine();
    new(&mut e, 1, 1, 10);
    modify(&mut e, 2, 101, 10);
    let result = e.on_report(Report {
        source: 1,
        sequence: 1,
        order_id: 1,
        kind: ReportKind::Replaced {
            request_id: 2,
            exchange_id: 101,
            price: 101,
            total_qty: 10,
        },
    });
    assert!(matches!(result, Err(Error::RequestMismatch)));
    assert!(e.order(1).unwrap().uncertain);
    assert_eq!(e.request(2).unwrap().state, RequestState::Queued);
    assert_eq!(e.last_sequence(1), 0);
    assert!(e.dispatch_next_order().unwrap().is_none());
}

#[test]
fn source_gap_quarantines_ready_single_intents() {
    let mut e = engine();
    new(&mut e, 1, 1, 10);
    modify(&mut e, 2, 101, 10);
    ack(&mut e, 1, 1, 10);
    assert!(matches!(
        e.on_report(Report {
            source: 1,
            sequence: 3,
            order_id: 1,
            kind: ReportKind::Expired
        }),
        Err(Error::SequenceGap { .. })
    ));
    assert_eq!(
        e.order_intent(1).unwrap().status,
        OrderIntentStatus::NeedsReconciliation
    );
    assert!(e.dispatch_next_order().unwrap().is_none());
}

#[test]
fn failed_intent_append_preserves_old_desired_pending_and_reserved_quantity() {
    let mut e = Engine::new(MemoryJournal::new(1), limits()).unwrap();
    new(&mut e, 1, 1, 10);
    let order = *e.order(1).unwrap();
    let intent = e.order_intent(1).unwrap().intent;
    assert!(matches!(
        e.apply(modify_event(&e, 2, 101, 20)),
        Err(Error::Journal(_))
    ));
    assert_eq!(*e.order(1).unwrap(), order);
    assert_eq!(e.order_intent(1).unwrap().intent, intent);
    assert!(e.request(2).is_none());
    assert_eq!(e.reserved_qty(book()), 10);
    assert!(e.halted());
}

#[test]
fn failed_deferred_dispatch_append_never_installs_pending_or_releases_intent() {
    let mut e = Engine::new(MemoryJournal::new(3), limits()).unwrap();
    new(&mut e, 1, 1, 10);
    modify(&mut e, 2, 101, 20);
    ack(&mut e, 1, 1, 10);
    let order = *e.order(1).unwrap();
    assert!(matches!(e.dispatch_next_order(), Err(Error::Journal(_))));
    assert_eq!(*e.order(1).unwrap(), order);
    assert_eq!(e.request(2).unwrap().state, RequestState::Queued);
    assert_eq!(e.reserved_qty(book()), 10);
    assert!(e.halted());
}

#[test]
fn every_wal_prefix_recovers_without_dispatch_and_closed_replay_matches() {
    let mut e = engine();
    new(&mut e, 1, 1, 10);
    modify(&mut e, 2, 101, 10);
    modify(&mut e, 3, 102, 10);
    ack(&mut e, 1, 1, 10);
    e.dispatch_next_order().unwrap().unwrap();
    cancel(&mut e, 1, 4);
    fill(&mut e, 3);
    replaced(&mut e, 3, 102, 10);
    e.dispatch_next_order().unwrap().unwrap();
    report(
        &mut e,
        1,
        ReportKind::Canceled {
            request_id: Some(4),
        },
    );
    let events = e.journal().events();
    for end in 1..=events.len() {
        let prefix = &events[..end];
        let mut r = Engine::recover(
            MemoryJournal::from_events(prefix, 1024).unwrap(),
            prefix,
            limits(),
        )
        .unwrap();
        let before = r.journal().events().len();
        assert!(r.dispatch_next_order().unwrap().is_none());
        assert_eq!(r.journal().events().len(), before);
        if end == events.len() {
            assert_eq!(r.order(1), e.order(1));
            assert_eq!(r.order_intent(1).unwrap(), e.order_intent(1).unwrap());
            for id in 1..=4 {
                assert_eq!(r.request(id), e.request(id));
            }
        } else {
            assert!(r.order(1).unwrap().uncertain);
        }
    }
}

#[test]
fn recovery_retains_cancel_latch_and_dispatches_once_after_snapshot() {
    let mut e = engine();
    new(&mut e, 1, 1, 10);
    cancel(&mut e, 1, 2);
    let events = e.journal().events();
    let mut e = Engine::recover(
        MemoryJournal::from_events(events, 1024).unwrap(),
        events,
        limits(),
    )
    .unwrap();
    ack(&mut e, 1, 1, 10);
    assert!(e.dispatch_next_order().unwrap().is_none());
    assert!(matches!(
        e.apply(modify_event(&e, 3, 101, 10)),
        Err(Error::CancelRequested)
    ));
    snapshot(&mut e);
    assert_eq!(change(e.dispatch_next_order().unwrap().unwrap()).id, 2);
    assert!(e.dispatch_next_order().unwrap().is_none());
}
