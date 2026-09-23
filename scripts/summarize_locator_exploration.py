#!/usr/bin/env python3
"""Summarize locator screening; never label it Engine latency."""
import csv
import statistics
import sys
from pathlib import Path

source = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('docs/results/locator-exploration-2026-09-21.csv')
rows = list(csv.DictReader(source.open()))
backends = ['standard', 'flat', 'adaptive4', 'adaptive8', 'adaptive16', 'hash_ordered', 'paged64']

def cell(scenario, metric, backend, field='p99_ns'):
    vals = [int(r[field]) for r in rows if r['pass'] == 'time' and r['scenario'] == scenario and r['metric'] == metric and r['backend'] == backend]
    return f'{statistics.median(vals):g} ({min(vals)}–{max(vals)})' if vals else '—'

def header(first):
    print('| ' + first + ' | ' + ' | '.join(backends) + ' |')
    print('|---|' + '---:|' * len(backends))

selected = [
    ('sparse_n4', 'hit'), ('sparse_n4', 'reprice'), ('sparse_n8', 'reprice'),
    ('4096books_4prices', 'hit'), ('sparse_n4096', 'hit'), ('sparse_n4096', 'miss'),
    ('sparse_n4096', 'range8'), ('sparse_n4096', 'reprice'),
    ('dense_n4096', 'hit'), ('dense_n4096', 'range8'), ('dense_n4096', 'reprice'),
    ('insert_after4', 'insert'), ('insert_after8', 'insert'), ('insert_after16', 'insert'),
    ('random_growth', 'insert'), ('4095_pending_only', 'refresh_best'),
]
print('# 替代價格定位器：實驗摘要\n')
print('僅測 locator，不含完整價格索引、回報 reducer、intent、WAL 或網路。不能與先前 Engine 數字直接比較。\n')
timing = [r for r in rows if r['pass'] == 'time' and r['backend'] != 'timer']
round_count = len({r['round'] for r in timing})
sample_counts = ', '.join(str(n) for n in sorted({int(r['n']) for r in timing}))
print(f'每格是 {round_count} 輪分位數的中位數（最小–最大），單位 ns；不是合併樣本的分位數。每輪每操作樣本數：{sample_counts}。\n')
for field, title in [('p99_ns','p99'), ('p999_ns','p99.9')]:
    print('## ' + title + '\n')
    header('情境／操作')
    for scenario, metric in selected:
        print('| ' + scenario + ' / ' + metric + ' | ' + ' | '.join(cell(scenario,metric,b,field) for b in backends) + ' |')
    print()
print('## 配置次數\n')
print('獨立 pass；僅計入受測函式的 alloc/realloc，初始建構不計入，promotion 的配置不排除。\n')
header('情境／操作')
for scenario, metric in [('sparse_n4','reprice'),('sparse_n4096','reprice'),('dense_n4096','reprice'),('insert_after8','insert'),('random_growth','insert')]:
    cells = []
    for b in backends:
        records = [r for r in rows if r['pass']=='alloc' and r['backend']==b and r['scenario']==scenario and r['metric']==metric]
        cells.append(records[0]['allocations'] if records else '—')
    print('| ' + scenario + ' / ' + metric + ' | ' + ' | '.join(cells) + ' |')
print('\n## 定位器記憶體（bytes）\n')
print('建構後仍存活的 allocator requested bytes 加 size_of(locator)，含 inline 陣列；不是 RSS、不含 allocator metadata，也不含 Level/Order/Member pools。\n')
header('情境')
for scenario in ['sparse_n4','sparse_n8','sparse_n16','sparse_n4096','dense_n4096']:
    cells = []
    for b in backends:
        records = [r for r in rows if r['pass']=='memory' and r['backend']==b and r['scenario']==scenario]
        cells.append(records[0]['allocated_bytes'] if records else '—')
    print('| ' + scenario + ' | ' + ' | '.join(cells) + ' |')
print('\n計時器基準 p99：' + cell('baseline','timer','timer') + ' ns。接近此數值的結果無法可靠排序。\n')
print('原始資料：[CSV](results/' + source.name + ')。所有 p50/p99/p99.9/max、累計配置 bytes 均保留。')
