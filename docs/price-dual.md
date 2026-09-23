# 價格 Hash＋有序樹實驗

本次參考 [How to Build a Fast Limit Order Book 保存版本](https://gist.github.com/halfelf/db1ae032dc34278968f8bf31ee999a25) 的多存取路徑設計，新增 C++ `HashOrderedLocator`。量測結果見 [結果表](price-dual-results.md)。

## 本次結果

- 完整 OMS 單次 price query p99 都為 125 ns，未辨識出改善；熱資料 batch64 p50 / 64 在 32 張訂單時為 B-tree 7.16 ns、分頁 5.20 ns、Hash＋B-tree 4.56 ns。4 張訂單則 B-tree 3.91 ns 比 hash 4.56 ns 更低。batch64 不是單次 p99。
- 32 張訂單的立即意圖轉命令 p99：B-tree 292 ns、分頁 292 ns、hash 312.5 ns；回報可見：1021／1000／1021 ns；ready dispatch：292／250／333 ns。本次雙索引沒有改善完整流程 p99。
- 20,000 個遠隔價位從空建立，B-tree 和 hash 的 new-price p99 同為 125 ns，但各輪 max 中位數為 1.646 µs 和 105.980 µs。稀疏分頁 p99 約 4.938 µs，每個遠隔價位都需一頁，使這個故意稀疏情境特別不利。
- 同一成長階段累計配置量約為 B-tree 0.707 MB、hash 2.345 MB、分頁 22.029 MB（十進位 MB；不是 RSS）。Hash 相比 B-tree 只增加 15 次配置，但少數大配置與搬移仍會形成尖峰。
- 判斷：保留新 backend 供查詢比例高的情境選擇；目前無證據支持改掉預設。若繼續優化，應先處理 hash 成長方式，而不是只依熱查詢速度決定。

## 擴容尖峰的獨立診斷

`cpp/price_dual_growth.cpp` 額外記錄每次 insert 前後 hash capacity，讀 capacity 在計時區間外，與六輪主比較分開。三次獨立執行的最慢插入都發生在零起算 step 14,336，容量從 16,383 升到 32,767，耗時分別約 199／118／94 µs。由於是新程序的獨立診斷，其數字不混入主比較，亦不代表正式硬體的固定上限。

這直接確認慢插入與擴容同時發生；它包含擴容時配置、搬移、觸頁等總成本，尚未以 profiler 分解各自占比。

診斷與原始 capacity 事件：[JSON](results/price-dual-growth-diagnostic-2026-09-23.json)。診斷建置／執行腳本：`scripts/check_price_dual_growth.py`（依賴已建置 Release benchmark 的相同連結函式庫；既有輸出不覆蓋）。

## 實作範圍

- `absl::flat_hash_map<Price, Handle>` 負責 exact lookup。
- 沿用 `BtreeLocator` 的 `absl::btree_map<Price, Handle>` 負責有序走訪，以及 confirmed set 負責 working 價格。
- Pool、generation handle、Book best cache、Membership、同價雙向成員串列，以及完整 OMS 的日誌／去重／歷史／風控完全共用。
- 新价位建立及刪除同步維護 hash 與有序索引；既有訂單的同價數量變更仍可直接透過 Membership 更新，不必查 hash。
- 無固定價位數或價格區間假設，hash 未預先 reserve，成長成本保留在測試中。測試筆數上限只保護 benchmark，不是索引容量上限。
- 未新增 working 價位雙向連結，也不是原文 binary tree 的逐行翻譯；本次是對現有 B-tree 加 hash 的受控比較。
- 此版本為可選實驗 backend，未改預設。多容器更新和既有索引一樣沒有配置失敗後的交易回滾保證；不是經過 OOM 恢復驗證的正式元件。

程式碼：`cpp/include/hash_ordered_index.hpp`。

## 驗證

Release 與 UBSan（halt_on_error=1）均通過：

- C++ 三版本對實際 Rust standard OMS 的同一份 1,202 步 trace，比對每步 outcome、state、journal 摘要。
- C++ 21 組測試；包含新 backend 20,000 次 locator、10,000 次 index 隨機轉移，以及 cancel/rehang、latest-intent flow。
- locator oracle 驗證極端正負價格、插入覆蓋、刪除、working 狀態、best、完整及提早停止的有序 range。
- UBSan 新 backend 診斷 benchmark 1,000 筆 smoke。
- 六輪完整 OMS，每輪每個 backend 的五組 workload 最終 state/journal digest 一致（18 組結果）。

逐步 hash 一致不是形式化正確性證明。此次未重新執行 ASan，也未修改 Rust 邏輯。

## 量測方法

macOS arm64、Apple Clang 17，Release `-O3 -DNDEBUG`，無 LTO。全部版本用同一支 C++ binary、相同完整 Engine。六輪遍歷三個 backend 的全部排列，序列執行、不與編譯或 sanitizer 並行。

完整 OMS 沿用先前語意對齊的 benchmark：每輪每操作 20,000 筆，4／32／128／1024 張同 Book 訂單；每張有一個初始價位，改單在途時可能另占新價位。衡量精確價格摘要、立即意圖轉命令、queued/coalesced intent、回報可見、重複回報、待送命令 dispatch，另含成交及去重。未包含網路或檔案 WAL。

獨立 locator 診斷，與完整 OMS 分開呈現：

- 4／32／128／1024 價位，以固定亂數流查詢，1/8 miss。
- 既有價位 assign、最多八個價位 range、最佳價位刪除＋找下一檔、重新插入最佳價位。
- 從空結構建立 20,000 個間距 1,000,003 的價位，再由最佳價往下刪除。每個價位使用不同 page，是刻意壓力情境；不代表日常近價集中。
- 此層只有 locator 和必要的 PagePool，不含 Order/Level/Member Pool 或日誌。`existing_price_assign` 是容器覆蓋成本，不是 OMS 同價成交快路徑。

配置 pass 用另一個 executable 覆寫 C++ new/delete 計數，不污染時間 pass。配置數為量測区間累計呼叫與 bytes，不是常駐或尖峰 RSS，也不包括全部 OMS 配置。時間 CSV 的 allocation 欄位 0 是格式佔位。

## 解讀限制

- 各表為「六輪分位數的中位數及輪間範圍」，不是所有樣本合併的分位數。
- 單次計時有約 42 ns 的粒度／基線，很多 locator p99 都為 42 ns；這不代表真實成本相等。完整 OMS 的單次 query p99 都為 125 ns，不能據此宣稱查詢改善。
- batch64 為每批 64 次查詢 p50 / 64，只反映熱資料下的平均成本，不能稱為單次 p99。targets 重複使用至多前 64 張訂單的價格、含 1/8 miss，不涵蓋整個大型 Book 的隨機存取。
- 每個完整 benchmark 程序仍以 4→32→128→1024 順序跑 workload；較小案例反而较慢可能受首次觸頁／配置器／快取影響，尚未歸因。只比較相同情境內的 backend，不由跨大小數值推論擴展性。
- 未綁定 CPU、隔離 OS 工作或在正式部署硬體量測。max 易受排程干擾；少量 rehash 不一定出現在 p99 或 p99.9。
- 沒有新增多 Book 記憶體規模測試、長時間 hash tombstone 壓力、預先 reserve 版本或原始指標版本。

## 重現

```sh
cmake -S cpp -B cpp/build -DCMAKE_BUILD_TYPE=Release -DCMAKE_CXX_COMPILER=/usr/bin/clang++
cmake --build cpp/build --target oms_parity index_tests price_dual_bench price_dual_alloc -j4
cargo build --offline --profile parity --example oms_parity
python3 scripts/run_price_dual.py --tag YOUR_FRESH_TAG
python3 scripts/summarize_price_dual.py YOUR_FRESH_TAG
```

runner 先通過逐步正確性比對再量測，拒絕覆蓋既有原始輸出，並保留環境、程式碼與 binary SHA256。彙整程式更新 `docs/price-dual-results.md`。
