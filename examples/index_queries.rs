use strategy_oms::{journal::MemoryJournal, *};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut oms = Engine::new(MemoryJournal::new(64), Limits::default())?;
    let book = Book {
        strategy: 1,
        account: 1,
        venue: 1,
        instrument: 2330,
        side: Side::Buy,
    };
    let handle = oms.register_book(book);
    for (id, price) in [(1, 100), (2, 100), (3, 1_000_000)] {
        let outcome = oms.apply(Event::New(NewOrder {
            order_id: id,
            request_id: id,
            book,
            price,
            total_qty: 10,
        }))?;
        assert!(outcome.outbound.is_some()); // Simulated immediate delivery.
        oms.on_report(Report {
            source: 1,
            sequence: id,
            order_id: id,
            kind: ReportKind::Accepted {
                request_id: id,
                exchange_id: id,
                price,
                total_qty: 10,
            },
        })?;
    }
    println!("100: {:?}", oms.level_summary(handle, 100));
    println!("Best confirmed: {:?}", oms.best_working_price_at(handle));
    if let Some(ids) = oms.orders_at_price(handle, 100) {
        for id in ids {
            println!("Order {id}: {:?}", oms.order(id));
        }
    }
    for (price, level) in oms.levels_in_range(handle, 100..=1_000_000) {
        println!("Price {price}: {:?}", level.totals);
    }
    println!("Pool statistics: {:?}", oms.index_stats());
    Ok(())
}
