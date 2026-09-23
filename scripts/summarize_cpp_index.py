#!/usr/bin/env python3
"""Keep C++ index results distinct from the reduced prototype flow and Rust OMS."""
import csv
import statistics
import sys
from pathlib import Path

sources = [Path(p) for p in sys.argv[1:]] or [
    Path('docs/results/cpp-index-time-2026-09-22.csv'),
    Path('docs/results/cpp-index-alloc-2026-09-22.csv'),
]
rows = [r for p in sources for r in csv.DictReader(p.open())]
backends = ['absl_btree','pool_pages']
timing = [r for r in rows if r['pass']=='time' and r['backend']!='timer']
rounds = len({r['round'] for r in timing})
samples = next(r['n'] for r in timing if r['scenario']=='rolling_4' and r['metric']=='price_query')

def cell(s,m,b,field):
    vals=[int(r[field]) for r in rows if r['pass']=='time' and r['scenario']==s and r['metric']==m and r['backend']==b]
    return f'{statistics.median(vals):g} ({min(vals)}–{max(vals)})' if vals else '—'

index_cases=[('rolling_4','price_query'),('rolling_4','index_ack'),('rolling_32','price_query'),
    ('rolling_32','near_range8'),('rolling_32','index_ack'),('outliers_32','price_query'),('outliers_32','far_query'),
    ('outliers_32','index_ack'),('boundary_32','cross_page_submit_subset'),('jump_32','jump_submit_subset'),
    ('4096books_4near_2far','price_query'),('new_book_growth','index_insert'),('new_book_growth','every64_subset'),
    ('new_book_growth','page_block_growth_subset')]
index_cases.extend(('cancel_rehang_4near',m) for m in ['index_cancel','index_new','index_accepted','index_canceled'])
flow_cases=[(s,m) for s in ['prototype_rolling_4_2far','prototype_rolling_32_2far','prototype_jump_32_2far']
    for m in ['intent_to_command','intent_queued','replace_report','ready_to_command']]

print('# C++ 價格索引比較\n')
print(f'C++20 / Abseil 20250127.1，Release。{rounds} 輪、一般操作每輪 {samples} 筆。實際編譯器、旗標與平台見同批 metadata JSON。\n')
print('單位 ns，每格為各輪分位數的中位數（最小–最大），不是合併分位數。time 與 alloc 使用不同執行檔；計時檔不覆寫 allocator。\n')
for cases,title in [(index_cases,'完整價格索引'),(flow_cases,'簡化流程原型（不等同完整 OMS）')]:
    print('## ' + title + '\n')
    if cases is flow_cases:
        print('有 latest intent、單筆 pending、版本／回報序號檢查及派送；沒有 WAL、回報去重、完整風控、恢復與 Group。不能直接與 Rust Engine 比速度。\n')
    for field,label in [('p99_ns','p99'),('p999_ns','p99.9')]:
        print('### '+label+'\n')
        print('| 情境／操作 | Abseil B-tree＋Pool | Pool 稀疏分頁 |\n|---|---:|---:|')
        for s,m in cases: print('| '+s+' / '+m+' | '+' | '.join(cell(s,m,b,field) for b in backends)+' |')
        print()
print('## 受測操作內配置次數\n')
print('獨立 executable 覆寫 new/delete，統計對應 C++ allocator 的 requested bytes；未計入初始建構。\n')
print('| 情境／操作 | Abseil B-tree＋Pool | Pool 稀疏分頁 |\n|---|---:|---:|')
for s,m in [('rolling_4','index_ack'),('rolling_32','index_submit'),('rolling_32','index_ack'),('outliers_32','index_ack'),
    ('prototype_rolling_32_2far','ready_to_command'),('new_book_growth','index_insert')]:
    values=[next(r['allocations'] for r in rows if r['pass']=='alloc' and r['backend']==b and r['scenario']==s and r['metric']==m) for b in backends]
    print('| '+s+' / '+m+' | '+' | '.join(values)+' |')
print('\n## Index 存活記憶體（requested bytes）\n')
print('含 inline Index、Book/Level/Member/Page pools 與目錄；不含外部 Order/Memberships、allocator metadata、完整流程狀態；不是 RSS。\n')
print('| 情境 | Abseil B-tree＋Pool | Pool 稀疏分頁 | 活頁／頁塊 |\n|---|---:|---:|---:|')
for s in dict.fromkeys(r['scenario'] for r in rows if r['pass']=='memory'):
    values=[next(r['allocated_bytes'] for r in rows if r['pass']=='memory' and r['backend']==b and r['scenario']==s) for b in backends]
    cap=next(r for r in rows if r['pass']=='capacity' and r['backend']=='pool_pages' and r['scenario']==s)
    print('| '+s+' | '+' | '.join(values)+' | '+cap['allocations']+' / '+cap['allocated_bytes']+' |')
print('\n計時器基準 p99：'+cell('baseline','timer','timer','p99_ns')+' ns。0/41/42 ns 等量化值附近不能可靠排序或推算倍數。')
print('\n原始資料：'+', '.join('[{}](results/{})'.format(p.name,p.name) for p in sources)+'。保留 p50/p99/p99.9/max 與 subset 樣本數。')
