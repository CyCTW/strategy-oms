use crate::{journal::Journal, *};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone)]
pub(crate) struct GroupRecord {
    book: Book,
    policy: GroupPolicy,
    target: Option<Target>,
    revision: u64,
    mode: GroupMode,
    recovery_hold: bool,
    children: BTreeSet<OrderId>,
    /// Active, pending, or uncertain children only; history is not scanned by planning.
    tracked: BTreeSet<OrderId>,
    cum_filled: Qty,
    completed_revision: Option<u64>,
    blocked: Option<BlockReason>,
    retry_count: u32,
    last_rejection: Option<RejectReason>,
}

#[derive(Clone, Copy)]
struct RequestContext {
    group_id: GroupId,
    revision: u64,
    dispatched_at_ms: u64,
    target: Option<Target>,
    cancel_for_stop: bool,
}

fn same_terms(a: Option<Target>, b: Option<Target>) -> bool {
    a.map(|t| (t.price, t.qty, t.quantity_mode)) == b.map(|t| (t.price, t.qty, t.quantity_mode))
}

#[derive(Default)]
pub(crate) struct Groups {
    pub entries: BTreeMap<GroupId, GroupRecord>,
    pub owners: HashMap<OrderId, GroupId>,
    contexts: HashMap<RequestId, RequestContext>,
    now_ms: u64,
    scheduler: SchedulerConfig,
    window_id: u64,
    used: u32,
    normal_used: u32,
    last_group: GroupId,
}

#[derive(Default)]
struct Stats {
    confirmed: Qty,
    projected: Qty,
    reserved: Qty,
    active: usize,
    inflight: usize,
    uncertain: usize,
}

struct Plan {
    status: GroupStatus,
    action: Option<ChildCommand>,
    priority: u8,
}
impl Plan {
    fn state(status: GroupStatus) -> Self {
        Self {
            status,
            action: None,
            priority: 3,
        }
    }
    fn blocked(reason: BlockReason) -> Self {
        Self::state(GroupStatus::Blocked(reason))
    }
}

impl<J: Journal> Engine<J> {
    fn group_stats(&self, g: &GroupRecord) -> Stats {
        let mut s = Stats::default();
        for id in &g.tracked {
            let o = &self.orders[id];
            s.confirmed += o.leaves;
            s.reserved += o.reserved_qty();
            s.uncertain += usize::from(o.uncertain);
            s.active += usize::from(!o.lifecycle.terminal() || o.pending.is_some());
            s.inflight += usize::from(o.pending.is_some());
            s.projected += match o.pending {
                Some(p) if p.kind == RequestKind::Cancel => 0,
                Some(p) => p.total_qty.saturating_sub(o.cum_filled),
                None => o.leaves,
            };
        }
        s
    }

    fn expired(&self, g: &GroupRecord) -> bool {
        g.target.is_some_and(|t| {
            t.expires_at_ms
                .is_some_and(|expiry| self.groups.now_ms >= expiry)
        })
    }

    fn required(&self, g: &GroupRecord) -> Qty {
        if g.mode == GroupMode::Stopping
            || self.expired(g)
            || g.completed_revision == Some(g.revision)
        {
            return 0;
        }
        g.target.map_or(0, |t| match t.quantity_mode {
            QuantityMode::MaintainLeaves => t.qty,
            QuantityMode::TotalExecution => t.qty.saturating_sub(g.cum_filled),
        })
    }

