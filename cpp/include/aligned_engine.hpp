#pragma once
#include "aligned_model.hpp"
#include <absl/container/flat_hash_map.h>

namespace aligned_oms {
struct KeyHash {
  std::size_t operator()(const ExecutionKey &k) const {
    return absl::Hash<std::tuple<Id, Id, Id, Id>>{}(
        {k.venue, k.account, k.trading_day, k.execution_id});
  }
};
// Port of core.rs + single.rs. Groups/FileJournal are deliberately out of
// scope. Like Rust MemoryJournal, append is bounded and failure latches halted
// before any OMS transition is committed. Allocation exhaustion is not
// recoverable.
template <class Locator = oms::BtreeLocator> class Engine {
  struct Entry {
    Order order;
    oms::Memberships memberships;
  };
  struct Transition {
    std::optional<Order> old;
    Order order;
    std::optional<Request> request;
    std::optional<std::pair<Id, ReportKind>> result;
    std::optional<Execution> execution;
    std::optional<Command> outbound;
    bool duplicate{};
  };
  Limits limits_;
  std::size_t journal_capacity_;
  std::vector<Event> journal_;
  bool halted_{};
  oms::Pool<Entry> orders_;
  absl::flat_hash_map<Id, oms::Handle> ids_;
  absl::flat_hash_map<Id, Request> requests_;
  absl::flat_hash_map<Id, ReportKind> request_results_;
  absl::flat_hash_map<ExecutionKey, Execution, KeyHash> executions_;
  absl::flat_hash_map<std::pair<Id, Id>, Report> reports_;
  absl::flat_hash_map<Id, Id> sequences_;
  oms::Index<Locator, Order> index_;
  absl::flat_hash_map<Id, Intent> intents_;
  absl::btree_set<Id> waiting_;
  Id order_high_water_{}, request_high_water_{};

  Entry &insert_order(Order o) {
    auto i = ids_.find(o.id);
    if (i == ids_.end())
      i = ids_.emplace(o.id, orders_.insert(Entry{o, {}})).first;
    auto &entry = oms::require(orders_.get(i->second));
    entry.order = o;
    return entry;
  }
  const Order &require_order(Id id) const {
    const auto *o = order(id);
    if (!o)
      throw Error::UnknownOrder;
    return *o;
  }
  void persist(const Event &e, bool enabled) {
    if (halted_)
      throw Error::Halted;
    if (enabled) {
      if (journal_.size() == journal_capacity_) {
        halted_ = true;
        throw Error::Journal;
      }
      journal_.push_back(e);
    }
  }
  void new_request(Id id) const {
    if (!id)
      throw Error::Invalid;
    if (requests_.contains(id))
      throw Error::DuplicateId;
    if (requests_.size() >= limits_.max_requests)
      throw Error::Capacity;
  }
  static void valid_qty(Qty qty) {
    if (!qty || qty > max_qty)
      throw Error::Invalid;
  }
  void command_qty(Qty qty) const {
    valid_qty(qty);
    if (qty > limits_.max_order_qty)
      throw Error::RiskLimit;
  }
  static void check_key(const Order &o, const ExecutionKey &k) {
    if (k.venue != o.book.venue || k.account != o.book.account ||
        !k.execution_id)
      throw Error::Invalid;
  }
  static void finish(Transition &t, Id id, std::optional<RequestKind> kind,
                     RequestState state, const ReportKind &result) {
    if (!t.order.pending || t.order.pending->id != id ||
        (kind && t.order.pending->kind != *kind))
      throw Error::RequestMismatch;
    auto r = *t.order.pending;
    r.state = state;
    t.request = r;
    t.result = {{id, result}};
    t.order.pending.reset();
  }
  void reduce(Transition &t, const ReportKind &kind) const {
    const auto req = std::visit(
        [](const auto &r) -> std::optional<Id> {
          if constexpr (requires { r.request_id; })
            return r.request_id;
          else
            return {};
        },
        kind);
    if (req) {
      if (const auto i = request_results_.find(*req);
          i != request_results_.end()) {
        if (requests_.at(*req).order_id != t.order.id || i->second != kind)
          throw Error::ConflictingDuplicate;
        t.duplicate = true;
        return;
      }
    }
    auto &o = t.order;
    std::visit(
        [&](const auto &r) {
          using R = std::decay_t<decltype(r)>;
          if constexpr (std::is_same_v<R, Accepted> ||
                        std::is_same_v<R, Replaced>) {
            valid_qty(r.total_qty);
            if (r.total_qty < o.cum_filled)
              throw Error::Invalid;
            finish(t, r.request_id,
                   std::is_same_v<R, Accepted> ? RequestKind::New
                                               : RequestKind::Replace,
                   RequestState::Accepted, kind);
            o.exchange_id = r.exchange_id;
            o.price = r.price;
            o.total_qty = r.total_qty;
            o.leaves = r.total_qty - o.cum_filled;
            o.lifecycle = o.leaves ? Lifecycle::Working : Lifecycle::Filled;
          } else if constexpr (std::is_same_v<R, Fill>) {
            check_key(o, r.key);
            valid_qty(r.qty);
            if (auto i = executions_.find(r.key); i != executions_.end()) {
              const auto &x = i->second;
              if (x.order_id != o.id || x.original_qty != r.qty ||
                  x.original_price != r.price)
                throw Error::ConflictingDuplicate;
              t.duplicate = true;
              return;
            }
            if (executions_.size() >= limits_.max_executions)
              throw Error::Capacity;
            if (o.lifecycle == Lifecycle::Rejected)
              throw Error::Invalid;
            auto cum = checked_add(o.cum_filled, r.qty);
            if (cum > o.total_qty)
              throw Error::Invalid;
            o.cum_filled = cum;
            if (o.lifecycle == Lifecycle::Working) {
              o.leaves = o.total_qty - cum;
              if (!o.leaves)
                o.lifecycle = Lifecycle::Filled;
            } else if (o.lifecycle == Lifecycle::PendingNew &&
                       cum == o.total_qty)
              o.lifecycle = Lifecycle::Filled;
            t.execution =
                Execution{r.key, o.id, r.qty, r.price, r.qty, r.price, 0};
          } else if constexpr (std::is_same_v<R, Canceled>) {
            if (r.request_id)
              finish(t, *r.request_id, RequestKind::Cancel,
                     RequestState::Accepted, kind);
            else if (o.pending)
              finish(t, o.pending->id, {}, RequestState::Superseded, kind);
            o.leaves = 0;
            o.lifecycle = Lifecycle::Canceled;
          } else if constexpr (std::is_same_v<R, Rejected> ||
                               std::is_same_v<R, RejectedWithReason>) {
            if (!o.pending)
              throw Error::RequestMismatch;
            if (o.pending->kind == RequestKind::New) {
              if (o.cum_filled)
                throw Error::Invalid;
              o.lifecycle = Lifecycle::Rejected;
              o.leaves = 0;
            }
            finish(t, r.request_id, {}, RequestState::Rejected, kind);
          } else if constexpr (std::is_same_v<R, Expired>) {
            if (o.pending)
              finish(t, o.pending->id, {}, RequestState::Superseded, kind);
            o.leaves = 0;
            o.lifecycle = Lifecycle::Expired;
          } else if constexpr (std::is_same_v<R, Corrected>) {
            check_key(o, r.key);
            const auto i = executions_.find(r.key);
            if (i == executions_.end())
              throw Error::UnknownExecution;
            auto x = i->second;
            if (x.order_id != o.id)
              throw Error::ConflictingDuplicate;
            if (r.revision && r.revision == x.revision && r.new_qty == x.qty &&
                r.new_price == x.price) {
              t.duplicate = true;
              return;
            }
            if (r.revision != increment(x.revision) || r.new_qty > max_qty)
              throw Error::Invalid;
            auto cum = checked_add(o.cum_filled - x.qty, r.new_qty);
            if (cum > o.total_qty)
              throw Error::Invalid;
            o.cum_filled = cum;
            o.uncertain = true;
            x.qty = r.new_qty;
            x.price = r.new_price;
            x.revision = r.revision;
            t.execution = x;
          } else if constexpr (std::is_same_v<R, Reconciled>) {
            valid_qty(r.total_qty);
            if (o.pending)
              throw Error::PendingRequest;
            if (r.cum_filled != o.cum_filled || r.cum_filled > r.total_qty ||
                r.leaves > r.total_qty - r.cum_filled)
              throw Error::Invalid;
            bool valid = false;
            switch (r.lifecycle) {
            case Lifecycle::Working:
              valid = r.leaves && r.leaves == r.total_qty - r.cum_filled;
              break;
            case Lifecycle::Filled:
              valid = !r.leaves && r.cum_filled == r.total_qty;
              break;
            case Lifecycle::Canceled:
            case Lifecycle::Expired:
              valid = !r.leaves;
              break;
            case Lifecycle::Rejected:
              valid = !r.leaves && !r.cum_filled;
              break;
            default:
              break;
            }
            if (!valid)
              throw Error::Invalid;
            o.price = r.price;
            o.total_qty = r.total_qty;
            o.leaves = r.leaves;
            o.lifecycle = r.lifecycle;
            o.uncertain = false;
          }
        },
        kind);
  }
  Transition prepare(const Event &e, bool queued = false) const {
    if (const auto *n = std::get_if<New>(&e)) {
      if (!n->order_id || !n->request_id)
        throw Error::Invalid;
      if (ids_.contains(n->order_id))
        throw Error::DuplicateId;
      new_request(n->request_id);
      if (ids_.size() >= limits_.max_orders)
        throw Error::Capacity;
      command_qty(n->total_qty);
      if (uncertain(n->book))
        throw Error::NeedsReconciliation;
      if (checked_add(reserved(n->book), n->total_qty, Error::RiskLimit) >
          limits_.max_open_qty_per_book)
        throw Error::RiskLimit;
      Request r{n->request_id, n->order_id,  RequestKind::New,
                n->price,      n->total_qty, RequestState::Pending};
      Order o{
          n->order_id,           n->book, {},    n->price, n->total_qty, 0, 0,
          Lifecycle::PendingNew, r,       false, 0};
      return {{}, o, r, {}, {}, Command{*n}, false};
    }
    const auto old = require_order(order_id(e));
    Transition t{old, old, {}, {}, {}, {}, false};
    std::visit(
        [&](const auto &v) {
          using T = std::decay_t<decltype(v)>;
          if constexpr (std::is_same_v<T, Cancel> ||
                        std::is_same_v<T, Replace>) {
            if (queued) {
              const auto i = requests_.find(v.request_id);
              if (i == requests_.end() || i->second.order_id != v.order_id ||
                  i->second.state != RequestState::Queued)
                throw Error::RequestMismatch;
            } else
              new_request(v.request_id);
            if (old.version != v.expected_version)
              throw Error::StaleVersion;
            if (old.uncertain)
              throw Error::NeedsReconciliation;
            if (old.pending)
              throw Error::PendingRequest;
            if (old.lifecycle != Lifecycle::Working)
              throw Error::Invalid;
            Request r{v.request_id, v.order_id,    RequestKind::Cancel,
                      old.price,    old.total_qty, RequestState::Pending};
            if constexpr (std::is_same_v<T, Replace>) {
              command_qty(v.total_qty);
              if (v.total_qty <= old.cum_filled)
                throw Error::Invalid;
              r.kind = RequestKind::Replace;
              r.price = v.price;
              r.total_qty = v.total_qty;
            }
            t.order.pending = r;
            const auto qty =
                reserved(old.book) - old.reserved() + t.order.reserved();
            if (t.order.reserved() > old.reserved() &&
                (uncertain(old.book) || qty > limits_.max_open_qty_per_book))
              throw Error::RiskLimit;
            t.request = r;
            t.outbound = Change{v.order_id, old.exchange_id, r};
          } else if constexpr (std::is_same_v<T, Timeout>) {
            if (!old.pending || old.pending->id != v.request_id)
              throw Error::RequestMismatch;
            t.order.uncertain = true;
          } else if constexpr (std::is_same_v<T, MarkUncertain>)
            t.order.uncertain = true;
          else if constexpr (std::is_same_v<T, Report>)
            reduce(t, v.kind);
          else
            throw Error::Invalid;
        },
        e);
    return t;
  }
  Action dispatch_action(const Intent &i) const {
    const auto v = require_order(i.order_id).version;
    if (i.desired)
      return Replace{i.order_id, i.request_id, v, i.desired->price,
                     i.desired->total_qty};
    return Cancel{i.order_id, i.request_id, v};
  }
  void settle(Id id) {
    const auto it = intents_.find(id);
    if (it == intents_.end())
      return;
    const auto &intent = it->second;
    const auto &o = require_order(id);
    auto &r = requests_.at(intent.request_id);
    if (r.state == RequestState::Queued && !o.uncertain && !o.pending) {
      if (!intent.desired && terminal(o.lifecycle))
        r.state = RequestState::NotNeeded;
      else if (intent.desired) {
        const auto d = *intent.desired;
        if (terminal(o.lifecycle) || d.total_qty <= o.cum_filled)
          r.state = RequestState::Unexecutable;
        else if (o.price == d.price && o.total_qty == d.total_qty)
          r.state = RequestState::NotNeeded;
      }
    }
    if (r.state == RequestState::Queued)
      waiting_.insert(id);
    else
      waiting_.erase(id);
  }
  bool validate_submit(const Action &a) const {
    if (halted_)
      throw Error::Halted;
    const auto e = event(a);
    if (std::holds_alternative<New>(a)) {
      prepare(e);
      return true;
    }
    const auto &old = require_order(order_id(a));
    new_request(request_id(a));
    auto v = std::visit(
        [](const auto &x) -> Id {
          if constexpr (requires { x.expected_version; })
            return x.expected_version;
          else
            return 0;
        },
        a);
    if (old.version != v)
      throw Error::StaleVersion;
    increment(old.version);
    const auto prev = intents_.find(old.id);
    increment(prev == intents_.end() ? 0 : prev->second.revision);
    if (terminal(old.lifecycle) && !old.pending)
      throw Error::Invalid;
    if (const auto *r = std::get_if<Replace>(&a)) {
      if ((prev != intents_.end() && !prev->second.desired) ||
          (old.pending && old.pending->kind == RequestKind::Cancel))
        throw Error::CancelRequested;
      if (terminal(old.lifecycle))
        throw Error::Invalid;
      command_qty(r->total_qty);
      if (r->total_qty <= old.cum_filled)
        throw Error::Invalid;
      if (!old.pending && !old.uncertain && old.price == r->price &&
          old.total_qty == r->total_qty)
        return false;
    }
    if (old.pending || old.uncertain)
      return false;
    prepare(e);
    return true;
  }
  Outcome apply_record(const Event &e, bool persist_enabled,
                       const std::optional<Single> &single = {},
                       bool queued = false) {
    if (halted_)
      throw Error::Halted;
    const auto *report = std::get_if<Report>(&e);
    if (report) {
      if (auto i = reports_.find({report->source, report->sequence});
          i != reports_.end()) {
        if (i->second != *report)
          throw Error::ConflictingDuplicate;
        const auto &o = require_order(report->order_id);
        const auto in = intents_.find(o.id);
        return {o.id,
                o.version,
                in == intents_.end() ? std::nullopt
                                     : std::optional(in->second.revision),
                true,
                {}};
      }
      if (report->sequence != increment(last_sequence(report->source)))
        throw Error::SequenceGap;
      if (reports_.size() >= limits_.max_reports)
        throw Error::Capacity;
    }
    auto t = prepare(e, queued);
    const bool changed = !t.old || *t.old != t.order;
    if (changed)
      t.order.version = increment(t.order.version);
    persist(single ? Event{*single} : e, persist_enabled);
    if (changed) {
      auto &entry = insert_order(t.order);
      index_.update(t.old ? &*t.old : nullptr, t.order, entry.memberships);
    }
    if (t.request) {
      requests_.insert_or_assign(t.request->id, *t.request);
      request_high_water_ = std::max(request_high_water_, t.request->id);
    }
    if (t.result)
      request_results_.insert_or_assign(t.result->first, t.result->second);
    if (t.execution)
      executions_.insert_or_assign(t.execution->key, *t.execution);
    if (report) {
      reports_.insert_or_assign({report->source, report->sequence}, *report);
      sequences_.insert_or_assign(report->source, report->sequence);
    }
    order_high_water_ = std::max(order_high_water_, t.order.id);
    if (!t.duplicate)
      settle(t.order.id);
    const auto in = intents_.find(t.order.id);
    return {t.order.id, t.order.version,
            in == intents_.end() ? std::nullopt
                                 : std::optional(in->second.revision),
            t.duplicate, persist_enabled ? t.outbound : std::nullopt};
  }
  Outcome apply_single(const Single &single, bool persist_enabled) {
    if (const auto *s = std::get_if<Submit>(&single)) {
      if (validate_submit(s->action) != s->dispatch)
        throw Error::InvalidDispatch;
      const auto id = order_id(s->action);
      std::optional<Intent> previous;
      if (auto it = intents_.find(id); it != intents_.end())
        previous = it->second;
      const auto revision = previous ? previous->revision + 1 : 1;
      const auto desired = std::visit(
          [](const auto &a) -> Desired {
            if constexpr (requires { a.price; })
              return Working{a.price, a.total_qty};
            else
              return {};
          },
          s->action);
      Outcome out;
      if (s->dispatch)
        out = apply_record(event(s->action), persist_enabled, single);
      else {
        auto o = require_order(id);
        ++o.version;
        Request r{request_id(s->action),
                  id,
                  desired ? RequestKind::Replace : RequestKind::Cancel,
                  desired ? desired->price : o.price,
                  desired ? desired->total_qty : o.total_qty,
                  RequestState::Queued};
        persist(Event{single}, persist_enabled);
        insert_order(o);
        requests_.insert_or_assign(r.id, r);
        request_high_water_ = std::max(request_high_water_, r.id);
        out = {id, o.version, {}, false, {}};
      }
      if (previous) {
        auto &old = requests_.at(previous->request_id);
        if (old.state == RequestState::Queued)
          old.state = RequestState::Superseded;
      }
      intents_.insert_or_assign(
          id, Intent{id, revision, request_id(s->action), desired});
      settle(id);
      out.intent_revision = revision;
      return out;
    }
    const auto d = std::get<Dispatch>(single);
    const auto view = intent(d.order_id);
    if (view.status != IntentStatus::Ready ||
        view.intent.request_id != d.request_id ||
        require_order(d.order_id).version != d.expected_version)
      throw Error::InvalidDispatch;
    return apply_record(event(dispatch_action(view.intent)), persist_enabled,
                        single, true);
  }
  Outcome inner(const Event &e, bool persist_enabled) {
    if (const auto *s = std::get_if<Single>(&e))
      return apply_single(*s, persist_enabled);
    return apply_record(e, persist_enabled);
  }

public:
  Engine(std::size_t capacity, Limits limits = {})
      : limits_(limits), journal_capacity_(capacity) {
    if (!limits.max_orders || limits.max_orders > 1'000'000 ||
        !limits.max_order_qty || limits.max_order_qty > max_qty)
      throw Error::Invalid;
    journal_.reserve(capacity);
    requests_.reserve(limits.max_requests);
    request_results_.reserve(limits.max_requests);
    executions_.reserve(limits.max_executions);
    reports_.reserve(limits.max_reports);
  }
  const Order *order(Id id) const {
    auto it = ids_.find(id);
    return it == ids_.end() ? nullptr
                            : &oms::require(orders_.get(it->second)).order;
  }
  const Request *request(Id id) const {
    auto it = requests_.find(id);
    return it == requests_.end() ? nullptr : &it->second;
  }
  const Execution *execution(const ExecutionKey &key) const {
    auto it = executions_.find(key);
    return it == executions_.end() ? nullptr : &it->second;
  }
  bool halted() const { return halted_; }
  const auto &journal() const { return journal_; }
  const auto &index() const { return index_; }
  Id last_sequence(Id source) const {
    auto i = sequences_.find(source);
    return i == sequences_.end() ? 0 : i->second;
  }
  Qty reserved(Book b) const {
    auto h = index_.book_handle(b);
    return h ? index_.reserved(*h) : 0;
  }
  Qty uncertain(Book b) const {
    auto h = index_.book_handle(b);
    return h ? index_.uncertain(*h) : 0;
  }
  IntentView intent(Id id) const {
    const auto &o = require_order(id);
    auto it = intents_.find(id);
    if (it == intents_.end())
      throw Error::Invalid;
    auto i = it->second;
    auto state = requests_.at(i.request_id).state;
    IntentStatus status;
    if (halted_)
      status = IntentStatus::Halted;
    else if (o.uncertain)
      status = IntentStatus::Reconcile;
    else if (o.pending)
      status = IntentStatus::Waiting;
    else if (terminal(o.lifecycle))
      status = IntentStatus::Closed;
    else if (state == RequestState::Queued) {
      status = IntentStatus::Ready;
      try {
        prepare(event(dispatch_action(i)), true);
      } catch (Error e) {
        if (e == Error::RiskLimit)
          status = IntentStatus::RiskBlocked;
        else if (e == Error::NeedsReconciliation)
          status = IntentStatus::Reconcile;
        else
          throw;
      }
    } else if (state == RequestState::Rejected)
      status = IntentStatus::Rejected;
    else if (state == RequestState::Unexecutable)
      status = IntentStatus::Unexecutable;
    else
      status = IntentStatus::Resolved;
    return {i, state, status};
  }
  Outcome apply(const Event &e) {
    auto action = std::visit(
        [](const auto &v) -> std::optional<Action> {
          using T = std::decay_t<decltype(v)>;
          if constexpr (std::is_same_v<T, New> || std::is_same_v<T, Cancel> ||
                        std::is_same_v<T, Replace>)
            return Action{v};
          else
            return {};
        },
        e);
    if (action) {
      const auto dispatch = validate_submit(*action);
      return apply_single(Submit{*action, dispatch}, true);
    }
    if (std::holds_alternative<Single>(e))
      throw Error::Invalid;
    return inner(e, true);
  }
  std::optional<Outcome> dispatch_next_order() {
    if (halted_)
      throw Error::Halted;
    std::optional<std::pair<std::pair<bool, Id>, Intent>> chosen;
    for (const auto id : waiting_) {
      const auto v = intent(id);
      if (v.status != IntentStatus::Ready)
        continue;
      const auto key = std::pair{v.intent.desired.has_value(), id};
      if (!chosen || key < chosen->first)
        chosen = {key, v.intent};
    }
    if (!chosen)
      return {};
    const auto i = chosen->second;
    return apply_single(
        Dispatch{i.order_id, i.request_id, require_order(i.order_id).version},
        true);
  }
  void hold_for_recovery() {
    std::vector<Id> ids;
    for (const auto &[id, h] : ids_) {
      const auto &o = oms::require(orders_.get(h)).order;
      if (!terminal(o.lifecycle) || o.pending || o.uncertain)
        ids.push_back(id);
    }
    std::sort(ids.begin(), ids.end());
    for (const auto id : ids)
      if (!require_order(id).uncertain)
        inner(MarkUncertain{id}, true);
  }
  Outcome on_report(const Report &r) {
    try {
      return apply(r);
    } catch (Error e) {
      if (!halted_)
        hold_for_recovery();
      throw e;
    }
  }
  static std::unique_ptr<Engine> recover(const std::vector<Event> &events,
                                         std::size_t capacity, Limits limits) {
    if (events.size() > capacity)
      throw Error::Journal;
    auto e = std::make_unique<Engine>(capacity, limits);
    e->journal_ = events;
    for (const auto &event : events)
      e->inner(event, false);
    std::vector<Id> ids;
    for (const auto &[id, h] : e->ids_) {
      const auto &o = oms::require(e->orders_.get(h)).order;
      if (!terminal(o.lifecycle) || o.pending)
        ids.push_back(id);
    }
    std::sort(ids.begin(), ids.end());
    for (const auto id : ids)
      e->inner(MarkUncertain{id}, true);
    return e;
  }
};
} // namespace aligned_oms
