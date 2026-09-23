#pragma once
#include <absl/container/btree_map.h>
#include <absl/container/btree_set.h>
#include <algorithm>
#include <array>
#include <atomic>
#include <bit>
#include <cstdint>
#include <limits>
#include <memory>
#include <optional>
#include <stdexcept>
#include <unordered_map>
#include <utility>
#include <vector>

namespace oms {
using Price = std::int64_t;
using Qty = std::uint64_t;
constexpr auto none = std::numeric_limits<std::size_t>::max();
struct Handle {
  std::size_t slot{};
  std::uint64_t generation{};
  bool operator==(const Handle &) const = default;
};
template <class T, std::size_t Block = 64> class Pool {
  static_assert(Block > 0);
  struct Slot {
    std::uint64_t generation{1};
    std::optional<T> value;
    std::size_t next{none};
  };
  std::vector<std::unique_ptr<Slot[]>> blocks_;
  std::size_t free_{none}, size_{};

public:
  Handle insert(T value) {
    if (free_ == none) {
      const auto base = blocks_.size() * Block;
      auto b = std::make_unique<Slot[]>(Block);
      for (std::size_t i = 0; i < Block; ++i)
        b[i].next = i + 1 < Block ? base + i + 1 : none;
      blocks_.push_back(std::move(b));
      free_ = base;
    }
    const auto i = free_;
    auto &s = blocks_[i / Block][i % Block];
    s.value.emplace(std::move(value));
    free_ = s.next;
    s.next = none;
    ++size_;
    return {i, s.generation};
  }
  const T *get(Handle h) const {
    if (h.slot / Block >= blocks_.size())
      return nullptr;
    const auto &s = blocks_[h.slot / Block][h.slot % Block];
    return s.generation == h.generation && s.value ? &*s.value : nullptr;
  }
  T *get(Handle h) { return const_cast<T *>(std::as_const(*this).get(h)); }
  bool remove(Handle h) {
    if (!get(h))
      return false;
    auto &s = blocks_[h.slot / Block][h.slot % Block];
    s.value.reset();
    --size_;
    if (s.generation != std::numeric_limits<std::uint64_t>::max()) {
      ++s.generation;
      s.next = free_;
      free_ = h.slot;
    }
    return true;
  }
  std::size_t size() const { return size_; }
  std::size_t blocks() const { return blocks_.size(); }
};
template <class T> T &require(T *p) {
  if (!p)
    throw std::logic_error("invalid pool handle");
  return *p;
}
// Floor division with nonnegative remainder, including INT64_MIN.
inline Price page_id(Price p) { return p / 64 - (p % 64 < 0 ? 1 : 0); }
inline unsigned offset(Price p) {
  auto r = p % 64;
  return static_cast<unsigned>(r < 0 ? r + 64 : r);
}
struct Page {
  std::array<Handle, 64> values{};
  std::uint64_t occupied{}, working{};
};
using PagePool = Pool<Page>;

struct BtreeLocator {
  absl::btree_map<Price, Handle> levels;
  absl::btree_set<Price> confirmed;
  std::optional<Handle> get(const PagePool &, Price p) const {
    auto i = levels.find(p);
    return i == levels.end() ? std::nullopt : std::optional(i->second);
  }
  void insert(PagePool &, Price p, Handle h, bool w) {
    levels.insert_or_assign(p, h);
    if (w)
      confirmed.insert(p);
    else
      confirmed.erase(p);
  }
  void remove(PagePool &, Price p) {
    levels.erase(p);
    confirmed.erase(p);
  }
  void working(PagePool &, Price p, bool w) {
    if (w)
      confirmed.insert(p);
    else
      confirmed.erase(p);
  }
  std::optional<Price> best(const PagePool &, bool buy) const {
    if (confirmed.empty())
      return {};
    return buy ? *confirmed.rbegin() : *confirmed.begin();
  }
  template <class F>
  void range(const PagePool &, Price lo, Price hi, F &&f) const {
    if (lo > hi)
      return;
    for (auto i = levels.lower_bound(lo); i != levels.end() && i->first <= hi;
         ++i)
      if (!f(i->first, i->second))
        break;
  }
};
struct PagedLocator {
  absl::btree_map<Price, Handle> pages;
  absl::btree_set<Price> confirmed;
  void changed(Price p, bool was, bool now) {
    if (was == now)
      return;
    if (now)
      confirmed.insert(p);
    else
      confirmed.erase(p);
  }
  std::optional<Handle> get(const PagePool &pool, Price p) const {
    auto i = pages.find(page_id(p));
    if (i == pages.end())
      return {};
    const auto &page = require(pool.get(i->second));
    const auto n = offset(p);
    return page.occupied & (std::uint64_t{1} << n)
               ? std::optional(page.values[n])
               : std::nullopt;
  }
  void insert(PagePool &pool, Price p, Handle value, bool w) {
    const auto key = page_id(p);
    const auto n = offset(p);
    const auto bit = std::uint64_t{1} << n;
    auto i = pages.find(key);
    if (i == pages.end()) {
      Page page;
      page.values[n] = value;
      page.occupied = bit;
      page.working = w ? bit : 0;
      const auto h = pool.insert(std::move(page));
      pages.emplace(key, h);
      changed(key, false, w);
    } else {
      auto &page = require(pool.get(i->second));
      const bool was = page.working != 0;
      page.values[n] = value;
      page.occupied |= bit;
      if (w)
        page.working |= bit;
      else
        page.working &= ~bit;
      changed(key, was, page.working != 0);
    }
  }
  void remove(PagePool &pool, Price p) {
    const auto key = page_id(p);
    auto i = pages.find(key);
    if (i == pages.end())
      return;
    const auto h = i->second;
    auto &page = require(pool.get(h));
    const bool was = page.working != 0;
    const auto bit = std::uint64_t{1} << offset(p);
    page.occupied &= ~bit;
    page.working &= ~bit;
    changed(key, was, page.working != 0);
    if (!page.occupied) {
      pages.erase(key);
      pool.remove(h);
    }
  }
  void working(PagePool &pool, Price p, bool w) {
    const auto key = page_id(p);
    auto i = pages.find(key);
    if (i == pages.end())
      return;
    auto &page = require(pool.get(i->second));
    const auto bit = std::uint64_t{1} << offset(p);
    if (!(page.occupied & bit))
      return;
    const bool was = page.working != 0;
    if (w)
      page.working |= bit;
    else
      page.working &= ~bit;
    changed(key, was, page.working != 0);
  }
  std::optional<Price> best(const PagePool &pool, bool buy) const {
    if (confirmed.empty())
      return {};
    const auto key = buy ? *confirmed.rbegin() : *confirmed.begin();
    const auto bits = require(pool.get(pages.at(key))).working;
    const auto n = buy ? 63 - std::countl_zero(bits) : std::countr_zero(bits);
    return key * 64 + n;
  }
  template <class F>
  void range(const PagePool &pool, Price lo, Price hi, F &&f) const {
    if (lo > hi)
      return;
    const auto first = page_id(lo), last = page_id(hi);
    for (auto i = pages.lower_bound(first);
         i != pages.end() && i->first <= last; ++i) {
      const auto &page = require(pool.get(i->second));
      auto bits = page.occupied;
      if (i->first == first)
        bits &= ~std::uint64_t{} << offset(lo);
      if (i->first == last)
        bits &= ~std::uint64_t{} >> (63 - offset(hi));
      while (bits) {
        const auto n = std::countr_zero(bits);
        bits &= bits - 1;
        if (!f(i->first * 64 + n, page.values[static_cast<std::size_t>(n)]))
          return;
      }
    }
  }
};

struct Book {
  std::uint64_t strategy{}, account{1}, venue{1}, instrument{1};
  bool buy{true};
  bool operator==(const Book &) const = default;
};
struct BookHash {
  std::size_t operator()(const Book &b) const {
    std::uint64_t h = 0x9e3779b97f4a7c15ULL;
    for (auto v :
         {b.strategy, b.account, b.venue, b.instrument, std::uint64_t(b.buy)})
      h ^= v + 0x9e3779b97f4a7c15ULL + (h << 6) + (h >> 2);
    return static_cast<std::size_t>(h);
  }
};
struct Totals {
  Qty confirmed{}, new_qty{}, cancel_qty{}, replace_in{}, replace_out{},
      uncertain{};
  bool operator==(const Totals &) const = default;
  void add(const Totals &x) {
    confirmed += x.confirmed;
    new_qty += x.new_qty;
    cancel_qty += x.cancel_qty;
    replace_in += x.replace_in;
    replace_out += x.replace_out;
    uncertain += x.uncertain;
  }
  void sub(const Totals &x) {
    confirmed -= x.confirmed;
    new_qty -= x.new_qty;
    cancel_qty -= x.cancel_qty;
    replace_in -= x.replace_in;
    replace_out -= x.replace_out;
    uncertain -= x.uncertain;
  }
};
enum class Kind { New, Replace, Cancel };
struct Pending {
  Kind kind{};
  Price price{};
  Qty qty{};
};
struct Order {
  std::uint64_t id{};
  Book book;
  Price price{};
  Qty total{1'000'000}, cum{}, leaves{1'000'000};
  bool terminal{}, uncertain{}, pending_new{};
  std::optional<Pending> pending;
  Qty reserved() const {
    const auto remaining = total > cum ? total - cum : Qty{};
    auto qty = pending_new ? remaining : leaves;
    if (uncertain)
      qty = std::max(qty, remaining);
    if (pending && pending->kind != Kind::Cancel)
      qty = std::max(qty, pending->qty > cum ? pending->qty - cum : Qty{});
    return qty;
  }
};
struct Contribution {
  Price price;
  Totals totals;
};
inline std::array<std::optional<Contribution>, 2>
contributions(const Order *o) {
  if (!o || (o->terminal && !o->pending))
    return {};
  Totals t;
  t.confirmed = o->leaves;
  t.uncertain = o->uncertain;
  std::optional<Contribution> second;
  if (o->pending) {
    const auto p = *o->pending;
    const auto qty = p.qty > o->cum ? p.qty - o->cum : Qty{};
    if (p.kind == Kind::New)
      t.new_qty = qty;
    else if (p.kind == Kind::Cancel)
      t.cancel_qty = o->leaves;
    else {
      t.replace_out = o->leaves;
      if (p.price == o->price)
        t.replace_in = qty;
      else {
        Totals x;
        x.replace_in = qty;
        x.uncertain = o->uncertain;
        second = Contribution{p.price, x};
      }
    }
  }
  return {Contribution{o->price, t}, second};
}
struct BookHandle {
  std::uint64_t owner;
  Handle slot;
};
struct Memberships {
  std::array<std::optional<Handle>, 2> slots{};
  std::optional<BookHandle> book;
};
struct Level {
  Totals totals;
  std::optional<Handle> head;
  std::size_t count{};
};
struct Member {
  std::uint64_t id;
  Handle level;
  std::optional<Handle> prev, next;
};
struct Summary {
  Totals totals;
  std::size_t count;
  bool operator==(const Summary &) const = default;
};
struct Stats {
  std::size_t books, levels, members, pages, page_blocks;
};
inline std::uint64_t new_owner() {
  static std::atomic<std::uint64_t> next{1};
  auto id = next.load(std::memory_order_relaxed);
  do {
    if (id == std::numeric_limits<std::uint64_t>::max())
      throw std::overflow_error("index owner exhausted");
  } while (!next.compare_exchange_weak(id, id + 1, std::memory_order_relaxed));
  return id;
}
template <class Locator, class OrderType = Order> class Index {
  struct BookState {
    Locator locator;
    Qty reserved{}, uncertain{};
    std::optional<Price> best;
  };
  std::uint64_t owner_ = new_owner();
  std::unordered_map<Book, BookHandle, BookHash> registry_;
  Pool<BookState> books_;
  Pool<Level> levels_;
  Pool<Member> members_;
  PagePool pages_;
  const BookState *state(BookHandle h) const {
    return h.owner == owner_ ? books_.get(h.slot) : nullptr;
  }
  void working(BookHandle h, bool buy, Price p, bool w) {
    auto &b = require(books_.get(h.slot));
    b.locator.working(pages_, p, w);
    if (w) {
      if (!b.best || (buy ? p > *b.best : p < *b.best))
        b.best = p;
    } else if (b.best == p)
      b.best = b.locator.best(pages_, buy);
  }
  void refresh(BookHandle h, bool buy) {
    auto &b = require(books_.get(h.slot));
    b.best = b.locator.best(pages_, buy);
  }

public:
  using Membership = Memberships;
  Index() = default;
  Index(const Index &) = delete;
  Index &operator=(const Index &) = delete;
  BookHandle register_book(const Book &b) {
    auto i = registry_.find(b);
    if (i != registry_.end())
      return i->second;
    BookHandle h{owner_, books_.insert(BookState{})};
    registry_.emplace(b, h);
    return h;
  }
  std::optional<BookHandle> book_handle(const Book &b) const {
    auto i = registry_.find(b);
    return i == registry_.end() ? std::nullopt : std::optional(i->second);
  }
  std::optional<Summary> summary(BookHandle h, Price p) const {
    const auto *b = state(h);
    if (!b)
      return {};
    const auto l = b->locator.get(pages_, p);
    if (!l)
      return {};
    const auto &v = require(levels_.get(*l));
    return Summary{v.totals, v.count};
  }
  std::optional<Price> best(BookHandle h) const {
    const auto *b = state(h);
    return b ? b->best : std::nullopt;
  }
  Qty reserved(BookHandle h) const {
    const auto *b = state(h);
    return b ? b->reserved : 0;
  }
  Qty uncertain(BookHandle h) const {
    const auto *b = state(h);
    return b ? b->uncertain : 0;
  }
  template <class F> void range(BookHandle h, Price lo, Price hi, F &&f) const {
    const auto *b = state(h);
    if (!b)
      return;
    b->locator.range(pages_, lo, hi, [&](Price p, Handle l) {
      const auto &v = require(levels_.get(l));
      return f(p, Summary{v.totals, v.count});
    });
  }
  template <class F> void orders_at(BookHandle h, Price p, F &&f) const {
    const auto *b = state(h);
    if (!b)
      return;
    const auto lh = b->locator.get(pages_, p);
    if (!lh)
      return;
    auto m = require(levels_.get(*lh)).head;
    while (m) {
      const auto &v = require(members_.get(*m));
      if (!f(v.id))
        return;
      m = v.next;
    }
  }
  Stats stats() const {
    return {books_.size(), levels_.size(), members_.size(), pages_.size(),
            pages_.blocks()};
  }
  // Internal single-writer transition: old/new must be same order and Book.
  // Caller owns the authoritative Order and these Memberships together.
  void update(const OrderType *old, const OrderType &n, Memberships &slots) {
    if (!slots.book)
      slots.book = register_book(n.book);
    const auto bh = *slots.book;
    auto &b = require(books_.get(bh.slot));
    if (bh.owner != owner_)
      throw std::logic_error("foreign membership");
    if (old) {
      b.reserved -= old->reserved();
      b.uncertain -= old->uncertain;
    }
    b.reserved += n.reserved();
    b.uncertain += n.uncertain;
    const auto before = contributions(old), after = contributions(&n);
    std::array<std::optional<Handle>, 2> next{};
    for (std::size_t i = 0; i < 2; ++i) {
      if (!before[i])
        continue;
      const auto [price, totals] = *before[i];
      const auto mh = slots.slots[i].value();
      const auto m = require(members_.get(mh));
      auto &level = require(levels_.get(m.level));
      const bool was = level.totals.confirmed > 0;
      level.totals.sub(totals);
      std::size_t j = 0;
      while (j < 2 && (!after[j] || after[j]->price != price))
        ++j;
      if (j < 2) {
        level.totals.add(after[j]->totals);
        const bool w = level.totals.confirmed > 0;
        if (was != w)
          working(bh, n.book.buy, price, w);
        next[j] = mh;
      } else {
        --level.count;
        if (m.prev)
          require(members_.get(*m.prev)).next = m.next;
        else
          level.head = m.next;
        if (m.next)
          require(members_.get(*m.next)).prev = m.prev;
        members_.remove(mh);
        const bool w = level.totals.confirmed > 0;
        if (!level.count) {
          b.locator.remove(pages_, price);
          levels_.remove(m.level);
          if (was)
            refresh(bh, n.book.buy);
        } else if (was != w)
          working(bh, n.book.buy, price, w);
      }
    }
    for (std::size_t i = 0; i < 2; ++i) {
      if (!after[i] || next[i])
        continue;
      const auto [price, totals] = *after[i];
      auto lh = b.locator.get(pages_, price);
      if (!lh) {
        lh = levels_.insert(Level{});
        b.locator.insert(pages_, price, *lh, false);
      }
      auto &l = require(levels_.get(*lh));
      const bool was = l.totals.confirmed > 0;
      const auto mh = members_.insert(Member{n.id, *lh, std::nullopt, l.head});
      if (l.head)
        require(members_.get(*l.head)).prev = mh;
      l.head = mh;
      ++l.count;
      l.totals.add(totals);
      const bool w = l.totals.confirmed > 0;
      if (was != w)
        working(bh, n.book.buy, price, w);
      next[i] = mh;
    }
    slots.slots = next;
  }
};
} // namespace oms
