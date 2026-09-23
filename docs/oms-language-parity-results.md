# Rust / C++ 同功能單張 OMS 量測

單位 ns；六輪各自分位數的中位數（輪間最小–最大），不是合併 p99。每輪每操作 20,000 筆；batch64 例外，每輪 2,000 批。完整方法、範圍與未對齊差異見 [說明](oms-language-parity.md)。

## standard 三段延遲 p99

| 情境 | 操作 | Rust | C++ |
|---|---|---:|---:|
| standard_single_4 | price_query | 42 (42–42) | 125 (125–125) |
| standard_single_4 | intent_to_command | 1333 (1292–1334) | 938 (917–1000) |
| standard_single_4 | guarded_report_visible | 1562.5 (1500–1584) | 1167 (1125–1250) |
| standard_single_4 | ready_to_command | 708 (667–708) | 312.5 (292–333) |
| standard_single_32 | price_query | 42 (42–42) | 125 (125–125) |
| standard_single_32 | intent_to_command | 687.5 (667–708) | 292 (292–292) |
| standard_single_32 | guarded_report_visible | 1208.5 (1208–1292) | 1041.5 (1000–1083) |
| standard_single_32 | ready_to_command | 667 (666–667) | 292 (292–292) |
| standard_single_128 | price_query | 83 (83–83) | 125 (125–125) |
| standard_single_128 | intent_to_command | 708 (708–709) | 333 (333–333) |
| standard_single_128 | guarded_report_visible | 562.5 (542–583) | 250 (250–292) |
| standard_single_128 | ready_to_command | 667 (667–667) | 333.5 (333–334) |
| standard_single_1024 | price_query | 84 (84–84) | 125 (125–125) |
| standard_single_1024 | intent_to_command | 750 (709–750) | 417 (417–417) |
| standard_single_1024 | guarded_report_visible | 584 (583–625) | 375 (375–375) |
| standard_single_1024 | ready_to_command | 708 (708–709) | 417 (417–417) |

## standard 意圖合併與去重 p99

| 情境 | 操作 | Rust | C++ |
|---|---|---:|---:|
| standard_single_4 | queued_intent | 771 (625–792) | 833 (792–875) |
| standard_single_4 | coalesced_intent | 958.5 (917–1000) | 812.5 (792–834) |
| standard_single_4 | report_transport_duplicate | 84 (84–84) | 125 (125–125) |
| standard_single_32 | queued_intent | 292 (292–292) | 125 (125–125) |
| standard_single_32 | coalesced_intent | 292 (292–292) | 84 (84–84) |
| standard_single_32 | report_transport_duplicate | 84 (84–84) | 125 (125–125) |
| standard_single_128 | queued_intent | 292 (292–292) | 125 (125–125) |
| standard_single_128 | coalesced_intent | 292 (292–292) | 84 (84–84) |
| standard_single_128 | report_transport_duplicate | 84 (84–84) | 125 (125–125) |
| standard_single_1024 | queued_intent | 292 (292–292) | 125 (125–167) |
| standard_single_1024 | coalesced_intent | 292 (292–292) | 125 (125–166) |
| standard_single_1024 | report_transport_duplicate | 84 (84–84) | 125 (125–125) |

## standard 成交及成交去重 p99

| 情境 | 操作 | Rust | C++ |
|---|---|---:|---:|
| standard_fills | guarded_fill | 917 (916–1000) | 708 (667–792) |
| standard_fills | fill_business_duplicate | 209 (209–209) | 166 (166–167) |
| standard_fills | fill_transport_duplicate | 84 (84–84) | 145.5 (125–167) |

## standard 關鍵路徑 p99.9

| 情境 | 操作 | Rust | C++ |
|---|---|---:|---:|
| standard_single_4 | intent_to_command | 1729.5 (1667–1750) | 1312 (1250–1542) |
| standard_single_4 | guarded_report_visible | 2541.5 (2292–2834) | 2125 (2083–2542) |
| standard_single_4 | ready_to_command | 1542 (1459–1542) | 1208.5 (1208–1333) |
| standard_single_32 | intent_to_command | 791.5 (750–833) | 375 (375–375) |
| standard_single_32 | guarded_report_visible | 1854 (1750–2333) | 1479 (1375–1583) |
| standard_single_32 | ready_to_command | 750 (750–834) | 375 (334–375) |
| standard_single_128 | intent_to_command | 792 (791–792) | 375 (375–375) |
| standard_single_128 | guarded_report_visible | 667 (666–708) | 729.5 (375–1000) |
| standard_single_128 | ready_to_command | 750 (750–792) | 375 (375–375) |
| standard_single_1024 | intent_to_command | 833 (792–834) | 1396 (1334–1584) |
| standard_single_1024 | guarded_report_visible | 792 (750–1167) | 1333 (1250–1500) |
| standard_single_1024 | ready_to_command | 792 (791–833) | 1396 (1292–1583) |

## pages 三段延遲 p99

