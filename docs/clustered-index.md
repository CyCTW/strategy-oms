# 移動近價群與少量遠價：Pool 稀疏分頁驗證

2026-09-21。已將 Pool 稀疏分頁接入相同的 Index 與 Engine，比較價格查詢、回報更新、意圖到待送命令。**預設維持 Standard，PooledPages 為可選實驗後端。** 本輪驗證使用者描述的分布，並不改變策略意圖契約、單張單只有一筆在途請求的限制或 WAL。

## 結論

近價集中且有數十個有效價格時，分頁在部分更新操作有收益；少量漲跌停遠價不需要配置中間空白頁。但只有四個近價時，完整 Engine 幾乎沒有收益。大量小 Book 及持續新增 Book 的測試則出現明顯退步，所以目前不足以替換通用預設。

| 指標（五輪 p99 中位數，ns） | Standard | PooledPages |
|---|---:|---:|
| 4 近價＋2 遠價：價格查詢 | 42 | 42 |
| 4 近價＋2 遠價：意圖到立即命令 | 625 | 625 |
| 4 近價＋2 遠價：改單回報 | 375 | 375 |
| 32 近價＋2 遠價：意圖到立即命令 | 625 | 625 |
| 32 近價＋2 遠價：改單回報 | 417 | 375 |
| 32 近價＋2 遠價：已就緒意圖到命令 | 708 | 667 |
| 4096 小 Book、各 4 近價＋2 遠價：隨機價格查詢 | 167 | 250 |
| 不斷新增 Book：索引插入 | 416 | 1250 |
| 4 個近價旁，另一張單反覆撤單／近遠價補單：New 到命令 | 875 | 875 |

這些是本機 CPU service time，非正式環境 SLO。計時器基準 p99 約 42 ns；不能對 42 ns 附近結果宣稱精確倍率。32 近價的回報收益約一個計時刻度，需在正式硬體與真實輸入分布再次確認。完整輪間範圍、p99.9、記憶體與配置數見[量測摘要](clustered-index-results.md)。

## 分頁實作

- `src/price_pages.rs`：每個 Book 持有 `page_id → PageHandle` 的 BTreeMap，以及有確認掛單的 page_id 集合。頁面物件放在 Index 共用的 PagePool，不是每個 Book 各建一個 Pool。
- 每頁 64 個原始整數價格位置，使用 `div_euclid(64)` 與 `rem_euclid(64)`，含負價與 i64 兩端。這不是總價格區間限制。
- 每頁保存 64 個 LevelHandle、occupied bitmap 與 working bitmap。未占用的位置不會解參考，即使殘留已失效 handle 也不會作為有效資料使用。
- range 只走訪已存在的頁面，再按 bitmap 枚舉頁內價格；不掃過近價到遠價之間的空頁。查詢不配置額外容器。
- best 仍使用 Index 的既有快取。失效後從已確認頁集合與 bitmap 找下一價格，不掃描所有 pending-only 價格。
- 頁面位置依價格固定，不跟隨行情最佳價重設。市場行情沒有直接輸入本索引，只有實際訂單／已送請求的變化會更新索引。
- 空頁立即退回 PagePool，generation 保護重用；Pool 已配置區塊保留。頁面目錄與 confirmed 集合仍由標準 allocator 管理，所以不是整條改價路徑保證零配置。
- **每個 Pool 區塊容納 64 頁，每頁又涵蓋 64 價格位置，兩者是不同設定。** 本原型沿用通用 Pool 的 64-slot 粒度，一次配置約 68 KB 的頁面儲存，帶來小 Book 初始記憶體與成長成本。

策略的近價密集度應以合法 tick 理解，但目前 core price 是原始固定精度整數；本輪假設一個原始單位等於一個 tick。尚未提供 tick 表、分段 tick 或 tick 規則變更的價格映射。實際價格乘上 100 後，不可仍把原始連續整數測試直接當成同一個工作負載。

## 工作負載

索引層使用與 Engine 相同的 Index、Level/Member pools 與差異更新邏輯。每個 moving order 先建立一筆 pending Replace，整批提交完才逐筆 ACK，因此同時存在舊確認價格與新待改價格；每張實體單仍最多一筆 pending。

| 情境 | 操作 |
|---|---|
| rolling_4/32/128 | 初始價格從 62 開始連續分布，每批中心前進一個價格單位 |
| outliers_4/32/128 | 同上，另維持 -1,000,000,000 與 +1,000,000,000 兩個遠價 |
| boundary_4/32/128 | 中心在 62/63 之間反覆移動，跨過 63/64 分頁邊界；舊新價格按在途狀態共存 |
| jump_4/32/128 | 每八批在相距 1,000,000 單位的兩區間跳動，區間內仍逐步移動 |
| 4096books_4near_2far | 隨機選 Book，以 BookHandle 查近價，檢驗大量小 Book 的工作集合成本 |
| new_book_growth | 從空索引逐筆加入 20,000 Book，各一個價格，包含所有 Pool／HashMap／B-tree 成長 |

`price_query` 查詢目標價格；`near_range8` 只查當前近價區間最多八層，遠價不在區間內；`far_query` 查遠價。`index_submit/index_ack` 是價格索引維護的拆解量測，不包含完整回報 reducer。

完整 Engine 測試使用相同實驗後端與 MemoryJournal：

