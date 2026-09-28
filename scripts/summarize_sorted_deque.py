#!/usr/bin/env python3
"""Summarize the sorted-deque / adaptive locator comparison.

Each cell is the median over rounds of that round's quantile, with the
min-max across rounds in parentheses (ns). It is NOT a pooled quantile.

usage: summarize_sorted_deque.py CLUSTERED_TIME LINEAR_TIME CLUSTERED_ALLOC LINEAR_ALLOC
"""
import csv
import statistics
import sys
from collections import defaultdict

BACKENDS = [
    "absl_btree",
    "pool_pages",
    "sorted_deque",
    "sorted_deque_soa",
    "adaptive",
    "linear_prices",
]


def load(paths, kind):
    out = defaultdict(lambda: defaultdict(list))
    for path in paths:
        with open(path) as f:
            for r in csv.DictReader(f):
                if r["pass"] != kind or r["backend"] == "timer":
                    continue
                out[(r["scenario"], r["metric"])][r["backend"]].append(r)
    return out


def cell(rows, field):
    if not rows:
        return "—"
    v = [float(r[field]) for r in rows]
    med = statistics.median(v)
    if len(v) == 1:
        return f"{med:.0f}"
    return f"{med:.0f} ({min(v):.0f}–{max(v):.0f})"


def table(data, keys, field, title):
    print(f"## {title}\n")
    print("| 情境／操作 | " + " | ".join(BACKENDS) + " |")
    print("|---|" + "---:|" * len(BACKENDS))
    for k in keys:
        if k not in data:
            continue
        print(f"| {k[0]} / {k[1]} | " + " | ".join(cell(data[k].get(b), field) for b in BACKENDS) + " |")
    print()


def main():
    ct, lt, ca, la = sys.argv[1:5]
    time = load([ct, lt], "time")
    alloc = load([ca, la], "alloc")
    baseline = []
    for path in (ct, lt):
        with open(path) as f:
            baseline += [r for r in csv.DictReader(f) if r["backend"] == "timer"]
    print("# 雙端排序陣列與自適應定位器：量測摘要\n")
    print("各格為各輪分位數的中位數（輪間最小–最大），單位 ns；不是合併樣本的分位數。")
    print("計時器未扣除；空操作基線 p50／p99：" + "、".join(f"{r['p50_ns']}／{r['p99_ns']}" for r in baseline) + " ns。")
    print("說明、方法與限制見 [sorted-deque.md](sorted-deque.md)。\n")

    clustered = sorted(k for k in time if not k[0].startswith(("prices_", "crowded_", "updates_", "reprice_", "mixed_", "growth", "prototype_4", "prototype_32", "prototype_128")) or k[0].startswith("prototype_rolling") or k[0].startswith("prototype_jump"))
    linear = sorted(k for k in time if k not in clustered)
    table(time, clustered, "p99_ns", "clustered_bench：p99")
    table(time, linear, "p99_ns", "linear_bench（含 reprice_random）：p99")
    keys = sorted(time)
    table(time, keys, "p999_ns", "全部情境：p99.9")
    table(time, keys, "p50_ns", "全部情境：p50")
    print("## 配置次數（獨立 allocation pass，一輪）\n")
    print("| 情境／操作 | " + " | ".join(BACKENDS) + " |")
    print("|---|" + "---:|" * len(BACKENDS))
    for k in sorted(alloc):
        row = alloc[k]
        if all(r and r[0]["allocations"] == "0" for r in row.values()):
            continue
        print(f"| {k[0]} / {k[1]} | " + " | ".join(
            (f"{row[b][0]['allocations']}／{row[b][0]['n']}" if row.get(b) else "—") for b in BACKENDS) + " |")
    print("\n全部 backend 皆為 0 次的列已省略。")


if __name__ == "__main__":
    main()
