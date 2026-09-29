// Batch micro benchmarks for the C++ price locators (port of the Rust
// benches/locator_micro.rs and benches/index_micro.rs).
//
// Each metric times batches of 256 operations and reports per-operation ns
// (median and p90 over 160 batches), so timer overhead and single interrupts
// are amortized and differences of a few ns are visible. Tail latency stays
// with clustered_bench / linear_bench.
//
//   locator level: the locator alone (search, insert/remove, working, best,
//                  range) on Books of 4..512 consecutive ticks + 2 far prices
//   index level:   the real Index::update / summary paths
#include "linear_index.hpp"
#include "oms_index.hpp"
#include "slide_locator.hpp"
#include <algorithm>
#include <array>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

using namespace oms;

namespace {
constexpr std::size_t batch = 256, batches = 160;

template <class T> inline void keep(const T &x) {
  asm volatile("" : : "g"(&x) : "memory");
}

std::uint64_t rng(std::uint64_t &s) {
  s = s * 6364136223846793005ULL + 1;
  return s >> 33;
}

// Per-op ns of each batch -> (median, p90).
template <class F> std::pair<double, double> measure(F &&op) {
  std::vector<double> per;
  per.reserve(batches);
  for (std::size_t b = 0; b < batches; ++b) {
    const auto start = std::chrono::steady_clock::now();
    for (std::size_t i = 0; i < batch; ++i)
      op();
    const auto end = std::chrono::steady_clock::now();
    per.push_back(
        double(std::chrono::duration_cast<std::chrono::nanoseconds>(end - start)
                   .count()) /
        double(batch));
  }
  std::sort(per.begin(), per.end());
  return {per[batches / 2], per[batches * 9 / 10]};
}

void emit(std::size_t round, const char *backend, const std::string &scenario,
          const char *metric, std::pair<double, double> r) {
  std::printf("%zu,%s,%s,%s,%.2f,%.2f\n", round, backend, scenario.c_str(),
              metric, r.first, r.second);
}

// ---------------------------------------------------------------- locator
template <class L> struct Book {
  PagePool pool;
  L loc;
  std::vector<Price> prices;
};
template <class L>
void make_book(Book<L> &b, std::size_t w, const std::vector<Handle> &hs) {
  for (std::size_t i = 0; i < w; ++i)
    b.prices.push_back(1'000'000 + Price(i));
  b.prices.push_back(1);
  b.prices.push_back(2'000'000'000);
  for (std::size_t i = 0; i < b.prices.size(); ++i)
    b.loc.insert(b.pool, b.prices[i], hs[i % hs.size()], true);
}
// Index-style new level: a lookup, then an insert when missing.
template <class L> void find_or_insert(Book<L> &b, Price p, Handle h, bool w) {
  if (!b.loc.get(b.pool, p))
    b.loc.insert(b.pool, p, h, w);
}

template <class L>
void locator_run(const char *name, std::size_t round,
                 const std::vector<Handle> &hs) {
  for (std::size_t w : {4U, 16U, 32U, 128U, 512U}) {
    const auto sc = "w" + std::to_string(w);
    {
      auto b = std::make_unique<Book<L>>();
      make_book(*b, w, hs);
      std::uint64_t s = 7;
      std::size_t i = 0;
      std::vector<Price> targets(batch * batches);
      for (auto &t : targets) {
        const auto r = rng(s);
        t = r % 5 == 0 ? -5 : b->prices[r % b->prices.size()];
      }
      emit(round, name, sc, "get", measure([&] {
             auto x = b->loc.get(b->pool, targets[i++]);
             keep(x);
           }));
      i = 0;
      emit(round, name, sc, "best", measure([&] {
             auto x = b->loc.best(b->pool, (i++ & 1) == 0);
             keep(x);
           }));
      emit(round, name, sc, "range8", measure([&] {
             std::size_t n = 0;
             b->loc.range(b->pool, 1'000'000, 1'000'000 + Price(w),
                          [&](Price, Handle) { return ++n < 8; });
             keep(n);
           }));
    }
    {
      auto b = std::make_unique<Book<L>>();
      make_book(*b, w, hs);
      std::uint64_t s = 11;
      std::size_t i = 0;
      std::vector<Price> targets(batch * batches);
      for (auto &t : targets)
        t = b->prices[rng(s) % b->prices.size()];
      emit(round, name, sc, "set_working", measure([&] {
             b->loc.working(b->pool, targets[i], (i & 1) == 0);
             ++i;
           }));
    }
    {
      // An order leaves a random live level for a new price just above the
      // cluster (interior delete + insert at the top end).
      auto b = std::make_unique<Book<L>>();
      make_book(*b, w, hs);
      std::vector<Price> live(b->prices.begin(), b->prices.begin() + Price(w));
      std::uint64_t s = 13;
      Price next = 1'000'000 + Price(w);
      std::size_t i = 0;
      emit(round, name, sc, "reprice_to_top", measure([&] {
             const auto k = rng(s) % live.size();
             b->loc.remove(b->pool, live[k]);
             find_or_insert(*b, next, hs[i++ % hs.size()], true);
             live[k] = next++;
           }));
    }
    {
      // Best level fills away, a new level one tick below the worst.
      auto b = std::make_unique<Book<L>>();
      make_book(*b, w, hs);
      Price top = 1'000'000 + Price(w) - 1, bottom = 1'000'000;
      std::size_t i = 0;
      emit(round, name, sc, "roll", measure([&] {
             b->loc.remove(b->pool, top--);
             find_or_insert(*b, --bottom, hs[i++ % hs.size()], true);
           }));
    }
    {
      // A new best far above the cluster, then it fills away.
      auto b = std::make_unique<Book<L>>();
      make_book(*b, w, hs);
      std::size_t i = 0;
      emit(round, name, sc, "aggressive_in_out", measure([&] {
             const Price p = 1'500'000 + Price(i % 7);
             find_or_insert(*b, p, hs[i++ % hs.size()], true);
             b->loc.remove(b->pool, p);
           }));
    }
  }
}

// ------------------------------------------------------------------ index
oms::Book book(std::size_t i) {
  oms::Book b;
  b.strategy = i;
  return b;
}
Order order(std::size_t id, oms::Book b, Price p) {
  Order o;
  o.id = id + 1;
  o.book = b;
  o.price = p;
  o.total = o.leaves = 1'000'000;
  return o;
}

template <class L>
void cluster(const char *name, std::size_t round, std::size_t width,
             const std::string &mode) {
  const bool far = mode == "outliers";
  std::vector<Order> orders;
  for (std::size_t i = 0; i < width; ++i)
    orders.push_back(order(i, book(0), 62 + Price(i)));
  if (far) {
    orders.push_back(order(width, book(0), -1'000'000'000));
    orders.push_back(order(width + 1, book(0), 1'000'000'000));
  }
  std::vector<Memberships> members(orders.size());
  auto index = std::make_unique<Index<L>>();
  for (std::size_t i = 0; i < orders.size(); ++i)
    index->update(nullptr, orders[i], members[i]);
  const auto h = *index->book_handle(book(0));
  std::size_t step = 0;
  const auto sc = mode + "_" + std::to_string(width);
  emit(round, name, sc, "update", measure([&] {
         const auto sweep = step / (2 * width);
         const auto i = (step / 2) % width;
         const Price center = mode == "jump"
                                  ? ((sweep / 8) % 2 == 0 ? 0 : 1'000'000) +
                                        62 + Price(sweep % 8)
                                  : 63 + Price(sweep);
         const auto old = orders[i];
         auto next = old;
         if (step % 2 == 0) {
           next.pending = Pending{Kind::Replace, center + Price(i), old.total};
         } else {
           next.price = old.pending->price;
           next.pending.reset();
         }
         keep(next);
         index->update(&old, next, members[i]);
         orders[i] = next;
         ++step;
       }));
  std::uint64_t s = 7;
  emit(round, name, sc, "query", measure([&] {
         const auto p = orders[rng(s) % width].price;
         auto x = index->summary(h, p);
         keep(x);
       }));
}

template <class L> void many_books(const char *name, std::size_t round) {
  auto index = std::make_unique<Index<L>>();
  std::vector<Order> orders;
  orders.reserve(4096 * 6);
  std::vector<Memberships> members(4096 * 6);
  for (std::size_t b = 0; b < 4096; ++b)
    for (std::size_t j = 0; j < 6; ++j) {
      const Price p = j < 4    ? 62 + Price(j)
                      : j == 4 ? -1'000'000'000
                               : 1'000'000'000;
      orders.push_back(order(b * 6 + j, book(b), p));
      index->update(nullptr, orders.back(), members[b * 6 + j]);
    }
  std::vector<BookHandle> handles;
  for (std::size_t b = 0; b < 4096; ++b)
    handles.push_back(*index->book_handle(book(b)));
  std::uint64_t s = 11;
  emit(round, name, "4096books", "query", measure([&] {
         s = s * 6364136223846793005ULL + 1;
         auto x = index->summary(handles[(s >> 32) % 4096], 62 + Price(s % 4));
         keep(x);
       }));
  emit(round, name, "4096books", "update", measure([&] {
         s = s * 6364136223846793005ULL + 1;
         const auto id = ((s >> 32) % 4096) * 6 + s % 4;
         const auto old = orders[id];
         auto next = old;
         next.price = 62 + (old.price - 62 + 1 + Price(s % 3)) % 8;
         index->update(&old, next, members[id]);
         orders[id] = next;
       }));
}

template <class L> void new_books(const char *name, std::size_t round) {
  auto index = std::make_unique<Index<L>>();
  std::vector<Memberships> members(batch * batches);
  std::size_t i = 0;
  emit(round, name, "new_books", "first_insert", measure([&] {
         index->update(nullptr, order(i, book(i), 62), members[i]);
         ++i;
       }));
}

template <class L>
void run(const char *name, std::size_t round, const std::vector<Handle> &hs) {
  locator_run<L>(name, round, hs);
  for (std::size_t width : {4U, 16U, 32U, 128U})
    for (const char *mode : {"rolling", "outliers", "jump"})
      cluster<L>(name, round, width, mode);
  many_books<L>(name, round);
  new_books<L>(name, round);
}
} // namespace

int main() {
  const auto er = std::getenv("OMS_BENCH_ROUNDS");
  const std::size_t rounds = er ? std::stoull(er) : 6;
  std::vector<Handle> hs;
  for (std::size_t i = 0; i < 64; ++i)
    hs.push_back(Handle{i, 1});
  std::printf("round,backend,scenario,metric,per_op_p50_ns,per_op_p90_ns\n");
  constexpr std::size_t B = 6;
  for (std::size_t r = 0; r < rounds; ++r)
    for (std::size_t k = 0; k < B; ++k) {
      const auto pos = (r / B) % 2 ? B - 1 - k : k;
      switch ((r + pos) % B) {
      case 0:
        run<BtreeLocator>("absl_btree", r, hs);
        break;
      case 1:
        run<PagedLocator>("pool_pages", r, hs);
        break;
      case 2:
        run<SortedDequeLocator>("sorted_deque", r, hs);
        break;
      case 3:
        run<AdaptiveLocator<>>("adaptive", r, hs);
        break;
      case 4:
        run<SlideLocator<64>>("window64", r, hs);
        break;
      default:
        run<SlideLocator<128>>("window128", r, hs);
      }
    }
}
