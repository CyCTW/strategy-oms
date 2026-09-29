#pragma once
#include "sorted_deque_locator.hpp"
#include <bit>
#include <type_traits>

namespace oms {
// Sliding-window price locator (port of Rust src/price_slide.rs): a circular
// direct-mapped window of N slots that follows the near-market prices, plus an
// AdaptiveLocator (sorted double-ended array, B-tree when large) for every
// price outside the window.
//
// Slots are addressed by price mod N, so sliding only moves `base`. Invariant:
// a price lives in the window iff base <= price < base + N; the outside
// locator never holds a price in that range. A new price within N/2 of the
// window slides it (N/4 of slack): levels that fall out are evicted to the
// outside locator, outside prices that fall in are pulled in. Farther prices
// (outliers) go to the outside locator directly. Inside the window every
// operation is O(1) bit/slot work; best and ordered ranges rotate the bitmaps
// into price order.
//
// A Book starts with the outside locator only; the window is created once the
// Book holds more than `tier` prices, so small Books and many-Book workloads
// never pay for it. Prices are raw units: with a tick > 1, N slots cover
// N / tick ticks.
__extension__ using u128 = unsigned __int128;
__extension__ using i128 = __int128;

namespace slide_bits {
template <class B> constexpr unsigned width = sizeof(B) * 8;
template <class B> constexpr B bit(unsigned i) { return B(1) << i; }
template <class B> constexpr B rotr(B x, unsigned n) {
  return n == 0 ? x : B((x >> n) | (x << (width<B> - n)));
}
inline unsigned clz(std::uint64_t x) { return unsigned(std::countl_zero(x)); }
inline unsigned ctz(std::uint64_t x) { return unsigned(std::countr_zero(x)); }
inline unsigned clz(u128 x) {
  const auto hi = std::uint64_t(x >> 64);
  return hi ? clz(hi) : 64 + clz(std::uint64_t(x));
}
inline unsigned ctz(u128 x) {
  const auto lo = std::uint64_t(x);
  return lo ? ctz(lo) : 64 + ctz(std::uint64_t(x >> 64));
}
// Bits lo..=hi set (lo <= hi < width).
template <class B> constexpr B span(unsigned lo, unsigned hi) {
  const B all = ~B(0);
  return B(all << lo) & B(all >> (width<B> - 1 - hi));
}
} // namespace slide_bits

template <std::size_t N = 128> struct SlideLocator {
  static_assert(N == 64 || N == 128);
  using Bits = std::conditional_t<N == 64, std::uint64_t, u128>;
  static constexpr std::size_t tier = 8;
  static constexpr Price n = Price(N);
  static constexpr Price max_base = std::numeric_limits<Price>::max() - (n - 1);

  struct Window {
    Price base{};
    Bits occupied{}, working{};
    std::array<Handle, N> slots{}; // indexed by price mod N
    bool contains(Price p) const {
      return std::uint64_t(p) - std::uint64_t(base) < std::uint64_t(N);
    }
    static unsigned slot(Price p) {
      return unsigned(std::uint64_t(p) & (N - 1));
    }
    bool has(Price p) const {
      return (occupied & slide_bits::bit<Bits>(slot(p))) != 0;
    }
    void put(Price p, Handle h, bool w) {
      const auto m = slide_bits::bit<Bits>(slot(p));
      slots[slot(p)] = h;
      occupied |= m;
      working = w ? Bits(working | m) : Bits(working & ~m);
    }
    void clear(Price p) {
      const auto m = Bits(~slide_bits::bit<Bits>(slot(p)));
      occupied &= m;
      working &= m;
    }
    // Rotates a slot-space bitmap so bit k is price base + k.
    Bits ordered(Bits x) const { return slide_bits::rotr(x, slot(base)); }
    std::optional<Price> best(bool buy) const {
      const auto r = ordered(working);
      if (!r)
        return {};
      return buy ? base + Price(N - 1 - slide_bits::clz(r))
                 : base + Price(slide_bits::ctz(r));
    }
  };

  std::unique_ptr<Window> window;
  AdaptiveLocator<> outside;

  std::optional<Handle> get(const PagePool &pool, Price p) const {
    if (window && window->contains(p)) {
      if (window->has(p))
        return window->slots[Window::slot(p)];
      return {};
    }
    return outside.get(pool, p);
  }
  void insert(PagePool &pool, Price p, Handle h, bool w) {
    if (window && window->contains(p)) {
      window->put(p, h, w);
      return;
    }
    const auto t = target(p);
    if (t.cover && !outside.get(pool, p)) {
      cover(pool, t, p, h);
      window->put(p, h, w);
    } else {
      outside.insert(pool, p, h, w);
    }
  }
  void remove(PagePool &pool, Price p) {
    if (window && window->contains(p))
      window->clear(p);
    else
      outside.remove(pool, p);
  }
  void working(PagePool &pool, Price p, bool w) {
    if (window && window->contains(p)) {
      if (window->has(p))
        window->put(p, window->slots[Window::slot(p)], w);
      return;
    }
    outside.working(pool, p, w);
  }
  std::optional<Price> best(const PagePool &pool, bool buy) const {
    const auto inner = window ? window->best(buy) : std::nullopt;
    const auto outer = outside.best(pool, buy);
    if (inner && outer)
      return buy ? std::max(*inner, *outer) : std::min(*inner, *outer);
    return inner ? inner : outer;
  }
  // Ascending: outside prices below the window, the window, then outside
  // prices above it; outside segments only when its bounds can intersect.
  template <class F>
  void range(const PagePool &pool, Price lo, Price hi, F &&f) const {
    if (lo > hi)
      return;
    if (!window) {
      outside.range(pool, lo, hi, f);
      return;
    }
    const auto &w = *window;
    const Price top = w.base + (n - 1);
    const auto bounds = outside.bounds();
    bool go = true;
    auto relay = [&](Price p, Handle h) { return go = f(p, h); };
    if (w.base > std::numeric_limits<Price>::min()) {
      const Price below_hi = std::min(hi, w.base - 1);
      if (lo <= below_hi && bounds && bounds->first <= below_hi)
        outside.range(pool, lo, below_hi, relay);
      if (!go)
        return;
    }
    const Price wlo = std::max(lo, w.base), whi = std::min(hi, top);
    if (wlo <= whi) {
      auto bits = Bits(w.ordered(w.occupied) &
                       slide_bits::span<Bits>(unsigned(wlo - w.base),
                                              unsigned(whi - w.base)));
      while (bits) {
        const Price p = w.base + Price(slide_bits::ctz(bits));
        bits &= Bits(bits - 1);
        if (!f(p, w.slots[Window::slot(p)]))
          return;
      }
    }
    if (hi > top && top < std::numeric_limits<Price>::max() && bounds &&
        bounds->second > top)
      outside.range(pool, std::max(lo, top + 1), hi, f);
  }

private:
  struct Target {
    bool cover;  // false: the price belongs to the outside locator
    bool create; // no window yet
    Price base;
  };
  static Price sat_sub(Price a, Price b) {
    return a < std::numeric_limits<Price>::min() + b
               ? std::numeric_limits<Price>::min()
               : a - b;
  }
  Target target(Price p) const {
    if (!window)
      return {outside.size() >= tier, true, sat_sub(p, n / 2)};
    const auto &w = *window;
    const auto above = std::uint64_t(p) - std::uint64_t(w.base);
    const auto below = std::uint64_t(w.base) - std::uint64_t(p);
    if (!w.occupied)
      return {true, false, sat_sub(p, n / 2)};
    if (p >= w.base && above < std::uint64_t(n + n / 2))
      return {true, false, p - (n - 1) + n / 4}; // leave n/4 above p
    if (p < w.base && below <= std::uint64_t(n / 2))
      return {true, false, sat_sub(p, n / 4)}; // leave n/4 below p
    return {false, false, 0};
  }
  void cover(PagePool &pool, Target t, Price p, Handle filler) {
    if (t.create) {
      window = std::make_unique<Window>();
      window->base = std::min(t.base, max_base);
      window->slots.fill(filler);
      slide(pool, window->base);
    } else {
      slide(pool, t.base);
    }
    (void)p;
  }
  // Moves the window to start at base: evicts occupied levels that fall out
  // to the outside locator and pulls outside prices that fall in.
  void slide(PagePool &pool, Price base) {
    auto &w = *window;
    base = std::min(base, max_base);
    const Price old = w.base;
    const i128 dist = i128(base) - i128(old);
    const i128 overlap = i128(N) - (dist < 0 ? -dist : dist);
    auto evict = [&](Bits bits) {
      while (bits) {
        const Price p = old + Price(slide_bits::ctz(bits));
        bits &= Bits(bits - 1);
        const auto s = Window::slot(p);
        outside.insert(pool, p, w.slots[s],
                       (w.working & slide_bits::bit<Bits>(s)) != 0);
        w.clear(p);
      }
    };
    if (base != old) {
      if (overlap <= 0 || !w.occupied)
        evict(w.ordered(w.occupied));
      else if (base > old)
        evict(Bits(w.ordered(w.occupied) &
                   slide_bits::span<Bits>(0, unsigned(base - old - 1))));
      else
        evict(Bits(w.ordered(w.occupied) &
                   slide_bits::span<Bits>(unsigned(overlap), N - 1)));
    }
    w.base = base;
    const Price top = base + (n - 1);
    for (;;) {
      std::optional<std::pair<Price, Handle>> next;
      outside.range(pool, base, top, [&](Price p, Handle h) {
        next = std::pair(p, h);
        return false;
      });
      if (!next)
        break;
      const bool working = outside.is_working(next->first);
      outside.remove(pool, next->first);
      w.put(next->first, next->second, working);
    }
  }
};
} // namespace oms
