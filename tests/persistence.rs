use std::{
    fs, io,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use strategy_oms::{
    journal::{Durability, FileJournal, Journal, MemoryJournal},
    *,
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
                "strategy-oms-test-{}-{}-{}.wal",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn limits() -> Limits {
    Limits {
        max_orders: 16,
        max_requests: 32,
        max_reports: 64,
        max_executions: 64,
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
fn new() -> Event {
    Event::New(NewOrder {
        order_id: 1,
        request_id: 1,
        book: book(),
        price: 100,
        total_qty: 10,
    })
}
fn ack() -> Event {
    Event::Report(Report {
        source: 1,
        sequence: 1,
        order_id: 1,
        kind: ReportKind::Accepted {
            request_id: 1,
            exchange_id: 55,
            price: 100,
            total_qty: 10,
        },
    })
}

#[test]
fn wal_reopen_continue_and_reopen_again() {
    let path = Temp::new();
    let journal = FileJournal::create(&path.0, Durability::SyncEveryEvent).unwrap();
    let mut oms = Engine::new(journal, limits()).unwrap();
    oms.apply(new()).unwrap();
    oms.apply(ack()).unwrap();
    drop(oms);
    let (journal, events) = FileJournal::open(&path.0, Durability::SyncEveryEvent).unwrap();
    assert_eq!(
        events,
        vec![
            Event::Single(SingleEvent::Submit {
                action: SingleAction::New(match new() {
                    Event::New(n) => n,
                    _ => unreachable!(),
                }),
                dispatch: true,
            }),
            ack()
        ]
    );
    let mut oms = Engine::recover(journal, &events, limits()).unwrap();
    assert!(oms.order(1).unwrap().uncertain);
    oms.apply(Event::Report(Report {
        source: 1,
        sequence: 2,
        order_id: 1,
        kind: ReportKind::Reconciled {
            price: 100,
            total_qty: 10,
            cum_filled: 0,
            leaves: 10,
            lifecycle: Lifecycle::Working,
        },
    }))
    .unwrap();
    oms.apply(Event::Cancel {
        order_id: 1,
        request_id: 2,
        expected_version: oms.order(1).unwrap().version,
    })
    .unwrap();
    oms.apply(Event::Report(Report {
        source: 1,
        sequence: 3,
        order_id: 1,
        kind: ReportKind::Canceled {
            request_id: Some(2),
        },
    }))
    .unwrap();
    let final_state = *oms.order(1).unwrap();
    drop(oms);
    let (journal, events) = FileJournal::open(&path.0, Durability::SyncEveryEvent).unwrap();
    let recovered = Engine::recover(journal, &events, limits()).unwrap();
    assert_eq!(*recovered.order(1).unwrap(), final_state);
}

#[test]
fn wal_refuses_overwrite_and_concurrent_writer() {
    let path = Temp::new();
    let _journal = FileJournal::create(&path.0, Durability::OsBuffered).unwrap();
    assert!(FileJournal::create(&path.0, Durability::OsBuffered).is_err());
    assert!(FileJournal::open(&path.0, Durability::OsBuffered).is_err());
}

#[test]
fn single_wal_reopens_queued_intents_and_deferred_dispatches() {
    let path = Temp::new();
    let journal = FileJournal::create(&path.0, Durability::SyncEveryEvent).unwrap();
    let mut e = Engine::new(journal, limits()).unwrap();
    e.apply(new()).unwrap();
    for (request_id, price) in [(2, 101), (3, 102)] {
        let out = e
            .apply(Event::Replace {
                order_id: 1,
                request_id,
                expected_version: e.order(1).unwrap().version,
                price,
                total_qty: 10,
            })
            .unwrap();
        assert!(out.outbound.is_none());
    }
    e.apply(ack()).unwrap();
    drop(e);
    let (journal, events) = FileJournal::open(&path.0, Durability::SyncEveryEvent).unwrap();
    let mut e = Engine::recover(journal, &events, limits()).unwrap();
    assert_eq!(e.request(2).unwrap().state, RequestState::Superseded);
    assert_eq!(e.request(3).unwrap().state, RequestState::Queued);
    assert!(e.dispatch_next_order().unwrap().is_none());
    e.on_report(Report {
        source: 1,
        sequence: 2,
        order_id: 1,
        kind: ReportKind::Reconciled {
            price: 100,
            total_qty: 10,
            cum_filled: 0,
            leaves: 10,
            lifecycle: Lifecycle::Working,
        },
    })
    .unwrap();
    let out = e.dispatch_next_order().unwrap().unwrap();
    assert!(matches!(
        out.outbound,
        Some(gateway::GatewayCommand::Change {
            request: Request {
                id: 3,
                price: 102,
                ..
            },
            ..
        })
    ));
    let final_order = *e.order(1).unwrap();
    drop(e);
    let (journal, events) = FileJournal::open(&path.0, Durability::SyncEveryEvent).unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Single(SingleEvent::Dispatch { request_id: 3, .. })
    )));
    let mut e = Engine::recover(journal, &events, limits()).unwrap();
    assert_eq!(e.order(1).unwrap().pending, final_order.pending);
    assert_eq!(e.order_intent(1).unwrap().intent.revision, 3);
    assert!(e.order(1).unwrap().uncertain);
    assert!(e.dispatch_next_order().unwrap().is_none());
}

#[test]
fn legacy_physical_wal_remains_readable_and_can_accept_new_intents() {
    let path = Temp::new();
    let mut journal = FileJournal::create(&path.0, Durability::SyncEveryEvent).unwrap();
    journal.append(&new()).unwrap();
    journal.append(&ack()).unwrap();
    drop(journal);
    let (journal, events) = FileJournal::open(&path.0, Durability::SyncEveryEvent).unwrap();
    assert_eq!(events, vec![new(), ack()]);
    let mut e = Engine::recover(journal, &events, limits()).unwrap();
    assert!(e.order_intent(1).is_err());
    let out = e
        .apply(Event::Cancel {
            order_id: 1,
            request_id: 2,
            expected_version: e.order(1).unwrap().version,
        })
        .unwrap();
    assert!(out.outbound.is_none());
    assert_eq!(out.intent_revision, Some(1));
    e.on_report(Report {
        source: 1,
        sequence: 2,
        order_id: 1,
        kind: ReportKind::Reconciled {
            price: 100,
            total_qty: 10,
            cum_filled: 0,
            leaves: 10,
            lifecycle: Lifecycle::Working,
        },
    })
    .unwrap();
    assert!(e.dispatch_next_order().unwrap().unwrap().outbound.is_some());
}

#[test]
fn checksum_corruption_and_truncated_tail_are_rejected() {
    let path = Temp::new();
    let mut journal = FileJournal::create(&path.0, Durability::OsBuffered).unwrap();
    journal.append(&new()).unwrap();
    journal.append(&ack()).unwrap();
    drop(journal);
    let original = fs::read(&path.0).unwrap();
    let mut corrupted = original.clone();
    let end = corrupted.len() - 1;
    corrupted[end] ^= 1;
    fs::write(&path.0, corrupted).unwrap();
    assert!(FileJournal::open(&path.0, Durability::OsBuffered).is_err());
    fs::write(&path.0, &original[..original.len() - 3]).unwrap();
    assert!(FileJournal::open(&path.0, Durability::OsBuffered).is_err());
    fs::write(&path.0, &original[..9]).unwrap();
    assert!(FileJournal::open(&path.0, Durability::OsBuffered).is_err());
}

#[test]
fn record_sequence_and_magic_are_verified() {
    let path = Temp::new();
    let mut journal = FileJournal::create(&path.0, Durability::OsBuffered).unwrap();
    journal.append(&new()).unwrap();
    drop(journal);
    let original = fs::read(&path.0).unwrap();
    let mut bad = original.clone();
    bad[8] = 2;
    fs::write(&path.0, bad).unwrap();
    assert!(FileJournal::open(&path.0, Durability::OsBuffered).is_err());
    let mut bad = original;
    bad[0] = b'X';
    fs::write(&path.0, bad).unwrap();
    assert!(FileJournal::open(&path.0, Durability::OsBuffered).is_err());
}

struct FailAfterOne {
    count: usize,
}
impl Journal for FailAfterOne {
    fn append(&mut self, _: &Event) -> io::Result<()> {
        self.count += 1;
        if self.count > 1 {
            Err(io::Error::other("injected disk failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn journal_failure_halts_engine_before_state_or_sequence_commit() {
    let mut oms = Engine::new(FailAfterOne { count: 0 }, limits()).unwrap();
    oms.apply(new()).unwrap();
    let before = *oms.order(1).unwrap();
    assert!(matches!(oms.apply(ack()), Err(Error::Journal(_))));
    assert_eq!(*oms.order(1).unwrap(), before);
    assert_eq!(oms.last_sequence(1), 0);
    assert_eq!(
        oms.at_price(book(), 100).unwrap().totals.pending_new_qty,
        10
    );
    assert!(oms.halted());
    assert!(matches!(oms.apply(ack()), Err(Error::Halted)));
}

#[test]
fn journal_failure_never_returns_a_dispatchable_new_order() {
    let mut oms = Engine::new(MemoryJournal::new(0), limits()).unwrap();
    assert!(matches!(oms.apply(new()), Err(Error::Journal(_))));
    assert!(oms.order(1).is_none());
    assert!(oms.halted());
}

fn group_ack<J: Journal>(e: &mut Engine<J>, d: Dispatch) {
    use strategy_oms::gateway::GatewayCommand;
    let kind = match d.command {
        GatewayCommand::New(n) => ReportKind::Accepted {
            request_id: n.request_id,
            exchange_id: 1000 + n.order_id,
            price: n.price,
            total_qty: n.total_qty,
        },
        GatewayCommand::Change { request: r, .. } if r.kind == RequestKind::Cancel => {
            ReportKind::Canceled {
                request_id: Some(r.id),
            }
        }
        GatewayCommand::Change { request: r, .. } => ReportKind::Replaced {
            request_id: r.id,
            exchange_id: 1000 + r.order_id,
            price: r.price,
            total_qty: r.total_qty,
        },
    };
    e.on_report(Report {
        source: 1,
        sequence: e.last_sequence(1) + 1,
        order_id: d.order_id,
        kind,
    })
    .unwrap();
}

#[test]
fn group_wal_replays_intents_controls_retries_and_rate_budget() {
    let path = Temp::new();
    let journal = FileJournal::create(&path.0, Durability::SyncEveryEvent).unwrap();
    let mut e = Engine::new(journal, limits()).unwrap();
    e.create_group(1, book(), GroupPolicy::default()).unwrap();
    e.configure_scheduler(SchedulerConfig {
        window_ms: 100,
        max_actions: 3,
        cancel_reserve: 1,
    })
    .unwrap();
    let target = |price| {
        Some(Target {
            price,
            qty: 5,
            quantity_mode: QuantityMode::MaintainLeaves,
            expires_at_ms: Some(1000),
        })
    };
    e.set_target(1, 1, target(100)).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    group_ack(&mut e, d);
    e.control_group(1, GroupControl::Pause).unwrap();
    e.set_target(1, 2, target(101)).unwrap();
    e.control_group(1, GroupControl::Resume).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    group_ack(&mut e, d);
    e.control_group(1, GroupControl::Stop).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    group_ack(&mut e, d);
    e.set_target(1, 3, None).unwrap();
    drop(e);

    let (journal, events) = FileJournal::open(&path.0, Durability::SyncEveryEvent).unwrap();
    let mut e = Engine::recover(journal, &events, limits()).unwrap();
    assert!(e.group_status(1).unwrap().recovery_hold);
    e.control_group(1, GroupControl::Resume).unwrap();
    e.set_target(1, 4, target(102)).unwrap();
    assert_eq!(
        e.group_status(1).unwrap().status,
        GroupStatus::Blocked(BlockReason::RateLimitUntil(100))
    );
    e.tick(100).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    e.on_report(Report {
        source: 1,
        sequence: e.last_sequence(1) + 1,
        order_id: d.order_id,
        kind: ReportKind::RejectedWithReason {
            request_id: d.request_id,
            reason: RejectReason::Transient,
        },
    })
    .unwrap();
    assert!(e.dispatch_next().unwrap().is_none());
    drop(e);

    let (journal, events) = FileJournal::open(&path.0, Durability::SyncEveryEvent).unwrap();
    let mut e = Engine::recover(journal, &events, limits()).unwrap();
    assert_eq!(e.group_status(1).unwrap().retry_count, 1);
    e.control_group(1, GroupControl::Resume).unwrap();
    assert_eq!(
        e.group_status(1).unwrap().status,
        GroupStatus::Blocked(BlockReason::RetryAt(200))
    );
    e.tick(200).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    group_ack(&mut e, d);
    e.control_group(1, GroupControl::Stop).unwrap();
    let d = e.dispatch_next().unwrap().unwrap();
    group_ack(&mut e, d);
    let order = *e.order(d.order_id).unwrap();
    drop(e);
    let (journal, events) = FileJournal::open(&path.0, Durability::SyncEveryEvent).unwrap();
    let e = Engine::recover(journal, &events, limits()).unwrap();
    assert_eq!(e.order(order.id), Some(&order));
    assert_eq!(e.group_status(1).unwrap().revision, 4);
    assert_eq!(e.group_status(1).unwrap().reserved_qty, 0);
}
