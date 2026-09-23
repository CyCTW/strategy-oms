#include "hash_ordered_index.hpp"
#include "measure.hpp"
#include <string>
using namespace oms;
using measurement::Sample;
// Locator-only diagnostics: isolate maintenance cost from the full OMS above it.
template <class L> void run(std::size_t n, std::string_view backend) {
  std::cout << "pass,round,backend,scenario,metric,n,p50_ns,p99_ns,p999_ns,max_ns,allocations,allocated_bytes\n";
  for (const auto width : {4U, 32U, 128U, 1024U}) {
    PagePool pool;
    L index;
    for (std::size_t i = 0; i < width; ++i)
      index.insert(pool, Price(i) * 4, Handle{i, 1}, true);
    Sample query(n), existing(n), remove(n), reinsert(n), range(n);
    std::uint64_t seed = 17;
    for (std::size_t i = 0; i < n; ++i) {
      seed = seed * 6364136223846793005ULL + 1;
      const Price p = Price((seed >> 32) % width) * 4;
      // Randomized hit/miss positions, shared deterministic stream.
      const Price target = p + (i % 8 == 0 ? 1 : 0);
      query.measure([&] { measurement::escape(target); return index.get(pool, target); });
      existing.measure([&] { index.insert(pool, p, Handle{std::size_t(p / 4), 1}, true); });
      range.measure([&] {
        std::uint64_t sum = 0;
        index.range(pool, p, p + 28, [&](Price, Handle h) { sum += h.slot; return true; });
        return sum;
      });
      const Price best = Price(width - 1) * 4;
      const auto next = remove.measure([&] { index.remove(pool, best); return index.best(pool, true); });
      if (next != std::optional<Price>(best - 4)) throw std::logic_error("best after remove");
      reinsert.measure([&] { index.insert(pool, best, Handle{width - 1, 1}, true); });
    }
    const auto scenario = "locator_" + std::to_string(width);
    for (auto [s, label] : std::initializer_list<std::pair<Sample *, const char *>>{
           {&query, "mixed_exact"}, {&existing, "existing_price_assign"},
           {&range, "range_up_to_8"}, {&remove, "remove_best_and_find_next"},
           {&reinsert, "reinsert_best"}})
      s->print(0, backend, scenario, label);
  }
  // Far-separated monotonic prices force new levels/pages. No global reserve.
  PagePool pool;
  L index;
  Sample growth(n), shrink(n);
  for (std::size_t i = 0; i < n; ++i)
    growth.measure([&] { index.insert(pool, Price(i) * 1000003 - 10000000000LL, Handle{i, 1}, true); });
  for (std::size_t i = n; i-- > 0;)
    shrink.measure([&] { index.remove(pool, Price(i) * 1000003 - 10000000000LL); return index.best(pool, true); });
  if (index.best(pool, true)) throw std::logic_error("not empty");
  growth.print(0, backend, "sparse_growth", "new_price");
  shrink.print(0, backend, "sparse_growth", "remove_best_and_find_next");
}
int main(int argc, char **argv) {
  if (argc != 3) return 2;
  const auto n = std::stoull(argv[2]);
  if (!n || n > 200000) return 2;
  const std::string backend = argv[1];
  if (backend == "standard") run<BtreeLocator>(n, backend);
  else if (backend == "pages") run<PagedLocator>(n, backend);
  else if (backend == "hash_ordered") run<HashOrderedLocator>(n, backend);
  else return 2;
}
