use strategy_oms::{gateway::GatewayCommand, journal::MemoryJournal, *};

fn limits() -> Limits {
    Limits {
        max_orders: 128,
        max_requests: 1024,
        max_executions: 1024,
        max_reports: 2048,
        max_order_qty: 1000,
        max_open_qty_per_book: 1000,
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
fn policy() -> GroupPolicy {
    GroupPolicy {
        max_child_leaves: 5,
        max_active_children: 8,
        max_inflight: 2,
        max_reserved_qty: 100,
        max_retries: 2,
        retry_delay_ms: 100,
        request_timeout_ms: 5000,
    }
}
fn setup(p: GroupPolicy) -> Engine<MemoryJournal> {
    let mut e = Engine::new(MemoryJournal::new(8192), limits()).unwrap();
    e.create_group(1, book(), p).unwrap();
    e
}
fn target(price: i64, qty: u64) -> Option<Target> {
    Some(Target {
        price,
        qty,
        quantity_mode: QuantityMode::MaintainLeaves,
        expires_at_ms: None,
    })
}
fn report(e: &mut Engine<MemoryJournal>, order_id: u64, kind: ReportKind) {
    e.apply(Event::Report(Report {
        source: 1,
        sequence: e.last_sequence(1) + 1,
        order_id,
        kind,
    }))
    .unwrap();
}
fn ack(e: &mut Engine<MemoryJournal>, d: Dispatch) {
    let kind = match d.command {
        GatewayCommand::New(n) => ReportKind::Accepted {
            request_id: n.request_id,
            exchange_id: n.order_id + 1000,
            price: n.price,
            total_qty: n.total_qty,
        },
        GatewayCommand::Change { request: r, .. } => match r.kind {
            RequestKind::Replace => ReportKind::Replaced {
                request_id: r.id,
                exchange_id: r.order_id + 1000,
                price: r.price,
                total_qty: r.total_qty,
            },
            RequestKind::Cancel => ReportKind::Canceled {
                request_id: Some(r.id),
            },
            _ => unreachable!(),
        },
    };
    report(e, d.order_id, kind);
}
fn reject(e: &mut Engine<MemoryJournal>, d: Dispatch, reason: RejectReason) {
    report(
        e,
        d.order_id,
        ReportKind::RejectedWithReason {
            request_id: d.request_id,
            reason,
        },
    );
}
fn key(exec: u64) -> ExecutionKey {
    ExecutionKey {
        venue: 1,
        account: 1,
        trading_day: 20260919,
        execution_id: exec,
    }
}
fn fill(e: &mut Engine<MemoryJournal>, order: u64, exec: u64, qty: u64) {
    report(
        e,
        order,
        ReportKind::Fill {
            key: key(exec),
            qty,
            price: 100,
        },
    );
}
fn converge(e: &mut Engine<MemoryJournal>) {
    for _ in 0..100 {
        let Some(d) = e.dispatch_next().unwrap() else {
            return;
        };
        ack(e, d);
    }
    panic!("planner did not converge");
}
fn snapshot(e: &mut Engine<MemoryJournal>, id: u64, leaves: u64, lifecycle: Lifecycle) {
    let o = *e.order(id).unwrap();
    report(
        e,
        id,
        ReportKind::Reconciled {
            price: o.price,
            total_qty: o.total_qty,
            cum_filled: o.cum_filled,
            leaves,
            lifecycle,
        },
    );
}
fn restored(e: &Engine<MemoryJournal>) -> Engine<MemoryJournal> {
    let events = e.journal().events();
    Engine::recover(
        MemoryJournal::from_events(events, 8192).unwrap(),
        events,
        limits(),
    )
    .unwrap()
}

#[test]
fn coalesces_101_102_103_without_queueing_stale_changes() {
    let mut e = setup(GroupPolicy {
        max_child_leaves: 10,
        ..policy()
    });
    e.set_target(1, 1, target(100, 10)).unwrap();
    converge(&mut e);
    e.set_target(1, 2, target(101, 10)).unwrap();
    let first = e.dispatch_next().unwrap().unwrap();
    e.set_target(1, 3, target(102, 10)).unwrap();
    e.set_target(1, 4, target(103, 10)).unwrap();
    assert!(e.dispatch_next().unwrap().is_none());
    assert_eq!(e.order(first.order_id).unwrap().price, 100);
    ack(&mut e, first);
    let next = e.dispatch_next().unwrap().unwrap();
    assert_eq!(next.target_revision, 4);
    assert!(matches!(
        next.command,
        GatewayCommand::Change {
            request: Request { price: 103, .. },
            ..
        }
    ));
    ack(&mut e, next);
    assert_eq!(e.group_status(1).unwrap().status, GroupStatus::Converged);
}

#[test]
fn splits_one_target_into_children_and_limits_inflight_per_group() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 12)).unwrap();
    let a = e.dispatch_next().unwrap().unwrap();
    let b = e.dispatch_next().unwrap().unwrap();
    assert_ne!(a.order_id, b.order_id);
    assert!(e.dispatch_next().unwrap().is_none());
    let view = e.group_status(1).unwrap();
    assert_eq!(
        (
            view.confirmed_leaves,
            view.projected_leaves,
            view.reserved_qty,
            view.inflight
        ),
        (0, 10, 10, 2)
    );
    ack(&mut e, a);
    let c = e.dispatch_next().unwrap().unwrap();
    assert!(matches!(
        c.command,
        GatewayCommand::New(NewOrder { total_qty: 2, .. })
    ));
    ack(&mut e, b);
    ack(&mut e, c);
    assert_eq!(e.group_status(1).unwrap().active_children, 3);
    assert_eq!(e.group_status(1).unwrap().status, GroupStatus::Converged);
}

