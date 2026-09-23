#pragma once
#include "oms_index.hpp"
#include <absl/container/flat_hash_map.h>

namespace oms {
// Same ordered/working representation as BtreeLocator; only exact lookup is
// redirected. Values are generation handles, never pointers into either map.
// No fixed capacity and no pre-reserve: growth remains part of the experiment.
struct HashOrderedLocator {
  absl::flat_hash_map<Price, Handle> exact;
  BtreeLocator ordered;
  std::optional<Handle> get(const PagePool &, Price p) const {
    const auto it = exact.find(p);
    return it == exact.end() ? std::nullopt : std::optional(it->second);
  }
  void insert(PagePool &pool, Price p, Handle h, bool w) {
    ordered.insert(pool, p, h, w);
    exact.insert_or_assign(p, h);
  }
  void remove(PagePool &pool, Price p) {
    ordered.remove(pool, p);
    exact.erase(p);
  }
  void working(PagePool &pool, Price p, bool w) {
    ordered.working(pool, p, w);
  }
  std::optional<Price> best(const PagePool &pool, bool buy) const {
    return ordered.best(pool, buy);
  }
  template <class F>
  void range(const PagePool &pool, Price lo, Price hi, F &&f) const {
    ordered.range(pool, lo, hi, std::forward<F>(f));
  }
};
} // namespace oms
