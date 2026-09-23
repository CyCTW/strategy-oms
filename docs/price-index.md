# v0.4 價格索引：介面、候選與採用決策

## 決策

預設採用 `IndexBackend::Standard`：保留標準 B-tree 做有序價格查找，搭配分段 Pool、同價差異更新、成員連結及最佳確認價快取。

`IndexBackend::PooledAvl` 是可執行且通過同一套測試的實驗候選，並未因為能零配置就設為預設。這不是 Pool B-tree：它使用平衡二元 AVL 樹，節點來自 Pool，parent 連結支援無配置有序走訪，subtree working flag 支援跳過只有 pending 的價格。測試結果只能代表這個候選，不能推論所有 Pool B-tree 都比較慢。

2026-09-21 新增 `IndexBackend::PooledPages` 實驗候選，頁面來自跨 Book 共用的 Pool；每頁覆蓋 64 個原始價格單位，頁面目錄仍使用 B-tree。它可支援近價集中與少數遠價，並通過相同 Engine 測試，但小 Book、多 Book 和擴容成本尚不足以支持替換預設。使用方式、`paged-index` feature、新增 stats 欄位與三段量測見 [移動近價群比較](clustered-index.md)。

目前的五輪本機比較中，AVL 的 4,096 價格查詢、區間走訪、持續换價與成長路徑較慢。共用差異更新的收益明確，但成長路徑相對 v0.3 舊索引存在回歸；沒有宣稱所有情境都變快。數據與取捨見 [量測摘要](index-benchmark-results.md)。

## 共用結構

- OrderStore：`OrderId → Handle` 的 HashMap 搭配分段訂單 Pool。既有 ID 更新不搬移物件；歷史仍依原本規則保留。
- Book registry：`Book → BookHandle`，策略可預先解析一次，熱查詢直接使用 Handle。
- PriceLevelPool：價格層只保存彙總數量、成員數與成員入口。
- MembershipPool：每個成員有 order ID、level handle、previous、next。訂單儲存最多兩個成員 Handle，跨價改單可同時涉及舊／新價格，同價改量只計一次。
- 有序查找：Standard 為 `BTreeMap<Price, LevelHandle>` 加 confirmed price set；PooledAvl 為 Pool 節點 AVL。
- Book 快取最佳已確認價格；unknown 訂單仍保留最後確認量，既有查詢語意不變。

Pool 每塊 64 slots，這是配置粒度，並非訂單數或價格層的上限。Slots 帶 generation；回收後的舊 handle 不會指向新物件，generation 耗盡時不重用該 slot。區塊由 Box 保持物件位址穩定，區塊目錄本身是可成長的 Vec，因此擴容仍可能配置並移動目錄項目；所有慢路徑都計入量測。

Pool 保留已取得的區塊供重用，沒有背景縮容或補充執行緒。Book registry、order ID map 的成長也可能配置。Standard 的價格樹與 confirmed set 仍可能配置。這不是整個 OMS 的零配置承諾。

## 更新方式

`Index::update` 比較訂單前後最多兩個價格的貢獻。成員價格不變時只調整 totals，保留同一個成員節點。價格關係改變時才連入／移出成員；確認量跨越 0 時才修改有效價格資料。純 desired 更新不進入價格索引。

書本 `reserved` 仍按每張實體單計算一次；跨價改單的成員可以有兩份，但保守曝險不能加兩次。終結後不確定訂單仍計入 Book 的 unknown 數，即使沒有價格層成員。

## API

```rust
let book_handle = oms.register_book(book);
let summary = oms.level_summary(book_handle, price);
let best = oms.best_working_price_at(book_handle);
if let Some(ids) = oms.orders_at_price(book_handle, price) {
    for order_id in ids {
        let order = oms.order(order_id);
    }
}
for (price, level) in oms.levels_in_range(book_handle, low..=high) {
    // level.totals 是固定大小的 copy，level.order_ids 是唯讀 iterator。
}
```

`register_book` 可在啟動時建立空價格簿，不是交易命令，不寫入 WAL。Handle 僅屬於當前 Engine，不能跨 Engine 或重啟保存使用；錯誤 Handle 查詢回傳 None／空 iterator。跨重啟仍以 Book 身分重新解析。

保留接受 Book 的方便介面：`at_price`、`price_range`、`best_working_price`、`reserved_qty`、`uncertain_orders`。`index_stats()` 提供目前有效價格層／成員數與各 Pool 已配置區塊數；不是整個程序記憶體使用量。

### v0.3 → v0.4 相容性

