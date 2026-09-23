#include "codec.hpp"
#include "hash_ordered_index.hpp"
#include "measure.hpp"
#include <fstream>
using namespace parity;
using measurement::Sample;
template <class L> void trace(const char *path) {
  std::ifstream file(path);
  if (!file)
    throw std::runtime_error("trace missing");
  Limits limits;
  std::size_t capacity = 10000;
  auto e = std::make_unique<Engine<L>>(capacity, limits);
  Known known;
  std::string line;
  std::size_t step = 0;
  while (std::getline(file, line)) {
    if (line.empty() || line[0] == '#')
      continue;
    std::istringstream in(line);
    std::vector<std::string> t;
    for (std::string s; in >> s;)
      t.push_back(s);
    Hash result;
    std::string status = "OK";
    try {
      std::optional<Outcome> out;
      if (t[0] == "RESET") {
        limits = {std::stoull(t[1]), std::stoull(t[2]), std::stoull(t[3]),
                  std::stoull(t[4]), std::stoull(t[5]), std::stoull(t[6])};
        capacity = std::stoull(t[7]);
        e = std::make_unique<Engine<L>>(capacity, limits);
        known = {};
      } else if (t[0] == "DISPATCH")
        out = e->dispatch_next_order();
      else if (t[0] == "RECOVER")
        e = Engine<L>::recover(e->journal(), capacity, limits);
      else if (t[0] == "HOLD")
        e->hold_for_recovery();
      else {
        auto ev = *parse(t, *e, known);
        if (t[0] == "REPORT")
          out = e->on_report(std::get<Report>(ev));
        else
          out = e->apply(ev);
      }
      result.add(out.has_value());
      if (out)
        encode(result, *out);
    } catch (Error error) {
      status = error_name(error);
    }
    Hash journal;
    for (const auto &event : e->journal())
      encode(journal, event);
    std::cout << step++ << ' ' << status << ' ' << result.value << ' '
              << snapshot(*e, known) << ' ' << journal.value << ' '
              << e->journal().size() << '\n';
  }
}
Limits bench_limits(std::size_t n, std::size_t width) {
  return {std::max(width, std::size_t{32}),
          4 * n + width + 64,
          n + 64,
          4 * n + width + 64,
          1'000'000,
          1'000'000'000};
}
template <class L>
void bench(std::size_t n, std::size_t width, std::string_view backend) {
  const auto limits = bench_limits(n, width);
  Engine<L> e(6 * n + 4 * width + 64, limits);
  Known known;
  known.books.insert({1, true});
  Id seq = 0;
  for (Id id = 1; id <= width; ++id) {
    e.apply(New{id, id, book(), 100 + Price(id) * 4, 10000});
    e.on_report(
        {1, ++seq, id, Accepted{id, 100 + id, 100 + Price(id) * 4, 10000}});
    known.orders.insert(id);
    known.requests.insert(id);
  }
  const auto bh = *e.index().book_handle(book());
  Sample query(n), immediate(n), queued(n), coalesced(n), ack(n), duplicate(n),
      dispatch(n);
  for (std::size_t step = 0; step < n; ++step) {
    const Id id = step % width + 1, req = width + step * 3 + 1;
    const auto o = *e.order(id);
    query.measure([&] {
      measurement::escape(o.price);
      return e.index().summary(bh, o.price);
    });
    const Price p =
        (step / width) % 2 == 0 ? 1000 + Price(id) * 4 : 100 + Price(id) * 4;
    const Event change = Replace{id, req, o.version, p, 10000};
    const auto a = immediate.measure([&] { return e.apply(change); });
    if (!a.outbound)
      throw std::logic_error("no immediate");
    const Event q = Replace{id, req + 1, e.order(id)->version, p + 1, 10000};
    queued.measure([&] { return e.apply(q); });
    const Event q2 = Replace{id, req + 2, e.order(id)->version, p + 2, 10000};
    coalesced.measure([&] { return e.apply(q2); });
    const Report r{1, ++seq, id, Replaced{req, 100 + id, p, 10000}};
    ack.measure([&] { return e.on_report(r); });
    duplicate.measure([&] { return e.on_report(r); });
    const auto d = dispatch.measure([&] { return e.dispatch_next_order(); });
    if (!d || !d->outbound)
      throw std::logic_error("no deferred");
    e.on_report({1, ++seq, id, Replaced{req + 2, 100 + id, p + 2, 10000}});
  }
  for (Id i = width + 1; i <= width + 3 * n; ++i)
    known.requests.insert(i);
  const auto scenario = "single_" + std::to_string(width);
  std::array<Price, 64> targets{};
  for (std::size_t j = 0; j < targets.size(); ++j)
    targets[j] = j % 8 == 0 ? -999999 : e.order(j % width + 1)->price;
  Sample batches(2000);
  for (std::size_t i = 0; i < 2000; ++i)
    batches.measure([&] {
      Qty sum{};
      for (auto p : targets) {
        measurement::escape(p);
        const auto v = e.index().summary(bh, p);
        sum += v ? v->totals.confirmed : 0;
      }
      return sum;
    });
  batches.print(0, "cpp", std::string(backend) + "_" + scenario,
                "price_query_batch64");
  for (auto [s, label] :
       std::initializer_list<std::pair<Sample *, const char *>>{
           {&query, "price_query"},
           {&immediate, "intent_to_command"},
           {&queued, "queued_intent"},
           {&coalesced, "coalesced_intent"},
           {&ack, "guarded_report_visible"},
           {&duplicate, "report_transport_duplicate"},
           {&dispatch, "ready_to_command"}})
    s->print(0, "cpp", std::string(backend) + "_" + scenario, label);
  Hash j;
  for (const auto &v : e.journal())
    encode(j, v);
  std::cerr << "DIGEST " << scenario << ' ' << snapshot(e, known) << ' '
            << j.value << ' ' << e.journal().size() << '\n';
}
template <class L> void fills(std::size_t n, std::string_view backend) {
  Engine<L> e(3 * n + 64, bench_limits(n, 1));
  Known known;
  known.orders.insert(1);
  known.requests.insert(1);
  known.books.insert({1, true});
  const Qty total = n + 1;
  e.apply(New{1, 1, book(), 100, total});
  e.on_report({1, 1, 1, Accepted{1, 101, 100, total}});
  Sample fill(n), business(n), duplicate(n);
  for (Id i = 1; i <= n; ++i) {
    const Report r{1, i + 1, 1, Fill{{1, 1, 20260922, i}, 1, 100}};
    fill.measure([&] { return e.on_report(r); });
    const Report b{2, i, 1, r.kind};
    business.measure([&] { return e.on_report(b); });
    duplicate.measure([&] { return e.on_report(b); });
  }
  for (Id i = 1; i <= n; ++i)
    known.executions.insert(i);
  for (auto [s, label] :
       std::initializer_list<std::pair<Sample *, const char *>>{
           {&fill, "guarded_fill"},
           {&business, "fill_business_duplicate"},
           {&duplicate, "fill_transport_duplicate"}})
    s->print(0, "cpp", std::string(backend) + "_fills", label);
  Hash j;
  for (const auto &v : e.journal())
    encode(j, v);
  std::cerr << "DIGEST fills " << snapshot(e, known) << ' ' << j.value << ' '
            << e.journal().size() << '\n';
}
template <class L> void run(int argc, char **argv) {
  if (std::string(argv[1]) == "trace") {
    trace<L>(argv[3]);
    return;
  }
  const auto n = argc > 3 ? std::stoull(argv[3]) : 20000;
  if (!n || n > 200000)
    throw std::runtime_error("benchmark sample guard");
  std::cout << "pass,round,backend,scenario,metric,n,p50_ns,p99_ns,p999_ns,max_"
               "ns,allocations,allocated_bytes\n";
  for (auto width : {4U, 32U, 128U, 1024U})
    bench<L>(n, width, argv[2]);
  fills<L>(n, argv[2]);
  Sample timer(n);
  for (std::size_t i = 0; i < n; ++i)
    timer.measure([] { asm volatile("" ::: "memory"); });
  timer.print(0, "cpp", "baseline", "timer");
}
int main(int argc, char **argv) {
  if (argc == 2 && std::string(argv[1]) == "sizes") {
    std::cout << "Order " << sizeof(Order) << " Request " << sizeof(Request)
              << " Report " << sizeof(Report) << " Event " << sizeof(Event)
              << "\n";
    return 0;
  }
  if (argc < 4)
    return 2;
  try {
    if (std::string(argv[2]) == "standard")
      run<oms::BtreeLocator>(argc, argv);
    else if (std::string(argv[2]) == "pages")
      run<oms::PagedLocator>(argc, argv);
    else if (std::string(argv[2]) == "hash_ordered")
      run<oms::HashOrderedLocator>(argc, argv);
    else
      return 2;
  } catch (Error e) {
    std::cerr << error_name(e) << '\n';
    return 1;
  }
}
