use strategy_oms::{
    gateway::{Gateway, SimGateway},
    journal::MemoryJournal,
    *,
};

fn drain(oms: &mut Engine<MemoryJournal>, gateway: &mut SimGateway) -> Result<(), Error> {
    while let Some(outcome) = oms.dispatch_next_order()? {
        gateway
            .send(outcome.outbound.expect("committed dispatch"))
            .unwrap();
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut oms = Engine::new(MemoryJournal::new(100), Limits::default())?;
    let mut gateway = SimGateway::default();
    let book = Book {
        strategy: 1,
        account: 1,
        venue: 1,
        instrument: 2330,
        side: Side::Buy,
    };
    let out = oms.apply(Event::New(NewOrder {
        order_id: 1,
        request_id: 1,
        book,
        price: 100,
        total_qty: 10,
    }))?;
    gateway.send(out.outbound.unwrap())?;
    for (request_id, price) in [(2, 101), (3, 102), (4, 103)] {
        let out = oms.apply(Event::Replace {
            order_id: 1,
            request_id,
            expected_version: oms.order(1).unwrap().version,
            price,
            total_qty: 10,
        })?;
        assert!(out.outbound.is_none());
    }
    assert_eq!(gateway.sent.len(), 1);
    println!(
        "New awaiting ACK; 101/102/103 accepted locally, latest intent: {:?}",
        oms.order_intent(1)?
    );

    oms.on_report(Report {
        source: 1,
        sequence: 1,
        order_id: 1,
        kind: ReportKind::Accepted {
            request_id: 1,
            exchange_id: 1001,
            price: 100,
            total_qty: 10,
        },
    })?;
    drain(&mut oms, &mut gateway)?;
    assert_eq!(oms.order(1).unwrap().pending.unwrap().price, 103);
    assert_eq!(gateway.sent.len(), 2);
    println!("New accepted; sent only the latest replace to 103.");

    let out = oms.apply(Event::Cancel {
        order_id: 1,
        request_id: 5,
        expected_version: oms.order(1).unwrap().version,
    })?;
    assert!(out.outbound.is_none());
    oms.on_report(Report {
        source: 1,
        sequence: 2,
        order_id: 1,
        kind: ReportKind::Fill {
            key: ExecutionKey {
                venue: 1,
                account: 1,
                trading_day: 20260920,
                execution_id: 1,
            },
            qty: 3,
            price: 100,
        },
    })?;
    drain(&mut oms, &mut gateway)?;
    assert_eq!(gateway.sent.len(), 2);
    oms.on_report(Report {
        source: 1,
        sequence: 3,
        order_id: 1,
        kind: ReportKind::Replaced {
            request_id: 4,
            exchange_id: 1001,
            price: 103,
            total_qty: 10,
        },
    })?;
    drain(&mut oms, &mut gateway)?;
    assert_eq!(
        oms.order(1).unwrap().pending.unwrap().kind,
        RequestKind::Cancel
    );
    println!("Replace accepted after 3 fills; automatically dispatched cancel of the remaining 7.");

    oms.on_report(Report {
        source: 1,
        sequence: 4,
        order_id: 1,
        kind: ReportKind::Canceled {
            request_id: Some(5),
        },
    })?;
    drain(&mut oms, &mut gateway)?;
    assert_eq!(gateway.sent.len(), 3);
    assert_eq!(oms.request(2).unwrap().state, RequestState::Superseded);
    assert_eq!(oms.request(3).unwrap().state, RequestState::Superseded);
    let events = oms.journal().events();
    let mut restored = Engine::recover(
        MemoryJournal::from_events(events, 100)?,
        events,
        Limits::default(),
    )?;
    assert_eq!(restored.order(1), oms.order(1));
    assert_eq!(restored.order_intent(1)?, oms.order_intent(1)?);
    assert!(restored.dispatch_next_order()?.is_none());
    println!("Canceled; 3 wire commands total. Replay matched without resending.");
    Ok(())
}
