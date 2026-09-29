#pragma once
#include "oms_index.hpp"
#include <cstring>
#include <type_traits>

namespace oms {
// Double-ended sorted array. Prices ascend in one contiguous buffer with spare
// slots at BOTH ends: bids keep their best price at the tail, asks at the head,
// so a new best (or new worst) price on either side is an O(1) push. Interior
// inserts/removes shift toward the nearer end. When an end runs out of space
// the buffer recenters if at least a quarter is spare, otherwise it doubles;
// there is no price-count or price-range limit. Level/Member handles live in
// the shared pools, so moving entries never invalidates them.
//
// best() scans inward from the side's best end past pending-only prices; it is
// O(k) in the number of consecutive non-working prices there. Index caches
// best and only calls this on refresh.
//
// Storage decides the memory layout only; the algorithm is identical:
//   AosStorage: {price, handle, working} together (32 bytes per price);
//               branch-free count (<= 16) / binary search.
//   SoaStorage: prices in their own array (8 bytes per price); moves copy two
//               arrays. Contiguous keys allow a branch-free search: count the
//               keys below p (<= 64 prices, vectorizable), otherwise a
//               branchless binary search.

struct AosStorage {
  struct Entry {
    Price price;
    Handle value;
    bool working;
  };
  static_assert(std::is_trivially_copyable_v<Entry>);
  static constexpr bool contiguous_keys = false;
  std::unique_ptr<Entry[]> buf;
  void allocate(std::size_t n) { buf = std::make_unique<Entry[]>(n); }
  Price price(std::size_t i) const { return buf[i].price; }
  Handle value(std::size_t i) const { return buf[i].value; }
  bool working(std::size_t i) const { return buf[i].working; }
  void set_working(std::size_t i, bool w) { buf[i].working = w; }
  void set(std::size_t i, Price p, Handle h, bool w) { buf[i] = {p, h, w}; }
  void move(std::size_t dst, std::size_t src, std::size_t n) {
    std::memmove(&buf[dst], &buf[src], n * sizeof(Entry));
  }
  void copy_from(const AosStorage &o, std::size_t dst, std::size_t src,
                 std::size_t n) {
    std::memcpy(&buf[dst], &o.buf[src], n * sizeof(Entry));
  }
};

struct SoaStorage {
  struct Value {
    Handle value;
    bool working;
  };
  static_assert(std::is_trivially_copyable_v<Value>);
  static constexpr bool contiguous_keys = true;
  std::unique_ptr<Price[]> keys;
  std::unique_ptr<Value[]> vals;
  void allocate(std::size_t n) {
    keys = std::make_unique<Price[]>(n);
    vals = std::make_unique<Value[]>(n);
  }
  Price price(std::size_t i) const { return keys[i]; }
  Handle value(std::size_t i) const { return vals[i].value; }
  bool working(std::size_t i) const { return vals[i].working; }
  void set_working(std::size_t i, bool w) { vals[i].working = w; }
  void set(std::size_t i, Price p, Handle h, bool w) {
    keys[i] = p;
    vals[i] = {h, w};
  }
  void move(std::size_t dst, std::size_t src, std::size_t n) {
    std::memmove(&keys[dst], &keys[src], n * sizeof(Price));
    std::memmove(&vals[dst], &vals[src], n * sizeof(Value));
  }
  void copy_from(const SoaStorage &o, std::size_t dst, std::size_t src,
                 std::size_t n) {
    std::memcpy(&keys[dst], &o.keys[src], n * sizeof(Price));
    std::memcpy(&vals[dst], &o.vals[src], n * sizeof(Value));
  }
};

template <class Storage> struct SortedDeque {
  static constexpr std::size_t linear_max = 16;

  Storage s;
  std::size_t cap{}, head{}, tail{};

  std::size_t size() const { return tail - head; }

  std::optional<Handle> get(const PagePool &, Price p) const {
    const auto i = lower(p);
    if (i < tail && s.price(i) == p)
      return s.value(i);
    return {};
  }
  void insert(PagePool &, Price p, Handle h, bool w) {
    auto i = lower(p);
    if (i < tail && s.price(i) == p) {
      s.set(i, p, h, w);
      return;
    }
    if (size() == cap)
      grow();
    i = lower(p);
    if (i == tail) { // new maximum
      if (tail == cap)
        make_room(false);
      s.set(tail++, p, h, w);
      return;
    }
    if (i == head) { // new minimum
      if (head == 0)
        make_room(true);
      s.set(--head, p, h, w);
      return;
    }
    // Shift the shorter side; if its end is full, recenter (or grow) first
    // instead of shifting the longer side.
    const bool left = i - head < tail - i;
    if (left && head == 0)
      make_room(true);
    else if (!left && tail == cap)
      make_room(false);
    i = lower(p);
    if (left) {
      s.move(head - 1, head, i - head);
      --head;
      s.set(i - 1, p, h, w);
    } else {
      s.move(i + 1, i, tail - i);
      ++tail;
      s.set(i, p, h, w);
    }
  }
  void remove(PagePool &, Price p) {
    const auto i = lower(p);
    if (i >= tail || s.price(i) != p)
      return;
    if (i == tail - 1)
      --tail;
    else if (i == head)
      ++head;
    else if (i - head < tail - i) {
      s.move(head + 1, head, i - head);
      ++head;
    } else {
      s.move(i, i + 1, tail - i - 1);
      --tail;
    }
    if (head == tail)
      head = tail = cap / 2;
  }
  void working(PagePool &, Price p, bool w) {
    const auto i = lower(p);
    if (i < tail && s.price(i) == p)
      s.set_working(i, w);
  }
  bool is_working(Price p) const {
    const auto i = lower(p);
    return i < tail && s.price(i) == p && s.working(i);
  }
  // Lowest / highest stored price; requires size() > 0.
  Price front_price() const { return s.price(head); }
  Price back_price() const { return s.price(tail - 1); }
  std::optional<Price> best(const PagePool &, bool buy) const {
    if (buy) {
      for (auto i = tail; i > head; --i)
        if (s.working(i - 1))
          return s.price(i - 1);
    } else {
      for (auto i = head; i < tail; ++i)
        if (s.working(i))
          return s.price(i);
    }
    return {};
  }
  template <class F>
  void range(const PagePool &, Price lo, Price hi, F &&f) const {
    if (lo > hi)
      return;
    for (auto i = lower(lo); i < tail && s.price(i) <= hi; ++i)
      if (!f(s.price(i), s.value(i)))
        return;
  }

private:
  // First position whose price >= p, in [head, tail].
  std::size_t lower(Price p) const {
    if constexpr (Storage::contiguous_keys) {
      const Price *k = s.keys.get() + head;
      std::size_t n = size();
      if (n <= 64) {
        std::size_t below = 0;
        for (std::size_t i = 0; i < n; ++i)
          below += k[i] < p;
        return head + below;
      }
      while (n > 1) {
        const auto half = n / 2;
        k = k[half] < p ? k + half : k;
        n -= half;
      }
      return std::size_t(k - s.keys.get()) + (*k < p);
    }
    if (size() <= linear_max) {
      // Branch-free: count the prices below p.
      std::size_t below = 0;
      for (auto i = head; i < tail; ++i)
        below += s.price(i) < p;
      return head + below;
    }
    std::size_t lo = head, hi = tail;
    while (lo < hi) {
      const auto mid = lo + (hi - lo) / 2;
      if (s.price(mid) < p)
        lo = mid + 1;
      else
        hi = mid;
    }
    return lo;
  }
  // Requires size() < cap. Recenter when at least a quarter is spare (amortized
  // O(1) per end push), otherwise double.
  void make_room(bool front) {
    if ((cap - size()) * 4 < cap) {
      grow();
      return;
    }
    const auto n = size(), spare = cap - n;
    const auto h = front ? (spare + 1) / 2 : spare / 2;
    s.move(h, head, n);
    head = h;
    tail = h + n;
  }
  void grow() {
    const auto n = size();
    const auto next = std::max<std::size_t>(8, cap * 2);
    Storage fresh;
    fresh.allocate(next);
    const auto h = (next - n) / 2;
    if (n)
      fresh.copy_from(s, h, head, n);
    s = std::move(fresh);
    cap = next;
    head = h;
    tail = h + n;
  }
};

using SortedDequeLocator = SortedDeque<AosStorage>;
using SortedDequeSoaLocator = SortedDeque<SoaStorage>;
} // namespace oms