- `at_price` 從 `Option<&PriceLevel>` 改為 `Option<PriceLevel<'_>>` 唯讀 facade；`.totals` 和 `.order_ids.len()` 仍可使用。
- `PriceLevel.order_ids` 不再是 BTreeSet。它提供 len／is_empty／iter，走訪回傳 OrderId 值，順序未指定，不是 ID 排序或交易所 FIFO。需要排序時由非熱路徑明確收集排序。
- `price_range` 的項目從 `(&Price, &PriceLevel)` 改為 `(Price, PriceLevel<'_>)`，原有 `*price` 改成 `price`。
- 查詢不傳回可修改內部結構的容器，不進行隱藏排序或 heap snapshot。
- WAL 格式與單張／群組交易語意不變。可用另一個索引 backend 重播，BookHandle 會重新建立。

建構候選：`Engine::new_with_index(journal, limits, IndexBackend::PooledAvl)`；恢復使用 `recover_with_index`。預設固定為 `IndexBackend::Standard`；`pooled-index` feature 名稱仍可用於舊建置指令，但不再改變預設 backend。

## 容量與保證範圍

價格索引不假設最大價格區間或最大價格層數，含負價格與 i64 邊界的查詢都有測試。Pool 自動成長，沒有為業務設定 64／128 等最大筆數。

Engine 既有 `Limits` 的訂單／請求／回報／成交紀錄容量檢查，以及日誌容量與數量算術界限仍保留。索引能成長不代表整個 OMS 已變成無界系統；歷史輪替、OOM 處理、容量政策及完整資源壓力流程不在本次索引比較的完成範圍。Rust allocator 無法配置時可能終止程序，這版沒有聲稱能在實體記憶體耗盡後照常處理回報。

已驗證的是：初始化後的指定查詢與同價成交路徑無配置／釋放；AVL 已配置區塊內的換價可重用節點。單張等待集合、Group 排程、檔案 WAL 編碼與同步寫入等成本仍存在。

## 重現測試與量測

```sh
cargo test --offline
cargo test --offline --features pooled-index
cargo clippy --offline --all-targets --all-features -- -D warnings
cargo bench --offline --bench index_comparison > docs/results/index-comparison-local.csv
python3 scripts/summarize_index_bench.py docs/results/index-comparison-local.csv
```

預設每情境每輪 20,000 操作，5 輪交替 backend 執行順序；用 `OMS_BENCH_SAMPLES`、`OMS_BENCH_ROUNDS` 調整測試尺度，不改變系統容量契約。測試情境：

| 名稱 | 工作負載 |
|---|---|
| small | 4 訂單／4 價格／1 Book，同價數量更新 |
| crowded | 4,096 訂單／1 價格，同價數量更新 |
| many_prices | 4,096 訂單／4,096 價格，稀疏價位 |
| many_books | 1,024 訂單／128 Book，每 Book 8 價格 |
| churn | 128 有效訂單持續移到新價格，回收舊價格層 |
| growth | 從空索引逐筆長到 20,000 不同價格；未排除第一次配置 |
| engine | 1 訂單，分別計時立即修改、在途意圖保存、ACK 更新與延後送出 |
| engine_growth | 完整 Engine 從 0 新增 20,000 訂單，包含 ID maps／訂單 Pool 成長 |
| burst | 每 100 μs 同時到達 64 筆 fill，量測 CPU service 與預定到達至狀態可见時間 |

`burst` 的到達時間事先固定，完成時間包含相對到達排程的積欠；測試程式不會因處理變慢而少產生輸入。它是單事件迴圈的排程重播，沒有另一个真實 producer／socket／佇列，不能當端到端 transport 延遲。可用 `OMS_BURST_SIZE`、`OMS_BURST_PERIOD_NS` 修改到達負載。

時間 pass 關閉配置計數；另一次獨立 pass 計算被測函式內的 alloc／realloc／free。初始化、測試輸入與樣本容器在量測外準備；growth 情境本身的配置仍在量測內。計時器成本不扣除。block_growth_subset 只標示 Pool 新增區塊的操作，不代表所有 B-tree／HashMap 配置事件。

本次在 Darwin arm64 / Rust 1.94.1 本機 release 執行，未固定 CPU、未控制背景負載、沒有 Linux 生產環境／NUMA／磁碟同步／網路量測。約 42 ns 已接近本機計時器基線，不能把它解讀成精確的操作耗時，或用它計算幾倍加速。各輪 p99 的中位數也不是跨輪合併 p99。所有 CSV 保留 p99.9 與最大值，不應只看摘要。
