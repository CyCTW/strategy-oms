#include "flow.hpp"
#include "hash_ordered_index.hpp"
#include "linear_index.hpp"
#include "oms_index.hpp"
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
  std::cout << "21 test groups passed; 20k locator + 10k index transitions per "
               "backend\n";
}