#[test]
fn pending_reduction_counts_once_across_children() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 10)).unwrap();
    converge(&mut e);
    e.set_target(1, 2, target(100, 8)).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    let view = e.group_status(1).unwrap();
    assert_eq!(
        (
            view.confirmed_leaves,
            view.projected_leaves,
            view.reserved_qty
        ),
        (10, 8, 10)
    );
    e.set_target(1, 3, target(100, 8)).unwrap();
    assert!(e.dispatch_next().unwrap().is_none());
    ack(&mut e, d);
    assert_eq!(e.group_status(1).unwrap().status, GroupStatus::Converged);
}

#[test]
fn changing_8_to_12_during_reduction_respects_temporary_risk() {
    let mut e = setup(GroupPolicy {
        max_reserved_qty: 12,
        ..policy()
    });
    e.set_target(1, 1, target(100, 10)).unwrap();
    converge(&mut e);
    e.set_target(1, 2, target(100, 8)).unwrap();
    let reduce = e.dispatch_next().unwrap().unwrap();
    e.set_target(1, 3, target(100, 12)).unwrap();
    let add = e.dispatch_next().unwrap().unwrap();
    assert!(matches!(
        add.command,
        GatewayCommand::New(NewOrder { total_qty: 2, .. })
    ));
    ack(&mut e, add);
    assert_eq!(e.group_status(1).unwrap().reserved_qty, 12);
    assert!(e.dispatch_next().unwrap().is_none());
    ack(&mut e, reduce);
    converge(&mut e);
    let view = e.group_status(1).unwrap();
    assert_eq!(
        (
            view.confirmed_leaves,
            view.projected_leaves,
            view.reserved_qty
        ),
        (12, 12, 12)
    );
}

#[test]
fn fill_during_reprice_replans_latest_remaining_target() {
    let mut e = setup(GroupPolicy {
        max_child_leaves: 10,
        ..policy()
    });
    e.set_target(1, 1, target(100, 10)).unwrap();
    converge(&mut e);
    e.set_target(1, 2, target(101, 10)).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    fill(&mut e, d.order_id, 1, 3);
    ack(&mut e, d);
    let replenish = e.dispatch_next().unwrap().unwrap();
    assert!(matches!(
        replenish.command,
        GatewayCommand::Change {
            request: Request {
                total_qty: 13,
                price: 101,
                ..
            },
            ..
        }
    ));
    ack(&mut e, replenish);
    assert_eq!(e.group_status(1).unwrap().confirmed_leaves, 10);
}

