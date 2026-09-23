# C++ 線性搜尋：量測摘要

所有時間單位為 ns。表格為各輪分位數的中位數（輪間最小–最大），不是合併 p99。
六輪使用三種 backend 的所有排列；通常每操作 20,000 筆，大 Book 上限 4,000 筆，range／成員列舉上限 2,000 筆。各列實際樣本數保留於原始 CSV。
完整語意、配置布局與限制見 [實驗說明](cpp-linear.md)。這是 C++ 同語言比較，未移植完整 OMS。

## 單價查詢 p99

| 情境 | 指標 | B-tree＋Pool | Linear prices＋Pool | Linear orders |
|---|---|---:|---:|---:|
| prices_4 | summary_first | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_4 | summary_last | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_4 | summary_miss | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_4 | summary_random | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_8 | summary_first | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_8 | summary_last | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_8 | summary_miss | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_8 | summary_random | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_16 | summary_first | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_16 | summary_last | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_16 | summary_miss | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_16 | summary_random | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_32 | summary_first | 42 (42–42) | 42 (42–42) | 84 (84–84) |
| prices_32 | summary_last | 42 (42–42) | 42 (42–42) | 84 (84–84) |
| prices_32 | summary_miss | 42 (42–42) | 42 (42–42) | 84 (84–84) |
| prices_32 | summary_random | 42 (42–42) | 42 (42–42) | 84 (84–84) |
| prices_128 | summary_first | 42 (42–42) | 42 (42–42) | 167 (167–167) |
| prices_128 | summary_last | 42 (42–42) | 84 (84–84) | 167 (167–167) |
| prices_128 | summary_miss | 42 (42–42) | 84 (84–84) | 167 (167–167) |
| prices_128 | summary_random | 42 (42–42) | 83 (83–84) | 167 (167–167) |
| prices_1024 | summary_first | 42 (42–42) | 42 (42–42) | 1396 (1334–1834) |
| prices_1024 | summary_last | 42 (42–42) | 375 (375–375) | 1396 (1334–1834) |
| prices_1024 | summary_miss | 42 (42–42) | 334 (334–334) | 1417 (1334–1834) |
| prices_1024 | summary_random | 42 (42–42) | 292 (292–333) | 1417 (1375–1834) |
| prices_4096 | summary_first | 42 (42–42) | 42 (42–42) | 7395.5 (7334–7417) |
| prices_4096 | summary_last | 42 (42–42) | 1209 (1209–1209) | 7395.5 (7375–7417) |
| prices_4096 | summary_miss | 42 (42–42) | 1209 (1209–1209) | 7396 (7375–7417) |
| prices_4096 | summary_random | 42 (42–42) | 1167 (1166–1167) | 7396 (7375–7417) |

## 短操作的批次輔助量測

| 情境 | 指標 | B-tree＋Pool | Linear prices＋Pool | Linear orders |
|---|---|---:|---:|---:|
| prices_4 | batch64_summary | 10.74 (10.42–11.06) | 11.72 (9.77–11.72) | 14.33 (14.33–16.28) |
| prices_8 | batch64_summary | 11.72 (11.72–12.38) | 13.67 (13.03–13.67) | 18.89 (18.88–20.84) |
| prices_16 | batch64_summary | 15.62 (15.62–15.62) | 14.97 (12.38–14.97) | 27.34 (27.34–28.66) |
| prices_32 | batch64_summary | 17.58 (17.58–18.23) | 18.23 (14.33–18.23) | 47.53 (47.53–48.83) |

此表為「64 次查詢整批耗時的 p50 ÷ 64」，包含亂數與迴圈成本，僅輔助觀察平均成本；絕對不是單次查詢 p50 或 p99。

## 聚合、多 Book 及有序查詢 p99

