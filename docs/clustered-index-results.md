# 移動近價群＋遠價訂單：量測摘要

各格為 5 輪分位數的中位數（輪間最小–最大），單位 ns；不是合併樣本的分位數。一般情境每輪 20000 筆，subset 只取符合條件的操作，樣本數見 CSV。

index_* 是完整價格索引更新，不含 reducer/WAL；engine_* 使用完整 Engine＋MemoryJournal。所有查詢使用 BookHandle。

## p99

| 情境／操作 | B-tree＋Pool | Pool 稀疏分頁 |
|---|---:|---:|
| rolling_4 / price_query | 42 (42–42) | 42 (42–42) |
| rolling_4 / index_ack | 84 (84–84) | 83 (83–83) |
| rolling_32 / price_query | 42 (42–42) | 42 (42–42) |
| rolling_32 / near_range8 | 42 (42–42) | 42 (42–42) |
| rolling_32 / index_ack | 84 (84–84) | 42 (42–42) |
| outliers_32 / price_query | 42 (42–42) | 42 (42–42) |
| outliers_32 / far_query | 42 (42–42) | 42 (42–42) |
| outliers_32 / index_ack | 84 (84–84) | 42 (42–42) |
| boundary_32 / cross_page_submit_subset | 84 (84–84) | 84 (84–84) |
| jump_32 / jump_submit_subset | 125 (125–125) | 84 (84–84) |
| 4096books_4near_2far / price_query | 167 (166–209) | 250 (250–333) |
| new_book_growth / index_insert | 416 (375–1000) | 1250 (1208–1333) |
| new_book_growth / page_block_growth_subset | — | 19125 (17834–32791) |
| engine_rolling_4_2far / price_query | 42 (42–42) | 42 (42–42) |
| engine_rolling_4_2far / intent_to_command | 625 (625–667) | 625 (625–625) |
| engine_rolling_4_2far / intent_queued | 292 (292–333) | 291 (291–292) |
| engine_rolling_4_2far / replace_report | 375 (375–500) | 375 (375–375) |
| engine_rolling_4_2far / ready_to_command | 667 (667–708) | 667 (625–667) |
| engine_rolling_32_2far / price_query | 42 (42–42) | 42 (42–42) |
| engine_rolling_32_2far / intent_to_command | 625 (625–666) | 625 (625–625) |
| engine_rolling_32_2far / intent_queued | 291 (291–292) | 291 (250–292) |
| engine_rolling_32_2far / replace_report | 417 (416–417) | 375 (375–375) |
| engine_rolling_32_2far / ready_to_command | 708 (667–708) | 667 (625–667) |
| engine_jump_32_2far / price_query | 42 (42–42) | 42 (42–42) |
| engine_jump_32_2far / intent_to_command | 625 (625–666) | 625 (625–625) |
| engine_jump_32_2far / intent_queued | 291 (291–292) | 292 (250–292) |
| engine_jump_32_2far / replace_report | 417 (417–417) | 375 (375–375) |
| engine_jump_32_2far / ready_to_command | 708 (667–708) | 667 (625–667) |
| engine_cancel_rehang_4near / cancel_to_command | 833 (792–1291) | 792 (792–833) |
| engine_cancel_rehang_4near / new_to_command | 875 (875–1625) | 875 (875–917) |
| engine_cancel_rehang_4near / accepted_report | 417 (417–1208) | 417 (417–417) |
| engine_cancel_rehang_4near / canceled_report | 417 (417–1208) | 417 (417–417) |

## p99.9