| 情境 | 操作 | Rust | C++ |
|---|---|---:|---:|
| pages_single_4 | price_query | 42 (42–42) | 125 (125–125) |
| pages_single_4 | intent_to_command | 1312.5 (1291–1375) | 958 (917–1042) |
| pages_single_4 | guarded_report_visible | 1541.5 (1416–1625) | 1187.5 (1166–1333) |
| pages_single_4 | ready_to_command | 708 (667–708) | 292 (292–333) |
| pages_single_32 | price_query | 42 (42–42) | 125 (125–125) |
| pages_single_32 | intent_to_command | 667 (666–667) | 292 (292–416) |
| pages_single_32 | guarded_report_visible | 1187.5 (1167–1250) | 1020.5 (1000–1042) |
| pages_single_32 | ready_to_command | 625 (625–666) | 250 (250–292) |
| pages_single_128 | price_query | 42 (42–42) | 125 (125–125) |
| pages_single_128 | intent_to_command | 667 (667–708) | 312.5 (292–333) |
| pages_single_128 | guarded_report_visible | 479.5 (459–500) | 250 (209–708) |
| pages_single_128 | ready_to_command | 645.5 (625–666) | 250 (250–291) |
| pages_single_1024 | price_query | 83 (83–83) | 125 (125–125) |
| pages_single_1024 | intent_to_command | 708 (667–750) | 375 (334–375) |
| pages_single_1024 | guarded_report_visible | 500 (500–834) | 292 (292–292) |
| pages_single_1024 | ready_to_command | 667 (666–667) | 292 (292–333) |

## pages 意圖合併與去重 p99

| 情境 | 操作 | Rust | C++ |
|---|---|---:|---:|
| pages_single_4 | queued_intent | 792 (417–833) | 812.5 (791–917) |
| pages_single_4 | coalesced_intent | 958 (958–1000) | 812.5 (750–875) |
| pages_single_4 | report_transport_duplicate | 84 (84–84) | 125 (125–125) |
| pages_single_32 | queued_intent | 292 (292–292) | 125 (125–209) |
| pages_single_32 | coalesced_intent | 292 (292–292) | 84 (84–209) |
| pages_single_32 | report_transport_duplicate | 84 (84–84) | 125 (125–125) |
| pages_single_128 | queued_intent | 292 (292–292) | 125 (125–166) |
| pages_single_128 | coalesced_intent | 292 (292–292) | 84 (84–84) |
| pages_single_128 | report_transport_duplicate | 84 (84–84) | 125 (125–125) |
| pages_single_1024 | queued_intent | 292 (292–292) | 166.5 (125–167) |
| pages_single_1024 | coalesced_intent | 292 (292–292) | 125 (125–166) |
| pages_single_1024 | report_transport_duplicate | 84 (84–84) | 125 (125–125) |

## pages 成交及成交去重 p99

| 情境 | 操作 | Rust | C++ |
|---|---|---:|---:|
| pages_fills | guarded_fill | 1000 (917–1042) | 708.5 (667–750) |
| pages_fills | fill_business_duplicate | 209 (209–250) | 166.5 (125–167) |
| pages_fills | fill_transport_duplicate | 84 (84–84) | 166 (125–166) |

## pages 關鍵路徑 p99.9

| 情境 | 操作 | Rust | C++ |
|---|---|---:|---:|
| pages_single_4 | intent_to_command | 1771 (1583–2041) | 1333.5 (1291–1417) |
| pages_single_4 | guarded_report_visible | 2624.5 (2375–2709) | 2208.5 (2000–2333) |
| pages_single_4 | ready_to_command | 1625 (1500–1750) | 1229.5 (1208–1333) |
| pages_single_32 | intent_to_command | 750 (709–1417) | 375 (334–459) |
| pages_single_32 | guarded_report_visible | 1895.5 (1625–2042) | 1437.5 (1334–1625) |
| pages_single_32 | ready_to_command | 750 (667–1375) | 333.5 (333–375) |
| pages_single_128 | intent_to_command | 791.5 (750–1458) | 375 (334–416) |
| pages_single_128 | guarded_report_visible | 583 (542–1167) | 500.5 (333–1083) |
| pages_single_128 | ready_to_command | 750 (709–1417) | 333 (333–334) |
| pages_single_1024 | intent_to_command | 792 (750–875) | 1312.5 (1250–1417) |
| pages_single_1024 | guarded_report_visible | 729 (583–1291) | 1208 (1166–1333) |
| pages_single_1024 | ready_to_command | 750 (750–791) | 1312.5 (1208–1458) |

## 單價查詢批次輔助量測

| 情境 | 操作 | Rust | C++ |
|---|---|---:|---:|
| standard_single_4 | price_query_batch64 | 5.2 (5.2–5.22) | 3.91 (3.91–3.91) |
| standard_single_32 | price_query_batch64 | 7.81 (7.81–7.81) | 7.16 (6.52–7.16) |
| standard_single_128 | price_query_batch64 | 9.77 (9.77–9.77) | 8.47 (8.47–8.47) |
| standard_single_1024 | price_query_batch64 | 9.77 (9.77–9.77) | 9.11 (9.11–9.11) |
| pages_single_4 | price_query_batch64 | 5.2 (5.2–5.2) | 5.2 (5.2–5.2) |
| pages_single_32 | price_query_batch64 | 5.86 (5.86–5.86) | 5.2 (5.2–5.2) |
| pages_single_128 | price_query_batch64 | 7.16 (7.16–7.16) | 5.86 (5.86–6.5) |
| pages_single_1024 | price_query_batch64 | 7.16 (6.52–7.16) | 5.86 (5.86–6.5) |

此表為每批 64 次 query 的 p50 / 64，含迴圈，只有平均成本參考用途；不是單次 query 的 p50 或 p99。預先產生的 targets 含 1/8 miss，其餘命中現有價格，兩邊一致。

## 計時器基線

| 情境 | 操作 | Rust | C++ |
|---|---|---:|---:|
| baseline | timer | 42 (42–42) | 42 (42–42) |

時間 pass 沒有配置計數，CSV 中 allocations / allocated_bytes 的 0 只是共用格式的佔位，不能當零配置證据。全部原始 p50／p99／p99.9／max 與樣本數保留於 CSV。

原始資料：[oms-parity-time-2026-09-22.csv](results/oms-parity-time-2026-09-22.csv)
