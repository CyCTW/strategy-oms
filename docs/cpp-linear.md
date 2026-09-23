# C++ 線性搜尋實驗

本次比較三種同語言實作。保持策略可見的 summary、best、ordered range、order IDs 與 pending contribution 語意，Rust OMS 的預設與交易 API 不變。

## 本機結果與決策

2026-09-22，Darwin arm64、Apple Clang 17、Release -O3 -DNDEBUG，六輪計時與一輪配置測試已完成。Release 正確性測試、UBSan 正確性測試與 UBSan 全情境縮小測試均通過；本次沒有取得 ASan 完成結果，先前環境問題見 [C++ 驗證紀錄](cpp-index.md)。

以下為六輪 p99 中位數，單位 ns；不是合併 p99。

| 操作 | B-tree | Linear prices | Linear orders |
|---|---:|---:|---:|
| 32 價格，隨機單價彙總 | 42 | 42 | 84 |
| 32 價格，簡化 ACK → 可見 | 167 | 125 | 42 |
| 32 價格，簡化立即 intent → command | 125 | 84 | 42 |
| 32 價格，fill 後 16 次查詢合計 | 375 | 334 | 792 |
| 128 價格，隨機單價彙總 | 42 | 83 | 167 |
| 4,096 價格，隨機單價彙總 | 42 | 1,167 | 7,396 |
| 4,096 張同價單，查彙總 | 42 | 42 | 6,937.5 |
| 4,096 張同價單，列舉全部 order IDs | 17,188 | 17,000 | 7,771 |

判斷：

1. Linear prices 在部分小 Book 更新／簡化派送路徑有收益，並未證明小 Book 單價查詢全面優於 B-tree。4～32 價格的單次量測多在 42 ns 計時粒度；batch64 的平均成本輔助觀察也沒有顯示它穩定勝出。
2. Linear orders 省掉價格聚合維護，更新與簡化派送成本低；查詢多時則要反覆掃描及計算。它在完整列舉大量同價 order IDs 的測試反而較快，不能說所有查詢都退步。summary 與列舉是不同工作。
3. Linear prices 到 128 價格開始在本次多個操作落後；4,096 價格和成長至 20,000 價格的退步明顯。這些採樣點不是已校準的自動切換門檻。
4. 排序區間查詢維持既有契約且不配置暫存，線性候選因此採反覆掃描。32 價格取前 8 層的 p99 為 B-tree 42、Linear prices 209、Linear orders 520.5 ns；不適合忽略這條路徑後宣稱整體較快。
5. 預設不變，保留 Linear prices 為小 Book 候選。若要做 small-inline + large-tree，需另量門檻附近的 promotion、Book 物件膨脹與真實查詢比例；本次沒有實作這個混合版本，也没有自動採用 8／16／32 為閾值。

計時器基線 p99 42 ns，不計算接近基線數值的精確加速倍數。完整輪間範圍、p99.9、配置次數及原始 max 保留在 [量測摘要](cpp-linear-results.md) 和 CSV；metadata 見 [本次建置與雜湊](results/cpp-linear-metadata-2026-09-22.json)。

## 實作

| backend | 價格查詢 | 更新時維護 | 訂單資料 |
|---|---|---|---|
| absl_btree | Abseil B-tree → LevelPool | Level totals、雙向 Member、confirmed tree、best cache | 權威 Order 放分段 Pool |
| linear_prices | 未排序的連續 vector<Entry> 線性找價格 → LevelPool | 和 B-tree 相同的 totals／Member／best；working 位元放 Entry | 同樣的權威 Order Pool |
| linear_orders | 每 Book 的有效訂單指標 vector，掃描權威 Order 並即時聚合 | 只維護有效清單與 Book reserved／uncertain；沒有價格 totals、price members、best cache | 直接指向同一份權威 Order，沒有 shadow copy |

`LinearLocator::Entry` 包含 price、LevelHandle、working。找到後刪除採 swap-with-last，Level 仍在 Pool；新價位先查有無重複，必要時 vector 成長。並未假設最大價格數、訂單數或價格區間。

`OrderScanIndex` 的 active entry 包含 Order 指標與 Membership 指標；Membership 保存清單位置，終結刪除可直接 swap-remove，並修正被換入項目的位置。權威 Order／Membership 必須有穩定位址；使用與 indexed 方案相同的分段 Pool，並非把整份 Order 緊密平鋪在 vector。本次結果不能概括所有 AoS／SoA 線性布局。

