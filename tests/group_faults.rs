use strategy_oms::{
    gateway::{FaultGateway, Gateway, GatewayCommand, SimulationError},
    journal::MemoryJournal,
    *,
};

fn limits() -> Limits {
    Limits {
        max_orders: 64,
        max_requests: 128,
        max_reports: 128,
        max_executions: 128,
        ..Limits::default()
    }
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
fn target(price: i64, qty: u64) -> Option<Target> {
    Some(Target {
        price,
        qty,
        quantity_mode: QuantityMode::MaintainLeaves,
        expires_at_ms: None,
    })
}
fn setup() -> Engine<MemoryJournal> {
    let mut e = Engine::new(MemoryJournal::new(1024), limits()).unwrap();
    e.create_group(
        1,
        book(),
        GroupPolicy {
            max_child_leaves: 5,
            request_timeout_ms: 50,
            ..GroupPolicy::default()
        },
    )
    .unwrap();
    e
}
fn acceptance(d: Dispatch, sequence: u64) -> Report {
    let kind = match d.command {
        GatewayCommand::New(n) => ReportKind::Accepted {
            request_id: n.request_id,
            exchange_id: 1000 + n.order_id,
            price: n.price,
            total_qty: n.total_qty,
        },
        GatewayCommand::Change { request, .. } => match request.kind {
            RequestKind::Replace => ReportKind::Replaced {
                request_id: request.id,
                exchange_id: 1000 + request.order_id,
                price: request.price,
                total_qty: request.total_qty,
            },
            RequestKind::Cancel => ReportKind::Canceled {
                request_id: Some(request.id),
            },
            _ => unreachable!(),
        },
    };
    Report {
        source: 1,
        sequence,
        order_id: d.order_id,
        kind,
    }
}

#[test]
fn delayed_and_duplicate_ack_only_dispatches_latest_target() {
    let mut e = setup();
    let mut g = FaultGateway::new(32);
    e.set_target(1, 1, target(100, 5)).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    g.send(d.command).unwrap();
    let ack = acceptance(d, 1);
    g.schedule_report(5, ack).unwrap();
    g.schedule_report(6, ack).unwrap();
    e.set_target(1, 2, target(101, 5)).unwrap();
    e.set_target(1, 3, target(103, 5)).unwrap();
    e.tick(4).unwrap();
    assert!(g.poll_report(4).is_none());
    assert!(e.dispatch_next().unwrap().is_none());
    e.tick(5).unwrap();
    e.on_report(g.poll_report(5).unwrap()).unwrap();
    let change = e.dispatch_next().unwrap().unwrap();
    g.send(change.command).unwrap();
    assert!(matches!(
        change.command,
        GatewayCommand::Change {
            request: Request { price: 103, .. },
            ..
        }
    ));
    let before = *e.order(d.order_id).unwrap();
    e.tick(6).unwrap();
    assert!(e.on_report(g.poll_report(6).unwrap()).unwrap().duplicate);
    assert_eq!(*e.order(d.order_id).unwrap(), before);
    assert!(e.dispatch_next().unwrap().is_none());
    assert_eq!(g.sent().len(), 2);
}

#[test]
fn source_gap_holds_all_groups_before_any_more_dispatches() {
    let mut e = setup();
    e.create_group(2, book(), GroupPolicy::default()).unwrap();
    e.set_target(1, 1, target(100, 5)).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    e.set_target(2, 1, target(101, 5)).unwrap();
    let error = e.on_report(acceptance(d, 3));
    assert!(matches!(error, Err(Error::SequenceGap { expected: 1, .. })));
    assert_eq!(e.last_sequence(1), 0);
    for id in [1, 2] {
        assert!(e.group_status(id).unwrap().recovery_hold);
    }
    assert_eq!(e.group_status(1).unwrap().reserved_qty, 5);
    assert!(e.dispatch_next().unwrap().is_none());
    e.on_report(acceptance(d, 1)).unwrap();
    assert!(e.order(d.order_id).unwrap().uncertain);
}

#[test]
fn ambiguous_send_failure_keeps_attempt_and_reservation() {
    let mut e = setup();
    let mut g = FaultGateway::new(8);
    e.set_target(1, 1, target(100, 5)).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    g.fail_next_send();
    assert_eq!(g.send(d.command), Err(SimulationError::DeliveryUnknown));
    e.hold_for_recovery().unwrap();
    assert_eq!(g.sent().len(), 1);
    assert_eq!(e.group_status(1).unwrap().reserved_qty, 5);
    assert!(e.order(d.order_id).unwrap().pending.is_some());
    assert!(e.dispatch_next().unwrap().is_none());
}

#[test]
fn partial_success_keeps_accepted_child_and_stop_drains_it() {
    let mut e = setup();
    e.set_target(1, 1, target(100, 10)).unwrap();
    let a = e.dispatch_next().unwrap().unwrap();
    let b = e.dispatch_next().unwrap().unwrap();
    e.on_report(acceptance(a, 1)).unwrap();
    e.on_report(Report {
        source: 1,
        sequence: 2,
        order_id: b.order_id,
        kind: ReportKind::RejectedWithReason {
            request_id: b.request_id,
            reason: RejectReason::Permanent,
        },
    })
    .unwrap();
    let v = e.group_status(1).unwrap();
    assert_eq!((v.confirmed_leaves, v.reserved_qty), (5, 5));
    assert!(e.dispatch_next().unwrap().is_none());
    e.control_group(1, GroupControl::Stop).unwrap();
    let cancel = e.dispatch_next().unwrap().unwrap();
    assert_eq!(cancel.order_id, a.order_id);
    e.on_report(acceptance(cancel, 3)).unwrap();
    assert_eq!(e.group_status(1).unwrap().reserved_qty, 0);
}

#[test]
fn target_expiry_during_new_inflight_waits_then_cancels() {
    let mut e = setup();
    e.set_target(
        1,
        1,
        Some(Target {
            expires_at_ms: Some(10),
            ..target(100, 5).unwrap()
        }),
    )
    .unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    e.tick(10).unwrap();
    assert!(e.dispatch_next().unwrap().is_none());
    e.on_report(acceptance(d, 1)).unwrap();
    let cancel = e.dispatch_next().unwrap().unwrap();
    assert!(matches!(
        cancel.command,
        GatewayCommand::Change {
            request: Request {
                kind: RequestKind::Cancel,
                ..
            },
            ..
        }
    ));
    e.on_report(acceptance(cancel, 2)).unwrap();
    assert_eq!(e.group_status(1).unwrap().status, GroupStatus::Expired);
}

#[test]
fn failed_dispatch_journal_does_not_assign_child_or_consume_exposure() {
    let mut e = Engine::new(MemoryJournal::new(2), limits()).unwrap();
    e.create_group(1, book(), GroupPolicy::default()).unwrap();
    e.set_target(1, 1, target(100, 5)).unwrap();
    assert!(matches!(e.dispatch_next(), Err(Error::Journal(_))));
    assert!(e.halted());
    assert_eq!(e.group_orders(1).unwrap().count(), 0);
    assert_eq!(e.group_status(1).unwrap().reserved_qty, 0);
}

#[test]
fn failed_target_journal_does_not_change_previous_intent() {
    let mut e = Engine::new(MemoryJournal::new(2), limits()).unwrap();
    e.create_group(1, book(), GroupPolicy::default()).unwrap();
    e.set_target(1, 1, target(100, 5)).unwrap();
    assert!(matches!(
        e.set_target(1, 2, target(101, 5)),
        Err(Error::Journal(_))
    ));
    assert_eq!(e.group_status(1).unwrap().target.unwrap().price, 100);
    assert_eq!(e.group_status(1).unwrap().revision, 1);
}

#[test]
fn bounded_fault_gateway_reports_capacity_without_dropping() {
    let mut e = setup();
    let mut g = FaultGateway::new(1);
    e.set_target(1, 1, target(100, 5)).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    g.schedule_report(1, acceptance(d, 1)).unwrap();
    assert_eq!(
        g.schedule_report(2, acceptance(d, 1)),
        Err(SimulationError::Capacity)
    );
    assert_eq!(g.poll_report(2).unwrap().sequence, 1);
    assert!(g.poll_report(2).is_none());
}