1. `engine_rolling/jump_4/32_2far`：建構 4 或 32 張近價單，加兩張遠價單。每次一張近價單收到立即 Replace，再在其 pending 時接收更新的 Replace 意圖；收到第一筆 ACK 後，dispatch 最新意圖，再 ACK。分開量測查詢、立即意圖、等待意圖、第一筆回報與 dispatch。其餘訂單逐次輪流移動；這與索引層的整批延遲 ACK 情境互補。
2. `engine_cancel_rehang_4near`：保留四張近價單，另一張單在近價與遠價之間反覆撤單、以新 order_id 補單。新單 Accepted 在舊單 Canceled 之前，因此兩張實體單短暫共存；每輪驗證預留量由 500→600→500，測 Cancel/New 到命令及兩種回報。歷史訂單與 ID map 的成長包含在量測內。

遠價僅為模擬的距離，沒有實際商品漲跌停驗證、交易所成交模型或市場最佳價 feed。這些參數是實驗點，不是正式容量上限，也未假設策略永遠只會有四張或三十二張單。

## 記憶體與尾延遲

記憶體包含 Index 及其 Book、Level、Member、Page pools 和目錄，是 allocator requested bytes，不是 RSS；不含 OrderStore、外部 Memberships、WAL 或整個 Engine。

| 分布 | Standard | PooledPages |
|---|---:|---:|
| 1 Book、4 近價＋2 遠價 | 21,116 B | 89,788 B |
| 128 Book、4 近價＋2 遠價 | 234,616 B | 783,608 B |
| 128 Book、32 近價＋2 遠價 | 1,225,336 B | 1,503,992 B |
| 128 Book、128 近價＋2 遠價 | 4,481,656 B | 4,111,224 B |

一個 Book 的四個近價從 62 開始，實際跨兩頁，加兩個遠價共四個活頁。它們仍只占一個共享配置區塊；空白中間頁沒有被建立。因此先前「每價各占一頁」的 16 倍 locator 成本不能直接套用，但低占用頁與 Pool 配置粒度仍有成本。

`new_book_growth` 的 pages 後端有 313 筆操作觸發新增頁塊：這個條件子集的 p99 中位數約 **19,125 ns**（輪間 17,834–32,791 ns）。這些操作也可能同時觸發 Book/Level/Member pools 或 ID map 成長；**不能將整段時間全歸因於頁面配置**。這不是每次查詢或每次報告的 p99。子集樣本少，尤其 p99.9 的解讀要保守。

每輪 new_book_growth 的最慢操作，Standard 約 275–384 μs，Pages 約 271–278 μs；沒有固定最壞延遲保證。沒有 profiler 資料可以把退步直接斷言為 cache miss。32 近價的 ready_to_command 配置數雖從 357 降到 0，也不能概括成整個 OMS 零配置。

要繼續改善此候選，優先研究頁面儲存的較小配置區塊、小型頁表示、預備頁與目錄配置成本，並將大量小 Book 的記憶體與延遲一併列為採用條件。本輪沒有因為某條路徑有收益就更換預設，也未增加隱藏背景執行緒或自動切換後端。

## 使用與相容性

```rust
let oms = Engine::new_with_index(journal, limits, IndexBackend::PooledPages)?;
```

recover 可透過 `recover_with_index` 選擇同一後端。價格頁屬於衍生索引，不改 WAL 格式；回復後重新建立頁面並維持既有隔離語意。BookHandle 的 owner/generation 驗證及失效條件不變。

新增 `IndexStats.page_blocks` 與 `live_pages`，可觀察已配置頁塊及活頁數。`paged-index` 與 `pooled-index` feature 名稱保留供既有建置指令使用，但不再改變建構／恢復預設。預設固定為 Standard B-tree；若要使用實驗後端，必須明確呼叫 `new_with_index`／`recover_with_index`。既有 Limits 仍生效。

增加公開 enum variant / stats fields 可能影響外部 exhaustive match 或直接建構 IndexStats 的程式，整合時需更新；一般策略查詢與送單呼叫方式不變。

## 重現與驗證

```sh
cargo test --offline
cargo test --offline --features pooled-index
cargo test --offline --features paged-index
cargo clippy --offline --all-targets --all-features -- -D warnings
cargo bench --offline --bench clustered_index > docs/results/clustered-index-2026-09-21.csv
python3 scripts/summarize_clustered_index.py > docs/clustered-index-results.md
```

當時以 feature 切換三種預設時，各通過 98 個測試；目前預設已固定為 Standard，但各候選仍由明確選擇的測試覆蓋。新增 20,000 步 page 隨機差異測試，包含負值／邊界、working 翻轉、range、Book 隔離、清空與回收；Index 的 10,000 步差異測試與既有價格查詢、極端價格成長、recovery 及配置測試也涵蓋 PooledPages。新增完整撤近掛遠測試，驗證在途紀錄、風控預留、best 排除未確認單及頁面重用。

量測於 Darwin arm64、Rust 1.94.1 release 執行，每一般情境每輪 20,000 筆、五輪交替後端順序。計時與配置計數分開；輸入準備及初始建構在計時外，growth 操作自身的配置不排除。Engine 預先配置 request/report maps 與 MemoryJournal；OrderStore 與新 Book/頁面依實際路徑成長。沒有綁核、NUMA、磁碟同步、網路、並行生產者或這一輪的排隊延遲量測。不能將以前 burst 數字當成此分頁後端結果。

`OMS_BENCH_SAMPLES` 與 `OMS_BENCH_ROUNDS` 可控制規模。原始 CSV 的 time/alloc 列保留分位數、最大值、樣本數、配置次數和累計配置 bytes；memory 列為存活 Index bytes；capacity 列的 allocations/allocated_bytes 欄位分別記錄活頁數與頁塊數（非配置次數／bytes）。摘要腳本明確解讀此格式。
