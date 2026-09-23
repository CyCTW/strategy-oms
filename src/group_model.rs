//! Strategy intent types. One group has one book and price; a ladder uses
//! several groups. Targets are state updates, not a queue of trading commands.
use crate::model::*;

pub type GroupId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantityMode {
    /// Deliberately replenish executions to maintain this much open quantity.
    MaintainLeaves,
    /// Lifetime execution budget across ALL children, including closed orders.
    TotalExecution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub price: Price,
    pub qty: Qty,
    pub quantity_mode: QuantityMode,
    /// Absolute milliseconds in the caller's clock domain. None is explicitly
    /// persistent until replaced/stopped. Expiry drains existing orders.
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupPolicy {
    /// Maximum projected remaining quantity per child (not lifetime fills).
    pub max_child_leaves: Qty,
    pub max_active_children: u32,
    pub max_inflight: u32,
    pub max_reserved_qty: Qty,
    pub max_retries: u32,
    pub retry_delay_ms: u64,
    pub request_timeout_ms: u64,
}
impl Default for GroupPolicy {
    fn default() -> Self {
        Self {
            max_child_leaves: 10,
            max_active_children: 16,
            max_inflight: 4,
            max_reserved_qty: 160,
            max_retries: 2,
            retry_delay_ms: 100,
            request_timeout_ms: 5_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupControl {
    Pause,
    Resume,
    Stop,
    Retry,
    /// Release a restart/source hold after reconciliation, preserving mode.
    ReleaseRecovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMode {
    Running,
    Paused,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    Permanent,
    Transient,
    RateLimited,
    Risk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerConfig {
    /// One fixed window shared by every managed group in this Engine.
    pub window_ms: u64,
    pub max_actions: u32,
    /// Normal operations may use at most max_actions - cancel_reserve.
    pub cancel_reserve: u32,
}
impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            window_ms: 1000,
            max_actions: 100,
            cancel_reserve: 10,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildCommand {
    New {
        order_id: OrderId,
        request_id: RequestId,
        price: Price,
        total_qty: Qty,
    },
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
}
impl ChildCommand {
    pub fn order_id(self) -> OrderId {
        match self {
            Self::New { order_id, .. }
            | Self::Cancel { order_id, .. }
            | Self::Replace { order_id, .. } => order_id,
        }
    }
    pub fn request_id(self) -> RequestId {
        match self {
            Self::New { request_id, .. }
            | Self::Cancel { request_id, .. }
            | Self::Replace { request_id, .. } => request_id,
        }
    }
    pub(crate) fn event(self, book: Book) -> Event {
        match self {
            Self::New {
                order_id,
                request_id,
                price,
                total_qty,
            } => Event::New(NewOrder {
                order_id,
                request_id,
                book,
                price,
                total_qty,
            }),
            Self::Cancel {
                order_id,
                request_id,
                expected_version,
            } => Event::Cancel {
                order_id,
                request_id,
                expected_version,
            },
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupDispatch {
    pub group_id: GroupId,
    pub target_revision: u64,
    pub command: ChildCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupEvent {
    Create {
        group_id: GroupId,
        book: Book,
        policy: GroupPolicy,
    },
    SetTarget {
        group_id: GroupId,
        revision: u64,
        target: Option<Target>,
    },
    Control {
        group_id: GroupId,
        control: GroupControl,
    },
    Clock {
        now_ms: u64,
    },
    ConfigureScheduler(SchedulerConfig),
    Dispatch(GroupDispatch),
    RecoveryHold {
        group_id: GroupId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    Paused,
    Recovering,
    Uncertain,
    Pending,
    Rejected(RejectReason),
    RetryExhausted,
    RetryAt(u64),
    RateLimitUntil(u64),
    RiskLimit,
    ChildCapacity,
    StorageCapacity,
    IdExhausted,
    Halted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupStatus {
    NoTarget,
    Converged,
    Completed,
    Expired,
    Stopped,
    Ready,
    Blocked(BlockReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupView {
    pub group_id: GroupId,
    pub revision: u64,
    pub target: Option<Target>,
    pub mode: GroupMode,
    pub recovery_hold: bool,
    pub required_leaves: Qty,
    pub confirmed_leaves: Qty,
    /// Assumes every pending request succeeds; NOT an exchange confirmation.
    pub projected_leaves: Qty,
    pub reserved_qty: Qty,
    pub cum_filled: Qty,
    pub active_children: usize,
    pub inflight: usize,
    pub uncertain_children: usize,
    pub status: GroupStatus,
    pub next_action: Option<ChildCommand>,
    pub last_rejection: Option<RejectReason>,
    pub retry_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetReceipt {
    pub group_id: GroupId,
    pub revision: u64,
    pub duplicate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dispatch {
    pub group_id: GroupId,
    pub target_revision: u64,
    pub order_id: OrderId,
    pub request_id: RequestId,
    /// Send immediately. This is an attempted dispatch boundary, not a venue ACK.
    pub command: crate::gateway::GatewayCommand,
}