| 情境 | 指標 | B-tree＋Pool | Linear prices＋Pool | Linear orders |
|---|---|---:|---:|---:|
| crowded_32 | summary_random | 42 (42–42) | 42 (42–42) | 42 (42–83) |
| crowded_4096 | summary_random | 42 (42–42) | 42 (42–42) | 6937.5 (6833–7084) |
| crowded_4096 | orders_at | 17188 (16875–19750) | 17000 (16875–17292) | 7771 (7000–8583) |
| 4096books_4near_2far | summary_random | 84 (84–125) | 83 (83–84) | 84 (84–125) |
| prices_4 | best | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_4 | sorted_range8 | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_32 | best | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prices_32 | sorted_range8 | 42 (42–42) | 209 (209–209) | 520.5 (500–541) |
| prices_4096 | best | 42 (42–42) | 42 (42–42) | 6334 (6333–6375) |
| prices_4096 | sorted_range8 | 42 (42–42) | 26875 (26792–28333) | 62708 (60750–63041) |

## 索引與權威訂單更新完成 p99

| 情境 | 指標 | B-tree＋Pool | Linear prices＋Pool | Linear orders |
|---|---|---:|---:|---:|
| updates_4 | index_fill_visible | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| updates_4 | index_replace_submit | 84 (84–84) | 42 (42–42) | 42 (42–42) |
| updates_4 | index_replace_ack_visible | 84 (84–84) | 84 (83–84) | 42 (42–42) |
| updates_4 | index_terminal_visible | 84 (84–84) | 42 (42–83) | 42 (42–42) |
| updates_32 | index_fill_visible | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| updates_32 | index_replace_submit | 125 (84–125) | 42 (42–83) | 42 (42–42) |
| updates_32 | index_replace_ack_visible | 125 (125–125) | 125 (125–125) | 42 (42–42) |
| updates_32 | index_terminal_visible | 125 (84–125) | 84 (84–84) | 42 (42–42) |
| updates_128 | index_fill_visible | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| updates_128 | index_replace_submit | 125 (125–125) | 125 (125–125) | 42 (42–42) |
| updates_128 | index_replace_ack_visible | 167 (167–167) | 292 (292–292) | 42 (42–42) |
| updates_128 | index_terminal_visible | 125 (125–125) | 250 (209–250) | 42 (42–42) |
| updates_4096 | index_fill_visible | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| updates_4096 | index_replace_submit | 167 (167–167) | 2417 (2417–2458) | 42 (42–42) |
| updates_4096 | index_replace_ack_visible | 208 (208–208) | 4583 (4542–6917) | 42 (42–42) |
| updates_4096 | index_terminal_visible | 166 (125–166) | 3417 (3416–5959) | 42 (42–42) |

## 回報更新後連續查詢的合計 p99

| 情境 | 指標 | B-tree＋Pool | Linear prices＋Pool | Linear orders |
|---|---|---:|---:|---:|
| mixed_4_q0 | fill_plus_queries | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| mixed_4_q1 | fill_plus_queries | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| mixed_4_q4 | fill_plus_queries | 125 (84–125) | 84 (84–84) | 84 (84–84) |
| mixed_4_q16 | fill_plus_queries | 292 (250–292) | 250 (250–250) | 250 (250–250) |
| mixed_32_q0 | fill_plus_queries | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| mixed_32_q1 | fill_plus_queries | 84 (84–84) | 42 (42–42) | 84 (84–84) |
| mixed_32_q4 | fill_plus_queries | 125 (125–166) | 125 (125–125) | 209 (209–209) |
| mixed_32_q16 | fill_plus_queries | 375 (375–417) | 334 (334–334) | 792 (792–833) |
| mixed_128_q0 | fill_plus_queries | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| mixed_128_q1 | fill_plus_queries | 84 (84–84) | 84 (84–84) | 167 (167–167) |
| mixed_128_q4 | fill_plus_queries | 166.5 (166–167) | 209 (209–209) | 645.5 (625–667) |
| mixed_128_q16 | fill_plus_queries | 437.5 (417–500) | 667 (667–667) | 2500 (2500–2542) |
| mixed_4096_q0 | fill_plus_queries | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| mixed_4096_q1 | fill_plus_queries | 125 (84–125) | 1209 (1209–1250) | 7229.5 (7167–7250) |
| mixed_4096_q4 | fill_plus_queries | 250 (250–250) | 4417 (4416–4458) | 29104 (28250–29875) |
| mixed_4096_q16 | fill_plus_queries | 708.5 (667–750) | 14479 (14334–35417) | 117916.5 (116792–119417) |

