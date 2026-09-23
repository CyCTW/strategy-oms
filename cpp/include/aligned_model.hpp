#pragma once
#include "oms_index.hpp"
#include <variant>

// Functional port of Rust model.rs / single_model.rs for single-order OMS.
// Native C++ layouts are intentional; this does not claim identical Rust ABI.
namespace aligned_oms {
using oms::Book;
using oms::Price;
using oms::Qty;
using Id = std::uint64_t;
constexpr Qty max_qty = 1'000'000'000'000;
enum class Lifecycle : std::uint8_t {
  PendingNew,
  Working,
  Filled,
  Canceled,
  Rejected,
  Expired
};
inline bool terminal(Lifecycle s) { return s >= Lifecycle::Filled; }
enum class RequestKind : std::uint8_t { New, Cancel, Replace };
enum class RequestState : std::uint8_t {
  Queued,
  Pending,
  Accepted,
  Rejected,
  Superseded,
  NotNeeded,
  Unexecutable
};
struct Request {
  Id id{}, order_id{};
  RequestKind kind{};
  Price price{};
  Qty total_qty{};
  RequestState state{};
  bool operator==(const Request &) const = default;
};
struct Order {
  Id id{};
  Book book;
  std::optional<Id> exchange_id;
  Price price{};
  Qty total_qty{}, cum_filled{}, leaves{};
  Lifecycle lifecycle{};
  std::optional<Request> pending;
  bool uncertain{};
  Id version{};
  Qty reserved() const {
    auto remainder = total_qty > cum_filled ? total_qty - cum_filled : 0;
    auto qty = lifecycle == Lifecycle::PendingNew ? remainder : leaves;
    if (uncertain)
      qty = std::max(qty, remainder);
    if (pending && pending->kind != RequestKind::Cancel)
      qty = std::max(qty, pending->total_qty > cum_filled
                              ? pending->total_qty - cum_filled
                              : 0);
    return qty;
  }
  bool operator==(const Order &) const = default;
};
// Same contribution algorithm as Rust; no conversion to the reduced prototype
// Order and no extra snapshot maintained solely for the index.
inline std::array<std::optional<oms::Contribution>, 2>
contributions(const Order *o) {
  if (!o || (terminal(o->lifecycle) && !o->pending))
    return {};
  oms::Totals t;
  t.confirmed = o->leaves;
  t.uncertain = o->uncertain;
  std::optional<oms::Contribution> second;
  if (o->pending) {
    auto p = *o->pending;
    auto qty = p.total_qty > o->cum_filled ? p.total_qty - o->cum_filled : 0;
    if (p.kind == RequestKind::New)
      t.new_qty = qty;
    else if (p.kind == RequestKind::Cancel)
      t.cancel_qty = o->leaves;
    else {
      t.replace_out = o->leaves;
      if (p.price == o->price)
        t.replace_in = qty;
      else {
        oms::Totals n;
        n.replace_in = qty;
        n.uncertain = o->uncertain;
        second = oms::Contribution{p.price, n};
      }
    }
  }
  return {oms::Contribution{o->price, t}, second};
}
struct ExecutionKey {
  Id venue{}, account{}, trading_day{}, execution_id{};
  bool operator==(const ExecutionKey &) const = default;
};
struct Execution {
  ExecutionKey key;
  Id order_id{};
  Qty original_qty{};
  Price original_price{};
  Qty qty{};
  Price price{};
  Id revision{};
};
struct New {
  Id order_id{}, request_id{};
  Book book;
  Price price{};
  Qty total_qty{};
  bool operator==(const New &) const = default;
};
struct Cancel {
  Id order_id{}, request_id{}, expected_version{};
  bool operator==(const Cancel &) const = default;
};
struct Replace {
  Id order_id{}, request_id{}, expected_version{};
  Price price{};
  Qty total_qty{};
  bool operator==(const Replace &) const = default;
};
using Action = std::variant<New, Cancel, Replace>;
struct Accepted {
  Id request_id{}, exchange_id{};
  Price price{};
  Qty total_qty{};
  bool operator==(const Accepted &) const = default;
};
struct Replaced {
  Id request_id{}, exchange_id{};
  Price price{};
  Qty total_qty{};
  bool operator==(const Replaced &) const = default;
};
struct Fill {
  ExecutionKey key;
  Qty qty{};
  Price price{};
  bool operator==(const Fill &) const = default;
};
struct Canceled {
  std::optional<Id> request_id;
  bool operator==(const Canceled &) const = default;
};
struct Rejected {
  Id request_id{};
  bool operator==(const Rejected &) const = default;
};
enum class RejectReason : std::uint8_t {
  Permanent,
  Transient,
  RateLimited,
  Risk
};
struct RejectedWithReason {
  Id request_id{};
  RejectReason reason{};
  bool operator==(const RejectedWithReason &) const = default;
};
struct Expired {
  bool operator==(const Expired &) const = default;
};
struct Corrected {
  ExecutionKey key;
  Id revision{};
  Qty new_qty{};
  Price new_price{};
  bool operator==(const Corrected &) const = default;
};
struct Reconciled {
  Price price{};
  Qty total_qty{}, cum_filled{}, leaves{};
  Lifecycle lifecycle{};
  bool operator==(const Reconciled &) const = default;
};
using ReportKind =
    std::variant<Accepted, Fill, Replaced, Canceled, Rejected,
                 RejectedWithReason, Expired, Corrected, Reconciled>;
struct Report {
  Id source{}, sequence{}, order_id{};
  ReportKind kind;
  bool operator==(const Report &) const = default;
};
struct Timeout {
  Id order_id{}, request_id{};
  bool operator==(const Timeout &) const = default;
};
struct MarkUncertain {
  Id order_id{};
  bool operator==(const MarkUncertain &) const = default;
};
struct Submit {
  Action action;
  bool dispatch{};
  bool operator==(const Submit &) const = default;
};
struct Dispatch {
  Id order_id{}, request_id{}, expected_version{};
  bool operator==(const Dispatch &) const = default;
};
using Single = std::variant<Submit, Dispatch>;
using Event =
    std::variant<New, Cancel, Replace, Timeout, MarkUncertain, Report, Single>;
inline Id order_id(const Action &a) {
  return std::visit([](const auto &v) { return v.order_id; }, a);
}
inline Id request_id(const Action &a) {
  return std::visit([](const auto &v) { return v.request_id; }, a);
}
inline Event event(const Action &a) {
  return std::visit([](const auto &v) -> Event { return v; }, a);
}
inline Id order_id(const Event &e) {
  return std::visit(
      [](const auto &v) -> Id {
        if constexpr (std::is_same_v<std::decay_t<decltype(v)>, Single>)
          return std::visit(
              [](const auto &s) -> Id {
                if constexpr (std::is_same_v<std::decay_t<decltype(s)>, Submit>)
                  return order_id(s.action);
                else
                  return s.order_id;
              },
              v);
        else
          return v.order_id;
      },
      e);
}
struct Working {
  Price price{};
  Qty total_qty{};
  bool operator==(const Working &) const = default;
};
using Desired = std::optional<Working>; // nullopt = sticky Cancel
struct Intent {
  Id order_id{}, revision{}, request_id{};
  Desired desired;
};
enum class IntentStatus : std::uint8_t {
  Ready,
  Waiting,
  Reconcile,
  RiskBlocked,
  Rejected,
  Unexecutable,
  Resolved,
  Closed,
  Halted
};
struct IntentView {
  Intent intent;
  RequestState request_state;
  IntentStatus status;
};
struct Change {
  Id order_id{};
  std::optional<Id> exchange_id;
  Request request;
};
using Command = std::variant<New, Change>;
struct Outcome {
  Id order_id{}, version{};
  std::optional<Id> intent_revision;
  bool duplicate{};
  std::optional<Command> outbound;
};
enum class Error {
  Invalid,
  UnknownOrder,
  UnknownExecution,
  DuplicateId,
  ConflictingDuplicate,
  Capacity,
  StaleVersion,
  SequenceGap,
  PendingRequest,
  RequestMismatch,
  NeedsReconciliation,
  RiskLimit,
  Journal,
  Halted,
  InvalidDispatch,
  CancelRequested
};
struct Limits {
  std::size_t max_orders{10000}, max_requests{40000}, max_executions{100000},
      max_reports{200000};
  Qty max_order_qty{1'000'000}, max_open_qty_per_book{10'000'000};
};
inline Id increment(Id x) {
  if (x == std::numeric_limits<Id>::max())
    throw Error::Invalid;
  return x + 1;
}
inline Qty checked_add(Qty a, Qty b, Error error = Error::Invalid) {
  if (a > std::numeric_limits<Qty>::max() - b)
    throw error;
  return a + b;
}
} // namespace aligned_oms
