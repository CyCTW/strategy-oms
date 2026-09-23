# Strategy OMS

v0.4 已完成價格索引候選比較：預設為 **標準 B-tree＋分段 Pool＋差異更新**。Pool AVL 保留為實驗候選，因本機量測並未顯示其 p99 較好。詳見 [索引設計與 API 遷移](docs/price-index.md)、[量測結果](docs/index-benchmark-results.md)。

目前 `Engine::new` 與 `recover` 在所有 Cargo feature 組合下均使用 B-tree。最新的[定案驗證與重跑結果](docs/btree-default.md)涵蓋在途遠價改單、重複／突發回報、價位成長及三段延遲；實驗後端需明確透過 `new_with_index`／`recover_with_index` 選擇。

另已完成 [其他定位器方案探索](docs/locator-exploration.md)：比較小陣列混合索引、sorted Vec、Hash＋有序索引與稀疏分頁，並記錄升級尖峰及記憶體取捨。這些是獨立研究原型，未替換正式預設。

針對「移動近價群＋少量遠價」已新增可選的 `IndexBackend::PooledPages`，接入完整 Engine 測試與三段延遲量測。預設仍為 Standard；分頁在小 Book、多 Book 與擴容方面有明確取捨，見 [情境比較](docs/clustered-index.md)。

另有 [C++20 測試版本](cpp/README.md)，比較 Abseil B-tree 與 Pool 稀疏分頁，包含索引差異測試、UBSan 及可重跑的五輪量測。它移植價格索引並提供簡化流程原型，尚非完整 OMS 重寫；見 [C++ 結果與驗證限制](docs/cpp-index.md)。

C++ 另已完成 [線性搜尋實驗](docs/cpp-linear.md)，以六輪量測比較 B-tree、掃描價格層與直接掃訂單，包含同價聚合、查詢／回報比例、成長及簡化意圖派送。保留線性價格目錄為小 Book 候選，預設尚未替換。

最新的 [Rust / C++ 同功能單張比較](docs/oms-language-parity.md) 已將 C++ 補齊 MemoryJournal、歷史／去重、風控、latest intent 與回報／恢復，逐步比對 1,202 個輸入，並以一致輸入與編譯最佳化等級重測三段延遲。這不是將旧簡化 C++ Flow 與完整 Rust Engine 直接相比；Group／FileJournal 仍列為未移植差異。

新增 [Hash＋有序樹價格索引實驗](docs/price-dual.md)：沿用完整 C++ OMS，比較 B-tree、稀疏分頁與價格 hash＋B-tree；查詢收益、更新成本及擴容尖峰見 [六輪結果](docs/price-dual-results.md)。

Rust 單一寫入者訂單核心：管理多策略掛單、查詢價格層、處理撤改單與成交回報，並透過事件日誌重建狀態。v0.3 的單張 `apply(New/Replace/Cancel)` 已支援最新意圖：在途時保存更新，等回報後執行最新撤改單。Group 則負責多張子單的共同目標、拆單、補量及群組政策。兩者位於同一個 Engine。此版本使用模擬 Gateway，沒有連接真實交易帳戶。

## 執行

需要 Rust 1.94 以上，無第三方套件，可離線建置。

```sh
cd /Users/cyctw/project/strategy-oms
cargo test --offline
cargo run --offline --example quote_lifecycle
cargo run --offline --example single_order
cargo run --offline --example index_queries
cargo run --offline --example group_quotes
cargo bench --offline --bench latency
cargo bench --offline --bench groups_latency
cargo bench --offline --bench index_comparison
```

範例重現：買進 10 張 → 成交 3 張 → 改價 100 到 101 → 改單途中再成交 2 張 → 重複成交回報 → 改單確認 → 撤單 → 重播比對。

`single_order` 示範單張新單在途時連續修改 101→102→103，只送最新改價；改單在途時保存 Cancel，確認後撤掉剩餘量。完整契約見 [單張訂單使用說明](docs/single-orders.md)。

`group_quotes` 範例示範多張子單、群組目標合併、延遲回報、限速等待與 Stop 撤單。完整 API 與行為契約見 [群組目標使用說明](docs/groups.md)。

## 已實作

- 穩定訂單 ID、不可重用的請求 ID、成交帳與更正版本。
- 單張訂單最新意圖、在途更新合併、持續有效的 Cancel、延後送出與意圖狀態查詢。
- 可成長訂單／價格層／成員 Pool、穩定 BookHandle、唯讀查詢 facade、同價差異更新及最佳確認價快取。
- 新單／撤單／原子改單、接受／拒絕、部分及全部成交、失效、外部撤單、成交更正／取消。
- 新單確認前成交；撤改單期間成交；拒絕回報不回退成交。
- 固定精度整數價格；依策略、帳戶、場所、商品、方向區分價格簿。
- 查詢價格層、最佳有效價格、價格範圍、訂單及待確認請求。
- 回報去重、跨來源成交去重、連續序號檢查、過期命令版本檢查。
- 每價格簿的未結委託數量上限、待確認額度保留、容量限制。
- 記憶體日誌、檔案 WAL、校驗碼、單一寫入檔案鎖、重播與重啟隔離。
- 群組目標版本、有效期限、維持掛單量／總成交預算、多子單配置及保守曝險。
- Pause／Stop／Resume、拒絕阻擋、有限重試、請求逾時及恢復隔離。
- 跨群組固定窗口限速、撤單預留額度、最新狀態送出決策與原因查詢。
- 可排程延遲、重複、缺口與不明送出結果的 FaultGateway。

