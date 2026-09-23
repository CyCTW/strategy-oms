//! Latest-intent management for one physical order. Uses the same physical
//! reducer as group children; it never creates a replacement physical order.
use crate::{journal::Journal, *};
use std::collections::{BTreeSet, HashMap};

#[derive(Default)]
pub(crate) struct Singles {
    pub intents: HashMap<OrderId, OrderIntent>,
    /// Only unsent latest intents, including temporarily blocked ones.
    waiting: BTreeSet<OrderId>,
}

impl<J: Journal> Engine<J> {
    pub fn order_intent(&self, order_id: OrderId) -> Result<OrderIntentView, Error> {
        let o = self.order(order_id).ok_or(Error::UnknownOrder(order_id))?;
        let intent = *self
            .singles
            .intents
            .get(&order_id)
            .ok_or(Error::Invalid("order has no single-order intent"))?;
        let request_state = self.requests[&intent.request_id].state;
        let status = if self.halted {
            OrderIntentStatus::Halted
        } else if o.uncertain {
            OrderIntentStatus::NeedsReconciliation
        } else if o.pending.is_some() {
            OrderIntentStatus::WaitingForReport
        } else if o.lifecycle.terminal() {
            OrderIntentStatus::Closed(o.lifecycle)
        } else {
            match request_state {
                RequestState::Queued => {
                    match self
                        .validate_queued_order_event(self.single_dispatch_action(intent).event())
                    {
                        Ok(()) => OrderIntentStatus::Ready,
                        Err(Error::RiskLimit) => OrderIntentStatus::RiskBlocked,
                        Err(Error::NeedsReconciliation) => OrderIntentStatus::NeedsReconciliation,
                        Err(error) => return Err(error),
                    }
                }
                RequestState::Rejected => OrderIntentStatus::Rejected,
                RequestState::Unexecutable => OrderIntentStatus::Unexecutable,
                _ => OrderIntentStatus::Resolved,
            }
        };
        Ok(OrderIntentView {
            intent,
            request_state,
            status,
        })
    }

    /// Commit at most one ready deferred intent. Call when the transport can
    /// accept a command, after each input; deliver outbound immediately.
    /// None means no ready work, not that every order has finished.
    /// Like direct apply commands, this path does not use the group scheduler.
    pub fn dispatch_next_order(&mut self) -> Result<Option<Outcome>, Error> {
        if self.halted {
            return Err(Error::Halted);
        }
        let mut chosen = None;
        for &order_id in &self.singles.waiting {
            let view = self.order_intent(order_id)?;
            if view.status != OrderIntentStatus::Ready {
                continue;
            }
            let key = (view.intent.desired != OrderDesired::Cancel, order_id);
            if chosen.is_none_or(|(previous, _)| key < previous) {
                chosen = Some((key, view.intent));
            }
        }
        let Some((_, intent)) = chosen else {
            return Ok(None);
        };
        let event = SingleEvent::Dispatch {
            order_id: intent.order_id,
            request_id: intent.request_id,
            expected_version: self.orders[&intent.order_id].version,
        };
        self.apply_single_event(event, true).map(Some)
    }

    pub(crate) fn validate_single_submit(&self, action: SingleAction) -> Result<bool, Error> {
        if self.halted {
            return Err(Error::Halted);
        }
        let event = action.event();
        let order_id = event.order_id();
        if self.groups.owners.contains_key(&order_id) {
            return Err(Error::ManagedOrder);
        }
        if matches!(action, SingleAction::New(_)) {
            self.validate_order_event(event)?;
            return Ok(true);
        }
        let old = self.order(order_id).ok_or(Error::UnknownOrder(order_id))?;
        self.new_request_id(action.request_id())?;
        let expected_version = match action {
            SingleAction::Replace {
                expected_version, ..
            }
            | SingleAction::Cancel {
                expected_version, ..
            } => expected_version,
            SingleAction::New(_) => unreachable!(),
        };
        if old.version != expected_version {
            return Err(Error::StaleVersion {
                expected: expected_version,
                actual: old.version,
            });
        }
        old.version
            .checked_add(1)
            .ok_or(Error::Invalid("version overflow"))?;
        self.singles
            .intents
            .get(&order_id)
            .map_or(0, |i| i.revision)
            .checked_add(1)
            .ok_or(Error::Invalid("intent revision overflow"))?;
        if old.lifecycle.terminal() && old.pending.is_none() {
            return Err(Error::Invalid("order is terminal"));
        }
        if let SingleAction::Replace {
            price, total_qty, ..
        } = action
        {
            if self
                .singles
                .intents
                .get(&order_id)
                .is_some_and(|i| i.desired == OrderDesired::Cancel)
                || old.pending.is_some_and(|r| r.kind == RequestKind::Cancel)
            {
                return Err(Error::CancelRequested);
            }
            if old.lifecycle.terminal() {
                return Err(Error::Invalid("order is terminal"));
            }
            self.command_qty(total_qty)?;
            if total_qty <= old.cum_filled {
                return Err(Error::Invalid("replace quantity must exceed known fills"));
            }
            if old.pending.is_none()
                && !old.uncertain
                && old.price == price
                && old.total_qty == total_qty
            {
                return Ok(false);
            }
        }
        if old.pending.is_some() || old.uncertain {
            return Ok(false);
        }
        // Preserve immediate validation of risk for an idle, known order.
        // Deferred commands are checked again against live risk at dispatch.
        self.validate_order_event(event)?;
        Ok(true)
    }