#[test]
fn total_execution_accounts_for_closed_children_and_latches_completion() {
    let mut e = setup(policy());
    let desired = Some(Target {
        price: 100,
        qty: 8,
        quantity_mode: QuantityMode::TotalExecution,
        expires_at_ms: None,
    });
    e.set_target(1, 1, desired).unwrap();
    converge(&mut e);
    let ids: Vec<_> = e
        .group_orders(1)
        .unwrap()
        .map(|o| (o.id, o.leaves))
        .collect();
    for (i, &(id, qty)) in ids.iter().enumerate() {
        fill(&mut e, id, i as u64 + 1, qty);
    }
    let view = e.group_status(1).unwrap();
    assert_eq!(
        (view.cum_filled, view.required_leaves, view.status),
        (8, 0, GroupStatus::Completed)
    );
    assert!(e.dispatch_next().unwrap().is_none());
    report(
        &mut e,
        ids[0].0,
        ReportKind::Corrected {
            key: key(1),
            revision: 1,
            new_qty: ids[0].1 - 1,
            new_price: 100,
        },
    );
    snapshot(&mut e, ids[0].0, 0, Lifecycle::Canceled);
    assert_eq!(e.group_status(1).unwrap().cum_filled, 7);
    assert!(e.dispatch_next().unwrap().is_none()); // no automatic reopening of a completed target
    e.set_target(1, 2, desired).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    assert!(matches!(
        d.command,
        GatewayCommand::New(NewOrder { total_qty: 1, .. })
    ));
}

#[test]
fn maintain_leaves_deliberately_replenishes_fills() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 5)).unwrap();
    converge(&mut e);
    fill(&mut e, 1, 1, 3);
    let d = e.dispatch_next().unwrap().unwrap();
    assert!(matches!(
        d.command,
        GatewayCommand::Change {
            request: Request { total_qty: 8, .. },
            ..
        }
    ));
    ack(&mut e, d);
    assert_eq!(
        (
            e.group_status(1).unwrap().cum_filled,
            e.group_status(1).unwrap().confirmed_leaves
        ),
        (3, 5)
    );
}

#[test]
fn stop_waits_for_inflight_and_does_not_resurrect_on_new_targets() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 5)).unwrap();
    let new = e.dispatch_next().unwrap().unwrap();
    e.control_group(1, GroupControl::Stop).unwrap();
    e.set_target(1, 2, target(101, 5)).unwrap();
    assert!(e.dispatch_next().unwrap().is_none());
    ack(&mut e, new);
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
    ack(&mut e, cancel);
    assert_eq!(e.group_status(1).unwrap().status, GroupStatus::Stopped);
    e.set_target(1, 3, target(102, 5)).unwrap();
    assert!(e.dispatch_next().unwrap().is_none());
    assert!(e.control_group(1, GroupControl::Pause).is_err());
    e.control_group(1, GroupControl::Resume).unwrap();
    assert!(e.dispatch_next().unwrap().is_some());
}

#[test]
fn pause_keeps_live_orders_and_latest_target_until_resume() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 5)).unwrap();
    converge(&mut e);
    e.control_group(1, GroupControl::Pause).unwrap();
    e.set_target(1, 2, target(103, 5)).unwrap();
    assert!(e.dispatch_next().unwrap().is_none());
    assert_eq!(e.order(1).unwrap().price, 100);
    e.control_group(1, GroupControl::Resume).unwrap();
    converge(&mut e);
    assert_eq!(e.order(1).unwrap().price, 103);
}

#[test]
fn expiry_drains_even_when_paused_and_never_sends_expired_new_target() {
    let mut e = setup(policy());
    let t = Some(Target {
        expires_at_ms: Some(100),
        ..target(100, 5).unwrap()
    });
    e.set_target(1, 1, t).unwrap();
    converge(&mut e);
    e.control_group(1, GroupControl::Pause).unwrap();
    e.tick(100).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    ack(&mut e, d);
    assert_eq!(e.group_status(1).unwrap().status, GroupStatus::Expired);
    assert!(e.set_target(1, 2, t).is_err());
    assert!(e.set_target(1, 1, t).unwrap().duplicate);
    assert!(e.tick(99).is_err());
}