## 群組介面

```rust
use strategy_oms::{journal::MemoryJournal, *};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::new(MemoryJournal::new(10_000), Limits::default())?;
    let book = Book { strategy: 1, account: 1, venue: 1, instrument: 2330, side: Side::Buy };
    engine.create_group(1, book, GroupPolicy {
        max_child_leaves: 5, max_inflight: 2, ..GroupPolicy::default()
    })?;
    engine.tick(1_000)?;
    engine.set_target(1, 1, Some(Target {
        price: 100_00, qty: 10, quantity_mode: QuantityMode::MaintainLeaves,
        expires_at_ms: Some(2_000),
    }))?;
    // dispatch_next 會記錄在途請求並保留額度；回傳命令應立即交给 Gateway。
    // 回報用 on_report；每次輸入／timer 後再次驅動 dispatch_next。
    println!("{:?}", engine.explain_pending_action(1)?);
    Ok(())
}
```

## 單張訂單介面（最新意圖）

```rust
use strategy_oms::{journal::MemoryJournal, *};

let mut oms = Engine::new(MemoryJournal::new(10_000), Limits::default())?;
let book = Book {
    strategy: 1, account: 1, venue: 1, instrument: 2330, side: Side::Buy,
};
let outcome = oms.apply(Event::New(NewOrder {
    order_id: 1, request_id: 1, book, price: 100_00, total_qty: 10,
}))?;

// outcome.outbound 存在時，日誌已到達設定的持久化邊界。
// 呼叫自己的 Gateway::send；send 成功仍不代表交易所已接受。
// 策略通知應在 apply 返回後發布，callback 的新命令交回事件迴圈。
assert_eq!(oms.at_price(book, 100_00).unwrap().totals.pending_new_qty, 10);
assert_eq!(oms.reserved_qty(book), 10);
# Ok::<(), strategy_oms::Error>(())
```

`apply(Replace/Cancel)` 在途時會保存意圖、回傳 `outbound=None`，不再單純因 pending 而拒絕。查詢 `order_intent(id)` 可取得最新意圖版本及等待原因。

正式整合的事件迴圈：命令用 `apply`、回報優先用 `on_report` → 立即發送回傳的 `outbound`（若有）→ transport 可接收時驅動 `dispatch_next_order()`，逐筆立即傳送 → 發布結果。Group 仍使用 `dispatch_next()`。Gateway 送出結果不明時，呼叫 `hold_for_recovery()` 並啟動外部查詢，禁止盲目重送。核心沒有隱藏 callback、執行緒或網路副作用。

## 查詢語意

| API／欄位 | 語意 |
|---|---|
| `order(id)` / `request(id)` / `execution(key)` | 預期 O(1) 雜湊查詢 |
| `order_intent(id)` | 單張最新意圖、版本、request 狀態及等待原因；不把接受當交易所確認 |
| `register_book(book)` / `book_handle(book)` | 建立／取得僅屬於當前 Engine 的 BookHandle |
| `level_summary(handle, price)` | 直接以 Handle 查固定大小的 Totals 與訂單數，不走訪明細 |
| `orders_at_price(handle, price)` | 無配置的唯讀訂單 ID iterator；順序未指定 |
| `best_working_price_at(handle)` | 直接讀取最佳已確認價格快取 |
| `levels_in_range(handle, range)` | 按價格遞增走訪價格層 |
| `at_price(book, price)` | O(log P) 價格層查詢，不掃描全部訂單 |
| `best_working_price(book)` | 已確認且剩餘量大於零的最佳價，忽略待新單／待改入價格 |
| `price_range(book, low..=high)` | 有序價格層，包含待確認意圖；反向範圍回傳空值 |
| `PriceLevel.order_ids` | 此價格層涉及的訂單 ID 唯讀 iterator，可含已提交送出的在途請求；不保證 ID 排序 |
| `confirmed_leaves` | 最新已確認剩餘量，包含待撤部分 |
| `pending_new_qty` | 新單要求總量扣除已知成交量 |
| `pending_cancel_leaves` | 已確認剩餘量的子集合，不能再加一次 |
| `pending_replace_in/out` | 待改單目標價格／舊價格的數量，非額外兩張單 |
| `reserved_qty(book)` | 每張實體單的保守剩餘量之和，原子改單取舊／新量較大者 |
| `uncertain_orders(book)` | 尚待對帳的訂單數，包含終結後成交更正的訂單 |

`Order.version` 適用於該訂單的撤改單樂觀檢查，接受新意圖也會增加版本。未送出的 desired 不計入價格層與保守預留量，真正送出前重新檢查額度。跨執行緒請發布不可變快照，或自行設計帶有全域事件序號的讀取投影；目前查詢借用受 Rust 型別系統保護，只適用於核心擁有者同步讀取，沒有跨程序一致性保證。