    pub(crate) fn apply_single_event(
        &mut self,
        single: SingleEvent,
        persist: bool,
    ) -> Result<Outcome, Error> {
        match single {
            SingleEvent::Submit { action, dispatch } => {
                if self.validate_single_submit(action)? != dispatch {
                    return Err(Error::InvalidDispatch);
                }
                let order_id = action.event().order_id();
                let previous = self.singles.intents.get(&order_id).copied();
                let revision = previous.map_or(1, |i| i.revision + 1);
                let desired = match action {
                    SingleAction::New(n) => OrderDesired::Working {
                        price: n.price,
                        total_qty: n.total_qty,
                    },
                    SingleAction::Replace {
                        price, total_qty, ..
                    } => OrderDesired::Working { price, total_qty },
                    SingleAction::Cancel { .. } => OrderDesired::Cancel,
                };
                let mut out = if dispatch {
                    self.apply_order_record(action.event(), persist, None, Some(single), false)?
                } else {
                    let mut order = self.orders[&order_id];
                    order.version += 1;
                    let (kind, price, total_qty) = match desired {
                        OrderDesired::Working { price, total_qty } => {
                            (RequestKind::Replace, price, total_qty)
                        }
                        OrderDesired::Cancel => (RequestKind::Cancel, order.price, order.total_qty),
                    };
                    let request = Request {
                        id: action.request_id(),
                        order_id,
                        kind,
                        price,
                        total_qty,
                        state: RequestState::Queued,
                    };
                    self.persist_event(&Event::Single(single), persist)?;
                    self.orders.insert(order_id, order);
                    self.requests.insert(request.id, request);
                    self.request_high_water = self.request_high_water.max(request.id);
                    Outcome {
                        group_id: None,
                        order_id,
                        version: order.version,
                        intent_revision: None,
                        duplicate: false,
                        outbound: None,
                    }
                };
                if let Some(previous) = previous {
                    let old = self
                        .requests
                        .get_mut(&previous.request_id)
                        .expect("intent request exists");
                    if old.state == RequestState::Queued {
                        old.state = RequestState::Superseded;
                    }
                }
                let intent = OrderIntent {
                    order_id,
                    revision,
                    request_id: action.request_id(),
                    desired,
                };
                self.singles.intents.insert(order_id, intent);
                self.settle_single_intent(order_id);
                out.intent_revision = Some(revision);
                Ok(out)
            }
            SingleEvent::Dispatch {
                order_id,
                request_id,
                expected_version,
            } => {
                let view = self.order_intent(order_id)?;
                if view.status != OrderIntentStatus::Ready
                    || view.intent.request_id != request_id
                    || self.orders[&order_id].version != expected_version
                {
                    return Err(Error::InvalidDispatch);
                }
                let action = self.single_dispatch_action(view.intent);
                self.apply_order_record(action.event(), persist, None, Some(single), true)
            }
        }
    }

    fn single_dispatch_action(&self, intent: OrderIntent) -> SingleAction {
        let expected_version = self.orders[&intent.order_id].version;
        match intent.desired {
            OrderDesired::Working { price, total_qty } => SingleAction::Replace {
                order_id: intent.order_id,
                request_id: intent.request_id,
                expected_version,
                price,
                total_qty,
            },
            OrderDesired::Cancel => SingleAction::Cancel {
                order_id: intent.order_id,
                request_id: intent.request_id,
                expected_version,
            },
        }
    }

    /// Deterministic consequence of the already-journaled input. Never sends.
    pub(crate) fn settle_single_intent(&mut self, order_id: OrderId) {
        let Some(intent) = self.singles.intents.get(&order_id) else {
            return;
        };
        let order = &self.orders[&order_id];
        let request = self
            .requests
            .get_mut(&intent.request_id)
            .expect("intent request exists");
        if request.state == RequestState::Queued && !order.uncertain && order.pending.is_none() {
            match intent.desired {
                OrderDesired::Cancel if order.lifecycle.terminal() => {
                    request.state = RequestState::NotNeeded
                }
                OrderDesired::Working { price, total_qty } => {
                    if order.lifecycle.terminal() || total_qty <= order.cum_filled {
                        request.state = RequestState::Unexecutable;
                    } else if order.price == price && order.total_qty == total_qty {
                        request.state = RequestState::NotNeeded;
                    }
                }
                _ => {}
            }
        }
        if request.state == RequestState::Queued {
            self.singles.waiting.insert(order_id);
        } else {
            self.singles.waiting.remove(&order_id);
        }
    }
}