#[test]
fn permanent_rejection_does_not_spin_but_new_revision_can_progress() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 5)).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    reject(&mut e, d, RejectReason::Permanent);
    for _ in 0..20 {
        assert!(e.dispatch_next().unwrap().is_none());
    }
    assert_eq!(
        e.group_status(1).unwrap().status,
        GroupStatus::Blocked(BlockReason::Rejected(RejectReason::Permanent))
    );
    e.set_target(1, 2, target(101, 5)).unwrap();
    assert!(e.dispatch_next().unwrap().is_some());
}

#[test]
fn rejection_for_superseded_intent_does_not_block_latest_revision() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 5)).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    e.set_target(1, 2, target(101, 5)).unwrap();
    reject(&mut e, d, RejectReason::Permanent);
    let next = e.dispatch_next().unwrap().unwrap();
    assert_eq!(next.target_revision, 2);
}

#[test]
fn transient_retries_are_delayed_and_bounded() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 5)).unwrap();
    for i in 0..3 {
        let d = e.dispatch_next().unwrap().unwrap();
        reject(&mut e, d, RejectReason::Transient);
        assert!(e.dispatch_next().unwrap().is_none());
        e.tick((i + 1) * 100).unwrap();
    }
    assert_eq!(
        e.group_status(1).unwrap().status,
        GroupStatus::Blocked(BlockReason::RetryExhausted)
    );
    e.control_group(1, GroupControl::Retry).unwrap();
    assert!(e.dispatch_next().unwrap().is_some());
}

#[test]
fn cancel_reserve_and_priority_work_across_groups() {
    let mut e = setup(policy());
    e.create_group(2, book(), policy()).unwrap();
    e.configure_scheduler(SchedulerConfig {
        window_ms: 100,
        max_actions: 3,
        cancel_reserve: 1,
    })
    .unwrap();
    for id in 1..=2 {
        e.set_target(id, 1, target(100, 5)).unwrap();
    }
    let a = e.dispatch_next().unwrap().unwrap();
    let b = e.dispatch_next().unwrap().unwrap();
    ack(&mut e, a);
    ack(&mut e, b);
    e.set_target(2, 2, target(101, 5)).unwrap();
    assert!(matches!(
        e.group_status(2).unwrap().status,
        GroupStatus::Blocked(BlockReason::RateLimitUntil(100))
    ));
    e.control_group(1, GroupControl::Stop).unwrap();
    let cancel = e.dispatch_next().unwrap().unwrap();
    assert_eq!(cancel.group_id, 1);
    ack(&mut e, cancel);
    assert!(e.dispatch_next().unwrap().is_none());
    e.tick(100).unwrap();
    assert_eq!(e.dispatch_next().unwrap().unwrap().group_id, 2);
}

#[test]
fn target_revisions_are_idempotent_and_conflicts_are_atomic() {
    let mut e = setup(policy());
    e.set_target(1, 12, target(100, 5)).unwrap();
    let count = e.journal().events().len();
    assert!(e.set_target(1, 12, target(100, 5)).unwrap().duplicate);
    assert!(matches!(
        e.set_target(1, 11, target(99, 5)),
        Err(Error::StaleTarget { .. })
    ));
    assert!(matches!(
        e.set_target(1, 12, target(101, 5)),
        Err(Error::ConflictingDuplicate)
    ));
    assert!(e.set_target(1, 13, target(100, 100)).is_err());
    assert_eq!(e.journal().events().len(), count);
    assert_eq!(e.group_status(1).unwrap().revision, 12);
}

#[test]
fn timeout_keeps_exposure_until_report_and_reconciliation() {
    let mut e = setup(GroupPolicy {
        request_timeout_ms: 50,
        ..policy()
    });
    e.set_target(1, 1, target(100, 5)).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    e.tick(50).unwrap();
    assert_eq!(
        e.group_status(1).unwrap().status,
        GroupStatus::Blocked(BlockReason::Uncertain)
    );
    assert_eq!(e.group_status(1).unwrap().reserved_qty, 5);
    assert!(e.dispatch_next().unwrap().is_none());
    ack(&mut e, d);
    snapshot(&mut e, d.order_id, 5, Lifecycle::Working);
    assert_eq!(e.group_status(1).unwrap().status, GroupStatus::Converged);
}

