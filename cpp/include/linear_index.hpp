#pragma once
#include "oms_index.hpp"

namespace oms {
// Compact unsorted directory. Pool Level/Member handles remain stable when
// entries move. Sorted ranges use repeated minima, with no hidden allocation.
struct LinearLocator {
  struct Entry {
    Price price;
    Handle value;
    bool working;
  };
  std::vector<Entry> entries;
  std::optional<Handle> get(const PagePool &, Price p) const {
    for (const auto &e : entries)
      if (e.price == p)
        return e.value;
    return {};
  }
  void insert(PagePool &, Price p, Handle h, bool w) {
    for (auto &e : entries)
      if (e.price == p) {
        e = {p, h, w};
        return;
      }
    entries.push_back({p, h, w});
  }
  void remove(PagePool &, Price p) {
    for (std::size_t i = 0; i < entries.size(); ++i)
      if (entries[i].price == p) {
        entries[i] = entries.back();
        entries.pop_back();
        return;
      }
  }
  void working(PagePool &, Price p, bool w) {
    for (auto &e : entries)
      if (e.price == p) {
        e.working = w;
        return;
      }
  }
  std::optional<Price> best(const PagePool &, bool buy) const {
    std::optional<Price> out;
    for (const auto &e : entries)
      if (e.working && (!out || (buy ? e.price > *out : e.price < *out)))
        out = e.price;
    return out;
  }
  template <class F>
  void range(const PagePool &, Price lo, Price hi, F &&f) const {
    std::optional<Price> previous;
    for (;;) {
      const Entry *next = nullptr;
      for (const auto &e : entries)
        if (e.price >= lo && e.price <= hi &&
            (!previous || e.price > *previous) &&
            (!next || e.price < next->price))
          next = &e;
      if (!next || !f(next->price, next->value))
        return;
      previous = next->price;
    }
  }
};

// No price directory, cached price aggregates, or cached best price. Active
// lists point at the SAME authoritative pooled Orders used by indexed variants;
// there is no shadow Order copy to refresh. Order and Membership addresses must
// stay stable. Single writer: update and assignment form one transition.
class OrderScanIndex {
public:
  struct Membership {
    std::optional<BookHandle> book;
    const Order *order{};
    std::size_t position{none};
  };

private:
  struct Entry {
    const Order *order;
    Membership *membership;
  };
  struct State {
    Book key;
    std::vector<Entry> active;
    Qty reserved{}, uncertain{};
  };
  std::uint64_t owner_ = new_owner();
  std::unordered_map<Book, BookHandle, BookHash> registry_;
  Pool<State> books_;
  const State *state(BookHandle h) const {
    return h.owner == owner_ ? books_.get(h.slot) : nullptr;
  }

public:
  OrderScanIndex() = default;
  OrderScanIndex(const OrderScanIndex &) = delete;
  OrderScanIndex &operator=(const OrderScanIndex &) = delete;
  void bind(Membership &m, const Order &o) { m.order = &o; }
  BookHandle register_book(const Book &key) {
    if (const auto i = registry_.find(key); i != registry_.end())
      return i->second;
    const BookHandle h{owner_, books_.insert(State{key, {}, 0, 0})};
    registry_.emplace(key, h);
    return h;
  }
  std::optional<BookHandle> book_handle(const Book &key) const {
    const auto i = registry_.find(key);
    return i == registry_.end() ? std::nullopt : std::optional(i->second);
  }
  void update(const Order *old, const Order &n, Membership &m) {
    if (!m.order || m.order->id != n.id || m.order->book != n.book)
      throw std::logic_error("unbound or mismatched authoritative order");
    if (!m.book)
      m.book = register_book(n.book);
    if (m.book->owner != owner_)
      throw std::logic_error("foreign membership");
    auto &s = require(books_.get(m.book->slot));
    if (old) {
      s.reserved -= old->reserved();
      s.uncertain -= old->uncertain;
    }
    s.reserved += n.reserved();
    s.uncertain += n.uncertain;
    const bool active = !n.terminal || n.pending.has_value();
    if (active && m.position == none) {
      m.position = s.active.size();
      s.active.push_back({m.order, &m});
    } else if (!active && m.position != none) {
      const auto pos = m.position;
      s.active[pos] = s.active.back();
      s.active[pos].membership->position = pos;
      s.active.pop_back();
      m.position = none;
    }
  }
  std::optional<Summary> summary(BookHandle h, Price p) const {
    const auto *s = state(h);
    if (!s)
      return {};
    Summary out{};
    for (const auto &e : s->active)
      for (const auto &c : contributions(e.order))
        if (c && c->price == p) {
          out.totals.add(c->totals);
          ++out.count;
        }
    return out.count ? std::optional(out) : std::nullopt;
  }
  std::optional<Price> best(BookHandle h) const {
    const auto *s = state(h);
    if (!s)
      return {};
    std::optional<Price> out;
    for (const auto &e : s->active)
      if (e.order->leaves && (!out || (s->key.buy ? e.order->price > *out
                                                  : e.order->price < *out)))
        out = e.order->price;
    return out;
  }
  Qty reserved(BookHandle h) const {
    const auto *s = state(h);
    return s ? s->reserved : 0;
  }
  Qty uncertain(BookHandle h) const {
    const auto *s = state(h);
    return s ? s->uncertain : 0;
  }
  template <class F> void orders_at(BookHandle h, Price p, F &&f) const {
    const auto *s = state(h);
    if (!s)
      return;
    for (const auto &e : s->active)
      for (const auto &c : contributions(e.order))
        if (c && c->price == p && !f(e.order->id))
          return;
  }
  template <class F> void range(BookHandle h, Price lo, Price hi, F &&f) const {
    const auto *s = state(h);
    if (!s)
      return;
    std::optional<Price> previous;
    for (;;) {
      std::optional<Price> next;
      Summary aggregate{};
      for (const auto &e : s->active)
        for (const auto &c : contributions(e.order)) {
          if (!c || c->price < lo || c->price > hi ||
              (previous && c->price <= *previous))
            continue;
          if (!next || c->price < *next) {
            next = c->price;
            aggregate = {};
          }
          if (c->price == *next) {
            aggregate.totals.add(c->totals);
            ++aggregate.count;
          }
        }
      if (!next || !f(*next, aggregate))
        return;
      previous = next;
    }
  }
};
} // namespace oms
