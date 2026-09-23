#!/usr/bin/env python3
"""Render medians of per-round quantiles; never call them pooled quantiles."""
import argparse
import collections
import csv
import statistics
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('tag')
a = p.parse_args()
root = Path(__file__).resolve().parents[1]
base = root / f'docs/results/price-dual-{a.tag}'
rows = list(csv.DictReader(Path(str(base) + '-time.csv').open()))
alloc = list(csv.DictReader(Path(str(base) + '-alloc.csv').open()))
groups = collections.defaultdict(list)
for r in rows:
    groups[r['workload'], r['scenario'], r['metric'], r['backend']].append(r)
backends = ['standard', 'pages', 'hash_ordered']
def value(w, s, m, b, column='p99_ns', scale=1):
    ns = [float(r[column]) / scale for r in groups[w, s, m, b]]
    return f'{statistics.median(ns):.2f} ({min(ns):.2f}–{max(ns):.2f})'
lines = ['# C++ 價格雙索引比較', '',
         '單位 ns；數值為每輪分位數的中位數（輪間最小–最大），不是合併樣本分位數。方法與限制見 [說明](price-dual.md)。', '']
for title, workload, scenarios, metrics, column, scale in [
    ('完整 OMS 三段延遲 p99', 'oms', [f'single_{n}' for n in (4,32,128,1024)],
     ['price_query','intent_to_command','guarded_report_visible','ready_to_command'], 'p99_ns', 1),
    ('完整 OMS 尾端 p99.9', 'oms', [f'single_{n}' for n in (4,32,128,1024)],
     ['intent_to_command','guarded_report_visible','ready_to_command'], 'p999_ns', 1),
    ('批次查詢 p50 / 64：平均成本參考，非單次 p50 或 p99', 'oms', [f'single_{n}' for n in (4,32,128,1024)],
     ['price_query_batch64'], 'p50_ns', 64),
    ('獨立 locator p99', 'locator', [f'locator_{n}' for n in (4,32,128,1024)],
     ['mixed_exact','existing_price_assign','range_up_to_8','remove_best_and_find_next','reinsert_best'], 'p99_ns', 1),
    ('稀疏成長 / 刪除 p99', 'locator', ['sparse_growth'],
     ['new_price','remove_best_and_find_next'], 'p99_ns', 1),
    ('稀疏成長 / 刪除 p99.9', 'locator', ['sparse_growth'],
     ['new_price','remove_best_and_find_next'], 'p999_ns', 1),
    ('稀疏成長 / 刪除 max（各輪 max 中位數與範圍）', 'locator', ['sparse_growth'],
     ['new_price','remove_best_and_find_next'], 'max_ns', 1),
]:
    lines += [f'## {title}', '', '| 情境 | 操作 | B-tree | 分頁 | Hash＋B-tree |', '|---|---|---:|---:|---:|']
    for s in scenarios:
        for m in metrics:
            lines.append('| ' + ' | '.join([s,m] + [value(workload,s,m,b,column,scale) for b in backends]) + ' |')
    lines.append('')
lines += ['## Locator 配置：獨立 pass，累計值非常駐記憶體', '', '| 情境 | 操作 | 版本 | 次數 | 累計配置 bytes |', '|---|---|---|---:|---:|']
for r in alloc:
    if int(r['allocations']) or r['scenario'] == 'sparse_growth':
        lines.append('| ' + ' | '.join(r[k] for k in ['scenario','metric','backend','allocations','allocated_bytes']) + ' |')
lines += ['', f'原始時間：[CSV](results/price-dual-{a.tag}-time.csv)。',
          f'配置：[CSV](results/price-dual-{a.tag}-alloc.csv)。',
          f'驗證與環境：[JSON](results/price-dual-{a.tag}-metadata.json)。', '']
(root / 'docs/price-dual-results.md').write_text('\n'.join(lines))
