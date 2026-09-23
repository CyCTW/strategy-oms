#!/usr/bin/env python3
"""Summarize clustered Index/Engine measurements, preserving run variation."""
import csv
import statistics
import sys
from pathlib import Path

source = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('docs/results/clustered-index-2026-09-21.csv')
rows = list(csv.DictReader(source.open()))
backends = ['standard', 'pages']
timing = [r for r in rows if r['pass'] == 'time' and r['backend'] != 'timer']
rounds = len({r['round'] for r in timing})
samples = next(r['n'] for r in timing if r['scenario']=='rolling_4' and r['metric']=='price_query')

def cell(s, m, b, field='p99_ns'):
    vals = [int(r[field]) for r in rows if r['pass']=='time' and r['scenario']==s and r['metric']==m and r['backend']==b]
    return f'{statistics.median(vals):g} ({min(vals)}–{max(vals)})' if vals else '—'

selected = [
    ('rolling_4','price_query'), ('rolling_4','index_ack'),
    ('rolling_32','price_query'), ('rolling_32','near_range8'), ('rolling_32','index_ack'),
    ('outliers_32','price_query'), ('outliers_32','far_query'), ('outliers_32','index_ack'),
    ('boundary_32','cross_page_submit_subset'), ('jump_32','jump_submit_subset'),
    ('4096books_4near_2far','price_query'), ('new_book_growth','index_insert'),
    ('new_book_growth','page_block_growth_subset'),
]
for s in ['engine_rolling_4_2far','engine_rolling_32_2far','engine_jump_32_2far']:
    selected.extend((s,m) for m in ['price_query','intent_to_command','intent_queued','replace_report','ready_to_command'])
selected.extend(('engine_cancel_rehang_4near',m) for m in ['cancel_to_command','new_to_command','accepted_report','canceled_report'])

print('# 移動近價群＋遠價訂單：量測摘要\n')
print(f'各格為 {rounds} 輪分位數的中位數（輪間最小–最大），單位 ns；不是合併樣本的分位數。一般情境每輪 {samples} 筆，subset 只取符合條件的操作，樣本數見 CSV。\n')
print('index_* 是完整價格索引更新，不含 reducer/WAL；engine_* 使用完整 Engine＋MemoryJournal。所有查詢使用 BookHandle。\n')
for field,title in [('p99_ns','p99'),('p999_ns','p99.9')]:
    print('## ' + title + '\n')
    print('| 情境／操作 | B-tree＋Pool | Pool 稀疏分頁 |\n|---|---:|---:|')
    for s,m in selected:
        print('| ' + s + ' / ' + m + ' | ' + ' | '.join(cell(s,m,b,field) for b in backends) + ' |')
    print()
print('## 獨立配置計數 pass\n')
print('計入受測操作的 alloc/realloc 次數，未排除首次等待集合配置或擴容；不含計時外初始建構。\n')
print('| 情境／操作 | B-tree＋Pool | Pool 稀疏分頁 |\n|---|---:|---:|')
for s,m in [('rolling_4','index_ack'),('rolling_32','index_submit'),('rolling_32','index_ack'),
    ('outliers_32','index_submit'),('outliers_32','index_ack'),('engine_rolling_32_2far','ready_to_command'),
    ('engine_cancel_rehang_4near','new_to_command'),('new_book_growth','index_insert')]:
    cells=[]
    for b in backends:
        record=next(r for r in rows if r['pass']=='alloc' and r['scenario']==s and r['metric']==m and r['backend']==b)
        cells.append(record['allocations'])
    print('| ' + s + ' / ' + m + ' | ' + ' | '.join(cells) + ' |')
print('\n## 索引記憶體（bytes）\n')
print('含 Book/Level/Member/Page pools、目錄及 inline Index。是仍存活的 allocator requested bytes，非 RSS；不含 OrderStore、每張訂單的 Memberships、WAL 或完整 Engine。\n')
print('| 情境 | B-tree＋Pool | Pool 稀疏分頁 | 活頁／已配置頁塊 |\n|---|---:|---:|---:|')
for s in dict.fromkeys(r['scenario'] for r in rows if r['pass']=='memory'):
    cells=[next(r['allocated_bytes'] for r in rows if r['pass']=='memory' and r['scenario']==s and r['backend']==b) for b in backends]
    capacity=next(r for r in rows if r['pass']=='capacity' and r['scenario']==s and r['backend']=='pages')
    print('| ' + s + ' | ' + ' | '.join(cells) + ' | ' + capacity['allocations'] + ' / ' + capacity['allocated_bytes'] + ' |')
print('\n每個頁塊容納 64 頁，每頁覆蓋 64 個原始價格單位；這是兩個不同粒度。空頁可重用，頁塊保留至 Index 銷毀。')
print('\n計時器基準 p99：' + cell('baseline','timer','timer') + ' ns，附近差異無法可靠排序。')
print('\n原始資料：[CSV](results/' + source.name + ')。p50/p99/p99.9/max、樣本數及配置 bytes 全部保留。')
