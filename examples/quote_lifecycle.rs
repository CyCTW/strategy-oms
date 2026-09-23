use strategy_oms::{
    gateway::{Gateway, SimGateway},
    journal::MemoryJournal,
    *,
};

fn show(oms: &Engine<MemoryJournal>, book: Book, caption: &str) {
    let o = oms.order(1).unwrap();
    println!(
        "{caption}: price={} filled={} leaves={} pending={:?} reserved={}",
        o.price,
        o.cum_filled,
        o.leaves,
        o.pending.map(|p| p.kind),
        oms.reserved_qty(book)
    );
    for (price, level) in oms.price_range(book, 100..=101) {
        println!("  price {price}: {:?}", level.totals);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let book = Book {
        strategy: 1,
        account: 1,
        venue: 1,
        instrument: 2330,
        side: Side::Buy,
    };
    let limits = Limits {
        max_orders: 32,
        max_requests: 128,
        max_executions: 128,
        max_reports: 256,
        ..Limits::default()
    };
    let mut oms = Engine::new(MemoryJournal::new(256), limits)?;
    let mut gateway = SimGateway::default();
    let outcome = oms.apply(Event::New(NewOrder {
        order_id: 1,
        request_id: 1,
        book,
        price: 100,
        total_qty: 10,
    }))?;
    gateway.send(outcome.outbound.unwrap())?;
    show(&oms, book, "Pending new");
    oms.apply(Event::Report(Report {
        source: 1,
        sequence: 1,
        order_id: 1,
        kind: ReportKind::Accepted {
            request_id: 1,
            exchange_id: 1001,
            price: 100,
            total_qty: 10,
        },
    }))?;
    let key = |id| ExecutionKey {
        venue: 1,
        account: 1,
        trading_day: 20260919,
        execution_id: id,
    };
    oms.apply(Event::Report(Report {
        source: 1,
        sequence: 2,
        order_id: 1,
        kind: ReportKind::Fill {
            key: key(1),
            qty: 3,
            price: 100,
        },
    }))?;
    let outcome = oms.apply(Event::Replace {
        order_id: 1,
        request_id: 2,
        expected_version: oms.order(1).unwrap().version,
        price: 101,
        total_qty: 10,
    })?;
    gateway.send(outcome.outbound.unwrap())?;
    show(&oms, book, "Replace in flight");
    let fill = Event::Report(Report {
        source: 1,
        sequence: 3,
        order_id: 1,
        kind: ReportKind::Fill {
            key: key(2),
            qty: 2,
            price: 100,
        },
    });
    oms.apply(fill)?;
    assert!(oms.apply(fill)?.duplicate);
    show(&oms, book, "Fill while replacing (duplicate ignored)");
    oms.apply(Event::Report(Report {
        source: 1,
        sequence: 4,
        order_id: 1,
        kind: ReportKind::Replaced {
            request_id: 2,
            exchange_id: 1002,
            price: 101,
            total_qty: 10,
        },
    }))?;
    show(&oms, book, "Replace accepted");
    let outcome = oms.apply(Event::Cancel {
        order_id: 1,
        request_id: 3,
        expected_version: oms.order(1).unwrap().version,
    })?;
    gateway.send(outcome.outbound.unwrap())?;
    oms.apply(Event::Report(Report {
        source: 1,
        sequence: 5,
        order_id: 1,
        kind: ReportKind::Canceled {
            request_id: Some(3),
        },
    }))?;
    show(&oms, book, "Cancel accepted");
    let restored = Engine::recover(
        MemoryJournal::from_events(oms.journal().events(), 256)?,
        oms.journal().events(),
        limits,
    )?;
    assert_eq!(restored.order(1), oms.order(1));
    println!(
        "Replay matched final state; {} outbound commands.",
        gateway.sent.len()
    );
    Ok(())
}