    fn plan_group(&self, g: &GroupRecord, s: &Stats) -> Plan {
        if self.halted {
            return Plan::blocked(BlockReason::Halted);
        }
        if g.recovery_hold {
            return Plan::blocked(BlockReason::Recovering);
        }
        if s.uncertain > 0 {
            return Plan::blocked(BlockReason::Uncertain);
        }
        if g.mode == GroupMode::Paused && !self.expired(g) {
            return Plan::blocked(BlockReason::Paused);
        }
        let required = self.required(g);
        if required == 0 && s.active == 0 {
            return Plan::state(if g.mode == GroupMode::Stopping {
                GroupStatus::Stopped
            } else if self.expired(g) {
                GroupStatus::Expired
            } else if g.completed_revision == Some(g.revision) {
                GroupStatus::Completed
            } else if g.revision == 0 {
                GroupStatus::NoTarget
            } else {
                GroupStatus::Converged
            });
        }
        let price = g.target.map_or(0, |t| t.price);
        let all_at_price = g.tracked.iter().all(|id| {
            let o = &self.orders[id];
            o.leaves == 0 || (o.price == price && o.leaves <= g.policy.max_child_leaves)
        });
        if required > 0 && s.inflight == 0 && s.confirmed == required && all_at_price {
            return Plan::state(GroupStatus::Converged);
        }
        if let Some(blocked) = g.blocked {
            match blocked {
                BlockReason::RetryAt(at) if self.groups.now_ms >= at => {}
                _ => return Plan::blocked(blocked),
            }
        }
        if s.inflight >= g.policy.max_inflight as usize {
            return Plan::blocked(BlockReason::Pending);
        }
        let Some(request_id) = self.request_high_water.checked_add(1) else {
            return Plan::blocked(BlockReason::IdExhausted);
        };
        let idle = |o: &&Order| o.lifecycle == Lifecycle::Working && o.pending.is_none();
        let orders = || {
            g.tracked
                .iter()
                .rev()
                .map(|id| &self.orders[id])
                .filter(idle)
        };
        let cancel = |o: &Order| ChildCommand::Cancel {
            order_id: o.id,
            request_id,
            expected_version: o.version,
        };
        let replace = |o: &Order, leaves: Qty| ChildCommand::Replace {
            order_id: o.id,
            request_id,
            expected_version: o.version,
            price,
            total_qty: o.cum_filled + leaves,
        };

        // Never withdraw a sent request; drain idle children and wait for others.
        if required == 0 {
            if let Some(o) = orders().next() {
                return self.check_action(g, s, cancel(o), 0);
            }
            return Plan::blocked(BlockReason::Pending);
        }
        // Preserve older children when reducing. Pending-success projection
        // prevents issuing the same reduction to a second child.
        if s.projected > required {
            let excess = s.projected - required;
            if let Some(o) = orders().next() {
                if o.leaves <= excess {
                    return self.check_action(g, s, cancel(o), 0);
                }
                return self.check_action(g, s, replace(o, o.leaves - excess), 1);
            }
            return Plan::blocked(BlockReason::Pending);
        }
        // First repair oversized or incorrectly-priced resting children without
        // increasing quantity. Venue-reported terms remain authoritative.
        for o in orders() {
            if o.leaves > g.policy.max_child_leaves {
                return self.check_action(g, s, replace(o, g.policy.max_child_leaves), 1);
            }
            if o.price != price {
                return self.check_action(g, s, replace(o, o.leaves), 1);
            }
        }
        if s.projected < required {
            let missing = required - s.projected;
            let headroom = g.policy.max_reserved_qty.saturating_sub(s.reserved).min(
                self.limits
                    .max_open_qty_per_book
                    .saturating_sub(self.reserved_qty(g.book)),
            );
            let addition = missing.min(headroom);
            if addition == 0 {
                return Plan::blocked(BlockReason::RiskLimit);
            }
            for o in orders() {
                let capacity = g
                    .policy
                    .max_child_leaves
                    .min(self.limits.max_order_qty.saturating_sub(o.cum_filled));
                if capacity > o.leaves {
                    return self.check_action(
                        g,
                        s,
                        replace(o, o.leaves + addition.min(capacity - o.leaves)),
                        2,
                    );
                }
            }
            if s.active >= g.policy.max_active_children as usize {
                return Plan::blocked(if s.inflight > 0 {
                    BlockReason::Pending
                } else {
                    BlockReason::ChildCapacity
                });
            }
            let Some(order_id) = self.order_high_water.checked_add(1) else {
                return Plan::blocked(BlockReason::IdExhausted);
            };
            return self.check_action(
                g,
                s,
                ChildCommand::New {
                    order_id,
                    request_id,
                    price,
                    total_qty: addition.min(g.policy.max_child_leaves),
                },
                2,
            );
        }
        Plan::blocked(BlockReason::Pending)
    }

