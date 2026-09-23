//! Scripted event loop: latest target wins, delayed reports, rate limiting,
//! cancellation reserve and a stop latch. Nothing connects to a real account.
use strategy_oms::{
    gateway::{FaultGateway, Gateway, GatewayCommand},
    journal::MemoryJournal,
    *,
};

fn acceptance(d: Dispatch, sequence: u64) -> Report {
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
    Report {
        source: 1,
        sequence,
        order_id: d.order_id,
        kind,
    }
}

fn pump(
    e: &mut Engine<MemoryJournal>,
    gateway: &mut FaultGateway,
    now: u64,
    sequence: &mut u64,
) -> Result<(), Box<dyn std::error::Error>> {
    e.tick(now)?;
    while let Some(report) = gateway.poll_report(now) {
        e.on_report(report)?;
    }
    while let Some(d) = e.dispatch_next()? {
        println!(
            "t={now}ms SEND group={} revision={} {:?}",
            d.group_id, d.target_revision, d.command
        );
        if let Err(error) = gateway.send(d.command) {
            e.hold_for_recovery()?;
            return Err(error.into());
        }
        *sequence += 1;
        gateway.schedule_report(now + 5, acceptance(d, *sequence))?;
    }
    Ok(())
}

fn show(e: &Engine<MemoryJournal>, label: &str) {
    let v = e.explain_pending_action(1).unwrap();
    println!(
        "\n{label}: desired={:?}, confirmed={}, projected={}, reserved={}, inflight={}, status={:?}\n",
        v.target.map(|t| (t.price, t.qty)),
        v.confirmed_leaves,
        v.projected_leaves,
        v.reserved_qty,
        v.inflight,
        v.status
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let limits = Limits {
        max_orders: 64,
        max_requests: 128,
        max_executions: 128,
        max_reports: 128,
        ..Limits::default()
    };
    let mut e = Engine::new(MemoryJournal::new(1024), limits)?;
    let book = Book {
        strategy: 1,
        account: 1,
        venue: 1,
        instrument: 2330,
        side: Side::Buy,
    };
    e.create_group(
        1,
        book,
        GroupPolicy {
            max_child_leaves: 5,
            max_active_children: 4,
            max_inflight: 2,
            max_reserved_qty: 12,
            ..GroupPolicy::default()
        },
    )?;
    e.configure_scheduler(SchedulerConfig {
        window_ms: 100,
        max_actions: 6,
        cancel_reserve: 2,
    })?;
    let target = |price, qty| {
        Some(Target {
            price,
            qty,
            quantity_mode: QuantityMode::MaintainLeaves,
            expires_at_ms: Some(200),
        })
    };
    let mut gateway = FaultGateway::new(128);
    let mut sequence = 0;
    e.set_target(1, 1, target(100, 10))?;
    pump(&mut e, &mut gateway, 0, &mut sequence)?;
    show(&e, "Split into two pending 5-unit children");
    for (now, price) in [(1, 101), (2, 102), (3, 103)] {
        e.tick(now)?;
        e.set_target(1, now + 1, target(price, 10))?;
        pump(&mut e, &mut gateway, now, &mut sequence)?;
    }
    show(&e, "Latest target retained while new orders are pending");
    pump(&mut e, &mut gateway, 5, &mut sequence)?;
    pump(&mut e, &mut gateway, 10, &mut sequence)?;
    show(&e, "Converged directly to 103; no 101/102 changes sent");
    e.set_target(1, 5, target(103, 12))?;
    pump(&mut e, &mut gateway, 11, &mut sequence)?;
    show(&e, "Extra quantity waits for normal-action rate budget");
    e.control_group(1, GroupControl::Stop)?;
    pump(&mut e, &mut gateway, 12, &mut sequence)?;
    pump(&mut e, &mut gateway, 17, &mut sequence)?;
    pump(&mut e, &mut gateway, 100, &mut sequence)?;
    show(
        &e,
        "Stop drained both children using the cancellation reserve",
    );
    assert_eq!(e.group_status(1)?.status, GroupStatus::Stopped);
    assert_eq!(gateway.sent().len(), 6);
    let events = e.journal().events();
    let recovered = Engine::recover(MemoryJournal::from_events(events, 1024)?, events, limits)?;
    assert_eq!(recovered.group_status(1)?.revision, 5);
    assert!(recovered.group_status(1)?.recovery_hold);
    println!("Replay retained the target and stop mode; no commands were retransmitted.");
    Ok(())
}
