#!/usr/bin/env python3
"""Summarize raw CSV without treating median-of-run p99 as pooled p99."""
import csv
import statistics
import sys
from pathlib import Path

source = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('docs/results/index-comparison-2026-09-20.csv')
rows = list(csv.DictReader(source.open()))
backends = ['legacy', 'standard', 'pooled']

def times(scenario, metric, backend):
    return [int(r['p99_ns']) for r in rows if r['pass'] == 'time' and r['scenario'] == scenario and r['metric'] == metric and r['backend'] == backend]

def cell(scenario, metric, backend):
    values = times(scenario, metric, backend)
    if not values:
        return '—'
    return f'{statistics.median(values):g} ({min(values)}–{max(values)})'

print('# 價格索引比較：量測摘要\n')
print('此表列出各輪 p99 的中位數（括號為各輪 p99 最小～最大值），單位 ns；不是合併所有樣本後的 p99。\n')
print('| 情境／操作 | v0.3 舊索引 | 共用 Pool＋標準 B-tree | 共用 Pool＋AVL 候選 |')
print('|---|---:|---:|---:|')
for scenario, metric in [
    ('small', 'price_hit'), ('small', 'handle_hit'), ('small', 'index_update'),
    ('crowded', 'index_update'), ('many_prices', 'price_hit'), ('many_prices', 'handle_hit'),
    ('many_prices', 'range_first8'), ('many_prices', 'index_update'),
    ('many_books', 'price_hit'), ('many_books', 'handle_hit'),
    ('churn', 'index_update'), ('growth', 'index_update'), ('growth', 'block_growth_subset'),
    ('engine', 'intent_immediate'), ('engine', 'intent_queued'), ('engine', 'ready_to_command'),
    ('engine', 'replace_report'), ('engine_growth', 'new_to_command'),
    ('burst64_period100000ns', 'fill_service'), ('burst64_period100000ns', 'scheduled_arrival_to_visible')
]:
    print('| ' + scenario + ' / ' + metric + ' | ' + ' | '.join(cell(scenario, metric, b) for b in backends) + ' |')
print('\n## 配置次數\n')
print('另一次獨立的配置計數 pass，計數器只在被測函式內啟用；表內未扣除第一筆配置，不把擴容排除。初始化及建立測試輸入的配置不在此表。realloc 計為一次配置及一次釋放。\n')
print('| 情境／操作 | v0.3 舊索引 | 共用 Pool＋標準 B-tree | 共用 Pool＋AVL 候選 |')
print('|---|---:|---:|---:|')
for scenario, metric in [('small','index_update'),('crowded','index_update'),('many_prices','index_update'),('churn','index_update'),('growth','index_update'),('engine','intent_queued'),('engine_growth','new_to_command')]:
    values = []
    for b in backends:
        records = [r for r in rows if r['pass']=='alloc' and r['scenario']==scenario and r['metric']==metric and r['backend']==b]
        values.append(records[0]['allocations'] if records else '—')
    print('| ' + scenario + ' / ' + metric + ' | ' + ' | '.join(values) + ' |')
print('\n原始資料：[CSV](results/' + source.name + ')。所有 p50／p99.9／最大值與釋放次數均保留在 CSV。')
