#pragma once
#include "aligned_engine.hpp"
#include <set>
#include <sstream>
namespace parity {
using namespace aligned_oms;
struct Hash {
  Id value{14695981039346656037ULL};
  void add(Id v) {
    for (int i = 0; i < 8; ++i) {
      value ^= v & 255;
      value *= 1099511628211ULL;
      v >>= 8;
    }
  }
  void opt(std::optional<Id> v) {
    add(v.has_value());
    if (v)
      add(*v);
  }
};
inline void encode(Hash &h, const Book &b) {
  h.add(b.strategy);
  h.add(b.account);
  h.add(b.venue);
  h.add(b.instrument);
  h.add(b.buy ? 0 : 1);
}
inline void encode(Hash &h, const ExecutionKey &k) {
  h.add(k.venue);
  h.add(k.account);
  h.add(k.trading_day);
  h.add(k.execution_id);
}
inline void encode(Hash &h, const Request &r) {
  h.add(r.id);
  h.add(r.order_id);
  h.add(static_cast<Id>(r.kind));
  h.add(r.price);
  h.add(r.total_qty);
  h.add(static_cast<Id>(r.state));
}
inline void encode(Hash &h, const New &n) {
  h.add(n.order_id);
  h.add(n.request_id);
  encode(h, n.book);
  h.add(n.price);
  h.add(n.total_qty);
}
inline void encode(Hash &h, const Cancel &c) {
  h.add(c.order_id);
  h.add(c.request_id);
  h.add(c.expected_version);
}
inline void encode(Hash &h, const Replace &r) {
  h.add(r.order_id);
  h.add(r.request_id);
  h.add(r.expected_version);
  h.add(r.price);
  h.add(r.total_qty);
}
inline void encode(Hash &h, const Action &a) {
  h.add(a.index());
  std::visit([&](const auto &v) { encode(h, v); }, a);
}
inline void encode(Hash &h, const ReportKind &k) {
  h.add(k.index());
  std::visit(
      [&](const auto &r) {
        using R = std::decay_t<decltype(r)>;
        if constexpr (std::is_same_v<R, Accepted> ||
                      std::is_same_v<R, Replaced>) {
          h.add(r.request_id);
          h.add(r.exchange_id);
          h.add(r.price);
          h.add(r.total_qty);
        } else if constexpr (std::is_same_v<R, Fill>) {
          encode(h, r.key);
          h.add(r.qty);
          h.add(r.price);
        } else if constexpr (std::is_same_v<R, Canceled>)
          h.opt(r.request_id);
        else if constexpr (std::is_same_v<R, Rejected>)
          h.add(r.request_id);
        else if constexpr (std::is_same_v<R, RejectedWithReason>) {
          h.add(r.request_id);
          h.add(static_cast<Id>(r.reason));
        } else if constexpr (std::is_same_v<R, Corrected>) {
          encode(h, r.key);
          h.add(r.revision);
          h.add(r.new_qty);
          h.add(r.new_price);
        } else if constexpr (std::is_same_v<R, Reconciled>) {
          h.add(r.price);
          h.add(r.total_qty);
          h.add(r.cum_filled);
          h.add(r.leaves);
          h.add(static_cast<Id>(r.lifecycle));
        }
      },
      k);
}
inline void encode(Hash &h, const Event &e) {
  h.add(e.index());
  std::visit(
      [&](const auto &v) {
        using T = std::decay_t<decltype(v)>;
        if constexpr (std::is_same_v<T, New> || std::is_same_v<T, Cancel> ||
                      std::is_same_v<T, Replace>)
          encode(h, v);
        else if constexpr (std::is_same_v<T, Timeout>) {
          h.add(v.order_id);
          h.add(v.request_id);
        } else if constexpr (std::is_same_v<T, MarkUncertain>)
          h.add(v.order_id);
        else if constexpr (std::is_same_v<T, Report>) {
          h.add(v.source);
          h.add(v.sequence);
          h.add(v.order_id);
          encode(h, v.kind);
        } else {
          h.add(v.index());
          std::visit(
              [&](const auto &s) {
                using S = std::decay_t<decltype(s)>;
                if constexpr (std::is_same_v<S, Submit>) {
                  encode(h, s.action);
                  h.add(s.dispatch);
                } else {
                  h.add(s.order_id);
                  h.add(s.request_id);
                  h.add(s.expected_version);
                }
              },
              v);
        }
      },
      e);
}
inline void encode(Hash &h, const Outcome &o) {
  h.add(o.order_id);
  h.add(o.version);
  h.opt(o.intent_revision);
  h.add(o.duplicate);
  h.add(o.outbound.has_value());
  if (o.outbound) {
    h.add(o.outbound->index());
    std::visit(
        [&](const auto &v) {
          using T = std::decay_t<decltype(v)>;
          if constexpr (std::is_same_v<T, New>)
            encode(h, v);
          else {
            h.add(v.order_id);
            h.opt(v.exchange_id);
            encode(h, v.request);
          }
        },
        *o.outbound);
  }
}
inline Book book(Id id = 1, bool buy = true) { return {id, 1, 1, 1, buy}; }
struct Known {
  std::set<Id> orders, requests, executions;
  std::set<std::pair<Id, bool>> books;
};
template <class L> Id snapshot(const Engine<L> &e, const Known &known) {
  Hash h;
  h.add(e.halted());
  for (Id i = 1; i <= 4; ++i)
    h.add(e.last_sequence(i));
  for (auto id : known.orders) {
    const auto *o = e.order(id);
    h.add(o != nullptr);
    if (!o)
      continue;
    h.add(o->id);
    encode(h, o->book);
    h.opt(o->exchange_id);
    h.add(o->price);
    h.add(o->total_qty);
    h.add(o->cum_filled);
    h.add(o->leaves);
    h.add(static_cast<Id>(o->lifecycle));
    h.add(o->pending.has_value());
    if (o->pending)
      encode(h, *o->pending);
    h.add(o->uncertain);
    h.add(o->version);
    const auto i = e.intent(id);
    h.add(i.intent.revision);
    h.add(i.intent.request_id);
    h.add(i.intent.desired.has_value());
    if (i.intent.desired) {
      h.add(i.intent.desired->price);
      h.add(i.intent.desired->total_qty);
    }
    h.add(static_cast<Id>(i.request_state));
    h.add(static_cast<Id>(i.status));
  }
  for (auto id : known.requests) {
    const auto *r = e.request(id);
    h.add(r != nullptr);
    if (r)
      encode(h, *r);
  }
  for (auto id : known.executions) {
    const auto *x = e.execution({1, 1, 20260922, id});
    h.add(x != nullptr);
    if (!x)
      continue;
    encode(h, x->key);
    h.add(x->order_id);
    h.add(x->original_qty);
    h.add(x->original_price);
    h.add(x->qty);
    h.add(x->price);
    h.add(x->revision);
  }
  for (auto [id, buy] : known.books) {
    auto b = book(id, buy);
    const auto bh = e.index().book_handle(b);
    h.add(e.reserved(b));
    h.add(e.uncertain(b));
    h.opt(bh ? e.index().best(*bh) : std::nullopt);
    if (bh)
      e.index().range(*bh, std::numeric_limits<Price>::min(),
                      std::numeric_limits<Price>::max(),
                      [&](Price p, oms::Summary s) {
                        h.add(p);
                        h.add(s.totals.confirmed);
                        h.add(s.totals.new_qty);
                        h.add(s.totals.cancel_qty);
                        h.add(s.totals.replace_in);
                        h.add(s.totals.replace_out);
                        h.add(s.totals.uncertain);
                        h.add(s.count);
                        std::vector<Id> ids;
                        e.index().orders_at(*bh, p, [&](Id i) {
                          ids.push_back(i);
                          return true;
                        });
                        std::sort(ids.begin(), ids.end());
                        for (auto i : ids)
                          h.add(i);
                        return true;
                      });
    h.add(0xabcdef);
  }
  return h.value;
}
inline const char *error_name(Error e) {
  switch (e) {
#define E(x)                                                                   \
  case Error::x:                                                               \
    return #x
    E(Invalid);
    E(UnknownOrder);
    E(UnknownExecution);
    E(DuplicateId);
    E(ConflictingDuplicate);
    E(Capacity);
    E(StaleVersion);
    E(SequenceGap);
    E(PendingRequest);
    E(RequestMismatch);
    E(NeedsReconciliation);
    E(RiskLimit);
    E(Journal);
    E(Halted);
    E(InvalidDispatch);
    E(CancelRequested);
#undef E
  }
  return "UNREACHABLE";
}
// Stable text trace protocol. Parsing and expected-version resolution are never
// included in benchmark timing. -1 version means read current Order.version.
template <class L>
std::optional<Event> parse(const std::vector<std::string> &t,
                           const Engine<L> &e, Known &known) {
  auto u = [&](std::size_t i) { return std::stoull(t.at(i)); };
  auto p = [&](std::size_t i) { return std::stoll(t.at(i)); };
  const auto id = u(1);
  known.orders.insert(id);
  auto version = [&](std::size_t i) {
    return t.at(i) == "-1" ? (e.order(id) ? e.order(id)->version : 0) : u(i);
  };
  if (t[0] == "NEW") {
    known.requests.insert(u(2));
    known.books.insert({u(3), u(4) == 0});
    return New{id, u(2), book(u(3), u(4) == 0), p(5), u(6)};
  }
  if (t[0] == "REPLACE") {
    known.requests.insert(u(2));
    return Replace{id, u(2), version(3), p(4), u(5)};
  }
  if (t[0] == "CANCEL") {
    known.requests.insert(u(2));
    return Cancel{id, u(2), version(3)};
  }
  if (t[0] == "TIMEOUT")
    return Timeout{id, u(2)};
  if (t[0] == "MARK")
    return MarkUncertain{id};
  if (t[0] != "REPORT" && t[0] != "RAW")
    throw std::runtime_error("unknown trace operation");
  Report r{u(2), u(3), id, Expired{}};
  const auto &kind = t.at(4);
  if (kind == "ACCEPT" || kind == "REPLACED") {
    if (kind == "ACCEPT")
      r.kind = Accepted{u(5), u(6), p(7), u(8)};
    else
      r.kind = Replaced{u(5), u(6), p(7), u(8)};
  } else if (kind == "FILL") {
    known.executions.insert(u(5));
    r.kind = Fill{{1, 1, 20260922, u(5)}, u(6), p(7)};
  } else if (kind == "CANCELED")
    r.kind = Canceled{u(5) ? std::optional<Id>(u(5)) : std::nullopt};
  else if (kind == "REJECT")
    r.kind = Rejected{u(5)};
  else if (kind == "REASON")
    r.kind = RejectedWithReason{u(5), static_cast<RejectReason>(u(6))};
  else if (kind == "EXPIRED")
    r.kind = Expired{};
  else if (kind == "CORRECT") {
    known.executions.insert(u(5));
    r.kind = Corrected{{1, 1, 20260922, u(5)}, u(6), u(7), p(8)};
  } else if (kind == "RECONCILE")
    r.kind = Reconciled{p(5), u(6), u(7), u(8), static_cast<Lifecycle>(u(9))};
  else
    throw std::runtime_error("unknown report kind");
  return r;
}
} // namespace parity
