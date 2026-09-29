#include "flow.hpp"
#include "linear_index.hpp"
#include "measure.hpp"
#include "slide_locator.hpp"
#include "sorted_deque_locator.hpp"
#include <string>

using namespace oms;
using namespace measurement;
Book make_book(std::size_t id = 0) {
  Book b;
  b.strategy = id;
  return b;
}
Order make_order(std::size_t id, Price p, std::size_t b = 0) {
  Order o;
  o.id = id + 1;
  o.book = make_book(b);
  o.price = p;
  o.total = o.leaves = 10'000'000;
  return o;
}
template <class I> struct Store {
  struct State {
    Order order;
    typename I::Membership membership;
  };
  I index;
  Pool<State> states;
  std::vector<Handle> ids;
  void add(Order o) {
    auto h = states.insert(State{o, {}});
    ids.push_back(h);
    auto &s = require(states.get(h));
    if constexpr (requires { index.bind(s.membership, s.order); })
      index.bind(s.membership, s.order);
    index.update(nullptr, s.order, s.membership);
  }
  State &at(std::size_t id) { return require(states.get(ids.at(id))); }
  void update(std::size_t id, const Order &n) {
    auto &s = at(id);
    index.update(&s.order, n, s.membership);
    s.order = n;
  }
};
std::uint64_t rng(std::uint64_t &s) {
  s = s * 6364136223846793005ULL + 1;
  return s >> 32;
}
template <class I>
void queries(std::string_view backend, std::size_t width, bool crowded,
             std::size_t n, std::size_t round) {
  Store<I> store;
  for (std::size_t i = 0; i < width; ++i)
    store.add(make_order(i, crowded ? 100 : 100 + Price(i)));
  const auto h = *store.index.book_handle(make_book());
  const auto scenario =
      std::string(crowded ? "crowded_" : "prices_") + std::to_string(width);
  Sample first(n), last(n), miss(n), random(n), best(n), ranges(n), members(n);
  std::uint64_t seed = 11;
  for (std::size_t i = 0; i < n; ++i) {
    const Price p = crowded ? 100 : 100 + Price(rng(seed) % width);
    first.measure([&] {
      escape(store);
      return store.index.summary(h, 100);
    });
    last.measure([&] {
      escape(store);
      return store.index.summary(h, crowded ? 100 : 99 + Price(width));
    });
    miss.measure([&] {
      escape(store);
      return store.index.summary(h, -1);
    });
    random.measure([&] {
      escape(p);
      return store.index.summary(h, p);
    });
    best.measure([&] {
      escape(store);
      return store.index.best(h);
    });
    if (i < std::min(n, std::size_t{2000})) {
      ranges.measure([&] {
        escape(store);
        Qty sum{};
        std::size_t count{};
        store.index.range(h, 100, 99 + Price(width), [&](Price, Summary s) {
          sum += s.totals.confirmed;
          return ++count < 8;
        });
        return sum;
      });
      members.measure([&] {
        escape(store);
        std::uint64_t sum{};
        store.index.orders_at(h, p, [&](auto id) {
          sum += id;
          return true;
        });
        return sum;
      });
    }
  }
  for (auto [s, label] :
       std::initializer_list<std::pair<Sample *, const char *>>{
           {&first, "summary_first"},
           {&last, "summary_last"},
           {&miss, "summary_miss"},
           {&random, "summary_random"},
           {&best, "best"},
           {&ranges, "sorted_range8"},
           {&members, "orders_at"}})
    s->print(round, backend, scenario, label);
  // Whole-batch samples, not single-operation p99. 64 varying queries amortize
  // timer quantization; batch quantiles / 64 are NOT individual-query
  // quantiles.
  if (!counting && width <= 32) {
    Sample batches(std::min(n, std::size_t{2000}));
    for (std::size_t i = 0; i < std::min(n, std::size_t{2000}); ++i)
      batches.measure([&] {
        Qty sum{};
        for (std::size_t j = 0; j < 64; ++j) {
          const Price p = crowded ? 100 : 100 + Price(rng(seed) % (width + 1));
          escape(store);
          const auto s = store.index.summary(h, p);
          sum += s ? s->totals.confirmed : 0;
        }
        return sum;
      });
    batches.print(round, backend, scenario, "batch64_summary");
  }
}
template <class I>
void updates(std::string_view backend, std::size_t width, std::size_t n,
             std::size_t round) {
  Store<I> store;
  for (std::size_t i = 0; i < width; ++i)
    store.add(make_order(i, 100 + Price(i)));
  Sample fills(n), submit(n), ack(n), cancel(n), terminal(n), rehang(n);
  for (std::size_t step = 0; step < n; ++step) {
    const auto id = step % width;
    auto next = store.at(id).order;
    --next.leaves;
    ++next.cum;
    fills.measure([&] {
      escape(next);
      store.update(id, next);
    });
    const Price p = next.price < 1'000'000'000 ? 1'000'000'000 + Price(id)
                                               : 100 + Price(id);
    next.pending = Pending{Kind::Replace, p, next.total};
    submit.measure([&] {
      escape(next);
      store.update(id, next);
    });
    next.price = p;
    next.pending.reset();
    ack.measure([&] {
      escape(next);
      store.update(id, next);
    });
    next.pending = Pending{Kind::Cancel, p, next.total};
    cancel.measure([&] {
      escape(next);
      store.update(id, next);
    });
    next.pending.reset();
    next.terminal = true;
    next.leaves = 0;
    terminal.measure([&] {
      escape(next);
      store.update(id, next);
    });
    // Synthetic index reactivation exercises swap-removal/reinsertion. This is
    // NOT a public API that reopens an exchange terminal order.
    next.terminal = false;
    next.leaves = next.total - next.cum;
    rehang.measure([&] {
      escape(next);
      store.update(id, next);
    });
  }
  const auto scenario = "updates_" + std::to_string(width);
  for (auto [s, label] :
       std::initializer_list<std::pair<Sample *, const char *>>{
           {&fills, "index_fill_visible"},
           {&submit, "index_replace_submit"},
           {&ack, "index_replace_ack_visible"},
           {&cancel, "index_cancel_submit"},
           {&terminal, "index_terminal_visible"},
           {&rehang, "index_reactivate"}})
    s->print(round, backend, scenario, label);
}
// Random reprice: a random order moves to a uniformly random sparse price, so
// price levels are created and removed at random positions. This is the worst
// case for contiguous arrays (memmove) and neutral for trees.
template <class I>
void reprice(std::string_view backend, std::size_t width, std::size_t n,
             std::size_t round) {
  Store<I> store;
  std::uint64_t seed = 29;
  for (std::size_t i = 0; i < width; ++i)
    store.add(make_order(i, Price(rng(seed) % 100'000'000)));
  Sample submit(n), ack(n);
  for (std::size_t step = 0; step < n; ++step) {
    const auto id = rng(seed) % width;
    auto next = store.at(id).order;
    const Price p = Price(rng(seed) % 100'000'000);
    next.pending = Pending{Kind::Replace, p, next.total};
    submit.measure([&] {
      escape(next);
      store.update(id, next);
    });
    next.price = p;
    next.pending.reset();
    ack.measure([&] {
      escape(next);
      store.update(id, next);
    });
  }
  const auto scenario = "reprice_random_" + std::to_string(width);
  submit.print(round, backend, scenario, "index_replace_submit");
  ack.print(round, backend, scenario, "index_replace_ack_visible");
}
template <class I>
void mixed(std::string_view backend, std::size_t width, std::size_t q,
           std::size_t n, std::size_t round) {
  Store<I> store;
  for (std::size_t i = 0; i < width; ++i)
    store.add(make_order(i, 100 + Price(i)));
  const auto h = *store.index.book_handle(make_book());
  Sample sample(n);
  std::uint64_t seed = 17;
  // Replay precomputed targets identically, with 20% misses and no RNG cost
  // inside mixed timing. Same-price partial fill followed by q summaries.
  std::vector<Price> targets(n * q);
  for (auto &p : targets) {
    const auto r = rng(seed);
    p = r % 5 == 0 ? -1 : 100 + Price(r % width);
  }
  for (std::size_t step = 0; step < n; ++step) {
    const auto id = step % width;
    auto next = store.at(id).order;
    --next.leaves;
    ++next.cum;
    sample.measure([&] {
      escape(next);
      store.update(id, next);
      Qty sum{};
      for (std::size_t j = 0; j < q; ++j) {
        const auto s = store.index.summary(h, targets[step * q + j]);
        sum += s ? s->totals.confirmed : 0;
      }
      return sum;
    });
  }
  sample.print(round, backend,
               "mixed_" + std::to_string(width) + "_q" + std::to_string(q),
               "fill_plus_queries");
}
template <class I>
void grow(std::string_view backend, std::size_t n, std::size_t round) {
  Store<I> store;
  Sample all(n), early(n), late(n), cadence(n);
  for (std::size_t i = 0; i < n; ++i) {
    auto o = make_order(i, 100 + Price(i));
    const auto a = all.allocations, b = all.bytes;
    all.measure([&] {
      escape(o);
      store.add(o);
    });
    if (i < 32)
      early.subset(all, a, b);
    if (i >= n * 3 / 4)
      late.subset(all, a, b);
    if (i % 64 == 0)
      cadence.subset(all, a, b);
  }
  all.print(round, backend, "growth", "new_order_price");
  early.print(round, backend, "growth", "first32_subset");
  late.print(round, backend, "growth", "last_quarter_subset");
  cadence.print(round, backend, "growth", "every64_subset");
}
template <class I>
void many_books(std::string_view backend, std::size_t n, std::size_t round) {
  Store<I> store;
  std::vector<BookHandle> handles;
  for (std::size_t b = 0; b < 4096; ++b) {
    for (std::size_t i = 0; i < 6; ++i)
      store.add(make_order(
          b * 6 + i, i < 4 ? 100 + Price(i) : Price(i) * 1'000'000'000, b));
    handles.push_back(*store.index.book_handle(make_book(b)));
  }
  Sample sample(n);
  std::uint64_t seed = 7;
  for (std::size_t i = 0; i < n; ++i) {
    const auto r = rng(seed);
    sample.measure([&] {
      escape(r);
      return store.index.summary(handles[r % handles.size()],
                                 r % 5 ? 100 + Price(r % 4) : -1);
    });
  }
  sample.print(round, backend, "4096books_4near_2far", "summary_random");
}
template <class L, class I>
void prototype(std::string_view backend, std::size_t width, std::size_t n,
               std::size_t round) {
  Flow<L, I> flow;
  for (std::size_t i = 0; i < width; ++i)
    flow.seed(make_order(i, 100 + Price(i)));
  Sample immediate(n), queued(n), ack(n), dispatch(n);
  std::uint64_t seq{};
  for (std::size_t step = 0; step < n; ++step) {
    const auto id = step % width + 1;
    const auto base = flow.price(id);
    const auto p = base < 1'000'000'000 ? 1'000'000'000 + Price(id) * 4
                                        : 100 + Price(id) * 4;
    const auto v = flow.version(id);
    const auto req = step * 2 + 1;
    const auto command =
        immediate.measure([&] { return flow.replace(id, req, v, p); });
    if (!command)
      throw std::logic_error("missing immediate command");
    const auto v2 = flow.version(id);
    const auto deferred =
        queued.measure([&] { return flow.replace(id, req + 1, v2, p + 1); });
    if (deferred)
      throw std::logic_error("more than one in-flight");
    ack.measure([&] { flow.acknowledge(id, req, ++seq, p); });
    const auto next = dispatch.measure([&] { return flow.dispatch(); });
    if (!next || next->request != req + 1 || next->price != p + 1)
      throw std::logic_error("latest desired not dispatched");
    flow.acknowledge(id, req + 1, ++seq, p + 1);
  }
  const auto scenario = "prototype_" + std::to_string(width);
  immediate.print(round, backend, scenario, "intent_to_command");
  queued.print(round, backend, scenario, "queued_intent");
  ack.print(round, backend, scenario, "report_visible");
  dispatch.print(round, backend, scenario, "ready_to_command");
}
template <class L, class I = Index<L>>
void run(std::string_view backend, std::size_t n, std::size_t round) {
  for (auto width : {4U, 8U, 16U, 32U, 128U, 1024U, 4096U}) {
    const auto count = width >= 1024 ? std::min(n, std::size_t{4000}) : n;
    queries<I>(backend, width, false, count, round);
    updates<I>(backend, width, count, round);
  }
  for (auto width : {32U, 4096U})
    queries<I>(backend, width, true, std::min(n, std::size_t{4000}), round);
  for (auto width : {8U, 32U, 128U, 1024U, 4096U})
    reprice<I>(backend, width,
               width >= 1024 ? std::min(n, std::size_t{4000}) : n, round);
  for (auto width : {4U, 32U, 128U, 4096U})
    for (auto q : {0U, 1U, 4U, 16U})
      mixed<I>(backend, width, q,
               width >= 1024 ? std::min(n, std::size_t{4000}) : n, round);
  grow<I>(backend, n, round);
  many_books<I>(backend, n, round);
  for (auto width : {4U, 32U, 128U})
    prototype<L, I>(backend, width, n, round);
}
int main() {
  const auto en = std::getenv("OMS_BENCH_SAMPLES"),
             er = std::getenv("OMS_BENCH_ROUNDS");
  const std::size_t n = en ? std::stoull(en) : 20000,
                    rounds = counting ? 1
                             : er     ? std::stoull(er)
                                      : 14;
  if (!n || !rounds || n > 1'000'000)
    throw std::invalid_argument(
        "benchmark resource guard (not index capacity)");
  std::cout << "pass,round,backend,scenario,metric,n,p50_ns,p99_ns,p999_ns,max_"
               "ns,allocations,allocated_bytes\n";
  // Rotate the backend order every round and reverse it every B rounds, so
  // each backend runs in every position equally often over 2B rounds.
  constexpr std::size_t B = 7;
  for (std::size_t r = 0; r < rounds; ++r)
    for (std::size_t k = 0; k < B; ++k) {
      const auto pos = (r / B) % 2 ? B - 1 - k : k;
      const auto b = (r + pos) % B;
      std::cerr << "round " << r + 1 << '/' << rounds << " backend " << b
                << (counting ? " allocation" : " timing") << '\n';
      if (b == 0)
        run<BtreeLocator>("absl_btree", n, r);
      else if (b == 1)
        run<LinearLocator>("linear_prices", n, r);
      else if (b == 2)
        run<LinearLocator, OrderScanIndex>("linear_orders", n, r);
      else if (b == 3)
        run<SortedDequeLocator>("sorted_deque", n, r);
      else if (b == 4)
        run<SortedDequeSoaLocator>("sorted_deque_soa", n, r);
      else if (b == 5)
        run<AdaptiveLocator<>>("adaptive", n, r);
      else
        run<SlideLocator<128>>("window128", n, r);
    }
  if constexpr (!counting) {
    Sample timer(n);
    for (std::size_t i = 0; i < n; ++i)
      timer.measure([] { asm volatile("" ::: "memory"); });
    timer.print(0, "timer", "baseline", "timer");
  }
}
