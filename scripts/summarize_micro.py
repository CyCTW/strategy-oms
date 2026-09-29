#!/usr/bin/env python3
"""Summarize batch micro benchmark CSVs (Rust locator_micro / index_micro,
C++ micro): round,backend,scenario,metric,per_op_p50_ns,per_op_p90_ns.

Each cell is the median over rounds of that round's per-operation p50
(batch time / batch size), with the min-max across rounds in parentheses.

usage: summarize_micro.py TITLE CSV BACKEND[,BACKEND...] [TITLE CSV BACKENDS ...]
"""
import csv
import statistics
import sys
from collections import defaultdict


def table(title, path, backends):
    data = defaultdict(lambda: defaultdict(list))
    order = []
    with open(path) as f:
        for r in csv.DictReader(f):
            key = (r["scenario"], r["metric"])
            if key not in order:
                order.append(key)
            data[key][r["backend"]].append(float(r["per_op_p50_ns"]))
    # Group rows by metric, then scenario, keeping first-seen order.
    metrics = list(dict.fromkeys(m for _, m in order))
    print(f"## {title}\n")
    print("| 情境／操作 | " + " | ".join(backends) + " |")
    print("|---|" + "---:|" * len(backends))
    for m in metrics:
        for s, mm in order:
            if mm != m:
                continue
            cells = []
            for b in backends:
                v = data[(s, m)].get(b)
                if not v:
                    cells.append("—")
                elif len(v) == 1:
                    cells.append(f"{v[0]:.1f}")
                else:
                    cells.append(f"{statistics.median(v):.1f} ({min(v):.1f}–{max(v):.1f})")
            print(f"| {s} / {m} | " + " | ".join(cells) + " |")
    print()


def main():
    args = sys.argv[1:]
    if not args or len(args) % 3:
        sys.exit(__doc__)
    for i in range(0, len(args), 3):
        table(args[i], args[i + 1], args[i + 2].split(","))


if __name__ == "__main__":
    main()
