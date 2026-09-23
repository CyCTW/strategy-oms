use std::collections::BTreeSet;
use strategy_oms::{gateway::GatewayCommand, journal::MemoryJournal, *};
pub struct Hash(pub u64);
impl Hash {
    pub fn new() -> Self {
        Self(14695981039346656037)
    }
    pub fn add(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(1099511628211);
        }
    }
    pub fn opt(&mut self, v: Option<u64>) {
        self.add(v.is_some() as u64);
        if let Some(v) = v {
            self.add(v);
        }
    }
}
pub fn book(id: u64, buy: bool) -> Book {
    Book {
        strategy: id,
        account: 1,
        venue: 1,
        instrument: 1,
        side: if buy { Side::Buy } else { Side::Sell },
    }
}
fn enc_book(h: &mut Hash, b: Book) {
    for v in [b.strategy, b.account, b.venue, b.instrument, b.side as u64] {
        h.add(v);
    }
}
fn key(h: &mut Hash, k: ExecutionKey) {
    for v in [k.venue, k.account, k.trading_day, k.execution_id] {
        h.add(v);
    }
}
fn req(h: &mut Hash, r: Request) {
    for v in [
        r.id,
        r.order_id,
        r.kind as u64,
        r.price as u64,
        r.total_qty,
        r.state as u64,
    ] {
        h.add(v);
    }
}
fn new(h: &mut Hash, n: NewOrder) {
    h.add(n.order_id);
    h.add(n.request_id);
    enc_book(h, n.book);
    h.add(n.price as u64);
    h.add(n.total_qty);
}
fn action(h: &mut Hash, a: SingleAction) {
    match a {
        SingleAction::New(n) => {
            h.add(0);
            new(h, n);
        }
        SingleAction::Cancel {
            order_id,
            request_id,
            expected_version,
        } => {
            for v in [1, order_id, request_id, expected_version] {
                h.add(v);
            }
        }
        SingleAction::Replace {
            order_id,
            request_id,
            expected_version,
            price,
            total_qty,
        } => {
            for v in [
                2,
                order_id,
                request_id,
                expected_version,
                price as u64,
                total_qty,
            ] {
                h.add(v);
            }
        }
    }
}
fn kind(h: &mut Hash, r: ReportKind) {
    match r {
        ReportKind::Accepted {
            request_id,
            exchange_id,
            price,
            total_qty,
        }
        | ReportKind::Replaced {
            request_id,
            exchange_id,
            price,
            total_qty,
        } => {
            h.add(if matches!(r, ReportKind::Accepted { .. }) {
                0
            } else {
                2
            });
            for v in [request_id, exchange_id, price as u64, total_qty] {
                h.add(v);
            }
        }
        ReportKind::Fill { key: k, qty, price } => {
            h.add(1);
            key(h, k);
            h.add(qty);
            h.add(price as u64);
        }
        ReportKind::Canceled { request_id } => {
            h.add(3);
            h.opt(request_id);
        }
        ReportKind::Rejected { request_id } => {
            h.add(4);
            h.add(request_id);
        }
        ReportKind::RejectedWithReason { request_id, reason } => {
            h.add(5);
            h.add(request_id);
            h.add(reason as u64);
        }
        ReportKind::Expired => h.add(6),
        ReportKind::Corrected {
            key: k,
            revision,
            new_qty,
            new_price,
        } => {
            h.add(7);
            key(h, k);
            h.add(revision);
            h.add(new_qty);
            h.add(new_price as u64);
        }
        ReportKind::Reconciled {
            price,
            total_qty,
            cum_filled,
            leaves,
            lifecycle,
        } => {
            for v in [
                8,
                price as u64,
                total_qty,
                cum_filled,
                leaves,
                lifecycle as u64,
            ] {
                h.add(v);
            }
        }
    }
}
pub fn event(h: &mut Hash, e: Event) {
    match e {
        Event::New(n) => {
            h.add(0);
            new(h, n);
        }
        Event::Cancel {
            order_id,
            request_id,
            expected_version,
        } => {
            for v in [1, order_id, request_id, expected_version] {
                h.add(v);
            }
        }
        Event::Replace {
            order_id,
            request_id,
            expected_version,
            price,
            total_qty,
        } => {
            for v in [
                2,
                order_id,
                request_id,
                expected_version,
                price as u64,
                total_qty,
            ] {
                h.add(v);
            }
        }
        Event::Timeout {
            order_id,
            request_id,
        } => {
            h.add(3);
            h.add(order_id);
            h.add(request_id);
        }
        Event::MarkUncertain { order_id } => {
            h.add(4);
            h.add(order_id);
        }
        Event::Report(r) => {
            for v in [5, r.source, r.sequence, r.order_id] {
                h.add(v);
            }
            kind(h, r.kind);
        }
        Event::Single(s) => {
            h.add(6);
            match s {
                SingleEvent::Submit {
                    action: a,
                    dispatch,
                } => {
                    h.add(0);
                    action(h, a);
                    h.add(dispatch as u64);
                }
                SingleEvent::Dispatch {
                    order_id,
                    request_id,
                    expected_version,
                } => {
                    for v in [1, order_id, request_id, expected_version] {
                        h.add(v);
                    }
                }
            }
        }
        Event::Group(_) => panic!("groups excluded from parity scope"),
    }
}
pub fn outcome(h: &mut Hash, o: Outcome) {
    h.add(o.order_id);
    h.add(o.version);
    h.opt(o.intent_revision);
    h.add(o.duplicate as u64);
    h.add(o.outbound.is_some() as u64);
    if let Some(c) = o.outbound {
        match c {
            GatewayCommand::New(n) => {
                h.add(0);
                new(h, n);
            }
            GatewayCommand::Change {
                order_id,
                exchange_id,
                request,
            } => {
                h.add(1);
                h.add(order_id);
                h.opt(exchange_id);
                req(h, request);
            }
        }
    }
}
#[derive(Default)]
pub struct Known {
    pub orders: BTreeSet<u64>,
    pub requests: BTreeSet<u64>,
    pub executions: BTreeSet<u64>,
    pub books: BTreeSet<(u64, bool)>,
}
pub fn snapshot(e: &Engine<MemoryJournal>, known: &Known) -> u64 {
    let mut h = Hash::new();
    h.add(e.halted() as u64);
    for i in 1..=4 {
        h.add(e.last_sequence(i));
    }
    for &id in &known.orders {
        let o = e.order(id);
        h.add(o.is_some() as u64);
        let Some(o) = o else { continue };
        h.add(o.id);
        enc_book(&mut h, o.book);
        h.opt(o.exchange_id);
        for v in [
            o.price as u64,
            o.total_qty,
            o.cum_filled,
            o.leaves,
            o.lifecycle as u64,
        ] {
            h.add(v);
        }
        h.add(o.pending.is_some() as u64);
        if let Some(r) = o.pending {
            req(&mut h, r);
        }
        h.add(o.uncertain as u64);
        h.add(o.version);
        let i = e.order_intent(id).unwrap();
        h.add(i.intent.revision);
        h.add(i.intent.request_id);
        match i.intent.desired {
            OrderDesired::Cancel => h.add(0),
            OrderDesired::Working { price, total_qty } => {
                h.add(1);
                h.add(price as u64);
                h.add(total_qty);
            }
        }
        h.add(i.request_state as u64);
        h.add(match i.status {
            OrderIntentStatus::Ready => 0,
            OrderIntentStatus::WaitingForReport => 1,
            OrderIntentStatus::NeedsReconciliation => 2,
            OrderIntentStatus::RiskBlocked => 3,
            OrderIntentStatus::Rejected => 4,
            OrderIntentStatus::Unexecutable => 5,
            OrderIntentStatus::Resolved => 6,
            OrderIntentStatus::Closed(_) => 7,
            OrderIntentStatus::Halted => 8,
        });
    }
    for &id in &known.requests {
        let r = e.request(id);
        h.add(r.is_some() as u64);
        if let Some(&r) = r {
            req(&mut h, r);
        }
    }
    for &id in &known.executions {
        let x = e.execution(&ExecutionKey {
            venue: 1,
            account: 1,
            trading_day: 20260922,
            execution_id: id,
        });
        h.add(x.is_some() as u64);
        if let Some(x) = x {
            key(&mut h, x.key);
            for v in [
                x.order_id,
                x.original_qty,
                x.original_price as u64,
                x.qty,
                x.price as u64,
                x.revision,
            ] {
                h.add(v);
            }
        }
    }
    for &(id, buy) in &known.books {
        let b = book(id, buy);
        h.add(e.reserved_qty(b));
        h.add(e.uncertain_orders(b));
        h.opt(e.best_working_price(b).map(|p| p as u64));
        for (p, s) in e.price_range(b, i64::MIN..=i64::MAX) {
            h.add(p as u64);
            let t = s.totals;
            for v in [
                t.confirmed_leaves,
                t.pending_new_qty,
                t.pending_cancel_leaves,
                t.pending_replace_in,
                t.pending_replace_out,
                t.uncertain_orders,
                s.order_ids.len() as u64,
            ] {
                h.add(v);
            }
            let mut ids: Vec<_> = s.order_ids.collect();
            ids.sort_unstable();
            for i in ids {
                h.add(i);
            }
        }
        h.add(0xabcdef);
    }
    h.0
}
pub fn error_name(e: &Error) -> &'static str {
    match e {
        Error::Invalid(_) => "Invalid",
        Error::UnknownOrder(_) => "UnknownOrder",
        Error::UnknownExecution => "UnknownExecution",
        Error::DuplicateId => "DuplicateId",
        Error::ConflictingDuplicate => "ConflictingDuplicate",
        Error::Capacity(_) => "Capacity",
        Error::StaleVersion { .. } => "StaleVersion",
        Error::SequenceGap { .. } => "SequenceGap",
        Error::PendingRequest => "PendingRequest",
        Error::RequestMismatch => "RequestMismatch",
        Error::NeedsReconciliation => "NeedsReconciliation",
        Error::RiskLimit => "RiskLimit",
        Error::Journal(_) => "Journal",
        Error::Halted => "Halted",
        Error::InvalidDispatch => "InvalidDispatch",
        Error::CancelRequested => "CancelRequested",
        _ => "OutOfScope",
    }
}
pub fn parse(t: &[&str], e: &Engine<MemoryJournal>, known: &mut Known) -> Event {
    let u = |i: usize| t[i].parse::<u64>().unwrap();
    let p = |i: usize| t[i].parse::<i64>().unwrap();
    let id = u(1);
    known.orders.insert(id);
    let ver = |i: usize| {
        if t[i] == "-1" {
            e.order(id).map_or(0, |o| o.version)
        } else {
            u(i)
        }
    };
    match t[0] {
        "NEW" => {
            known.requests.insert(u(2));
            known.books.insert((u(3), u(4) == 0));
            Event::New(NewOrder {
                order_id: id,
                request_id: u(2),
                book: book(u(3), u(4) == 0),
                price: p(5),
                total_qty: u(6),
            })
        }
        "REPLACE" => {
            known.requests.insert(u(2));
            Event::Replace {
                order_id: id,
                request_id: u(2),
                expected_version: ver(3),
                price: p(4),
                total_qty: u(5),
            }
        }
        "CANCEL" => {
            known.requests.insert(u(2));
            Event::Cancel {
                order_id: id,
                request_id: u(2),
                expected_version: ver(3),
            }
        }
        "TIMEOUT" => Event::Timeout {
            order_id: id,
            request_id: u(2),
        },
        "MARK" => Event::MarkUncertain { order_id: id },
        "REPORT" | "RAW" => {
            let kind = match t[4] {
                "ACCEPT" => ReportKind::Accepted {
                    request_id: u(5),
                    exchange_id: u(6),
                    price: p(7),
                    total_qty: u(8),
                },
                "REPLACED" => ReportKind::Replaced {
                    request_id: u(5),
                    exchange_id: u(6),
                    price: p(7),
                    total_qty: u(8),
                },
                "FILL" => {
                    known.executions.insert(u(5));
                    ReportKind::Fill {
                        key: ExecutionKey {
                            venue: 1,
                            account: 1,
                            trading_day: 20260922,
                            execution_id: u(5),
                        },
                        qty: u(6),
                        price: p(7),
                    }
                }
                "CANCELED" => ReportKind::Canceled {
                    request_id: if u(5) == 0 { None } else { Some(u(5)) },
                },
                "REJECT" => ReportKind::Rejected { request_id: u(5) },
                "REASON" => ReportKind::RejectedWithReason {
                    request_id: u(5),
                    reason: match u(6) {
                        0 => RejectReason::Permanent,
                        1 => RejectReason::Transient,
                        2 => RejectReason::RateLimited,
                        3 => RejectReason::Risk,
                        _ => panic!("reason"),
                    },
                },
                "EXPIRED" => ReportKind::Expired,
                "CORRECT" => {
                    known.executions.insert(u(5));
                    ReportKind::Corrected {
                        key: ExecutionKey {
                            venue: 1,
                            account: 1,
                            trading_day: 20260922,
                            execution_id: u(5),
                        },
                        revision: u(6),
                        new_qty: u(7),
                        new_price: p(8),
                    }
                }
                "RECONCILE" => ReportKind::Reconciled {
                    price: p(5),
                    total_qty: u(6),
                    cum_filled: u(7),
                    leaves: u(8),
                    lifecycle: match u(9) {
                        0 => Lifecycle::PendingNew,
                        1 => Lifecycle::Working,
                        2 => Lifecycle::Filled,
                        3 => Lifecycle::Canceled,
                        4 => Lifecycle::Rejected,
                        5 => Lifecycle::Expired,
                        _ => panic!("lifecycle"),
                    },
                },
                _ => panic!("unknown kind"),
            };
            Event::Report(Report {
                source: u(2),
                sequence: u(3),
                order_id: id,
                kind,
            })
        }
        _ => panic!("unknown operation"),
    }
}
