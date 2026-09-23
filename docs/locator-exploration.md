# 其他價格索引方案：探索與篩選

2026-09-21。結論：目前沒有證據支持全面替換正式預設。保留 Standard B-tree＋共用 Pool＋差異更新；這次新增的程式只在 benches/support 與 tests 使用，未改 Engine、交易語意、WAL 或公開 API。

## 為什麼不是直接找另一棵樹

必須分開考慮價格數 P、每價訂單數 M、Book 數 B，以及新增／刪除價格的頻率。P 小不代表 M 小；大量訂單可能集中同價。反之，同一張在途改單可能同時占舊價及新價，使價格數增加。

目前同價成交已透過 membership handle 更新 Totals，通常不查樹；已確認量跨過零才調整 working 索引。未送出的最新意圖也不改價格索引。因此換樹最直接影響精確價格查詢、區間查詢與價格層新增／移除，不能由定位器 benchmark 推論整體回報或意圖處理的速度。

## 實際比較的原型

所有原型維持相同的 price → 16-byte LevelHandle-like value、working flag、有序 range、雙向 best 語意。value 僅模擬正式 Handle 尺寸，不實際解參考 Level pool。

| 原型 | 設計 | 要驗證的問題 |
|---|---|---|
| Standard | BTreeMap＋BTreeSet | 對應目前 locator 的基準 |
| Flat | 按價格排序的可成長 Vec | 查詢局部性是否值得付出 O(P) 搬移成本 |
| Adaptive 4/8/16 | inline 小陣列，超過門檻後轉 Standard | 普遍小 Book 是否值得特化 |
| HashOrdered | HashMap 精確查詢＋Standard 有序／working 索引 | 精確查詢收益能否抵銷雙份資料與更新成本 |
| Paged64 | BTreeMap 定位稀疏 page，page 內 64 格＋occupancy/working bitmap | 價格密集時能否攤薄樹操作 |

Adaptive 的 K 是實作切換門檻，不是訂單／價格數上限。升級後不在熱路徑降級，避免 K/K+1 反覆重建。本原型升級後仍保留 inline 空間，這部分記憶體已計入；真正整合時需評估 enum 或獨立小節點 pool 等布局，不能把原型結果當成最終布局結果。

Paged64 使用 signed price.div_euclid(64) 與 rem_euclid(64)，支援負價格及 i64 兩端。只為用到的 page 配置空間；64 是局部粒度，並未限制價格區間。原型 page 使用 Box，尚未做 page pool。這裡的 1 是原始價格儲存單位；若商品 tick 大於 1，連續合法報價未必是連續儲存整數，不能直接套用密集情境的結果。

## 方法與限制

- Darwin arm64、Rust 1.94.1、release、thin LTO；未固定 CPU、未控制系統背景工作，非正式交易機環境。
- 每情境每操作 20,000 筆，5 輪；輪替起始候選並交替方向。不同候選使用相同價格輸入。標準 HashMap 的 hash seed 由 runtime 決定。
- 單次操作用 Instant 計時；計時器基準 p99 為 42 ns。42 ns 附近無法可靠細分，不能宣稱「精確快一倍」。沒有從分位數直接扣除 timer。
- 配置計數另跑一次，時間量測不啟用計數。realloc 計為一次配置。記憶體是仍存活的 allocator requested bytes 加 inline struct 大小，不是 RSS，不含 allocator metadata 或共享 OMS pools。
- 稀疏情境每個價格相隔 1,000,003 儲存單位；密集情境相隔 1。測 1/4/8/16/64/4096 價格及 4096 個小 Book。這些是實驗點，不是業務容量假設。
- hit/miss/range 為已建構資料的查詢。reprice 為隨機移除一個有效價格，再加入新的價格，保持總數，**不是完整 OMS Replace**。
- working_off_best_on 包含關閉一個 working flag、重查 best、再開啟；4095_pending_only 量測只有最低價有效、其餘 4095 價為 pending-only 時，買方重新找 best 的成本。正式 best 查詢通常直接讀快取，這是快取失效後的重建工作。
- promotion 分別量 4→5、8→9、16→17；每筆先在計時外重建，計時包含該筆升級。這會暖化 allocator，不代表最糟冷啟動。random_growth 從空成長到 20,000，以 u64 奇數乘法置換後的 i64 key 打散插入位置，不用只有尾端 append 的偏好輸入。
- 原始 CSV 包含 p50、p99、p99.9、max。表格是各輪分位數的中位數及範圍，不是所有樣本合併的分位數。罕見擴容也可能落在 p99 之外，因此另列升級事件。
- 本輪是 **locator 單獨篩選**。沒有測 pool 解參考、回報 reducer、WAL、意圖排程、網路或排隊；也沒有以此次數字宣稱三條完整路徑改善。