#[test]
fn managed_children_cannot_bypass_target_api_with_raw_changes() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 5)).unwrap();
    converge(&mut e);
    let o = e.order(1).unwrap();
    assert!(matches!(
        e.apply(Event::Cancel {
            order_id: 1,
            request_id: 50,
            expected_version: o.version
        }),
        Err(Error::ManagedOrder)
    ));
}

#[test]
fn recovery_preserves_unsent_latest_target_and_requires_explicit_resume() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 5)).unwrap();
    let pending = e.dispatch_next().unwrap().unwrap();
    e.set_target(1, 2, target(103, 5)).unwrap();
    let mut recovered = restored(&e);
    assert_eq!(
        recovered.group_status(1).unwrap().target.unwrap().price,
        103
    );
    assert_eq!(recovered.group_for_order(pending.order_id), Some(1));
    assert!(recovered.dispatch_next().unwrap().is_none());
    assert!(recovered.control_group(1, GroupControl::Resume).is_err());
    ack(&mut recovered, pending);
    snapshot(&mut recovered, pending.order_id, 5, Lifecycle::Working);
    assert!(recovered.dispatch_next().unwrap().is_none());
    recovered.control_group(1, GroupControl::Resume).unwrap();
    let d = recovered.dispatch_next().unwrap().unwrap();
    assert!(matches!(
        d.command,
        GatewayCommand::Change {
            request: Request { price: 103, .. },
            ..
        }
    ));
    ack(&mut recovered, d);
    let again = restored(&recovered);
    assert_eq!(again.group_status(1).unwrap().revision, 2);
    assert_eq!(again.order(d.order_id).unwrap().price, 103);
}

#[test]
fn heartbeat_revisions_and_expiry_renewal_do_not_reset_rejection_gate() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 5)).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    e.set_target(
        1,
        2,
        Some(Target {
            expires_at_ms: Some(1000),
            ..target(100, 5).unwrap()
        }),
    )
    .unwrap();
    reject(&mut e, d, RejectReason::Permanent);
    for revision in 3..10 {
        e.set_target(1, revision, target(100, 5)).unwrap();
        assert!(e.dispatch_next().unwrap().is_none());
    }
    e.control_group(1, GroupControl::Retry).unwrap();
    assert!(e.dispatch_next().unwrap().is_some());
}

#[test]
fn stop_cancel_rejection_cannot_be_reset_by_strategy_price_updates() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 5)).unwrap();
    converge(&mut e);
    e.control_group(1, GroupControl::Stop).unwrap();
    let cancel = e.dispatch_next().unwrap().unwrap();
    e.set_target(1, 2, target(101, 5)).unwrap();
    reject(&mut e, cancel, RejectReason::Permanent);
    e.set_target(1, 3, target(102, 5)).unwrap();
    assert!(e.dispatch_next().unwrap().is_none());
    assert_eq!(
        e.group_status(1).unwrap().status,
        GroupStatus::Blocked(BlockReason::Rejected(RejectReason::Permanent))
    );
    e.control_group(1, GroupControl::Retry).unwrap();
    let retry = e.dispatch_next().unwrap().unwrap();
    assert!(matches!(
        retry.command,
        GatewayCommand::Change {
            request: Request {
                kind: RequestKind::Cancel,
                ..
            },
            ..
        }
    ));
}

#[test]
fn release_recovery_continues_stopping_without_reactivating_target() {
    let mut e = setup(policy());
    e.set_target(1, 1, target(100, 5)).unwrap();
    converge(&mut e);
    e.control_group(1, GroupControl::Stop).unwrap();
    let mut e = restored(&e);
    snapshot(&mut e, 1, 5, Lifecycle::Working);
    e.control_group(1, GroupControl::ReleaseRecovery).unwrap();
    assert_eq!(e.group_status(1).unwrap().mode, GroupMode::Stopping);
    let d = e.dispatch_next().unwrap().unwrap();
    assert!(matches!(
        d.command,
        GatewayCommand::Change {
            request: Request {
                kind: RequestKind::Cancel,
                ..
            },
            ..
        }
    ));
    ack(&mut e, d);
    assert_eq!(e.group_status(1).unwrap().status, GroupStatus::Stopped);
    assert!(e.dispatch_next().unwrap().is_none());
}