## Gateway 必須遵守的契約

1. 每個 `Report.source` 是包含 session epoch 的無碰撞來源識別；`sequence` 是 **Adapter 正規化後從 1 開始的連續序號**，不是直接搬用包含心跳等訊息的 FIX MsgSeqNum。原始協定序號、缺口、重送由 Adapter 處理。
2. 外部委託 ID、ClOrdID／OrigClOrdID 及舊版本 ID 需保留映射。核心只保存最新的 `exchange_id`，不實作外部別名索引。
3. `ExecutionKey` 包含場所、帳戶、交易日及成交 ID；主回報與 Drop Copy 必須能對應到同一個 key。成交更正必須正規化為遞增 `revision`；不能用累計量直接覆蓋成交帳。
4. `Accepted`／`Replaced.total_qty` 是包含成交的總量。若場所回報剩餘量，Adapter 必須依該協定及已知成交轉換。原子改單的接受回報須在依賴新數量的成交之前進入核心；不符合時先恢復或正規化。
5. `Canceled` 表示整張單剩餘量歸零。部分刪量需正規化為實際修改後的條件。本版不處理特殊單型、交易所重定價、冰山可見量等市場特有語意。
6. `apply` 回傳序號缺口、矛盾回報或容量錯誤時，呼叫者必須停止正常送單，對受影響訂單套用 `MarkUncertain`，並啟動來源恢復／對帳，不能直接忽略錯誤後繼續交易。Journal 錯誤會由核心自行鎖定停止。
7. 失效或外部撤單可終止待確認操作。被終止的操作若後續又有回報，本版會回傳矛盾／關聯錯誤，由 Adapter 查核；不猜測交易所狀態。

## 持久化與恢復

```rust
use strategy_oms::{journal::{FileJournal, Durability}, Engine, Limits};

let (journal, events) = FileJournal::open("orders.wal", Durability::SyncEveryEvent)?;
let oms = Engine::recover(journal, &events, Limits::default())?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

- `FileJournal::create` 使用 create-new，拒絕覆寫；`open` 在獨占鎖內驗證日誌後繼續附加。鎖為合作式，不防止其他程式繞過鎖直接修改檔案。
- `SyncEveryEvent` 每筆呼叫 `sync_all`；`OsBuffered` 只寫入 OS 快取；`MemoryJournal` 完全不提供程序崩潰後的持久化。
- 實際掉電保證依檔案系統／儲存裝置決定。macOS 的完整裝置 flush、首次建檔的目錄持久化，以及副本容錯仍須依正式環境強化，不能宣稱零資料遺失。
- 寫入失敗可能發生於部分資料已寫入之後，因此核心停止運作，且不回傳可送出的命令。損壞或截斷的 WAL 拒絕自動開啟，必須先保存原檔、對帳並人工處理；不自動截尾。
- `recover` 重播既有事件，不重新發送命令；接著將未結單與待確認單的 `MarkUncertain`、所有群組的 `RecoveryHold` 邊界寫入原日誌。請傳入與 `events` 相對應的 journal；記憶體恢復使用 `MemoryJournal::from_events`。群組完成對帳後需明確 ReleaseRecovery，並保留原有停止模式。
- 先補足成交／請求結果，再輸入 `Reconciled` 快照。快照不得自行產生成交，也不得默默清除待確認請求。對帳完成前不能對該單撤改，同價格簿也不能新增訂單。
- 核心保留歷史訂單、請求、成交與回報直到設定的容量上限；Pool 可成長但不繞過 Limits。此版沒有快照壓縮、歷史清理、日誌輪替或線上修改 Limits。事件重播須使用與原始執行相容的核心 Limits；群組政策與 SchedulerConfig 已寫入日誌。

## 延遲與範圍

`cargo bench --bench latency` 分別量測計時器基線、固定價格查詢、最佳價查詢，以及成交回報到索引可見的延遲，輸出 p50／p99／p99.9。使用 1,000 張有效單、100 個價格層、單一價格簿與記憶體日誌；不包含網路、磁碟同步、並行生產者或交易所耗時。單次計時包含計時器開銷，不能當成端到端或正式 SLA。

本次結果與量測限制見 [本機量測紀錄](docs/benchmark.md)。

訂單、價格層與成員採分段 Pool；預設有序查找仍為 B-tree，支援實驗 Pool AVL。初始化後的查詢與同價成交已有無配置測試；Pool 擴充、ID maps、有序索引及檔案編碼仍可能配置，並非整個 OMS 零配置或固定最壞延遲。查詢不走資料庫。v0.4 的三段延遲、擴容與排程突發量測見 [比較結果](docs/index-benchmark-results.md)。

風控涵蓋每個群組及價格簿的掛單數量；群組送出另有本地固定窗口限速。不涵蓋帳戶部位、資金、名目金額、價格偏離、交易所真實限速或跨策略的完整帳戶額度。接真實市場前，需依 [設計文件](docs/design.md) 補足 Gateway、session 恢復、完整風控、行情、運維與市場相容性測試。