## 結果與判斷

以下均為五輪 p99 中位數，單位 ns；完整範圍與 p99.9 見[量測摘要](locator-exploration-results.md)。

| 情境 | Standard | 對照候選 | 解讀 |
|---|---:|---:|---|
| 4 價格精確查詢 | 42 | Adaptive8：42 | 已到計時解析度，無法選出勝者 |
| 加入第 9 個價格 | 42 | Adaptive8：166 | 混合結構有可見的升級尖峰 |
| 4096 稀疏價格重定價 | 375 | Flat：1541 | 連續陣列的搬移成本浮現 |
| 隨機成長至 20,000 價 | 250 | Flat：8625 | 即使只配置 14 次，也不能保證低尾延遲 |
| 4096 價只有最低價有效，重找買方 best | 42 | Flat：1209 | 沒有 working 摘要的線性掃描不適合極端狀況 |
| 4096 稀疏價格精確查詢 | 84 | HashOrdered：42 | 有局部收益，但 42 ns 已接近計時下限 |
| 隨機成長至 20,000 價 | 250 | HashOrdered：291 | 額外維護 hash 的成本需納入 |
| 4096 密集價格重定價 | 375 | Paged64：166 | 適合價格分布密集的條件 |
| 4096 稀疏價格重定價 | 375 | Paged64：541 | 不能把密集情境收益套用到任意價格分布 |

4096 價格的 locator 記憶體：Standard 約 280 KB、HashOrdered 約 485 KB；Paged64 密集時約 70 KB，稀疏時約 4.48 MB（約 Standard 的 16 倍）。單一 4 價格 locator：Standard 432 bytes、Adaptive4 192、Adaptive8 320、Adaptive16 576。門檻愈大不是必然更好。

Flat 在 4096 價重定價的 20,000 次操作內零配置，但仍顯著慢於 Standard；這直接說明配置次數不能單獨當作延遲代理指標。

小型 B-tree 本來就可以只用單一多鍵節點，節點內的 key 已連續存放；它不像每個價格各占一節點的 AVL。因此 small-array 主要可望節省初始配置、間接存取或 metadata，不能先假定它在少量資料下必然顯著更快。

## 還值得探索、尚未實測的設計

### 小型根節點＋真正由 Pool 管理的 B+ tree

這是下一個通用候選：root 起初直接容納少量價格；成長後分裂成多個 leaf，內部節點只保存 separator 與 child handle。每節點容納多鍵，葉節點維持鏈結，range 跨葉走訪；Level/Member 仍留在既有 pools。

節點另存 working 摘要，避免第二棵 confirmed BTreeSet。可以是 subtree_has_working，或每節點的有效 child/entry bitmap，使最後一張確認單離開某價時不用掃過大量 pending-only 價格。需量測摘要維護的成本，不能只量精確查詢。

