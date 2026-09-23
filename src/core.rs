use crate::{
    BookHandle, GroupDispatch, GroupEvent, GroupId, IndexBackend, IndexStats, LevelSummary,
    OrderIds, PriceLevel, SingleAction, SingleEvent, gateway::GatewayCommand, groups::Groups,
    index::Index, journal::Journal, model::*, order_store::OrderStore, single::Singles,
};
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::ops::RangeInclusive;

const MAX_QTY: Qty = 1_000_000_000_000;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_orders: usize,
    pub max_requests: usize,
    pub max_executions: usize,
    pub max_reports: usize,
    pub max_order_qty: Qty,
    /// Gross outstanding quantity for ONE strategy/account/venue/product/side.
    /// This is not an account position or notional risk limit.
    pub max_open_qty_per_book: Qty,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_orders: 10_000,
            max_requests: 40_000,
            max_executions: 100_000,
            max_reports: 200_000,
            max_order_qty: 1_000_000,
            max_open_qty_per_book: 10_000_000,
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Invalid(&'static str),
    UnknownOrder(OrderId),
    UnknownExecution,
    DuplicateId,
    ConflictingDuplicate,
    Capacity(&'static str),
    StaleVersion {
        expected: u64,
        actual: u64,
    },
    SequenceGap {
        source: u64,
        expected: u64,
        actual: u64,
    },
    PendingRequest,
    RequestMismatch,
    NeedsReconciliation,
    RiskLimit,
    Journal(io::Error),
    Halted,
    UnknownGroup(GroupId),
    StaleTarget {
        current: u64,
        received: u64,
    },
    ManagedOrder,
    InvalidDispatch,
    CancelRequested,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub group_id: Option<GroupId>,
    /// Zero for group administration events; group APIs return typed receipts.
    pub order_id: OrderId,
    pub version: u64,
    /// Latest single-order intent revision; acceptance is not a venue ACK.
    pub intent_revision: Option<u64>,
    /// Either exact report retransmission, or a business-level duplicate.
    pub duplicate: bool,
    /// A committed wire command to deliver immediately. Accepted queued/no-op
    /// intents have None. Never populated during recovery.
    pub outbound: Option<GatewayCommand>,
}

struct Transition {
    old: Option<Order>,
    order: Order,
    request: Option<Request>,
    result: Option<(RequestId, ReportKind)>,
    execution: Option<Execution>,
    outbound: Option<GatewayCommand>,
    duplicate: bool,
}

/// Own this on one event-loop thread. `&mut self` serializes updates; immutable
/// borrows cannot outlive a subsequent mutation. Publish copies for other threads.
pub struct Engine<J: Journal> {
    journal: J,
    pub(crate) limits: Limits,
    pub(crate) orders: OrderStore,
    pub(crate) requests: HashMap<RequestId, Request>,
    request_results: HashMap<RequestId, ReportKind>,
    executions: HashMap<ExecutionKey, Execution>,
    reports: HashMap<(u64, u64), Report>,
    sequences: HashMap<u64, u64>,
    index: Index,
    pub(crate) halted: bool,
    pub(crate) groups: Groups,
    pub(crate) singles: Singles,
    pub(crate) order_high_water: u64,
    pub(crate) request_high_water: u64,
}

impl<J: Journal> Engine<J> {
    pub fn new(journal: J, limits: Limits) -> Result<Self, Error> {
        Self::new_with_index(journal, limits, IndexBackend::default())
    }

    /// Select an index implementation without changing order or WAL semantics.
    pub fn new_with_index(
        journal: J,
        limits: Limits,
        backend: IndexBackend,
    ) -> Result<Self, Error> {
        if limits.max_orders == 0
            || limits.max_orders > 1_000_000
            || limits.max_order_qty == 0
            || limits.max_order_qty > MAX_QTY
        {
            return Err(Error::Invalid("limits outside supported numeric bounds"));
        }
        Ok(Self {
            journal,
            limits,
            orders: OrderStore::new(),
            requests: HashMap::with_capacity(limits.max_requests),
            request_results: HashMap::with_capacity(limits.max_requests),
            executions: HashMap::with_capacity(limits.max_executions),
            reports: HashMap::with_capacity(limits.max_reports),
            sequences: HashMap::new(),
            index: Index::new(backend),
            halted: false,
            groups: Groups::default(),
            singles: Singles::default(),
            order_high_water: 0,
            request_high_water: 0,
        })
    }

    /// Rebuild without appending to the already-loaded journal or dispatching.
    /// Outstanding orders are quarantined until authoritative reconciliation.
    pub fn recover(journal: J, events: &[Event], limits: Limits) -> Result<Self, Error> {
        Self::recover_with_index(journal, events, limits, IndexBackend::default())
    }

    pub fn recover_with_index(
        journal: J,
        events: &[Event],
        limits: Limits,
        backend: IndexBackend,
    ) -> Result<Self, Error> {
        let mut engine = Self::new_with_index(journal, limits, backend)?;
        for &event in events {
            engine.apply_inner(event, false)?;
        }
        let mut ids: Vec<_> = engine
            .orders
            .values()
            .filter(|o| !o.lifecycle.terminal() || o.pending.is_some())
            .map(|o| o.id)
            .collect();
        ids.sort_unstable();
        // Persist recovery boundaries: otherwise a later replay would miss
        // uncertainty/version changes that influenced subsequent commands.
        for order_id in ids {
            engine.apply_inner(Event::MarkUncertain { order_id }, true)?;
        }
        let group_ids: Vec<_> = engine.groups.entries.keys().copied().collect();
        for group_id in group_ids {
            engine.apply_inner(Event::Group(GroupEvent::RecoveryHold { group_id }), true)?;
        }
        Ok(engine)
    }

    pub fn order(&self, id: OrderId) -> Option<&Order> {
        self.orders.get(&id)
    }
    pub fn request(&self, id: RequestId) -> Option<&Request> {
        self.requests.get(&id)
    }
    pub fn execution(&self, key: &ExecutionKey) -> Option<&Execution> {
        self.executions.get(key)
    }
    pub fn journal(&self) -> &J {
        &self.journal
    }
    pub fn halted(&self) -> bool {
        self.halted
    }
    pub fn last_sequence(&self, source: u64) -> u64 {
        self.sequences.get(&source).copied().unwrap_or(0)
    }
    pub fn orders(&self) -> impl Iterator<Item = &Order> {
        self.orders.values()
    }
    pub fn at_price(&self, book: Book, price: Price) -> Option<PriceLevel<'_>> {
        self.index.at_handle(self.index.book_handle(book)?, price)
    }

    /// Resolve a process-local handle, optionally before the first order.
    /// This derived-index operation is not journaled. Re-resolve after recovery.
    pub fn register_book(&mut self, book: Book) -> BookHandle {
        self.index.register_book(book)
    }
    pub fn book_handle(&self, book: Book) -> Option<BookHandle> {
        self.index.book_handle(book)
    }
    /// Fixed-size summary; no membership traversal. Foreign handles return None.
    pub fn level_summary(&self, book: BookHandle, price: Price) -> Option<LevelSummary> {
        self.index.summary(book, price)
    }
    /// Allocation-free iterator of ID values in unspecified order.
    pub fn orders_at_price(&self, book: BookHandle, price: Price) -> Option<OrderIds<'_>> {
        Some(self.index.at_handle(book, price)?.order_ids)
    }
    pub fn best_working_price_at(&self, book: BookHandle) -> Option<Price> {
        self.index.best(book)
    }
    pub fn index_stats(&self) -> IndexStats {
        self.index.stats()
    }
    pub fn price_range(
        &self,
        book: Book,
        range: RangeInclusive<Price>,
    ) -> impl Iterator<Item = (Price, PriceLevel<'_>)> {
        self.index.range(self.index.book_handle(book), range)
    }
    pub fn levels_in_range(
        &self,
        book: BookHandle,
        range: RangeInclusive<Price>,
    ) -> impl Iterator<Item = (Price, PriceLevel<'_>)> {
        self.index.range(Some(book), range)
    }
    pub fn best_working_price(&self, book: Book) -> Option<Price> {
        self.index.best(self.index.book_handle(book)?)
    }
    pub fn reserved_qty(&self, book: Book) -> Qty {
        self.index.reserved(self.index.book_handle(book))
    }
    pub fn uncertain_orders(&self, book: Book) -> u64 {
        self.index.uncertain(self.index.book_handle(book))
    }

    /// New/Replace/Cancel accept latest single-order intent. An in-flight
    /// request defers changes instead of rejecting them; drain ready changes
    /// through dispatch_next_order after inputs. Use on_report for guarded ingress.
    pub fn apply(&mut self, event: Event) -> Result<Outcome, Error> {
        if let Some(action) = SingleAction::from_event(event) {
            let dispatch = self.validate_single_submit(action)?;
            return self.apply_single_event(SingleEvent::Submit { action, dispatch }, true);
        }
        if matches!(event, Event::Single(_)) {
            return Err(Error::Invalid("single journal records are internal"));
        }
        self.apply_inner(event, true)
    }

    pub(crate) fn apply_inner(&mut self, event: Event, persist: bool) -> Result<Outcome, Error> {
        if let Event::Group(group_event) = event {
            return self.apply_group_event(group_event, persist);
        }
        if let Event::Single(single_event) = event {
            return self.apply_single_event(single_event, persist);
        }
        self.apply_order_event(event, persist, None)
    }

    pub(crate) fn persist_event(&mut self, event: &Event, persist: bool) -> Result<(), Error> {
        if self.halted {
            return Err(Error::Halted);
        }
        if persist && let Err(err) = self.journal.append(event) {
            self.halted = true;
            return Err(Error::Journal(err));
        }
        Ok(())
    }

    pub(crate) fn validate_order_event(&self, event: Event) -> Result<(), Error> {
        self.prepare(event).map(|_| ())
    }

    pub(crate) fn validate_queued_order_event(&self, event: Event) -> Result<(), Error> {
        self.prepare_with_queued(event, true).map(|_| ())
    }

    pub(crate) fn apply_order_event(
        &mut self,
        event: Event,
        persist: bool,
        managed: Option<GroupDispatch>,
    ) -> Result<Outcome, Error> {
        self.apply_order_record(event, persist, managed, None, false)
    }

    pub(crate) fn apply_order_record(
        &mut self,
        event: Event,
        persist: bool,
        managed: Option<GroupDispatch>,
        single: Option<SingleEvent>,
        queued: bool,
    ) -> Result<Outcome, Error> {
        if self.halted {
            return Err(Error::Halted);
        }
        if managed.is_none()
            && self.groups.owners.contains_key(&event.order_id())
            && matches!(event, Event::Cancel { .. } | Event::Replace { .. })
        {
            return Err(Error::ManagedOrder);
        }
        if let Event::Report(r) = event {
            if let Some(previous) = self.reports.get(&(r.source, r.sequence)) {
                if *previous != r {
                    return Err(Error::ConflictingDuplicate);
                }
                return Ok(Outcome {
                    group_id: self.groups.owners.get(&r.order_id).copied(),
                    order_id: r.order_id,
                    version: self.orders[&r.order_id].version,
                    intent_revision: self.singles.intents.get(&r.order_id).map(|i| i.revision),
                    duplicate: true,
                    outbound: None,
                });
            }
            let expected = self
                .last_sequence(r.source)
                .checked_add(1)
                .ok_or(Error::Invalid("sequence overflow"))?;
            if r.sequence != expected {
                return Err(Error::SequenceGap {
                    source: r.source,
                    expected,
                    actual: r.sequence,
                });
            }
            if self.reports.len() >= self.limits.max_reports {
                return Err(Error::Capacity("reports"));
            }
        }
        let mut t = self.prepare_with_queued(event, queued)?;
        let changed = t.old != Some(t.order);
        if changed {
            t.order.version = t
                .order
                .version
                .checked_add(1)
                .ok_or(Error::Invalid("version overflow"))?;
        }
        // No externally fallible state transition is allowed after the append.
        // An I/O error could mean a partial write; latch a halt until recovery.
        let logged = single
            .map(Event::Single)
            .unwrap_or_else(|| managed.map_or(event, |d| Event::Group(GroupEvent::Dispatch(d))));
        self.persist_event(&logged, persist)?;
        if changed {
            let entry = self.orders.insert(t.order.id, t.order);
            self.index
                .update(t.old.as_ref(), &t.order, &mut entry.memberships);
        }
        if let Some(r) = t.request {
            self.requests.insert(r.id, r);
            self.request_high_water = self.request_high_water.max(r.id);
        }
        if let Some((id, result)) = t.result {
            self.request_results.insert(id, result);
        }
        if let Some(x) = t.execution {
            self.executions.insert(x.key, x);
        }
        if let Event::Report(r) = event {
            self.reports.insert((r.source, r.sequence), r);
            self.sequences.insert(r.source, r.sequence);
        }
        self.order_high_water = self.order_high_water.max(t.order.id);
        if let Some(d) = managed {
            self.commit_group_dispatch(d);
        }
        if !t.duplicate {
            self.group_order_updated(event, t.old, t.order);
            self.settle_single_intent(t.order.id);
        }
        Ok(Outcome {
            group_id: self.groups.owners.get(&t.order.id).copied(),
            order_id: t.order.id,
            version: t.order.version,
            intent_revision: self.singles.intents.get(&t.order.id).map(|i| i.revision),
            duplicate: t.duplicate,
            outbound: if persist { t.outbound } else { None },
        })
    }

    fn prepare(&self, event: Event) -> Result<Transition, Error> {
        self.prepare_with_queued(event, false)
    }

    fn prepare_with_queued(&self, event: Event, queued: bool) -> Result<Transition, Error> {
        if let Event::New(n) = event {
            if n.order_id == 0 || n.request_id == 0 {
                return Err(Error::Invalid("zero ID"));
            }
            if self.orders.contains_key(&n.order_id) {
                return Err(Error::DuplicateId);
            }
            self.new_request_id(n.request_id)?;
            if self.orders.len() >= self.limits.max_orders {
                return Err(Error::Capacity("orders"));
            }
            self.command_qty(n.total_qty)?;
            if self.uncertain_orders(n.book) > 0 {
                return Err(Error::NeedsReconciliation);
            }
            if self
                .reserved_qty(n.book)
                .checked_add(n.total_qty)
                .ok_or(Error::RiskLimit)?
                > self.limits.max_open_qty_per_book
            {
                return Err(Error::RiskLimit);
            }
            let r = Request {
                id: n.request_id,
                order_id: n.order_id,
                kind: RequestKind::New,
                price: n.price,
                total_qty: n.total_qty,
                state: RequestState::Pending,
            };
            let o = Order {
                id: n.order_id,
                book: n.book,
                exchange_id: None,
                price: n.price,
                total_qty: n.total_qty,
                cum_filled: 0,
                leaves: 0,
                lifecycle: Lifecycle::PendingNew,
                pending: Some(r),
                uncertain: false,
                version: 0,
            };
            return Ok(Transition {
                old: None,
                order: o,
                request: Some(r),
                result: None,
                execution: None,
                outbound: Some(GatewayCommand::New(n)),
                duplicate: false,
            });
        }
        let old = *self
            .orders
            .get(&event.order_id())
            .ok_or(Error::UnknownOrder(event.order_id()))?;
        let mut t = Transition {
            old: Some(old),
            order: old,
            request: None,
            result: None,
            execution: None,
            outbound: None,
            duplicate: false,
        };
        match event {
            Event::New(_) => unreachable!(),
            Event::Group(_) => unreachable!("group events use the group reducer"),
            Event::Single(_) => unreachable!("single events use the intent reducer"),
            Event::Cancel {
                order_id,
                request_id,
                expected_version,
            }
            | Event::Replace {
                order_id,
                request_id,
                expected_version,
                ..
            } => {
                if queued {
                    let r = self
                        .requests
                        .get(&request_id)
                        .ok_or(Error::RequestMismatch)?;
                    if r.order_id != order_id || r.state != RequestState::Queued {
                        return Err(Error::RequestMismatch);
                    }
                } else {
                    self.new_request_id(request_id)?;
                }
                if old.version != expected_version {
                    return Err(Error::StaleVersion {
                        expected: expected_version,
                        actual: old.version,
                    });
                }
                if old.uncertain {
                    return Err(Error::NeedsReconciliation);
                }
                if old.pending.is_some() {
                    return Err(Error::PendingRequest);
                }
                if old.lifecycle != Lifecycle::Working {
                    return Err(Error::Invalid("order is not working"));
                }
                let (kind, price, total_qty) = if let Event::Replace {
                    price, total_qty, ..
                } = event
                {
                    self.command_qty(total_qty)?;
                    if total_qty <= old.cum_filled {
                        return Err(Error::Invalid("replace quantity must exceed known fills"));
                    }
                    (RequestKind::Replace, price, total_qty)
                } else {
                    (RequestKind::Cancel, old.price, old.total_qty)
                };
                let r = Request {
                    id: request_id,
                    order_id,
                    kind,
                    price,
                    total_qty,
                    state: RequestState::Pending,
                };
                t.order.pending = Some(r);
                let reserved =
                    self.reserved_qty(old.book) - old.reserved_qty() + t.order.reserved_qty();
                // Never block a cancel or non-increasing replace due to a limit
                // that an authoritative report has already exceeded.
                if t.order.reserved_qty() > old.reserved_qty()
                    && (self.uncertain_orders(old.book) > 0
                        || reserved > self.limits.max_open_qty_per_book)
                {
                    return Err(Error::RiskLimit);
                }
                t.request = Some(r);
                t.outbound = Some(GatewayCommand::Change {
                    order_id,
                    exchange_id: old.exchange_id,
                    request: r,
                });
            }
            Event::Timeout { request_id, .. } => {
                if old.pending.map(|r| r.id) != Some(request_id) {
                    return Err(Error::RequestMismatch);
                }
                t.order.uncertain = true;
            }
            Event::MarkUncertain { .. } => t.order.uncertain = true,
            Event::Report(report) => self.reduce_report(&mut t, report.kind)?,
        }
        Ok(t)
    }

    pub(crate) fn new_request_id(&self, id: RequestId) -> Result<(), Error> {
        if id == 0 {
            return Err(Error::Invalid("zero request ID"));
        }
        if self.requests.contains_key(&id) {
            return Err(Error::DuplicateId);
        }
        if self.requests.len() >= self.limits.max_requests {
            return Err(Error::Capacity("requests"));
        }
        Ok(())
    }
    pub(crate) fn command_qty(&self, qty: Qty) -> Result<(), Error> {
        Self::valid_qty(qty)?;
        if qty > self.limits.max_order_qty {
            return Err(Error::RiskLimit);
        }
        Ok(())
    }
    fn valid_qty(qty: Qty) -> Result<(), Error> {
        if qty == 0 || qty > MAX_QTY {
            return Err(Error::Invalid("quantity outside numeric bounds"));
        }
        Ok(())
    }
    fn finish_request(
        t: &mut Transition,
        id: RequestId,
        kind: Option<RequestKind>,
        state: RequestState,
        result: ReportKind,
    ) -> Result<(), Error> {
        let mut r = t.order.pending.ok_or(Error::RequestMismatch)?;
        if r.id != id || kind.is_some_and(|k| r.kind != k) {
            return Err(Error::RequestMismatch);
        }
        r.state = state;
        t.request = Some(r);
        t.result = Some((id, result));
        t.order.pending = None;
        // Uncertainty is deliberately NOT cleared here: it may originate from
        // recovery or a trade correction, not just this request's timeout.
        Ok(())
    }

    fn reduce_report(&self, t: &mut Transition, kind: ReportKind) -> Result<(), Error> {
        let request_id = match kind {
            ReportKind::Accepted { request_id, .. }
            | ReportKind::Replaced { request_id, .. }
            | ReportKind::Rejected { request_id }
            | ReportKind::RejectedWithReason { request_id, .. } => Some(request_id),
            ReportKind::Canceled { request_id } => request_id,
            _ => None,
        };
        if let Some(id) = request_id
            && let Some(previous) = self.request_results.get(&id)
        {
            if self.requests[&id].order_id != t.order.id || *previous != kind {
                return Err(Error::ConflictingDuplicate);
            }
            t.duplicate = true;
            return Ok(());
        }
        match kind {
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
                Self::valid_qty(total_qty)?;
                if total_qty < t.order.cum_filled {
                    return Err(Error::Invalid("accepted total below known fills"));
                }
                let req_kind = if matches!(kind, ReportKind::Accepted { .. }) {
                    RequestKind::New
                } else {
                    RequestKind::Replace
                };
                Self::finish_request(t, request_id, Some(req_kind), RequestState::Accepted, kind)?;
                t.order.exchange_id = Some(exchange_id);
                t.order.price = price;
                t.order.total_qty = total_qty;
                t.order.leaves = total_qty - t.order.cum_filled;
                t.order.lifecycle = if t.order.leaves == 0 {
                    Lifecycle::Filled
                } else {
                    Lifecycle::Working
                };
            }
            ReportKind::Fill { key, qty, price } => {
                self.check_key(&t.order, key)?;
                Self::valid_qty(qty)?;
                if let Some(x) = self.executions.get(&key) {
                    if x.order_id != t.order.id
                        || x.original_qty != qty
                        || x.original_price != price
                    {
                        return Err(Error::ConflictingDuplicate);
                    }
                    t.duplicate = true;
                    return Ok(());
                }
                if self.executions.len() >= self.limits.max_executions {
                    return Err(Error::Capacity("executions"));
                }
                if t.order.lifecycle == Lifecycle::Rejected {
                    return Err(Error::Invalid(
                        "fill on rejected order requires reconciliation",
                    ));
                }
                let cum = t
                    .order
                    .cum_filled
                    .checked_add(qty)
                    .ok_or(Error::Invalid("fill overflow"))?;
                if cum > t.order.total_qty {
                    return Err(Error::Invalid(
                        "overfill: missing replace or inconsistent report",
                    ));
                }
                t.order.cum_filled = cum;
                if t.order.lifecycle == Lifecycle::Working {
                    t.order.leaves = t.order.total_qty - cum;
                    if t.order.leaves == 0 {
                        t.order.lifecycle = Lifecycle::Filled;
                    }
                } else if t.order.lifecycle == Lifecycle::PendingNew && cum == t.order.total_qty {
                    t.order.lifecycle = Lifecycle::Filled;
                }
                t.execution = Some(Execution {
                    key,
                    order_id: t.order.id,
                    original_qty: qty,
                    original_price: price,
                    qty,
                    price,
                    revision: 0,
                });
            }
            ReportKind::Canceled { request_id } => {
                if let Some(id) = request_id {
                    Self::finish_request(
                        t,
                        id,
                        Some(RequestKind::Cancel),
                        RequestState::Accepted,
                        kind,
                    )?;
                } else if let Some(r) = t.order.pending {
                    Self::finish_request(t, r.id, None, RequestState::Superseded, kind)?;
                }
                t.order.leaves = 0;
                t.order.lifecycle = Lifecycle::Canceled;
            }
            ReportKind::Rejected { request_id }
            | ReportKind::RejectedWithReason { request_id, .. } => {
                let req_kind = t.order.pending.ok_or(Error::RequestMismatch)?.kind;
                if req_kind == RequestKind::New {
                    if t.order.cum_filled != 0 {
                        return Err(Error::Invalid("new rejection after fills"));
                    }
                    t.order.lifecycle = Lifecycle::Rejected;
                    t.order.leaves = 0;
                }
                Self::finish_request(t, request_id, None, RequestState::Rejected, kind)?;
            }
            ReportKind::Expired => {
                if let Some(r) = t.order.pending {
                    Self::finish_request(t, r.id, None, RequestState::Superseded, kind)?;
                }
                t.order.leaves = 0;
                t.order.lifecycle = Lifecycle::Expired;
            }
            ReportKind::Corrected {
                key,
                revision,
                new_qty,
                new_price,
            } => {
                self.check_key(&t.order, key)?;
                let mut x = *self.executions.get(&key).ok_or(Error::UnknownExecution)?;
                if x.order_id != t.order.id {
                    return Err(Error::ConflictingDuplicate);
                }
                if revision != 0
                    && revision == x.revision
                    && new_qty == x.qty
                    && new_price == x.price
                {
                    t.duplicate = true;
                    return Ok(());
                }
                if revision
                    != x.revision
                        .checked_add(1)
                        .ok_or(Error::Invalid("revision overflow"))?
                {
                    return Err(Error::Invalid("missing or conflicting execution revision"));
                }
                if new_qty > MAX_QTY {
                    return Err(Error::Invalid("correction quantity outside bounds"));
                }
                let cum = (t.order.cum_filled - x.qty)
                    .checked_add(new_qty)
                    .ok_or(Error::Invalid("correction overflow"))?;
                if cum > t.order.total_qty {
                    return Err(Error::Invalid("correction exceeds order total"));
                }
                t.order.cum_filled = cum;
                // Preserve the last known leaves and terminal state. Only an
                // authoritative reconciliation can establish whether a bust
                // reinstated quantity at the venue.
                t.order.uncertain = true;
                x.qty = new_qty;
                x.price = new_price;
                x.revision = revision;
                t.execution = Some(x);
            }
            ReportKind::Reconciled {
                price,
                total_qty,
                cum_filled,
                leaves,
                lifecycle,
            } => {
                Self::valid_qty(total_qty)?;
                if t.order.pending.is_some() {
                    return Err(Error::PendingRequest);
                }
                if cum_filled != t.order.cum_filled {
                    return Err(Error::Invalid(
                        "replay missing/corrected executions before snapshot",
                    ));
                }
                if cum_filled > total_qty || leaves > total_qty - cum_filled {
                    return Err(Error::Invalid("inconsistent snapshot quantities"));
                }
                match lifecycle {
                    Lifecycle::Working if leaves > 0 && leaves == total_qty - cum_filled => {}
                    Lifecycle::Filled if leaves == 0 && cum_filled == total_qty => {}
                    Lifecycle::Canceled | Lifecycle::Expired if leaves == 0 => {}
                    Lifecycle::Rejected if leaves == 0 && cum_filled == 0 => {}
                    _ => return Err(Error::Invalid("inconsistent snapshot state")),
                }
                t.order.price = price;
                t.order.total_qty = total_qty;
                t.order.leaves = leaves;
                t.order.lifecycle = lifecycle;
                t.order.uncertain = false;
            }
        }
        Ok(())
    }
    fn check_key(&self, o: &Order, key: ExecutionKey) -> Result<(), Error> {
        if key.venue != o.book.venue || key.account != o.book.account || key.execution_id == 0 {
            return Err(Error::Invalid("execution scope mismatch"));
        }
        Ok(())
    }
}
