# B-tree 預設索引定案驗證

2026-09-23。`Engine::new`、`Engine::recover` 與 `IndexBackend::default()` 現在一律使用 `Standard`（標準 B-tree 加分段 Pool），不受 `pooled-index`／`paged-index` 編譯 feature 影響。兩個 feature 名稱保留供既有建置指令使用；實驗索引仍可由 `new_with_index`／`recover_with_index` 明確選擇。C++ 比較程式的後端選擇與先前測得的 Hash＋B-tree 結果沒有改動。

價格索引的實際儲存是 `Price → LevelHandle` 的有序 B-tree，加上一棵只記錄有已確認剩餘量價位的 B-tree set。`Level`、`Member` 在可成長分段 Pool 中，訂單的 Membership 可以直接找到 Level；Book 快取最佳 working price。沒有設定最大價格距離或最大價位數；Engine 的既有容量與風控 Limits 仍獨立生效。

新增[預設後端整合測試](../tests/default_btree.rs)：檢查遠價改單在途時不提前成為最佳價、後續最新意圖在 ACK 後才送出、完全重複回報不重計、連續成交使最佳價位消失後切換到下一價，以及預設 recovery 重建索引並讓舊 BookHandle 失效。每種 feature 組合都執行相同測試。既有價格索引測試另外涵蓋極端 `i64` 價格、513 個相距很遠的價位、Pool 擴充、range、best 與恢復。

驗證命令已通過：

```sh
cargo fmt --all -- --check
cargo test --offline
cargo test --offline --features pooled-index
cargo test --offline --features paged-index
cargo test --offline --all-features
cargo clippy --offline --all-targets --all-features -- -D warnings
```

再次執行 Rust `index_comparison` benchmark，Apple arm64／Rust 1.94.1，release profile，每情境每輪 20,000 筆，三輪交替後端執行順序；計時與配置分開執行。結果為各輪 p99 中位數，單位 ns：

| 情境／操作 | B-tree | Pool AVL |
|---|---:|---:|
| 4 個價位／精確查詢 | 84 | 84 |
| 4,096 個價位／精確查詢 | 125 | 209 |
| 20,000 筆價位成長／索引更新 | 375 | 1,000 |
| 完整 OMS／立即意圖到命令 | 667 | 625 |
| 完整 OMS／改單回報可見 | 458 | 458 |
| 完整 OMS／待送意圖到命令 | 667 | 708 |
| 每批 64 筆突發成交／單筆服務時間 | 416 | 416 |
| 同批突發成交／預排到達至可見 | 23,166 | 23,166 |

該突發情境的每批預排間隔為 100,000 ns；「預排到達至可見」包含單一事件迴圈內的等待，這個模擬不包含真實網路或訊息佇列成本。所有單次計時都含約 42 ns 的計時器基線；84 ns 附近差異不宜解讀成可靠倍率。上述突發到達延遲同樣不是正式環境的端到端 SLA。

成長測試的 B-tree p99.9 為 708 ns；完整 OMS 新單到命令的 p99.9 為 2,292 ns，且各輪最高值的中位數約 473 µs。這些最大值含配置／排程干擾，沒有固定最壞延遲保證。獨立配置 pass 顯示成長操作在 B-tree 共配置 7,309 次；它包括 Level/Member Pool、目錄與其他更新配置，不等於常駐記憶體或索引單一結構的成本。

較早的[近價群＋遠價實驗](clustered-index.md)仍顯示稀疏分頁在部分近價情境較快；[Hash＋B-tree 比較](price-dual.md)顯示熱資料批次查價更快，但沒有改善完整 OMS p99，且 hash 擴容有尖峰。因此 B-tree 的定案是**目前綜合延遲、價位分布及結構維護成本最合適的預設**。當正式工作負載的 profiling 證實價格查詢或最佳價切換是主要瓶頸，才需要重開索引選型。

重現：

```sh
OMS_BENCH_SAMPLES=20000 OMS_BENCH_ROUNDS=3 cargo bench --offline --bench index_comparison > docs/results/btree-default-2026-09-23.csv
python3 scripts/summarize_index_bench.py docs/results/btree-default-2026-09-23.csv > docs/btree-default-results.md
```

完整 p50／p99／p99.9／max、輪間範圍與配置次數見[量測摘要](btree-default-results.md)及[原始 CSV](results/btree-default-2026-09-23.csv)。重跑時請使用新的檔名保存結果。