    fn check_action(
        &self,
        g: &GroupRecord,
        s: &Stats,
        command: ChildCommand,
        priority: u8,
    ) -> Plan {
        let extra = match command {
            ChildCommand::New { total_qty, .. } => total_qty,
            ChildCommand::Cancel { .. } => 0,
            ChildCommand::Replace {
                order_id,
                total_qty,
                ..
            } => {
                let o = &self.orders[&order_id];
                total_qty
                    .saturating_sub(o.cum_filled)
                    .saturating_sub(o.reserved_qty())
            }
        };
        if extra > 0
            && (s.reserved.saturating_add(extra) > g.policy.max_reserved_qty
                || self.uncertain_orders(g.book) > 0)
        {
            return Plan {
                status: GroupStatus::Blocked(BlockReason::RiskLimit),
                action: Some(command),
                priority,
            };
        }
        if let Err(error) = self.validate_order_event(command.event(g.book)) {
            let why = match error {
                Error::RiskLimit => BlockReason::RiskLimit,
                Error::NeedsReconciliation => BlockReason::Uncertain,
                Error::Capacity(_) => BlockReason::StorageCapacity,
                Error::PendingRequest => BlockReason::Pending,
                _ => BlockReason::StorageCapacity,
            };
            return Plan {
                status: GroupStatus::Blocked(why),
                action: Some(command),
                priority,
            };
        }
        let cancel = matches!(command, ChildCommand::Cancel { .. });
        let config = self.groups.scheduler;
        if self.groups.used >= config.max_actions
            || (!cancel && self.groups.normal_used >= config.max_actions - config.cancel_reserve)
        {
            let until = self
                .groups
                .window_id
                .saturating_add(1)
                .saturating_mul(config.window_ms);
            return Plan {
                status: GroupStatus::Blocked(BlockReason::RateLimitUntil(until)),
                action: Some(command),
                priority,
            };
        }
        Plan {
            status: GroupStatus::Ready,
            action: Some(command),
            priority,
        }
    }

    pub fn create_group(
        &mut self,
        group_id: GroupId,
        book: Book,
        policy: GroupPolicy,
    ) -> Result<(), Error> {
        self.apply_inner(
            Event::Group(GroupEvent::Create {
                group_id,
                book,
                policy,
            }),
            true,
        )?;
        Ok(())
    }

    /// Persist the latest intent only. It is NOT a trading acknowledgement.
    /// Call dispatch_next after each input to progress all eligible groups.
    pub fn set_target(
        &mut self,
        group_id: GroupId,
        revision: u64,
        target: Option<Target>,
    ) -> Result<TargetReceipt, Error> {
        let out = self.apply_inner(
            Event::Group(GroupEvent::SetTarget {
                group_id,
                revision,
                target,
            }),
            true,
        )?;
        Ok(TargetReceipt {
            group_id,
            revision,
            duplicate: out.duplicate,
        })
    }

    pub fn control_group(&mut self, group_id: GroupId, control: GroupControl) -> Result<(), Error> {
        self.apply_inner(
            Event::Group(GroupEvent::Control { group_id, control }),
            true,
        )?;
        Ok(())
    }

    pub fn configure_scheduler(&mut self, config: SchedulerConfig) -> Result<(), Error> {
        self.apply_inner(Event::Group(GroupEvent::ConfigureScheduler(config)), true)?;
        Ok(())
    }

    pub fn now_ms(&self) -> u64 {
        self.groups.now_ms
    }

