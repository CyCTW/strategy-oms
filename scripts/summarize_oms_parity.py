#!/usr/bin/env python3
import csv,statistics,sys
from collections import defaultdict
from pathlib import Path
root=Path(__file__).resolve().parents[1]
p=Path(sys.argv[1]) if len(sys.argv)>1 else root/'docs/results/oms-parity-time-2026-09-22.csv'
rows=list(csv.DictReader(p.open()));groups=defaultdict(list)
for r in rows:groups[r['backend'],r['scenario'],r['metric']].append(r)
def val(lang,scenario,metric,field='p99_ns',div=1):
    vals=[int(r[field])/div for r in groups[lang,scenario,metric]]
    if not vals:return '—'
    f=lambda v:f'{v:.2f}'.rstrip('0').rstrip('.')
    return f'{f(statistics.median(vals))} ({f(min(vals))}–{f(max(vals))})'
def table(title,scenarios,metrics,field='p99_ns',div=1):
    print(f'\n## {title}\n\n| 情境 | 操作 | Rust | C++ |\n|---|---|---:|---:|')
    for s in scenarios:
        for m in metrics:print(f'| {s} | {m} | {val("rust",s,m,field,div)} | {val("cpp",s,m,field,div)} |')
print('# Rust / C++ 同功能單張 OMS 量測\n')
print('單位 ns；六輪各自分位數的中位數（輪間最小–最大），不是合併 p99。每輪每操作 20,000 筆；batch64 例外，每輪 2,000 批。完整方法、範圍與未對齊差異見 [說明](oms-language-parity.md)。')
for backend in ['standard','pages']:
    table(f'{backend} 三段延遲 p99',[f'{backend}_single_{n}' for n in [4,32,128,1024]],['price_query','intent_to_command','guarded_report_visible','ready_to_command'])
    table(f'{backend} 意圖合併與去重 p99',[f'{backend}_single_{n}' for n in [4,32,128,1024]],['queued_intent','coalesced_intent','report_transport_duplicate'])
    table(f'{backend} 成交及成交去重 p99',[f'{backend}_fills'],['guarded_fill','fill_business_duplicate','fill_transport_duplicate'])
    table(f'{backend} 關鍵路徑 p99.9',[f'{backend}_single_{n}' for n in [4,32,128,1024]],['intent_to_command','guarded_report_visible','ready_to_command'],field='p999_ns')
table('單價查詢批次輔助量測',[f'{b}_single_{n}' for b in ['standard','pages'] for n in [4,32,128,1024]],['price_query_batch64'],field='p50_ns',div=64)
print('\n此表為每批 64 次 query 的 p50 / 64，含迴圈，只有平均成本參考用途；不是單次 query 的 p50 或 p99。預先產生的 targets 含 1/8 miss，其餘命中現有價格，兩邊一致。')
table('計時器基線',['baseline'],['timer'])
print('\n時間 pass 沒有配置計數，CSV 中 allocations / allocated_bytes 的 0 只是共用格式的佔位，不能當零配置證据。全部原始 p50／p99／p99.9／max 與樣本數保留於 CSV。')
print(f'\n原始資料：[{p.name}](results/{p.name})')
