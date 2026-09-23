pub type OrderId = u64;
pub type RequestId = u64;
pub type Price = i64;
pub type Qty = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Book {
    pub strategy: u64,
    pub account: u64,
    pub venue: u64,
    pub instrument: u64,
    pub side: Side,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    PendingNew,
    Working,
    Filled,
    Canceled,
    Rejected,
    Expired,
}
impl Lifecycle {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Filled | Self::Canceled | Self::Rejected | Self::Expired
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    New,
    Cancel,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestState {
    /// Accepted local intent; no wire request has been committed yet.
    Queued,
    Pending,
    Accepted,
    Rejected,
    Superseded,
    /// No wire change was needed; inspect the order lifecycle for the reason.
    NotNeeded,
    /// The unsent modification became impossible (for example, after fills).
    Unexecutable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub id: RequestId,
    pub order_id: OrderId,
    pub kind: RequestKind,
    pub price: Price,
    /// Total quantity including fills; never a requested remaining quantity.
    pub total_qty: Qty,
    pub state: RequestState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub id: OrderId,
    pub book: Book,
    pub exchange_id: Option<u64>,
    /// Last acknowledged terms, or original requested terms before acceptance.
    pub price: Price,
    pub total_qty: Qty,
    pub cum_filled: Qty,
    pub leaves: Qty,
    pub lifecycle: Lifecycle,
    pub pending: Option<Request>,
    /// Requires external reconciliation before dispatch. New local intents may
    /// still be accepted without changing confirmed terms or reservations.
    pub uncertain: bool,
    pub version: u64,
}

impl Order {
    /// Gross potential remaining quantity for this physical order. Pending
    /// atomic replace is max(old, new); cancel never releases a reservation.
    pub fn reserved_qty(&self) -> Qty {
        let mut qty = if self.lifecycle == Lifecycle::PendingNew {
            self.total_qty.saturating_sub(self.cum_filled)
        } else {
            self.leaves
        };
        if self.uncertain {
            qty = qty.max(self.total_qty.saturating_sub(self.cum_filled));
        }
        if let Some(p) = self.pending
            && matches!(p.kind, RequestKind::New | RequestKind::Replace)
        {
            qty = qty.max(p.total_qty.saturating_sub(self.cum_filled));
        }
        qty
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExecutionKey {
    pub venue: u64,
    pub account: u64,
    pub trading_day: u64,
    pub execution_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Execution {
    pub key: ExecutionKey,
    pub order_id: OrderId,
    /// Keep the original terms to identify delayed duplicate trade reports
    /// even after the execution has been corrected.
    pub original_qty: Qty,
    pub original_price: Price,
    pub qty: Qty,
    pub price: Price,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewOrder {
    pub order_id: OrderId,
    pub request_id: RequestId,
    pub book: Book,
    pub price: Price,
    pub total_qty: Qty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    Accepted {
        request_id: RequestId,
        exchange_id: u64,
        price: Price,
        total_qty: Qty,
    },
    Fill {
        key: ExecutionKey,
        qty: Qty,
        price: Price,
    },
    Replaced {
        request_id: RequestId,
        exchange_id: u64,
        price: Price,
        total_qty: Qty,
    },
    /// Whole-order cancel. None denotes an unsolicited venue cancellation.
    Canceled {
        request_id: Option<RequestId>,
    },
    Rejected {
        request_id: RequestId,
    },
    RejectedWithReason {
        request_id: RequestId,
        reason: crate::RejectReason,
    },
    Expired,
    /// Revision starts at 1 and increases by exactly one for this execution.
    /// new_qty == 0 is a trade bust. Adapter must normalize venue semantics.
    Corrected {
        key: ExecutionKey,
        revision: u64,
        new_qty: Qty,
        new_price: Price,
    },
    /// Authoritative order snapshot AFTER missing executions have been replayed.
    /// Does not silently create fills or resolve an outstanding request.
    Reconciled {
        price: Price,
        total_qty: Qty,
        cum_filled: Qty,
        leaves: Qty,
        lifecycle: Lifecycle,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// Collision-free identity for a normalized ordered stream, including its
    /// session epoch. Starts at sequence 1, independent from raw FIX MsgSeqNum.
    pub source: u64,
    pub sequence: u64,
    pub order_id: OrderId,
    pub kind: ReportKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    New(NewOrder),
    Cancel {
        order_id: OrderId,
        request_id: RequestId,
        expected_version: u64,
    },
    Replace {
        order_id: OrderId,
        request_id: RequestId,
        expected_version: u64,
        price: Price,
        total_qty: Qty,
    },
    Timeout {
        order_id: OrderId,
        request_id: RequestId,
    },
    /// Persisted uncertainty boundary for a disconnect, source gap or restart.
    MarkUncertain {
        order_id: OrderId,
    },
    Report(Report),
    Group(crate::GroupEvent),
    /// Internal journal records. Submit New/Replace/Cancel through apply instead.
    Single(crate::SingleEvent),
}

impl Event {
    pub fn order_id(self) -> OrderId {
        match self {
            Self::New(n) => n.order_id,
            Self::Cancel { order_id, .. }
            | Self::Replace { order_id, .. }
            | Self::Timeout { order_id, .. }
            | Self::MarkUncertain { order_id } => order_id,
            Self::Report(r) => r.order_id,
            Self::Group(crate::GroupEvent::Dispatch(d)) => d.command.order_id(),
            Self::Group(_) => 0,
            Self::Single(s) => s.order_id(),
        }
    }
}
