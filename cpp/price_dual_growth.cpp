#include "hash_ordered_index.hpp"
#include "measure.hpp"
// Diagnostic only: recording capacity is outside the timed insertion. Do not
// merge these samples with the uninstrumented comparison benchmark.
int main() {
  constexpr std::size_t n = 20000;
  oms::PagePool pool;
  oms::HashOrderedLocator index;
  measurement::Sample samples(n);
  std::vector<std::size_t> capacities;
  capacities.reserve(n + 1);
  capacities.push_back(index.exact.capacity());
  for (std::size_t i = 0; i < n; ++i) {
    samples.measure([&] { index.insert(pool, oms::Price(i) * 1000003 - 10000000000LL, oms::Handle{i, 1}, true); });
    capacities.push_back(index.exact.capacity());
  }
  std::cout << "step,capacity_before,capacity_after,ns\n";
  for (std::size_t i = 0; i < n; ++i)
    std::cout << i << ',' << capacities[i] << ',' << capacities[i + 1] << ',' << samples.times[i] << '\n';
}