| 情境／操作 | B-tree＋Pool | Pool 稀疏分頁 |
|---|---:|---:|
| rolling_4 / price_query | 42 (42–42) | 42 (42–42) |
| rolling_4 / index_ack | 84 (84–84) | 84 (84–84) |
| rolling_32 / price_query | 42 (42–42) | 42 (42–42) |
| rolling_32 / near_range8 | 42 (42–83) | 42 (42–42) |
| rolling_32 / index_ack | 167 (167–167) | 83 (83–84) |
| outliers_32 / price_query | 42 (42–42) | 42 (42–42) |
| outliers_32 / far_query | 42 (42–42) | 42 (42–42) |
| outliers_32 / index_ack | 167 (125–167) | 84 (84–84) |
| boundary_32 / cross_page_submit_subset | 84 (84–125) | 84 (84–84) |
| jump_32 / jump_submit_subset | 166 (125–167) | 84 (84–84) |
| 4096books_4near_2far / price_query | 250 (250–333) | 375 (334–875) |
| new_book_growth / index_insert | 917 (875–2584) | 4125 (2208–5375) |
| new_book_growth / page_block_growth_subset | — | 137917 (136750–141083) |
| engine_rolling_4_2far / price_query | 42 (42–42) | 42 (42–42) |
| engine_rolling_4_2far / intent_to_command | 709 (709–1625) | 708 (708–1250) |
| engine_rolling_4_2far / intent_queued | 334 (334–1167) | 333 (333–875) |
| engine_rolling_4_2far / replace_report | 458 (458–1333) | 458 (458–959) |
| engine_rolling_4_2far / ready_to_command | 750 (750–1625) | 750 (750–1292) |
| engine_rolling_32_2far / price_query | 84 (84–84) | 42 (42–42) |
| engine_rolling_32_2far / intent_to_command | 750 (709–833) | 708 (708–709) |
| engine_rolling_32_2far / intent_queued | 333 (333–375) | 333 (292–334) |
| engine_rolling_32_2far / replace_report | 500 (459–542) | 417 (417–458) |
| engine_rolling_32_2far / ready_to_command | 792 (750–875) | 750 (709–750) |
| engine_jump_32_2far / price_query | 84 (84–84) | 42 (42–42) |
| engine_jump_32_2far / intent_to_command | 750 (708–791) | 708 (708–750) |
| engine_jump_32_2far / intent_queued | 334 (333–417) | 333 (333–375) |
| engine_jump_32_2far / replace_report | 500 (500–541) | 417 (417–417) |
| engine_jump_32_2far / ready_to_command | 792 (750–833) | 750 (709–792) |
| engine_cancel_rehang_4near / cancel_to_command | 1000 (959–2083) | 1000 (959–1334) |
| engine_cancel_rehang_4near / new_to_command | 1250 (1250–2583) | 1375 (1250–1833) |
| engine_cancel_rehang_4near / accepted_report | 500 (500–2167) | 500 (459–1041) |
| engine_cancel_rehang_4near / canceled_report | 500 (459–2125) | 500 (500–959) |

## 獨立配置計數 pass

計入受測操作的 alloc/realloc 次數，未排除首次等待集合配置或擴容；不含計時外初始建構。

| 情境／操作 | B-tree＋Pool | Pool 稀疏分頁 |
|---|---:|---:|
| rolling_4 / index_ack | 0 | 0 |
| rolling_32 / index_submit | 90 | 0 |
| rolling_32 / index_ack | 90 | 0 |
| outliers_32 / index_submit | 90 | 1 |
| outliers_32 / index_ack | 89 | 0 |
| engine_rolling_32_2far / ready_to_command | 357 | 0 |
| engine_cancel_rehang_4near / new_to_command | 331 | 331 |
| new_book_growth / index_insert | 40977 | 41298 |

## 索引記憶體（bytes）

含 Book/Level/Member/Page pools、目錄及 inline Index。是仍存活的 allocator requested bytes，非 RSS；不含 OrderStore、每張訂單的 Memberships、WAL 或完整 Engine。

| 情境 | B-tree＋Pool | Pool 稀疏分頁 | 活頁／已配置頁塊 |
|---|---:|---:|---:|
| 1books_4near_0far | 21116 | 89788 | 2 / 1 |
| 1books_4near_2far | 21116 | 89788 | 4 / 1 |
| 1books_32near_0far | 22844 | 89788 | 2 / 1 |
| 1books_32near_2far | 23228 | 89788 | 4 / 1 |
| 1books_128near_0far | 42172 | 102588 | 3 / 1 |
| 1books_128near_2far | 54972 | 115388 | 5 / 1 |
| 128books_4near_0far | 183160 | 457656 | 256 / 4 |
| 128books_4near_2far | 234616 | 783608 | 512 / 8 |
| 128books_32near_0far | 1122936 | 1176248 | 256 / 4 |
| 128books_32near_2far | 1225336 | 1503992 | 512 / 8 |
| 128books_128near_0far | 4422264 | 3777272 | 384 / 6 |
| 128books_128near_2far | 4481656 | 4111224 | 640 / 10 |

每個頁塊容納 64 頁，每頁覆蓋 64 個原始價格單位；這是兩個不同粒度。空頁可重用，頁塊保留至 Index 銷毀。

計時器基準 p99：42 (42–42) ns，附近差異無法可靠排序。

原始資料：[CSV](results/clustered-index-2026-09-21.csv)。p50/p99/p99.9/max、樣本數及配置 bytes 全部保留。
