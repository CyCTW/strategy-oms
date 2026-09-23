#!/usr/bin/env python3
"""Summarize the standalone C++ linear-search experiment, never language speed."""
import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path

root = Path(__file__).resolve().parents[1]
paths = [Path(x) for x in sys.argv[1:]] or [
    root / 'docs/results/cpp-linear-time-2026-09-22.csv',
    root / 'docs/results/cpp-linear-alloc-2026-09-22.csv',
]
rows = []
for path in paths:
    with path.open() as stream:
        rows.extend(csv.DictReader(stream))
groups = defaultdict(list)
for row in rows:
    groups[row['pass'], row['backend'], row['scenario'], row['metric']].append(row)
backends = ['absl_btree', 'linear_prices', 'linear_orders']

def number(v):
    return f'{v:.2f}'.rstrip('0').rstrip('.')

def cell(backend, scenario, metric, field='p99_ns', divisor=1, pass_name='time'):
    rs = groups[pass_name, backend, scenario, metric]
    if not rs:
        return '—'
    vals = [int(r[field]) / divisor for r in rs]
    return f'{number(statistics.median(vals))} ({number(min(vals))}–{number(max(vals))})'

def table(title, selections, field='p99_ns', divisor=1, pass_name='time'):
    print(f'\n## {title}\n')
    print('| 情境 | 指標 | B-tree＋Pool | Linear prices＋Pool | Linear orders |')
    print('|---|---|---:|---:|---:|')
    for scenario, metric in selections:
        values = [cell(b, scenario, metric, field, divisor, pass_name) for b in backends]
        print('| ' + ' | '.join([scenario, metric, *values]) + ' |')

print('# C++ 線性搜尋：量測摘要\n')
print('所有時間單位為 ns。表格為各輪分位數的中位數（輪間最小–最大），不是合併 p99。')
print('六輪使用三種 backend 的所有排列；通常每操作 20,000 筆，大 Book 上限 4,000 筆，range／成員列舉上限 2,000 筆。各列實際樣本數保留於原始 CSV。')
print('完整語意、配置布局與限制見 [實驗說明](cpp-linear.md)。這是 C++ 同語言比較，未移植完整 OMS。')
table('單價查詢 p99', [(f'prices_{n}', m) for n in [4, 8, 16, 32, 128, 1024, 4096]
                         for m in ['summary_first', 'summary_last', 'summary_miss', 'summary_random']])
table('短操作的批次輔助量測', [(f'prices_{n}', 'batch64_summary') for n in [4, 8, 16, 32]],
      field='p50_ns', divisor=64)
print('\n此表為「64 次查詢整批耗時的 p50 ÷ 64」，包含亂數與迴圈成本，僅輔助觀察平均成本；絕對不是單次查詢 p50 或 p99。')
table('聚合、多 Book 及有序查詢 p99', [
    ('crowded_32', 'summary_random'), ('crowded_4096', 'summary_random'),
    ('crowded_4096', 'orders_at'), ('4096books_4near_2far', 'summary_random'),
    *[(f'prices_{n}', m) for n in [4, 32, 4096] for m in ['best', 'sorted_range8']]])
table('索引與權威訂單更新完成 p99', [(f'updates_{n}', m) for n in [4, 32, 128, 4096]
    for m in ['index_fill_visible', 'index_replace_submit', 'index_replace_ack_visible', 'index_terminal_visible']])
table('回報更新後連續查詢的合計 p99', [(f'mixed_{n}_q{q}', 'fill_plus_queries')
    for n in [4, 32, 128, 4096] for q in [0, 1, 4, 16]])
table('簡化流程 p99', [(f'prototype_{n}', m) for n in [4, 32, 128]
    for m in ['intent_to_command', 'queued_intent', 'report_visible', 'ready_to_command']])
print('\nprototype_* 不含 WAL、完整風控、回報去重、歷史、恢復、Group、transport。不能把它當完整 OMS SLA。')
table('成長路徑 p99', [('growth', m) for m in ['new_order_price', 'last_quarter_subset', 'every64_subset']])
print('\n每 64 筆子集是三者相同序號，不代表精確捕捉所有配置事件。原始資料 first32_subset 只有 32 筆，不作穩定尾分位數推論。')
table('部分尾部 p99.9', [
    ('prices_32', 'summary_random'), ('prices_4096', 'summary_random'),
    ('mixed_32_q4', 'fill_plus_queries'), ('prototype_32', 'report_visible'),
    ('growth', 'new_order_price')], field='p999_ns')
table('量測區間內配置次數（獨立 allocation pass）', [
    ('prices_32', 'summary_random'), ('prices_32', 'sorted_range8'),
    ('updates_32', 'index_fill_visible'), ('updates_32', 'index_replace_ack_visible'),
    ('mixed_32_q4', 'fill_plus_queries'), ('prototype_32', 'ready_to_command'),
    ('growth', 'new_order_price')], field='allocations', pass_name='alloc')
timer = groups['time', 'timer', 'baseline', 'timer']
if timer:
    print(f"\n計時器空操作基線 p99：{timer[0]['p99_ns']} ns。接近此數字的單次量測不作細微排名。")
print('\n原始資料：')
for path in paths:
    print(f'- [{path.name}](results/{path.name})')
