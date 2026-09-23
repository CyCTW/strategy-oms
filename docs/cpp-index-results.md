# C++ 價格索引比較

C++20 / Abseil 20250127.1，Release。5 輪、一般操作每輪 20000 筆。實際編譯器、旗標與平台見同批 metadata JSON。

單位 ns，每格為各輪分位數的中位數（最小–最大），不是合併分位數。time 與 alloc 使用不同執行檔；計時檔不覆寫 allocator。

## 完整價格索引

### p99

| 情境／操作 | Abseil B-tree＋Pool | Pool 稀疏分頁 |
|---|---:|---:|
| rolling_4 / price_query | 42 (42–42) | 42 (42–42) |
| rolling_4 / index_ack | 84 (84–125) | 42 (42–83) |
| rolling_32 / price_query | 42 (42–42) | 42 (42–42) |
| rolling_32 / near_range8 | 42 (42–42) | 42 (42–42) |
| rolling_32 / index_ack | 84 (84–84) | 42 (42–42) |
| outliers_32 / price_query | 42 (42–42) | 42 (42–42) |
| outliers_32 / far_query | 42 (42–42) | 42 (42–42) |
| outliers_32 / index_ack | 84 (84–84) | 42 (42–83) |
| boundary_32 / cross_page_submit_subset | 83 (83–84) | 42 (42–42) |
| jump_32 / jump_submit_subset | 125 (84–125) | 125 (84–125) |
| 4096books_4near_2far / price_query | 42 (42–167) | 250 (250–500) |
| new_book_growth / index_insert | 584 (542–1083) | 2500 (2416–2958) |
| new_book_growth / every64_subset | 1250 (958–2667) | 3708 (3500–9250) |
| new_book_growth / page_block_growth_subset | — | 3708 (3500–9250) |
| cancel_rehang_4near / index_cancel | 42 (42–42) | 42 (42–42) |
| cancel_rehang_4near / index_new | 84 (84–84) | 125 (125–125) |
| cancel_rehang_4near / index_accepted | 42 (42–42) | 42 (42–42) |
| cancel_rehang_4near / index_canceled | 84 (84–84) | 84 (84–84) |

### p99.9

| 情境／操作 | Abseil B-tree＋Pool | Pool 稀疏分頁 |
|---|---:|---:|
| rolling_4 / price_query | 42 (42–84) | 42 (42–42) |
| rolling_4 / index_ack | 84 (84–167) | 84 (84–84) |
| rolling_32 / price_query | 42 (42–84) | 42 (42–83) |
| rolling_32 / near_range8 | 83 (42–84) | 42 (42–84) |
| rolling_32 / index_ack | 125 (125–167) | 84 (83–84) |
| outliers_32 / price_query | 42 (42–84) | 42 (42–42) |
| outliers_32 / far_query | 42 (42–83) | 42 (42–83) |
| outliers_32 / index_ack | 125 (125–125) | 84 (84–84) |
| boundary_32 / cross_page_submit_subset | 84 (84–84) | 83 (42–125) |
| jump_32 / jump_submit_subset | 166 (125–209) | 125 (125–167) |
| 4096books_4near_2far / price_query | 125 (84–334) | 416 (375–1042) |
| new_book_growth / index_insert | 1041 (833–2125) | 3334 (3291–7500) |
| new_book_growth / every64_subset | 1542 (1125–3083) | 3875 (3542–19000) |
| new_book_growth / page_block_growth_subset | — | 3875 (3542–19000) |
| cancel_rehang_4near / index_cancel | 42 (42–42) | 42 (42–83) |
| cancel_rehang_4near / index_new | 125 (84–125) | 125 (125–125) |
| cancel_rehang_4near / index_accepted | 42 (42–83) | 42 (42–84) |
| cancel_rehang_4near / index_canceled | 84 (84–125) | 84 (84–125) |

## 簡化流程原型（不等同完整 OMS）

有 latest intent、單筆 pending、版本／回報序號檢查及派送；沒有 WAL、回報去重、完整風控、恢復與 Group。不能直接與 Rust Engine 比速度。

### p99

