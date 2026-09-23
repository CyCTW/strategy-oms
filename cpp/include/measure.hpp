#pragma once
#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <limits>
#include <new>
#include <string_view>
#include <type_traits>
#include <vector>

namespace measurement {
inline thread_local bool tracking = false;
inline thread_local std::uint64_t calls = 0, allocated = 0, freed = 0;
#ifdef OMS_ALLOC_COUNT
constexpr bool counting = true;
struct Header {
  void *base;
  std::size_t size;
};
inline void *allocate(std::size_t n, std::size_t alignment) {
  alignment = std::max(alignment, alignof(std::max_align_t));
  n = std::max(n, std::size_t{1});
  if (n > std::numeric_limits<std::size_t>::max() - alignment - sizeof(Header))
    throw std::bad_alloc();
  void *base = std::malloc(n + alignment + sizeof(Header));
  if (!base)
    throw std::bad_alloc();
  const auto raw = reinterpret_cast<std::uintptr_t>(base) + sizeof(Header);
  const auto aligned = (raw + alignment - 1) & ~(alignment - 1);
  auto *h = reinterpret_cast<Header *>(aligned) - 1;
  ::new (static_cast<void *>(h)) Header{base, n};
  if (tracking) {
    ++calls;
    allocated += n;
  }
  return reinterpret_cast<void *>(aligned);
}
inline void release(void *p) noexcept {
  if (!p)
    return;
  const auto h = *(reinterpret_cast<Header *>(p) - 1);
  if (tracking)
    freed += h.size;
  std::free(h.base);
}
#else
constexpr bool counting = false;
#endif
template <class T> inline void escape(const T &x) {
  asm volatile("" : : "g"(&x) : "memory");
}
struct Sample {
  std::vector<std::uint64_t> times;
  std::uint64_t allocations{}, bytes{};
  explicit Sample(std::size_t n) { times.reserve(n); }
  template <class F> auto measure(F &&f) {
    const auto a = calls, b = allocated;
    std::chrono::steady_clock::time_point start;
    if constexpr (counting)
      tracking = true;
    else {
      std::atomic_signal_fence(std::memory_order_seq_cst);
      start = std::chrono::steady_clock::now();
    }
    auto finish = [&] {
      if constexpr (counting) {
        tracking = false;
        allocations += calls - a;
        bytes += allocated - b;
        times.push_back(0);
      } else {
        std::atomic_signal_fence(std::memory_order_seq_cst);
        const auto end = std::chrono::steady_clock::now();
        times.push_back(static_cast<std::uint64_t>(
            std::chrono::duration_cast<std::chrono::nanoseconds>(end - start)
                .count()));
      }
    };
    if constexpr (std::is_void_v<std::invoke_result_t<F>>) {
      f();
      finish();
    } else {
      auto result = f();
      escape(result);
      finish();
      return result;
    }
  }
  void subset(const Sample &source, std::uint64_t a, std::uint64_t b) {
    times.push_back(source.times.back());
    allocations += source.allocations - a;
    bytes += source.bytes - b;
  }
  void print(std::size_t round, std::string_view backend,
             std::string_view scenario, std::string_view metric) {
    if (times.empty())
      return;
    std::sort(times.begin(), times.end());
    auto q = [&](std::size_t p) {
      return times[(times.size() - 1) * p / 1000];
    };
    std::cout << (counting ? "alloc" : "time") << ',' << round << ',' << backend
              << ',' << scenario << ',' << metric << ',' << times.size() << ','
              << q(500) << ',' << q(990) << ',' << q(999) << ',' << times.back()
              << ',' << allocations << ',' << bytes << '\n';
  }
};
} // namespace measurement

// Only the allocation executable overrides allocation. The timing executable
// uses the untouched system allocator: instrumentation does not skew latency.
#ifdef OMS_ALLOC_COUNT
void *operator new(std::size_t n) {
  return measurement::allocate(n, alignof(std::max_align_t));
}
void *operator new[](std::size_t n) { return ::operator new(n); }
void operator delete(void *p) noexcept { measurement::release(p); }
void operator delete[](void *p) noexcept { measurement::release(p); }
void operator delete(void *p, std::size_t) noexcept { measurement::release(p); }
void operator delete[](void *p, std::size_t) noexcept {
  measurement::release(p);
}
void *operator new(std::size_t n, std::align_val_t a) {
  return measurement::allocate(n, static_cast<std::size_t>(a));
}
void *operator new[](std::size_t n, std::align_val_t a) {
  return ::operator new(n, a);
}
void operator delete(void *p, std::align_val_t) noexcept {
  measurement::release(p);
}
void operator delete[](void *p, std::align_val_t) noexcept {
  measurement::release(p);
}
void operator delete(void *p, std::size_t, std::align_val_t) noexcept {
  measurement::release(p);
}
void operator delete[](void *p, std::size_t, std::align_val_t) noexcept {
  measurement::release(p);
}
#endif
