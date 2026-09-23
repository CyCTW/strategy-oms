use crate::model::*;

/// Latest intent for one physical order. Total quantity always includes fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderDesired {
    Working { price: Price, total_qty: Qty },
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderIntent {
    pub order_id: OrderId,
    pub revision: u64,
    pub request_id: RequestId,
    pub desired: OrderDesired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderIntentStatus {
    Ready,
    WaitingForReport,
    NeedsReconciliation,
    RiskBlocked,
    Rejected,
    Unexecutable,
    /// Latest request resolved; compare confirmed terms if a venue adjusted them.
    Resolved,
    Closed(Lifecycle),
    Halted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderIntentView {
    pub intent: OrderIntent,
    pub request_state: RequestState,
    pub status: OrderIntentStatus,
}

/// Journal representation of the strategy command, separate from wire dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleAction {
    New(NewOrder),
    Replace {
        order_id: OrderId,
        request_id: RequestId,
        expected_version: u64,
        price: Price,
        total_qty: Qty,
    },
    Cancel {
        order_id: OrderId,
        request_id: RequestId,
        expected_version: u64,
    },
}

impl SingleAction {
    pub(crate) fn from_event(event: Event) -> Option<Self> {
        match event {
            Event::New(n) => Some(Self::New(n)),
            Event::Replace {
                order_id,
                request_id,
                expected_version,
                price,
                total_qty,
            } => Some(Self::Replace {
                order_id,
                request_id,
                expected_version,
                price,
                total_qty,
            }),
            Event::Cancel {
                order_id,
                request_id,
                expected_version,
            } => Some(Self::Cancel {
                order_id,
                request_id,
                expected_version,
            }),
            _ => None,
        }
    }

    pub(crate) fn event(self) -> Event {
        match self {
            Self::New(n) => Event::New(n),
            Self::Replace {
                order_id,
                request_id,
                expected_version,
                price,
                total_qty,
            } => Event::Replace {
                order_id,
                request_id,
                expected_version,
                price,
                total_qty,
            },
            Self::Cancel {
                order_id,
                request_id,
                expected_version,
            } => Event::Cancel {
                order_id,
                request_id,
                expected_version,
            },
        }
    }

    pub(crate) fn request_id(self) -> RequestId {
        match self {
            Self::New(n) => n.request_id,
            Self::Replace { request_id, .. } | Self::Cancel { request_id, .. } => request_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleEvent {
    /// One atomic WAL record: intent acceptance and optional immediate dispatch.
    Submit {
        action: SingleAction,
        dispatch: bool,
    },
    /// Dispatch of a previously accepted, still-current intent.
    Dispatch {
        order_id: OrderId,
        request_id: RequestId,
        expected_version: u64,
    },
}

impl SingleEvent {
    pub fn order_id(self) -> OrderId {
        match self {
            Self::Submit { action, .. } => action.event().order_id(),
            Self::Dispatch { order_id, .. } => order_id,
        }
    }
}