三者共用六種 contribution 統計。跨價 Replace 同時涉及舊價與 pending 新價；同價 Replace 只出現一個 member。直接掃訂單的 summary 即使命中第一張也必須掃完，因為仍可能有其他同價訂單。newest desired 尚未實際送出時不進入統計。

所有 backend 提供遞增排序的區間結果、早停、無隱藏 heap snapshot。兩個未排序線性候選以反覆找下一個最小價格維持契約，因此取 k 個價位約需 k 次全掃描；這個代價明確計入。未另外建立有序目錄或每次配置排序暫存。best 在 B-tree／linear_prices 使用快取，linear_orders 每次掃描。

共同 Store 包含權威 Order Pool、訂單 handle vector 與 backend。`index_*_visible` 包含定位該 State、更新 backend 與寫回 Order，完成後可立即查詢。它不是完整回報 reducer，不含回報協定驗證／ID HashMap／去重／日誌。新增成長包含共同 Store 的 Pool 和 handle vector 配置。

## 測試與量測

- 正確性：17 組測試；每個 locator 20,000 隨機操作；四種 Index（含原 Paged）各 10,000 訂單狀態轉換，對照獨立重建參考模型。涵蓋 negative／i64 extremes、pending New／Replace／Cancel、uncertain、終結／重加入、相同價格聚合、排序區間／早停、member 列舉與外部 handle。另有取消／遠價新單及 latest-intent 測試。
- Release 與 UBSan 執行正確性測試；UBSan 另跑縮小的全部 benchmark 情境。
- 計時與配置為不同執行檔。計時版不覆寫 allocator；配置版不計時。
- 六輪依序採 B/L/O、O/L/B、L/O/B、B/O/L、O/B/L、L/B/O，平衡執行位置與前一 backend。
- 單次樣本通常每輪 20,000 筆；1,024／4,096 價格的重情境上限 4,000，range／order member 列舉上限 2,000。這些限制屬 benchmark 工作量，不是元件容量。
- 查詢依序量 first、last、miss、random、best；相鄰查詢會暖化資料，不能當獨立冷 cache 測試。多 Book 情境另外以偽隨機 Book 查詢擴大工作集，但也不保證每次 cold cache。
- 小 Book 另量每批 64 次查詢，含亂數和迴圈，用整批 p50 / 64 輔助觀察；不是單次 p99。
- 更新涵蓋 partial fill、近／遠價 Replace、ACK、Cancel、terminal、重新加入。重新加入是索引狀態測試，並不是允許重開已終結交易所訂單的 API。
- mixed 為同價 partial fill 後 0／1／4／16 次 summary，含約 20% miss；targets 預先產生，計時內無 RNG。它不代表所有策略的查詢分布。
- prototype 使用同一個 Flow 模板，只有 Index type 不同；計時 immediate intent→command、queued intent、ACK→visible、ready→command。包含 ID HashMap／版本／關聯驗證，但沒有 WAL、完整風控、去重、歷史、恢復、Group、transport。預期版本的準備在計時外。
- 成長情境由空 Book 新增至 20,000 個 Order／Price。保留全程、最後四分之一、每 64 筆相同位置子集；first32 只有 32 筆，不用來判斷穩定 p99。
- 沒有 CPU pinning、Linux/NUMA、生產 socket、真實生產佇列或磁碟同步。量的是函式服務時間，不包含突發輸入的排隊延遲。混合稀有極端情境的整體 p99 不能代替各情境尾部。
- 單一寫入者；借用／callback 期間不可修改索引。配置仍可能成長；未新增 OOM 回滾或跨執行緒快照保證。

## 重現

```sh
cmake -S cpp -B cpp/build -DCMAKE_BUILD_TYPE=Release -DCMAKE_CXX_COMPILER=/usr/bin/clang++
cmake --build cpp/build --target index_tests linear_bench linear_alloc -j4
ctest --test-dir cpp/build --output-on-failure
python3 scripts/run_cpp_linear.py --tag YOUR_UNIQUE_TAG
python3 scripts/summarize_cpp_linear.py docs/results/cpp-linear-time-YOUR_UNIQUE_TAG.csv docs/results/cpp-linear-alloc-YOUR_UNIQUE_TAG.csv
```

runner 保存 compiler、flags、Abseil commit、來源與 binary hashes，禁止覆寫既有 tag。完整數字見 [量測摘要](cpp-linear-results.md)。