與 Adaptive 先搬到另一套樹不同，這是同一結構由小根節點自然分裂；但仍可能一路分裂到根，不是零尖峰。刪除的 merge/rebalance、leaf link 與 handle 穩定性增加驗證負擔。Pool 無空位時仍會擴容；應包含 block 成長、頁面首次觸碰與節點分裂的條件延遲。尚未寫此原型，也沒有宣稱它比 Standard 快。

多鍵節點的 cache 局部性與記憶體權衡可參考 [Abseil B-tree 設計](https://abseil.io/about/design/btree) 與 [Rust BTreeMap 文件](https://doc.rust-lang.org/std/collections/struct.BTreeMap.html)。這些是研究動機，非本 OMS 的效能證明。

### ART／壓縮 radix tree

以價格的位元組前綴導航，節點按子節點數採不同容量，壓縮共同前綴，仍可有序走訪。i64 價格可先轉成 `(price as u64) ^ (1u64 << 63)`，再按 big-endian bytes 導航，保留 signed 數值排序；不是直接依主機端序比較記憶體。

64-bit key 對應八個 byte 位置，但不等於八次機器指令或固定延遲；節點間接存取、節點容量轉換與記憶體來源仍需量測。適合納入大且稀疏 Book 的下一輪比較，對大量極小 Book 不一定划算。未實作／未實測。原始設計參考 [Leis 等人的 ART 論文](https://db.in.tum.de/~leis/papers/ART.pdf)。

### 熱門價格直接持有可驗證的 LevelHandle

若策略一直查相同幾個價格，可考慮 optional cache token：驗證 index owner、book 關聯與 slot generation，再直接讀 Level；價格層消失後 token 失效，回退價格查找。不能讓舊 handle 指到重用 slot 的不同價格，也不能跨 recovery 使用。這會減少查樹需求，但需要先有策略查詢重用率證據；不是本輪已提供的新 API。

## 建議的下一步與採用門檻

1. 通用下一候選選「小根節點＋Pool B+ tree」，與目前 Standard 比較；保留策略介面與交易語意，不因候選更換對外契約。
2. HashOrdered 只在精確價格查詢占絕大多數且額外記憶體可接受時進一步整合；Paged 只在觀測到價格局部密度穩定時採專用後端，不能默認全部商品適用。
3. 不把無限長的 sorted Vec 當通用更新索引。小型路徑可以使用，但需要有序成長方案、working 摘要與條件尖峰量測。
4. 候選通過差異測試後，接回既有三段量測：價格查詢、回報到索引可見、意圖到待送命令。測多 Book、價格新增／消失、真正改單在途雙價格、拒絕與重播，並把 burst 排隊時間算入。
5. 在目標正式硬體量測 p99/p99.9/max、配置／分裂事件、記憶體高水位；不能只以本機 42 ns 解析度下的小差距換預設。cache-miss 原因應由 profiler 證實，本輪未量 hardware counters。

不預先限制訂單數／價格區間，與任意突發成長都保證固定延遲，是兩個不同要求。可用分段成長、可回收備用 block、提早補充與每輪有預算的準備工作降低機率；備用耗盡仍有同步成長或明確資源壓力處置，不能丟回報或假裝狀態已更新。若用背景補充，交接、回收與可見性本身也需要設計。本輪未新增這些機制，既有 Engine Limits 也仍存在。

## 重現與驗證

```sh
cargo test --offline --test locator_candidates
cargo bench --offline --bench locator_exploration > docs/results/locator-exploration-2026-09-21.csv
python3 scripts/summarize_locator_exploration.py > docs/locator-exploration-results.md
```

`OMS_BENCH_SAMPLES`、`OMS_BENCH_ROUNDS` 可調整樣本數及輪數，無關正式容量。每個原型以 20,000 筆隨機新增、覆寫、刪除、working 轉換與參考 BTreeMap 比對，驗證極端 signed 價格、負數 page 邊界、上下界 range、early stop、雙向 best、清空及重用。7 個新測試通過；完整測試共 96 個通過，all-targets/all-features clippy 通過。
