# 價格索引比較：量測摘要

此表列出各輪 p99 的中位數（括號為各輪 p99 最小～最大值），單位 ns；不是合併所有樣本後的 p99。

| 情境／操作 | v0.3 舊索引 | 共用 Pool＋標準 B-tree | 共用 Pool＋AVL 候選 |
|---|---:|---:|---:|
| small / price_hit | 84 (84–125) | 84 (84–84) | 84 (84–84) |
| small / handle_hit | — | 42 (42–42) | 42 (42–42) |
| small / index_update | 125 (125–334) | 42 (42–42) | 42 (42–42) |
| crowded / index_update | 250 (209–250) | 42 (42–42) | 42 (42–42) |
| many_prices / price_hit | 125 (125–125) | 125 (125–125) | 209 (208–209) |
| many_prices / handle_hit | — | 83 (83–83) | 125 (84–125) |
| many_prices / range_first8 | 83 (83–84) | 84 (84–84) | 167 (167–167) |
| many_prices / index_update | 584 (584–584) | 42 (42–42) | 42 (42–42) |
| many_books / price_hit | 84 (84–84) | 125 (84–125) | 125 (125–125) |
| many_books / handle_hit | — | 42 (42–42) | 42 (42–42) |
| churn / index_update | 334 (334–334) | 292 (292–292) | 542 (542–542) |
| growth / index_update | 250 (209–958) | 375 (375–1000) | 1000 (959–1042) |
| growth / block_growth_subset | — | 1000 (958–2666) | 3000 (2667–3041) |
| engine / intent_immediate | — | 667 (625–1083) | 625 (625–708) |
| engine / intent_queued | — | 292 (292–875) | 292 (292–292) |
| engine / ready_to_command | — | 667 (667–708) | 708 (708–709) |
| engine / replace_report | — | 458 (458–1375) | 458 (458–500) |
| engine_growth / new_to_command | — | 1375 (1292–2208) | 1917 (1916–2584) |
| burst64_period100000ns / fill_service | — | 416 (416–1000) | 416 (416–416) |
| burst64_period100000ns / scheduled_arrival_to_visible | — | 23166 (23084–29750) | 23166 (23083–23542) |

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

原始資料：[CSV](results/btree-default-2026-09-23.csv)。所有 p50／p99.9／最大值與釋放次數均保留在 CSV。