    /// Gateway ingress. Invalid/gapped reports quarantine live single orders and
    /// managed groups because one normalized source may span multiple books. Raw apply
    /// remains available for existing low-level integrations with their own gate.
    pub fn on_report(&mut self, report: Report) -> Result<Outcome, Error> {
        match self.apply(Event::Report(report)) {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                if !self.halted {
                    self.hold_for_recovery()?;
                }
                Err(error)
            }
        }
    }

    /// Explicit connection/source failure boundary. Does not send cancels over
    /// an unknown connection or release any outstanding reservation.
    pub fn hold_for_recovery(&mut self) -> Result<(), Error> {
        let ids: Vec<_> = self.groups.entries.keys().copied().collect();
        for group_id in &ids {
            self.apply_inner(
                Event::Group(GroupEvent::RecoveryHold {
                    group_id: *group_id,
                }),
                true,
            )?;
        }
        let orders: Vec<_> = self
            .orders
            .values()
            .filter(|o| !o.lifecycle.terminal() || o.pending.is_some() || o.uncertain)
            .map(|o| o.id)
            .collect();
        let mut orders = orders;
        orders.sort_unstable();
        for order_id in orders {
            if !self.orders[&order_id].uncertain {
                self.apply_inner(Event::MarkUncertain { order_id }, true)?;
            }
        }
        Ok(())
    }

    /// Drive this from the owner's event loop even when no reports arrive.
    /// Clock values must be nondecreasing and in the same domain as expiries.
    /// Timeouts preserve requests/exposure; they never resend a command.
    pub fn tick(&mut self, now_ms: u64) -> Result<(), Error> {
        self.apply_inner(Event::Group(GroupEvent::Clock { now_ms }), true)?;
        let mut due = Vec::new();
        for (id, group) in &self.groups.entries {
            for oid in &group.tracked {
                let order = &self.orders[oid];
                if let Some(request) = order.pending
                    && !order.uncertain
                {
                    let ctx = self.groups.contexts[&request.id];
                    debug_assert_eq!(ctx.group_id, *id);
                    if now_ms.saturating_sub(ctx.dispatched_at_ms)
                        >= group.policy.request_timeout_ms
                    {
                        due.push((*oid, request.id));
                    }
                }
            }
        }
        for (order_id, request_id) in due {
            self.apply_inner(
                Event::Timeout {
                    order_id,
                    request_id,
                },
                true,
            )?;
        }
        Ok(())
    }

    pub fn group_for_order(&self, order_id: OrderId) -> Option<GroupId> {
        self.groups.owners.get(&order_id).copied()
    }
    pub fn group_orders(&self, group_id: GroupId) -> Result<impl Iterator<Item = &Order>, Error> {
        let group = self
            .groups
            .entries
            .get(&group_id)
            .ok_or(Error::UnknownGroup(group_id))?;
        Ok(group.children.iter().map(|id| &self.orders[id]))
    }

    /// No queued command is returned: compute from the current target, current
    /// fills and current risk NOW. Cancels outrank normal work; equal priorities
    /// rotate across groups. One successful call reserves and journals one action.
    pub fn dispatch_next(&mut self) -> Result<Option<Dispatch>, Error> {
        if self.halted {
            return Err(Error::Halted);
        }
        let mut chosen: Option<((u8, bool, GroupId), GroupDispatch)> = None;
        for (&id, group) in &self.groups.entries {
            let stats = self.group_stats(group);
            let plan = self.plan_group(group, &stats);
            if plan.status != GroupStatus::Ready {
                continue;
            }
            if let Some(command) = plan.action {
                let key = (plan.priority, id <= self.groups.last_group, id);
                if chosen.as_ref().is_none_or(|(previous, _)| key < *previous) {
                    chosen = Some((
                        key,
                        GroupDispatch {
                            group_id: id,
                            target_revision: group.revision,
                            command,
                        },
                    ));
                }
            }
        }
        let Some((_, record)) = chosen else {
            return Ok(None);
        };
        let out = self.apply_inner(Event::Group(GroupEvent::Dispatch(record)), true)?;
        Ok(Some(Dispatch {
            group_id: record.group_id,
            target_revision: record.target_revision,
            order_id: record.command.order_id(),
            request_id: record.command.request_id(),
            command: out.outbound.expect("committed managed dispatch"),
        }))
    }

    pub fn group_status(&self, group_id: GroupId) -> Result<GroupView, Error> {
        let group = self
            .groups
            .entries
            .get(&group_id)
            .ok_or(Error::UnknownGroup(group_id))?;
        let s = self.group_stats(group);
        let plan = self.plan_group(group, &s);
        Ok(GroupView {
            group_id,
            revision: group.revision,
            target: group.target,
            mode: group.mode,
            recovery_hold: group.recovery_hold,
            required_leaves: self.required(group),
            confirmed_leaves: s.confirmed,
            projected_leaves: s.projected,
            reserved_qty: s.reserved,
            cum_filled: group.cum_filled,
            active_children: s.active,
            inflight: s.inflight,
            uncertain_children: s.uncertain,
            status: plan.status,
            next_action: plan.action,
            last_rejection: group.last_rejection,
            retry_count: group.retry_count,
        })
    }

    pub fn explain_pending_action(&self, group_id: GroupId) -> Result<GroupView, Error> {
        self.group_status(group_id)
    }

    pub(crate) fn apply_group_event(
        &mut self,
        event: GroupEvent,
        persist: bool,
    ) -> Result<Outcome, Error> {
        if self.halted {
            return Err(Error::Halted);
        }
        if let GroupEvent::Dispatch(record) = event {
            let g = self
                .groups
                .entries
                .get(&record.group_id)
                .ok_or(Error::UnknownGroup(record.group_id))?;
            let plan = self.plan_group(g, &self.group_stats(g));
            if record.target_revision != g.revision
                || plan.status != GroupStatus::Ready
                || plan.action != Some(record.command)
            {
                return Err(Error::InvalidDispatch);
            }
            let raw = record.command.event(g.book);
            return self.apply_order_event(raw, persist, Some(record));
        }
        let mut out = Outcome {
            intent_revision: None,
            group_id: None,
            order_id: 0,
            version: 0,
            duplicate: false,
            outbound: None,
        };
        // Validate everything before append; mutations below are infallible.
        match event {
            GroupEvent::Create {
                group_id, policy, ..
            } => {
                if group_id == 0 || self.groups.entries.contains_key(&group_id) {
                    return Err(Error::DuplicateId);
                }
                if self.groups.entries.len() >= self.limits.max_orders {
                    return Err(Error::Capacity("groups"));
                }
                if policy.max_child_leaves == 0
                    || policy.max_child_leaves > self.limits.max_order_qty
                    || policy.max_active_children == 0
                    || policy.max_active_children as usize > self.limits.max_orders
                    || policy.max_inflight == 0
                    || policy.max_inflight > policy.max_active_children
                    || policy.max_reserved_qty == 0
                    || policy.request_timeout_ms == 0
                    || policy.retry_delay_ms == 0
                {
                    return Err(Error::Invalid("invalid group policy"));
                }
                out.group_id = Some(group_id);
            }
            GroupEvent::SetTarget {
                group_id,
                revision,
                target,
            } => {
                let g = self
                    .groups
                    .entries
                    .get(&group_id)
                    .ok_or(Error::UnknownGroup(group_id))?;
                if revision == 0 || revision < g.revision {
                    return Err(Error::StaleTarget {
                        current: g.revision,
                        received: revision,
                    });
                }
                out.group_id = Some(group_id);
                out.version = revision;
                if revision == g.revision {
                    if target != g.target {
                        return Err(Error::ConflictingDuplicate);
                    }
                    out.duplicate = true;
                    return Ok(out);
                }
                if let Some(t) = target {
                    if t.qty == 0
                        || t.qty > 1_000_000_000_000
                        || t.expires_at_ms.is_some_and(|x| x <= self.groups.now_ms)
                    {
                        return Err(Error::Invalid(
                            "zero/out-of-range quantity or expired target",
                        ));
                    }
                    if t.quantity_mode == QuantityMode::MaintainLeaves
                        && t.qty
                            > g.policy.max_child_leaves * u64::from(g.policy.max_active_children)
                    {
                        return Err(Error::Invalid("target exceeds child allocation capacity"));
                    }
                }
            }
            GroupEvent::Control { group_id, control } => {
                let g = self
                    .groups
                    .entries
                    .get(&group_id)
                    .ok_or(Error::UnknownGroup(group_id))?;
                if control == GroupControl::Pause && g.mode == GroupMode::Stopping {
                    return Err(Error::Invalid("stopping can only be released by Resume"));
                }
                if matches!(
                    control,
                    GroupControl::Resume | GroupControl::ReleaseRecovery
                ) && self.group_stats(g).uncertain > 0
                {
                    return Err(Error::NeedsReconciliation);
                }
                out.group_id = Some(group_id);
                out.version = g.revision;
            }
            GroupEvent::RecoveryHold { group_id } => {
                if !self.groups.entries.contains_key(&group_id) {
                    return Err(Error::UnknownGroup(group_id));
                }
                out.group_id = Some(group_id);
            }
            GroupEvent::Clock { now_ms } => {
                if now_ms < self.groups.now_ms {
                    return Err(Error::Invalid("clock moved backwards"));
                }
                if now_ms == self.groups.now_ms {
                    out.duplicate = true;
                    return Ok(out);
                }
            }
            GroupEvent::ConfigureScheduler(config) => {
                if config.window_ms == 0
                    || config.max_actions == 0
                    || config.cancel_reserve >= config.max_actions
                {
                    return Err(Error::Invalid("invalid scheduler configuration"));
                }
                if self.groups.used > 0 {
                    return Err(Error::Invalid(
                        "configure only before dispatch or in a fresh window",
                    ));
                }
            }
            GroupEvent::Dispatch(_) => unreachable!(),
        }
        self.persist_event(&Event::Group(event), persist)?;
        match event {
            GroupEvent::Create {
                group_id,
                book,
                policy,
            } => {
                self.groups.entries.insert(
                    group_id,
                    GroupRecord {
                        book,
                        policy,
                        target: None,
                        revision: 0,
                        mode: GroupMode::Running,
                        recovery_hold: false,
                        children: BTreeSet::new(),
                        tracked: BTreeSet::new(),
                        cum_filled: 0,
                        completed_revision: None,
                        blocked: None,
                        retry_count: 0,
                        last_rejection: None,
                    },
                );
            }
            GroupEvent::SetTarget {
                group_id,
                revision,
                target,
            } => {
                let g = self.groups.entries.get_mut(&group_id).unwrap();
                // A heartbeat/new revision with identical trading terms must
                // not repeatedly re-arm a permanently rejected operation.
                if !same_terms(g.target, target) && g.mode != GroupMode::Stopping {
                    g.blocked = None;
                    g.retry_count = 0;
                    g.last_rejection = None;
                }
                g.target = target;
                g.revision = revision;
                g.completed_revision = target
                    .filter(|t| {
                        t.quantity_mode == QuantityMode::TotalExecution && g.cum_filled >= t.qty
                    })
                    .map(|_| revision);
            }
            GroupEvent::Control { group_id, control } => {
                let g = self.groups.entries.get_mut(&group_id).unwrap();
                match control {
                    GroupControl::Pause => g.mode = GroupMode::Paused,
                    GroupControl::Resume => {
                        g.mode = GroupMode::Running;
                        g.recovery_hold = false;
                    }
                    GroupControl::Stop => {
                        // A repeated Stop must not reset a rejected cancel loop.
                        if g.mode != GroupMode::Stopping {
                            g.blocked = None;
                            g.retry_count = 0;
                        }
                        g.mode = GroupMode::Stopping;
                    }
                    GroupControl::Retry => {
                        g.blocked = None;
                        g.retry_count = 0;
                    }
                    GroupControl::ReleaseRecovery => g.recovery_hold = false,
                }
            }
            GroupEvent::RecoveryHold { group_id } => {
                self.groups
                    .entries
                    .get_mut(&group_id)
                    .unwrap()
                    .recovery_hold = true
            }
            GroupEvent::Clock { now_ms } => {
                self.groups.now_ms = now_ms;
                let window = now_ms / self.groups.scheduler.window_ms;
                if window != self.groups.window_id {
                    self.groups.window_id = window;
                    self.groups.used = 0;
                    self.groups.normal_used = 0;
                }
            }
            GroupEvent::ConfigureScheduler(config) => {
                self.groups.scheduler = config;
                self.groups.window_id = self.groups.now_ms / config.window_ms;
            }
            GroupEvent::Dispatch(_) => unreachable!(),
        }
        Ok(out)
    }

    pub(crate) fn commit_group_dispatch(&mut self, d: GroupDispatch) {
        let g = self.groups.entries.get_mut(&d.group_id).unwrap();
        if matches!(d.command, ChildCommand::New { .. }) {
            g.children.insert(d.command.order_id());
            self.groups.owners.insert(d.command.order_id(), d.group_id);
        }
        self.groups.contexts.insert(
            d.command.request_id(),
            RequestContext {
                group_id: d.group_id,
                revision: d.target_revision,
                dispatched_at_ms: self.groups.now_ms,
                target: g.target,
                cancel_for_stop: g.mode == GroupMode::Stopping
                    && matches!(d.command, ChildCommand::Cancel { .. }),
            },
        );
        self.groups.used += 1;
        if !matches!(d.command, ChildCommand::Cancel { .. }) {
            self.groups.normal_used += 1;
        }
        self.groups.last_group = d.group_id;
    }

    pub(crate) fn group_order_updated(&mut self, event: Event, old: Option<Order>, order: Order) {
        let Some(&group_id) = self.groups.owners.get(&order.id) else {
            return;
        };
        let g = self.groups.entries.get_mut(&group_id).unwrap();
        if !order.lifecycle.terminal() || order.pending.is_some() || order.uncertain {
            g.tracked.insert(order.id);
        } else {
            g.tracked.remove(&order.id);
        }
        g.cum_filled = g.cum_filled - old.map_or(0, |o| o.cum_filled) + order.cum_filled;
        if let Some(t) = g.target
            && t.quantity_mode == QuantityMode::TotalExecution
            && g.cum_filled >= t.qty
        {
            g.completed_revision = Some(g.revision);
        }
        if let Event::Report(r) = event {
            let rejection = match r.kind {
                ReportKind::Rejected { request_id } => Some((request_id, RejectReason::Permanent)),
                ReportKind::RejectedWithReason { request_id, reason } => Some((request_id, reason)),
                _ => None,
            };
            if let Some((request_id, reason)) = rejection {
                let ctx = self.groups.contexts[&request_id];
                g.last_rejection = Some(reason);
                if ctx.revision != g.revision
                    && !same_terms(ctx.target, g.target)
                    && !(ctx.cancel_for_stop && g.mode == GroupMode::Stopping)
                {
                    return;
                }
                g.blocked = Some(match reason {
                    RejectReason::Permanent | RejectReason::Risk => BlockReason::Rejected(reason),
                    RejectReason::Transient | RejectReason::RateLimited => {
                        if g.retry_count >= g.policy.max_retries {
                            BlockReason::RetryExhausted
                        } else {
                            g.retry_count += 1;
                            BlockReason::RetryAt(
                                self.groups.now_ms.saturating_add(g.policy.retry_delay_ms),
                            )
                        }
                    }
                });
            }
        }
    }
}
