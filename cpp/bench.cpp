#include "flow.hpp"
#include "linear_index.hpp"
#include "measure.hpp"
#include "oms_index.hpp"
#include "sorted_deque_locator.hpp"
#include <string>
using namespace oms;
using namespace measurement;

Book book(std::size_t id) {
  Book b;
  b.strategy = id;
  return b;
}
Order order(std::size_t id, Book b, Price p) {
  Order o;
  o.id = id + 1;
  o.book = b;
  o.price = p;
  return o;
}
template <class L>
void index_run(std::string_view name, std::size_t width, std::string_view mode,
               std::size_t n, std::size_t round) {
  const bool far = mode == "outliers";
  std::vector<Order> orders;
  for (std::size_t i = 0; i < width; ++i)
    orders.push_back(order(i, book(0), 62 + Price(i)));
  if (far) {
    orders.push_back(order(width, book(0), -1'000'000'000));
    orders.push_back(order(width + 1, book(0), 1'000'000'000));
  }
  std::vector<Memberships> slots(orders.size());
  Index<L> index;
  for (std::size_t i = 0; i < orders.size(); ++i)
    index.update(nullptr, orders[i], slots[i]);
  const auto h = *index.book_handle(book(0));
  Sample query(n), range(n), submit(n), ack(n), cross(n), created(n), growth(n),
      jump(n), far_query(n);
  for (std::size_t step = 0; step < n;) {
    const auto batch = step / width;
    const Price center =
        mode == "boundary" ? (batch % 2 == 0 ? 63 : 62)
        : mode == "jump"
            ? ((batch / 8) % 2 == 0 ? 0 : 1'000'000) + 62 + Price(batch % 8)
            : 63 + Price(batch);
    const auto size = std::min(width, n - step);
    for (std::size_t i = 0; i < size; ++i) {
      const auto old = orders[i];
      auto next = old;
      const Price target = center + Price(i);
      next.pending = Pending{Kind::Replace, target, old.total};
      const auto before = index.stats();
      const auto a = submit.allocations, b = submit.bytes;
      submit.measure([&] {
        escape(next);
        index.update(&old, next, slots[i]);
      });
      const auto after = index.stats();
      if (page_id(old.price) != page_id(target))
        cross.subset(submit, a, b);
      if (after.pages > before.pages)
        created.subset(submit, a, b);
      if (after.page_blocks > before.page_blocks)
        growth.subset(submit, a, b);
      if (std::abs(old.price - target) > 10000)
        jump.subset(submit, a, b);
      query.measure([&] {
        escape(target);
        return index.summary(h, target);
      });
      range.measure([&] {
        Qty sum{};
        std::size_t seen{};
        index.range(h, center, center + Price(width) - 1,
                    [&](Price, Summary s) {
                      sum += s.totals.confirmed;
                      return ++seen < 8;
                    });
        return sum;
      });
      if (far)
        far_query.measure([&] { return index.summary(h, 1'000'000'000); });
      orders[i] = next;
    }
    for (std::size_t i = 0; i < size; ++i) {
      const auto old = orders[i];
      auto next = old;
      next.price = next.pending->price;
      next.pending.reset();
      ack.measure([&] {
        escape(next);
        index.update(&old, next, slots[i]);
      });
      orders[i] = next;
    }
    step += size;
  }
  const auto scenario = std::string(mode) + "_" + std::to_string(width);
  for (auto [s, m] : std::initializer_list<std::pair<Sample *, const char *>>{
           {&query, "price_query"},
           {&range, "near_range8"},
           {&submit, "index_submit"},
           {&ack, "index_ack"},
           {&cross, "cross_page_submit_subset"},
           {&created, "page_creation_subset"},
           {&growth, "page_block_growth_subset"},
           {&jump, "jump_submit_subset"},
           {&far_query, "far_query"}})
    s->print(round, name, scenario, m);
}
template <class L>
void many_books(std::string_view name, std::size_t n, std::size_t round) {
  Index<L> index;
  std::vector<Memberships> slots(4096 * 6);
  std::vector<BookHandle> handles;
  for (std::size_t b = 0; b < 4096; ++b) {
    for (std::size_t j = 0; j < 6; ++j) {
      auto o = order(b * 6 + j, book(b),
                     j < 4    ? 62 + Price(j)
                     : j == 4 ? -1'000'000'000
                              : 1'000'000'000);
      index.update(nullptr, o, slots[b * 6 + j]);
    }
    handles.push_back(*index.book_handle(book(b)));
  }
  Sample q(n);
  std::uint64_t seed = 11;
  for (std::size_t i = 0; i < n; ++i) {
    seed = seed * 6364136223846793005ULL + 1;
    q.measure([&] {
      escape(seed);
      return index.summary(handles[(seed >> 32) % handles.size()],
                           62 + Price(seed % 4));
    });
  }
  q.print(round, name, "4096books_4near_2far", "price_query");
}
template <class L>
void grow(std::string_view name, std::size_t n, std::size_t round) {
  Index<L> index;
  std::vector<Memberships> slots(n);
  Sample all(n), growth(n), cadence(n);
  for (std::size_t i = 0; i < n; ++i) {
    auto o = order(i, book(i), 1'000'000'000);
    const auto before = index.stats().page_blocks;
    const auto a = all.allocations, b = all.bytes;
    all.measure([&] {
      escape(o);
      index.update(nullptr, o, slots[i]);
    });
    if (index.stats().page_blocks > before)
      growth.subset(all, a, b);
    // Same ordinal positions for both backends, avoiding conditional mismatch.
    if (i % 64 == 0)
      cadence.subset(all, a, b);
  }
  all.print(round, name, "new_book_growth", "index_insert");
  growth.print(round, name, "new_book_growth", "page_block_growth_subset");
  cadence.print(round, name, "new_book_growth", "every64_subset");
}
template <class L>
void footprint(std::string_view name, std::size_t count, std::size_t width,
               bool far) {
  const auto per = width + (far ? 2 : 0);
  std::vector<Order> orders;
  std::vector<Memberships> slots(count * per);
  for (std::size_t i = 0; i < count * per; ++i) {
    const auto j = i % per;
    orders.push_back(order(i, book(i / per),
                           j < width    ? 62 + Price(j)
                           : j == width ? -1'000'000'000
                                        : 1'000'000'000));
  }
  const auto a = calls, b = allocated, f = freed;
  tracking = true;
  Index<L> index;
  for (std::size_t i = 0; i < orders.size(); ++i)
    index.update(nullptr, orders[i], slots[i]);
  tracking = false;
  const auto live = allocated - b - (freed - f) + sizeof(index);
  const auto stats = index.stats();
  const auto scenario = std::to_string(count) + "books_" +
                        std::to_string(width) + "near_" + (far ? "2" : "0") +
                        "far";
  std::cout << "memory,0," << name << ',' << scenario
            << ",resident_index_bytes,1,0,0,0,0," << calls - a << ',' << live
            << '\n';
  std::cout << "capacity,0," << name << ',' << scenario
            << ",pages_and_blocks,1,0,0,0,0," << stats.pages << ','
            << stats.page_blocks << '\n';
}
template <class L>
void flow_run(std::string_view name, std::size_t width, std::string_view mode,
              std::size_t n, std::size_t round) {
  Flow<L> flow;
  for (std::size_t i = 0; i < width + 2; ++i) {
    auto o = order(i, book(0),
                   i < width    ? 62 + Price(i)
                   : i == width ? -1'000'000'000
                                : 1'000'000'000);
    o.total = o.leaves = 100;
    flow.seed(o);
  }
  Sample query(n), immediate(n), queued(n), report(n), ready(n);
  std::uint64_t seq = 0;
  const auto h = *flow.index().book_handle(book(0));
  for (std::size_t step = 0; step < n; ++step) {
    const auto id = step % width + 1, gen = step / width,
               req = width + 3 + step * 2;
    const Price target = (mode == "jump" && (gen / 8) % 2 ? 1'000'000 : 0) +
                         66 + Price(gen) * 4 + Price(step % width);
    const auto old = flow.price(id);
    auto v = flow.version(id);
    query.measure([&] {
      escape(old);
      return flow.index().summary(h, old);
    });
    if (!immediate.measure([&] { return flow.replace(id, req, v, target); }))
      throw std::logic_error("expected command");
    v = flow.version(id);
    if (queued.measure(
            [&] { return flow.replace(id, req + 1, v, target + 1); }))
      throw std::logic_error("expected queued intent");
    ++seq;
    report.measure([&] { flow.acknowledge(id, req, seq, target); });
    if (!ready.measure([&] { return flow.dispatch(); }))
      throw std::logic_error("missing deferred command");
    flow.acknowledge(id, req + 1, ++seq, target + 1);
  }
  const auto scenario =
      "prototype_" + std::string(mode) + "_" + std::to_string(width) + "_2far";
  for (auto [s, m] : std::initializer_list<std::pair<Sample *, const char *>>{
           {&query, "price_query"},
           {&immediate, "intent_to_command"},
           {&queued, "intent_queued"},
           {&report, "replace_report"},
           {&ready, "ready_to_command"}})
    s->print(round, name, scenario, m);
}
template <class L>
void cancel_rehang(std::string_view name, std::size_t n, std::size_t round) {
  Index<L> index;
  std::vector<Memberships> slots(n + 5);
  std::vector<Order> history;
  history.reserve(n + 5);
  for (std::size_t i = 0; i < 5; ++i) {
    auto o = order(i, book(0), 63 + Price(i));
    o.total = o.leaves = 100;
    index.update(nullptr, o, slots[i]);
    history.push_back(o);
  }
  const auto h = *index.book_handle(book(0));
  std::size_t active = 4;
  Sample cancel(n), submitted(n), accepted(n), canceled(n);
  for (std::size_t step = 0; step < n; ++step) {
    auto old = history[active], next = old;
    next.pending = Pending{Kind::Cancel, old.price, old.total};
    cancel.measure([&] { index.update(&old, next, slots[active]); });
    history[active] = next;
    const auto id = history.size();
    auto o = order(id, book(0),
                   step % 2 == 0 ? -1'000'000'000 : 68 + Price(step % 32));
    o.total = 100;
    o.leaves = 0;
    o.pending_new = true;
    o.pending = Pending{Kind::New, o.price, 100};
    submitted.measure([&] { index.update(nullptr, o, slots[id]); });
    history.push_back(o);
    next = o;
    next.leaves = 100;
    next.pending_new = false;
    next.pending.reset();
    accepted.measure([&] { index.update(&o, next, slots[id]); });
    history[id] = next;
    if (index.reserved(h) != 600)
      throw std::logic_error("lost inflight risk");
    old = history[active];
    next = old;
    next.leaves = 0;
    next.pending.reset();
    next.terminal = true;
    canceled.measure([&] { index.update(&old, next, slots[active]); });
    history[active] = next;
    if (index.reserved(h) != 500)
      throw std::logic_error("incorrect terminal risk");
    active = id;
  }
  for (auto [s, m] : std::initializer_list<std::pair<Sample *, const char *>>{
           {&cancel, "index_cancel"},
           {&submitted, "index_new"},
           {&accepted, "index_accepted"},
           {&canceled, "index_canceled"}})
    s->print(round, name, "cancel_rehang_4near", m);
}
template <class L>
void run(std::string_view name, std::size_t n, std::size_t round) {
  for (auto width : {4U, 32U, 128U})
    for (auto mode : {"rolling", "outliers", "boundary", "jump"})
      index_run<L>(name, width, mode, n, round);
  many_books<L>(name, n, round);
  grow<L>(name, n, round);
  cancel_rehang<L>(name, n, round);
  for (auto width : {4U, 32U})
    for (auto mode : {"rolling", "jump"})
      flow_run<L>(name, width, mode, n, round);
  if constexpr (counting)
    for (auto books : {1U, 128U})
      for (auto width : {4U, 32U, 128U})
        for (auto far : {false, true})
          footprint<L>(name, books, width, far);
}
int main() {
  const auto env_n = std::getenv("OMS_BENCH_SAMPLES"),
             env_r = std::getenv("OMS_BENCH_ROUNDS");
  const std::size_t n = env_n ? std::stoull(env_n) : 20000,
                    rounds = counting ? 1
                             : env_r  ? std::stoull(env_r)
                                      : 12;
  if (n == 0 || rounds == 0 || n > 1'000'000)
    throw std::invalid_argument(
        "invalid benchmark size (guard, not index limit)");
  std::cout << "pass,round,backend,scenario,metric,n,p50_ns,p99_ns,p999_ns,max_"
               "ns,allocations,allocated_bytes\n";
  // Rotate the backend order every round and reverse it every B rounds, so
  // each backend runs in every position equally often over 2B rounds.
  constexpr std::size_t B = 6;
  for (std::size_t r = 0; r < rounds; ++r)
    for (std::size_t k = 0; k < B; ++k) {
      const auto pos = (r / B) % 2 ? B - 1 - k : k;
      const auto b = (r + pos) % B;
      if (b == 0)
        run<BtreeLocator>("absl_btree", n, r);
      else if (b == 1)
        run<PagedLocator>("pool_pages", n, r);
      else if (b == 2)
        run<SortedDequeLocator>("sorted_deque", n, r);
      else if (b == 3)
        run<SortedDequeSoaLocator>("sorted_deque_soa", n, r);
      else if (b == 4)
        run<LinearLocator>("linear_prices", n, r);
      else
        run<AdaptiveLocator<>>("adaptive", n, r);
    }
  if constexpr (!counting) {
    Sample timer(n);
    for (std::size_t i = 0; i < n; ++i)
      timer.measure([] { asm volatile("" ::: "memory"); });
    timer.print(0, "timer", "baseline", "timer");
  }
}
