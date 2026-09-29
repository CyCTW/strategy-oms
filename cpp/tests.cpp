#include "flow.hpp"
#include "hash_ordered_index.hpp"
#include "linear_index.hpp"
#include "oms_index.hpp"
#include "slide_locator.hpp"
#include "sorted_deque_locator.hpp"
#include <iostream>
#include <map>
#include <set>
#include <string>

using namespace oms;
#define CHECK(...)                                                             \
  do {                                                                         \
    if (!(__VA_ARGS__))                                                        \
      throw std::runtime_error("check failed at line " +                       \
                               std::to_string(__LINE__) + ": " #__VA_ARGS__);  \
  } while (false)
constexpr auto low = std::numeric_limits<Price>::min(),
               high = std::numeric_limits<Price>::max();
Book book(std::size_t b) {
  Book v;
  v.strategy = b;
  v.buy = (b % 2 == 0);
  return v;
}
Order sample(std::size_t id, Price p) {
  Order o;
  o.id = id + 1;
  o.book = book(id % 4);
  o.price = p;
  o.total = 100;
  o.leaves = 100;
  return o;
}

void pool_test() {
  Pool<int> p;
  auto first = p.insert(7);
  const auto *address = p.get(first);
  for (int i = 0; i < 4096; ++i)
    p.insert(i);
  CHECK(p.get(first) == address);
  CHECK(p.remove(first));
  CHECK(!p.get(first));
  auto next = p.insert(8);
  CHECK(next.slot == first.slot);
  CHECK(next != first);
  CHECK(!p.remove(first));
  CHECK(*p.get(next) == 8);
  CHECK(!p.get(Handle{none, 1}));
}
void reserved_test() {
  auto o = sample(0, 1);
  o.leaves = 40;
  o.cum = 3;
  o.uncertain = true;
  CHECK(o.reserved() == 97);
  o.terminal = true;
  o.leaves = 0;
  CHECK(o.reserved() == 97);
  o.uncertain = false;
  CHECK(o.reserved() == 0);
  o.terminal = false;
  o.pending_new = true;
  CHECK(o.reserved() == 97);
  o.pending_new = false;
  o.pending = Pending{Kind::Replace, 2, 200};
  CHECK(o.reserved() == 197);
}
template <class L> void locator_test() {
  PagePool pool;
  L t;
  std::map<Price, std::pair<Handle, bool>> ref;
  std::uint64_t seed = 17;
  for (std::size_t step = 0; step < 20000; ++step) {
    seed = seed * 6364136223846793005ULL + 1;
    const auto p = step % 41 == 0   ? low
                   : step % 41 == 1 ? high
                                    : Price((seed >> 32) % 1024) - 512;
    const bool w = (seed & 16) != 0;
    const Handle h{step, seed};
    switch (seed % 4) {
    case 0:
      t.remove(pool, p);
      ref.erase(p);
      break;
    case 1:
      if (ref.contains(p)) {
        t.working(pool, p, w);
        ref[p].second = w;
      }
      break;
    default:
      t.insert(pool, p, h, w);
      ref[p] = {h, w};
    }
    CHECK(t.get(pool, p) ==
          (ref.contains(p) ? std::optional(ref.at(p).first) : std::nullopt));
    std::optional<Price> buy, sell;
    for (const auto &[key, v] : ref)
      if (v.second) {
        buy = key;
        if (!sell)
          sell = key;
      }
    CHECK(t.best(pool, true) == buy);
    CHECK(t.best(pool, false) == sell);
    if (step % 127 == 0)
      for (auto [a, b] : std::array<std::pair<Price, Price>, 5>{
               {{low, high}, {-65, 64}, {p, p}, {2, 1}, {high, high}}}) {
        std::vector<std::pair<Price, Handle>> got, expected;
        t.range(pool, a, b, [&](Price key, Handle v) {
          got.emplace_back(key, v);
          return true;
        });
        if (a <= b)
          for (auto i = ref.lower_bound(a); i != ref.end() && i->first <= b;
               ++i)
            expected.emplace_back(i->first, i->second.first);
        CHECK(got == expected);
        got.clear();
        t.range(pool, a, b, [&](Price key, Handle v) {
          got.emplace_back(key, v);
          return got.size() < 3;
        });
        if (expected.size() > 3)
          expected.resize(3);
        CHECK(got == expected);
      }
  }
  for (auto [p, v] : ref) {
    (void)v;
    t.remove(pool, p);
  }
  CHECK(!t.best(pool, true));
  CHECK(pool.size() == 0);
}
struct Reference {
  Totals totals;
  std::set<std::uint64_t> ids;
};
// Rebuild reference from authoritative snapshots; no membership deltas or
// bitmap.
std::map<Price, Reference>
reference(const std::vector<std::optional<Order>> &orders, const Book &b) {
  std::map<Price, Reference> out;
  for (const auto &v : orders) {
    if (!v || v->book != b || (v->terminal && !v->pending))
      continue;
    const auto &o = *v;
    auto &x = out[o.price];
    x.ids.insert(o.id);
    x.totals.confirmed += o.leaves;
    x.totals.uncertain += o.uncertain;
    if (!o.pending)
      continue;
    const auto p = *o.pending;
    const auto q = p.qty > o.cum ? p.qty - o.cum : Qty{};
    if (p.kind == Kind::New)
      x.totals.new_qty += q;
    else if (p.kind == Kind::Cancel)
      x.totals.cancel_qty += o.leaves;
    else {
      x.totals.replace_out += o.leaves;
      auto &y = out[p.price];
      y.totals.replace_in += q;
      y.ids.insert(o.id);
      if (p.price != o.price)
        y.totals.uncertain += o.uncertain;
    }
  }
  return out;
}
template <class L, class I = Index<L>> void index_test() {
  I index;
  std::vector<std::optional<Order>> orders(128);
  std::vector<typename I::Membership> slots(128);
  std::uint64_t seed = 11;
  for (std::size_t step = 0; step < 10000; ++step) {
    seed = seed * 6364136223846793005ULL + 1;
    const auto id = (seed >> 32) % 128;
    const Price p = step % 31 == 0   ? low
                    : step % 31 == 1 ? high
                                     : Price((seed >> 16) % 257) - 128;
    auto n = sample(id, p);
    n.cum = seed % 50;
    n.leaves -= n.cum;
    n.uncertain = (step % 7 == 0);
    switch (step % 8) {
    case 0:
      n.terminal = true;
      n.leaves = 0;
      break;
    case 1:
      n.leaves = 0;
      n.pending_new = true;
      n.pending = Pending{Kind::New, p, 100};
      break;
    case 6:
      n.pending = Pending{Kind::Cancel, p, 100};
      break;
    case 7:
      break;
    default:
      n.pending =
          Pending{Kind::Replace, step % 2 == 0 ? p : (p == high ? high : p + 1),
                  80 + seed % 80};
    }
    const auto old = orders[id];
    if (!orders[id])
      orders[id] = n;
    if constexpr (requires { index.bind(slots[id], *orders[id]); })
      index.bind(slots[id], *orders[id]);
    index.update(old ? &*old : nullptr, n, slots[id]);
    orders[id] = n;
    if (step % 17 == 0)
      for (std::size_t b = 0; b < 4; ++b) {
        const auto key = book(b);
        const auto handle = index.book_handle(key);
        if (!handle)
          continue;
        const auto expected = reference(orders, key);
        std::map<Price, Summary> got;
        std::optional<Price> previous;
        index.range(*handle, low, high, [&](Price p, Summary v) {
          CHECK(!previous || p > *previous);
          previous = p;
          got.emplace(p, v);
          return true;
        });
        CHECK(got.size() == expected.size());
        std::vector<Price> bounded, expected_bounded;
        index.range(*handle, -65, 64, [&](Price p, Summary) {
          bounded.push_back(p);
          return bounded.size() < 3;
        });
        for (const auto &[p, ignored] : expected) {
          (void)ignored;
          if (p >= -65 && p <= 64 && expected_bounded.size() < 3)
            expected_bounded.push_back(p);
        }
        CHECK(bounded == expected_bounded);
        index.range(*handle, 2, 1, [](Price, Summary) {
          CHECK(false);
          return true;
        });
        CHECK(!index.summary(*handle, 987654321));
        std::optional<Price> best;
        Qty reserved{}, uncertain{};
        for (const auto &[price, v] : expected) {
          const Summary summary{v.totals, v.ids.size()};
          CHECK(got.at(price) == summary);
          CHECK(index.summary(*handle, price) == summary);
          std::set<std::uint64_t> ids;
          index.orders_at(*handle, price, [&](auto i) {
            ids.insert(i);
            return true;
          });
          CHECK(ids == v.ids);
          std::size_t visited{};
          index.orders_at(*handle, price, [&](auto) {
            ++visited;
            return false;
          });
          CHECK(visited == 1);
          if (v.totals.confirmed &&
              (!best || (key.buy ? price > *best : price < *best)))
            best = price;
        }
        for (const auto &o : orders)
          if (o && o->book == key) {
            reserved += o->reserved();
            uncertain += o->uncertain;
          }
        CHECK(index.best(*handle) == best);
        CHECK(index.reserved(*handle) == reserved);
        CHECK(index.uncertain(*handle) == uncertain);
        I other;
        CHECK(!other.summary(*handle, 0));
      }
  }
  for (std::size_t i = 0; i < orders.size(); ++i)
    if (orders[i]) {
      const auto old = *orders[i];
      auto n = old;
      n.terminal = true;
      n.pending.reset();
      n.leaves = 0;
      n.uncertain = false;
      index.update(&old, n, slots[i]);
      orders[i] = n;
    }
  if constexpr (requires { index.stats(); }) {
    CHECK(index.stats().levels == 0);
    CHECK(index.stats().members == 0);
    CHECK(index.stats().pages == 0);
  }
  for (std::size_t b = 0; b < 4; ++b) {
    const auto h = *index.book_handle(book(b));
    CHECK(!index.best(h));
    index.range(h, low, high, [](Price, Summary) {
      CHECK(false);
      return true;
    });
  }
}
template <class L, class I = Index<L>> void cancel_rehang() {
  I index;
  auto a = sample(0, 63), b = sample(4, 1'000'000'000);
  typename I::Membership ma, mb;
  if constexpr (requires { index.bind(ma, a); }) {
    index.bind(ma, a);
    index.bind(mb, b);
  }
  index.update(nullptr, a, ma);
  const auto h = *index.book_handle(a.book);
  auto n = a;
  n.pending = Pending{Kind::Cancel, a.price, a.total};
  index.update(&a, n, ma);
  a = n;
  b.leaves = 0;
  b.pending_new = true;
  b.pending = Pending{Kind::New, b.price, b.total};
  index.update(nullptr, b, mb);
  CHECK(index.summary(h, 63)->totals.cancel_qty == 100);
  CHECK(index.best(h) == 63);
  CHECK(index.reserved(h) == 200);
  n = b;
  n.leaves = 100;
  n.pending_new = false;
  n.pending.reset();
  index.update(&b, n, mb);
  b = n;
  CHECK(index.best(h) == b.price);
  n = a;
  n.leaves = 0;
  n.terminal = true;
  n.pending.reset();
  index.update(&a, n, ma);
  a = n;
  CHECK(!index.summary(h, 63));
  CHECK(index.reserved(h) == 100);
}
template <class L, class I = Index<L>> void flow_test() {
  Flow<L, I> flow;
  auto a = sample(0, 63);
  flow.seed(a);
  CHECK(flow.replace(1, 1, flow.version(1), 64));
  CHECK(!flow.replace(1, 2, flow.version(1), 65));
  CHECK(!flow.replace(1, 3, flow.version(1), 66));
  CHECK(!flow.dispatch());
  flow.acknowledge(1, 1, 1, 64);
  const auto cmd = flow.dispatch();
  CHECK(cmd && cmd->request == 3 && cmd->price == 66);
  flow.acknowledge(1, 3, 2, 66);
  CHECK(!flow.dispatch());
  CHECK(flow.price(1) == 66);
  bool rejected = false;
  try {
    flow.replace(1, 4, 0, 67);
  } catch (const std::invalid_argument &) {
    rejected = true;
  }
  CHECK(rejected);
}
// Both-end pushes, both-end removals and growth/recentering of the
// double-ended array, checked against std::map after every step.
template <class L> void sorted_deque_edges() {
  PagePool pool;
  L t;
  std::map<Price, Handle> ref;
  auto check = [&] {
    std::vector<std::pair<Price, Handle>> got, expected(ref.begin(), ref.end());
    t.range(pool, low, high, [&](Price p, Handle h) {
      got.emplace_back(p, h);
      return true;
    });
    CHECK(got == expected);
    CHECK(t.best(pool, true) ==
          (ref.empty() ? std::nullopt : std::optional(ref.rbegin()->first)));
    CHECK(t.best(pool, false) ==
          (ref.empty() ? std::nullopt : std::optional(ref.begin()->first)));
  };
  std::uint64_t seed = 5;
  for (std::size_t round = 0; round < 4; ++round) {
    for (std::size_t i = 0; i < 700; ++i) {
      // Alternate new maximum, new minimum and a random interior price.
      seed = seed * 6364136223846793005ULL + 1;
      const Price p = i % 3 == 0   ? Price(i) * 1000 + 1
                      : i % 3 == 1 ? -Price(i) * 1000 - 1
                                   : Price((seed >> 33) % 100000) - 50000;
      const Handle h{i, seed};
      t.insert(pool, p, h, true);
      ref[p] = h;
      check();
    }
    while (!ref.empty()) {
      seed = seed * 6364136223846793005ULL + 1;
      const auto p =
          seed % 3 == 0 ? ref.begin()->first
          : seed % 3 == 1
              ? ref.rbegin()->first
              : std::next(ref.begin(), long((seed >> 33) % ref.size()))->first;
      t.remove(pool, p);
      ref.erase(p);
      check();
    }
  }
}
// Keep the price count oscillating across both AdaptiveLocator thresholds so
// promotion and demotion both run with mixed working flags; compare every step.
void adaptive_threshold_test() {
  PagePool pool;
  AdaptiveLocator<16, 4> t;
  std::map<Price, std::pair<Handle, bool>> ref;
  std::uint64_t seed = 99;
  std::size_t target = 0, converted = 0;
  bool was_big = false;
  for (std::size_t step = 0; step < 40000; ++step) {
    seed = seed * 6364136223846793005ULL + 1;
    if (step % 64 == 0)
      target = (seed >> 40) % 2 ? 24 : 1; // swing between above 16 and below 4
    const Price p = Price((seed >> 33) % 200) - 100;
    const bool w = (seed >> 20) & 1;
    if (ref.size() < target && !ref.contains(p)) {
      t.insert(pool, p, Handle{step, seed}, w);
      ref[p] = {Handle{step, seed}, w};
    } else if (ref.size() > target && !ref.empty()) {
      const auto victim =
          std::next(ref.begin(), long((seed >> 12) % ref.size()))->first;
      t.remove(pool, victim);
      ref.erase(victim);
    } else if (ref.contains(p)) {
      t.working(pool, p, w);
      ref[p].second = w;
    }
    converted += t.big != was_big;
    was_big = t.big;
    CHECK(t.get(pool, p) ==
          (ref.contains(p) ? std::optional(ref.at(p).first) : std::nullopt));
    std::optional<Price> buy, sell;
    for (const auto &[key, v] : ref)
      if (v.second) {
        buy = key;
        if (!sell)
          sell = key;
      }
    CHECK(t.best(pool, true) == buy);
    CHECK(t.best(pool, false) == sell);
    std::vector<Price> got, expected;
    t.range(pool, low, high, [&](Price key, Handle) {
      got.push_back(key);
      return true;
    });
    for (const auto &[key, v] : ref)
      expected.push_back(key);
    CHECK(got == expected);
  }
  CHECK(converted > 100); // both directions really ran many times
}
// Sanitizer builds run the sliding-window walks shorter (they check the whole
// book after every step, which is slow under ASan/UBSan instrumentation).
#if defined(__SANITIZE_ADDRESS__)
constexpr std::size_t walk_steps = 3000;
#elif defined(__has_feature)
#if __has_feature(address_sanitizer) ||                                        \
    __has_feature(undefined_behavior_sanitizer)
constexpr std::size_t walk_steps = 3000;
#else
constexpr std::size_t walk_steps = 20000;
#endif
#else
constexpr std::size_t walk_steps = 20000;
#endif
// Sliding window: drifting center, far outliers and i64 extremes, every
// operation, checked against std::map after each step (port of the Rust
// random_walk test). Returns (steps whose price was in the window, total).
template <std::size_t N>
std::pair<std::size_t, std::size_t>
slide_walk(std::uint64_t seed, std::uint64_t spread, std::uint64_t drift) {
  PagePool pool;
  SlideLocator<N> t;
  std::map<Price, std::pair<Handle, bool>> ref;
  std::uint64_t s = seed;
  auto rng = [&] {
    s = s * 6364136223846793005ULL + 1;
    return s >> 33;
  };
  Price center = 0;
  std::size_t inside = 0, total = 0;
  for (std::size_t step = 0; step < walk_steps; ++step) {
    const auto r = rng();
    if (step % 10 == 0)
      center += Price(r % (2 * drift + 1)) - Price(drift);
    const Price p = step % 101 == 0   ? low
                    : step % 101 == 1 ? high
                    : step % 101 == 2 ? high - 5
                    : step % 101 == 3 ? low + 7
                    : step % 101 == 4
                        ? 1'000'000'000
                        : center + Price(r % spread) - Price(spread / 2);
    const Handle h{step, r};
    const bool w = (r & 8) != 0;
    switch (r % 5) {
    case 0:
    case 1:
      t.remove(pool, p);
      ref.erase(p);
      break;
    case 2:
      if (ref.contains(p)) {
        t.working(pool, p, w);
        ref[p].second = w;
      }
      break;
    default:
      t.insert(pool, p, h, w);
      ref[p] = {h, w};
    }
    if (ref.contains(p)) {
      ++total;
      inside += t.window && t.window->contains(p) && t.window->has(p);
    }
    CHECK(t.get(pool, p) ==
          (ref.contains(p) ? std::optional(ref.at(p).first) : std::nullopt));
    std::optional<Price> buy, sell;
    for (const auto &[key, v] : ref)
      if (v.second) {
        buy = key;
        if (!sell)
          sell = key;
      }
    CHECK(t.best(pool, true) == buy);
    CHECK(t.best(pool, false) == sell);
    for (auto [a, b] : std::array<std::pair<Price, Price>, 4>{
             {{low, high}, {p, p}, {low, p}, {center - 70, center + 70}}}) {
      if (a > b)
        continue;
      std::vector<std::pair<Price, Handle>> got, expected;
      t.range(pool, a, b, [&](Price key, Handle v) {
        got.emplace_back(key, v);
        return true;
      });
      for (auto i = ref.lower_bound(a); i != ref.end() && i->first <= b; ++i)
        expected.emplace_back(i->first, i->second.first);
      CHECK(got == expected);
      got.clear();
      t.range(pool, a, b, [&](Price key, Handle v) {
        got.emplace_back(key, v);
        return got.size() < 3;
      });
      if (expected.size() > 3)
        expected.resize(3);
      CHECK(got == expected);
    }
  }
  return {inside, total};
}
void slide_tests() {
  for (auto [seed, spread, drift] : std::array<std::array<std::uint64_t, 3>, 4>{
           {{1, 20, 3}, {2, 60, 10}, {3, 150, 40}, {4, 3000, 300}}}) {
    auto [in64, all64] = slide_walk<64>(seed, spread, drift);
    CHECK(in64 > 0 && in64 < all64);
    auto [in128, all128] = slide_walk<128>(seed, spread * 2, drift);
    CHECK(in128 > 0 && in128 < all128);
  }
  // Rolling book: the top level fills, a new level one tick below the
  // bottom; every level stays in the window by sliding.
  PagePool pool;
  SlideLocator<64> t;
  Price top = 1000, bottom = 970;
  for (Price p = bottom; p <= top; ++p)
    t.insert(pool, p, Handle{std::size_t(p), 1}, p % 3 == 0);
  for (std::size_t step = 0; step < 5000; ++step) {
    t.remove(pool, top--);
    t.insert(pool, --bottom, Handle{step, 2}, step % 2 == 0);
    CHECK(t.window && t.window->has(bottom) && t.window->has(top));
    if (step % 2 == 0) // the new bottom is working: it is the best ask
      CHECK(t.best(pool, false) == std::optional(bottom));
  }
}
int main() {
  pool_test();
  reserved_test();
  locator_test<BtreeLocator>();
  locator_test<PagedLocator>();
  index_test<BtreeLocator>();
  index_test<PagedLocator>();
  cancel_rehang<BtreeLocator>();
  cancel_rehang<PagedLocator>();
  flow_test<BtreeLocator>();
  flow_test<PagedLocator>();
  locator_test<HashOrderedLocator>();
  index_test<HashOrderedLocator>();
  cancel_rehang<HashOrderedLocator>();
  flow_test<HashOrderedLocator>();
  locator_test<LinearLocator>();
  index_test<LinearLocator>();
  cancel_rehang<LinearLocator>();
  flow_test<LinearLocator>();
  index_test<LinearLocator, OrderScanIndex>();
  cancel_rehang<LinearLocator, OrderScanIndex>();
  flow_test<LinearLocator, OrderScanIndex>();
  locator_test<SortedDequeLocator>();
  index_test<SortedDequeLocator>();
  cancel_rehang<SortedDequeLocator>();
  flow_test<SortedDequeLocator>();
  sorted_deque_edges<SortedDequeLocator>();
  locator_test<SortedDequeSoaLocator>();
  index_test<SortedDequeSoaLocator>();
  cancel_rehang<SortedDequeSoaLocator>();
  flow_test<SortedDequeSoaLocator>();
  sorted_deque_edges<SortedDequeSoaLocator>();
  // Tiny thresholds so the random tests promote and demote constantly.
  locator_test<AdaptiveLocator<16, 4>>();
  index_test<AdaptiveLocator<16, 4>>();
  cancel_rehang<AdaptiveLocator<16, 4>>();
  flow_test<AdaptiveLocator<16, 4>>();
  sorted_deque_edges<AdaptiveLocator<16, 4>>();
  locator_test<AdaptiveLocator<>>();
  adaptive_threshold_test();
  locator_test<SlideLocator<64>>();
  locator_test<SlideLocator<128>>();
  index_test<SlideLocator<128>>();
  cancel_rehang<SlideLocator<128>>();
  flow_test<SlideLocator<128>>();
  sorted_deque_edges<SlideLocator<128>>();
  slide_tests();
  std::cout << "45 test groups passed; 20k locator + 10k index transitions per "
               "backend\n";
}
