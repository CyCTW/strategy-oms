#pragma once
#include "oms_index.hpp"
#include <set>

namespace oms {
// Benchmark-only replace flow. Not a port of Engine: no WAL, dedup, risk,
// recovery, groups, request history, fills/corrections, or transport semantics.
template <class Locator, class IndexType = Index<Locator>> class Flow {
  struct State {
    Order order;
    typename IndexType::Membership memberships;
    std::uint64_t version{1}, pending_request{};
    std::optional<std::pair<std::uint64_t, Price>> desired;
  };
  IndexType index_;
  Pool<State> states_;
  std::unordered_map<std::uint64_t, Handle> ids_;
  std::set<std::uint64_t> ready_;
  std::uint64_t sequence_{};
  State &state(std::uint64_t id) { return require(states_.get(ids_.at(id))); }

public:
  struct Command {
    std::uint64_t id, request;
    Price price;
  };
  void seed(Order o) {
    if (ids_.contains(o.id))
      throw std::invalid_argument("duplicate order");
    const auto h = states_.insert(State{o, {}, 1, 0, {}});
    ids_.emplace(o.id, h);
    auto &s = require(states_.get(h));
    if constexpr (requires { index_.bind(s.memberships, s.order); })
      index_.bind(s.memberships, s.order);
    index_.update(nullptr, s.order, s.memberships);
  }
  std::uint64_t version(std::uint64_t id) { return state(id).version; }
  Price price(std::uint64_t id) { return state(id).order.price; }
  std::optional<Command> replace(std::uint64_t id, std::uint64_t req,
                                 std::uint64_t expected, Price price) {
    auto &s = state(id);
    if (expected != s.version)
      throw std::invalid_argument("stale version");
    if (s.order.terminal || s.order.uncertain)
      throw std::invalid_argument("not dispatchable");
    ++s.version;
    if (s.order.pending) {
      s.desired = {{req, price}};
      return {};
    }
    if (s.order.price == price) {
      s.desired.reset();
      ready_.erase(id);
      return {};
    }
    auto n = s.order;
    n.pending = Pending{Kind::Replace, price, n.total};
    index_.update(&s.order, n, s.memberships);
    s.order = n;
    s.pending_request = req;
    s.desired.reset();
    ready_.erase(id);
    return Command{id, req, price};
  }
  void acknowledge(std::uint64_t id, std::uint64_t req, std::uint64_t sequence,
                   Price price) {
    auto &s = state(id);
    if (sequence != sequence_ + 1 || !s.order.pending ||
        s.pending_request != req || s.order.pending->price != price)
      throw std::invalid_argument("invalid report");
    auto n = s.order;
    n.price = price;
    n.pending.reset();
    index_.update(&s.order, n, s.memberships);
    s.order = n;
    ++s.version;
    sequence_ = sequence;
    if (s.desired) {
      if (s.desired->second == price)
        s.desired.reset();
      else
        ready_.insert(id);
    }
  }
  std::optional<Command> dispatch() {
    if (ready_.empty())
      return {};
    const auto id = *ready_.begin();
    auto &s = state(id);
    if (s.order.pending || !s.desired)
      throw std::logic_error("invalid ready state");
    const auto [req, p] = *s.desired;
    return replace(id, req, s.version, p);
  }
  const IndexType &index() const { return index_; }
};
} // namespace oms