| 情境／操作 | Abseil B-tree＋Pool | Pool 稀疏分頁 |
|---|---:|---:|
| prototype_rolling_4_2far / intent_to_command | 42 (42–42) | 84 (84–84) |
| prototype_rolling_4_2far / intent_queued | 42 (42–42) | 42 (42–42) |
| prototype_rolling_4_2far / replace_report | 84 (84–125) | 84 (84–125) |
| prototype_rolling_4_2far / ready_to_command | 84 (84–125) | 125 (125–125) |
| prototype_rolling_32_2far / intent_to_command | 84 (84–84) | 83 (83–83) |
| prototype_rolling_32_2far / intent_queued | 42 (42–42) | 42 (42–42) |
| prototype_rolling_32_2far / replace_report | 167 (166–167) | 84 (84–84) |
| prototype_rolling_32_2far / ready_to_command | 125 (125–125) | 84 (84–125) |
| prototype_jump_32_2far / intent_to_command | 84 (84–84) | 83 (83–84) |
| prototype_jump_32_2far / intent_queued | 42 (42–42) | 42 (42–42) |
| prototype_jump_32_2far / replace_report | 167 (167–167) | 84 (84–84) |
| prototype_jump_32_2far / ready_to_command | 125 (125–125) | 84 (84–125) |

### p99.9

| 情境／操作 | Abseil B-tree＋Pool | Pool 稀疏分頁 |
|---|---:|---:|
| prototype_rolling_4_2far / intent_to_command | 125 (84–125) | 84 (84–125) |
| prototype_rolling_4_2far / intent_queued | 42 (42–42) | 42 (42–83) |
| prototype_rolling_4_2far / replace_report | 167 (125–167) | 125 (125–125) |
| prototype_rolling_4_2far / ready_to_command | 167 (125–167) | 167 (167–167) |
| prototype_rolling_32_2far / intent_to_command | 84 (84–125) | 84 (84–84) |
| prototype_rolling_32_2far / intent_queued | 42 (42–42) | 42 (42–42) |
| prototype_rolling_32_2far / replace_report | 167 (167–209) | 125 (125–125) |
| prototype_rolling_32_2far / ready_to_command | 167 (125–167) | 125 (125–125) |
| prototype_jump_32_2far / intent_to_command | 125 (125–125) | 125 (125–125) |
| prototype_jump_32_2far / intent_queued | 42 (42–42) | 42 (42–42) |
| prototype_jump_32_2far / replace_report | 209 (208–209) | 125 (125–167) |
| prototype_jump_32_2far / ready_to_command | 167 (167–167) | 166 (125–167) |

## 受測操作內配置次數

獨立 executable 覆寫 new/delete，統計對應 C++ allocator 的 requested bytes；未計入初始建構。

| 情境／操作 | Abseil B-tree＋Pool | Pool 稀疏分頁 |
|---|---:|---:|
| rolling_4 / index_ack | 0 | 0 |
| rolling_32 / index_submit | 57 | 0 |
| rolling_32 / index_ack | 0 | 0 |
| outliers_32 / index_ack | 0 | 0 |
| prototype_rolling_32_2far / ready_to_command | 227 | 0 |
| new_book_growth / index_insert | 60983 | 61306 |

## Index 存活記憶體（requested bytes）

含 inline Index、Book/Level/Member/Page pools 與目錄；不含外部 Order/Memberships、allocator metadata、完整流程狀態；不是 RSS。

| 情境 | Abseil B-tree＋Pool | Pool 稀疏分頁 | 活頁／頁塊 |
|---|---:|---:|---:|
| 1books_4near_0far | 19960 | 88000 | 2 / 1 |
| 1books_4near_2far | 20088 | 88064 | 4 / 1 |
| 1books_32near_0far | 21928 | 88000 | 2 / 1 |
| 1books_32near_2far | 22440 | 88064 | 4 / 1 |
| 1books_128near_0far | 38504 | 100880 | 3 / 1 |
| 1books_128near_2far | 51848 | 113840 | 5 / 1 |
| 128books_4near_0far | 148392 | 412616 | 256 / 4 |
| 128books_4near_2far | 216104 | 744552 | 512 / 8 |
| 128books_32near_0far | 1117992 | 1130312 | 256 / 4 |
| 128books_32near_2far | 1235752 | 1463144 | 512 / 8 |
| 128books_128near_0far | 4059944 | 3735400 | 384 / 6 |
| 128books_128near_2far | 4180776 | 4079528 | 640 / 10 |

計時器基準 p99：42 (42–42) ns。0/41/42 ns 等量化值附近不能可靠排序或推算倍數。

原始資料：[cpp-index-time-2026-09-22.csv](results/cpp-index-time-2026-09-22.csv), [cpp-index-alloc-2026-09-22.csv](results/cpp-index-alloc-2026-09-22.csv)。保留 p50/p99/p99.9/max 與 subset 樣本數。
