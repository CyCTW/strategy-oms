# 價格索引比較：量測摘要

此表列出各輪 p99 的中位數（括號為各輪 p99 最小～最大值），單位 ns；不是合併所有樣本後的 p99。

| 情境／操作 | v0.3 舊索引 | 共用 Pool＋標準 B-tree | 共用 Pool＋AVL 候選 |
|---|---:|---:|---:|
| small / price_hit | 84 (84–166) | 84 (83–84) | 84 (84–84) |
| small / handle_hit | — | 42 (42–42) | 42 (42–42) |
| small / index_update | 167 (125–334) | 42 (42–42) | 42 (42–42) |
| crowded / index_update | 250 (250–292) | 42 (42–125) | 42 (42–125) |
| many_prices / price_hit | 125 (125–125) | 125 (125–167) | 209 (209–292) |
| many_prices / handle_hit | — | 84 (84–84) | 125 (125–125) |
| many_prices / range_first8 | 84 (84–84) | 84 (84–84) | 209 (208–209) |
| many_prices / index_update | 667 (666–708) | 42 (42–42) | 42 (42–42) |
| many_books / price_hit | 84 (84–84) | 125 (125–125) | 125 (125–125) |
| many_books / handle_hit | — | 42 (42–42) | 42 (42–42) |
| churn / index_update | 375 (375–416) | 334 (333–334) | 584 (584–584) |
| growth / index_update | 250 (250–1083) | 417 (417–1334) | 1167 (1125–1375) |
| growth / block_growth_subset | — | 1083 (916–5583) | 2458 (2167–5541) |
| engine / intent_immediate | — | 709 (708–1417) | 709 (709–750) |
| engine / intent_queued | — | 333 (333–1084) | 333 (333–334) |
| engine / ready_to_command | — | 709 (708–750) | 709 (709–750) |
| engine / replace_report | — | 583 (542–1584) | 584 (542–584) |
| engine_growth / new_to_command | — | 1500 (1334–2500) | 2250 (2208–2958) |
| burst64_period100000ns / fill_service | — | 542 (500–1166) | 541 (500–708) |
| burst64_period100000ns / scheduled_arrival_to_visible | — | 30500 (29875–119125) | 32709 (27541–52709) |

## 配置次數

另一次獨立的配置計數 pass，計數器只在被測函式內啟用；表內未扣除第一筆配置，不把擴容排除。初始化及建立測試輸入的配置不在此表。realloc 計為一次配置及一次釋放。

| 情境／操作 | v0.3 舊索引 | 共用 Pool＋標準 B-tree | 共用 Pool＋AVL 候選 |
|---|---:|---:|---:|
| small / index_update | 20000 | 0 | 0 |
| crowded / index_update | 1140 | 0 | 0 |
| many_prices / index_update | 22280 | 0 | 0 |
| churn / index_update | 25696 | 5696 | 0 |
| growth / index_update | 26665 | 7309 | 966 |
| engine / intent_queued | — | 1 | 1 |
| engine_growth / new_to_command | — | 4326 | 1315 |

原始資料：[CSV](results/index-comparison-2026-09-20.csv)。所有 p50／p99.9／最大值與釋放次數均保留在 CSV。