namespace oms {
// Double-ended array while a Book has few prices, Abseil B-tree once it grows
// past Promote prices; back to the array below Demote (hysteresis, so a Book
// hovering near one threshold does not convert back and forth). A conversion
// copies every price once: an O(P) spike on that single operation, measured
// in the growth benchmarks.
template <std::size_t Promote = 1024, std::size_t Demote = 256>
struct AdaptiveLocator {
  static_assert(Demote < Promote);
  SortedDequeLocator small;
  BtreeLocator large;
  bool big{false};

  std::optional<Handle> get(const PagePool &pool, Price p) const {
    return big ? large.get(pool, p) : small.get(pool, p);
  }
  void insert(PagePool &pool, Price p, Handle h, bool w) {
    if (big) {
      large.insert(pool, p, h, w);
      return;
    }
    small.insert(pool, p, h, w);
    if (small.size() > Promote) {
      for (auto i = small.head; i < small.tail; ++i)
        large.insert(pool, small.s.price(i), small.s.value(i),
                     small.s.working(i));
      small = SortedDequeLocator{};
      big = true;
    }
  }
  void remove(PagePool &pool, Price p) {
    if (!big) {
      small.remove(pool, p);
      return;
    }
    large.remove(pool, p);
    if (large.levels.size() < Demote) {
      for (const auto &[price, handle] : large.levels) // ascending: tail pushes
        small.insert(pool, price, handle, large.confirmed.contains(price));
      large = BtreeLocator{};
      big = false;
    }
  }
  void working(PagePool &pool, Price p, bool w) {
    big ? large.working(pool, p, w) : small.working(pool, p, w);
  }
  std::optional<Price> best(const PagePool &pool, bool buy) const {
    return big ? large.best(pool, buy) : small.best(pool, buy);
  }
  template <class F>
  void range(const PagePool &pool, Price lo, Price hi, F &&f) const {
    big ? large.range(pool, lo, hi, std::forward<F>(f))
        : small.range(pool, lo, hi, std::forward<F>(f));
  }
  std::size_t size() const { return big ? large.levels.size() : small.size(); }
  bool is_working(Price p) const {
    return big ? large.confirmed.contains(p) : small.is_working(p);
  }
  // Lowest and highest stored price, O(1).
  std::optional<std::pair<Price, Price>> bounds() const {
    if (big)
      return large.levels.empty()
                 ? std::nullopt
                 : std::optional(std::pair(large.levels.begin()->first,
                                           large.levels.rbegin()->first));
    return small.size() ? std::optional(std::pair(small.front_price(),
                                                  small.back_price()))
                        : std::nullopt;
  }
};
} // namespace oms