## 簡化流程 p99

| 情境 | 指標 | B-tree＋Pool | Linear prices＋Pool | Linear orders |
|---|---|---:|---:|---:|
| prototype_4 | intent_to_command | 84 (84–84) | 83 (42–83) | 42 (42–42) |
| prototype_4 | queued_intent | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prototype_4 | report_visible | 125 (125–125) | 84 (84–84) | 42 (42–42) |
| prototype_4 | ready_to_command | 125 (84–125) | 84 (84–84) | 42 (42–42) |
| prototype_32 | intent_to_command | 125 (84–125) | 84 (84–84) | 42 (42–42) |
| prototype_32 | queued_intent | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prototype_32 | report_visible | 167 (166–167) | 125 (125–125) | 42 (42–42) |
| prototype_32 | ready_to_command | 166 (125–167) | 84 (84–84) | 42 (42–42) |
| prototype_128 | intent_to_command | 125 (125–125) | 167 (167–167) | 42 (42–42) |
| prototype_128 | queued_intent | 42 (42–42) | 42 (42–42) | 42 (42–42) |
| prototype_128 | report_visible | 208 (167–208) | 292 (292–292) | 42 (42–42) |
| prototype_128 | ready_to_command | 167 (167–167) | 167 (167–167) | 42 (42–42) |

prototype_* 不含 WAL、完整風控、回報去重、歷史、恢復、Group、transport。不能把它當完整 OMS SLA。

## 成長路徑 p99

| 情境 | 指標 | B-tree＋Pool | Linear prices＋Pool | Linear orders |
|---|---|---:|---:|---:|
| growth | new_order_price | 854 (792–1708) | 16354.5 (16292–16583) | 292 (250–833) |
| growth | last_quarter_subset | 1042 (834–2167) | 17250 (16458–17916) | 646 (333–1041) |
| growth | every64_subset | 2145.5 (1500–6125) | 20291.5 (17583–22875) | 2208 (1375–7375) |

每 64 筆子集是三者相同序號，不代表精確捕捉所有配置事件。原始資料 first32_subset 只有 32 筆，不作穩定尾分位數推論。

## 部分尾部 p99.9

| 情境 | 指標 | B-tree＋Pool | Linear prices＋Pool | Linear orders |
|---|---|---:|---:|---:|
| prices_32 | summary_random | 42 (42–42) | 42 (42–42) | 84 (84–84) |
| prices_4096 | summary_random | 83 (42–84) | 1187.5 (1167–1250) | 10895.5 (8417–13541) |
| mixed_32_q4 | fill_plus_queries | 167 (167–208) | 125 (125–166) | 250 (250–291) |
| prototype_32 | report_visible | 208.5 (208–209) | 125 (125–166) | 42 (42–42) |
| growth | new_order_price | 1750 (1209–3542) | 18874.5 (17292–19958) | 1250 (1125–1458) |

## 量測區間內配置次數（獨立 allocation pass）

| 情境 | 指標 | B-tree＋Pool | Linear prices＋Pool | Linear orders |
|---|---|---:|---:|---:|
| prices_32 | summary_random | 0 (0–0) | 0 (0–0) | 0 (0–0) |
| prices_32 | sorted_range8 | 0 (0–0) | 0 (0–0) | 0 (0–0) |
| updates_32 | index_fill_visible | 0 (0–0) | 0 (0–0) | 0 (0–0) |
| updates_32 | index_replace_ack_visible | 0 (0–0) | 0 (0–0) | 0 (0–0) |
| mixed_32_q4 | fill_plus_queries | 0 (0–0) | 0 (0–0) | 0 (0–0) |
| prototype_32 | ready_to_command | 938 (938–938) | 0 (0–0) | 0 (0–0) |
| growth | new_order_price | 3670 (3670–3670) | 1005 (1005–1005) | 359 (359–359) |

計時器空操作基線 p99：42 ns。接近此數字的單次量測不作細微排名。

原始資料：
- [cpp-linear-time-2026-09-22.csv](results/cpp-linear-time-2026-09-22.csv)
- [cpp-linear-alloc-2026-09-22.csv](results/cpp-linear-alloc-2026-09-22.csv)
