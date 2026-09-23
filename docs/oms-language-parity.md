# Rust / C++ 同功能單張 OMS 比較

依本次確認範圍，C++ 新增完整單張流程移植；Rust 使用既有 `Engine<MemoryJournal>`，沒有另寫一個精簡 Rust OMS。群組與檔案 WAL 不在本次移植範圍。既有 C++ `Flow` 的簡化數字不再拿來和完整 Rust Engine 比較。

## 本次結果

2026-09-22，本機 Darwin arm64，Rust 1.94.1 / LLVM 21.1.8、Apple Clang 17。六輪、兩個 backend，每操作每輪 20,000 筆。以下是 **standard backend、32 張有效訂單**的各輪 p99 中位數，ns：

| 操作 | Rust | C++ | 本次觀察 |
|---|---:|---:|---|
| 單價查詢 | 42 | 125 | Rust 的單次尾延遲較低，42 ns 接近計時基線 |
| 立即意圖 → 待送命令 | 687.5 | 292 | C++ 約為 Rust 耗時的 42% |
| guarded ACK → 狀態可見 | 1,208.5 | 1,041.5 | C++ 約為 Rust 耗時的 86% |
| 等待意圖 → 待送命令 | 667 | 292 | C++ 約為 Rust 耗時的 44% |
| 在途意圖保存 | 292 | 125 | C++ 較低 |
| 完全相同回報重傳 | 84 | 125 | Rust 較低 |

價格查詢的 batch64 輔助量測呈現不同面向：32 單 standard 的整批 p50 / 64 為 Rust 7.81、C++ 7.16 ns。這是重複暖查詢平均成本的參考，不能代替在 OMS 事件迴圈內量到的單次 p99，也不能從 batch 平均反推 C++ 查詢尾延遲較低。

**尾部不能忽略。** standard、1,024 張單時，立即意圖的 p99 是 Rust 750 / C++ 417 ns，但 p99.9 是 Rust 833 / C++ 1,396 ns；ACK 的 p99.9 是 Rust 792 / C++ 1,333 ns。C++ 在本次多數命令路徑 p99 較低，並不等於所有尾分位數更穩定。

同一個 process 依序量 4、32、128、1,024 單，早期小 Book 情境的數個寫入 p99 反而较高；目前沒有 profiler 證據能把它完全歸因於 page fault、allocator 重用或 CPU/cache 狀態。記憶體 reserve 不等於 prefault，且後續工作負載會受到先前初始化影響；**不要用這份表推論「訂單越多就越快」或切換門檻**。跨語言的同一情境才是本次直接比較單位；是否隔離每個情境為新 process / 預先觸頁，應另做明確控制的實驗。

結論是本機的這份 **C++ 同功能實作在多數命令／更新路徑 p99 較低，Rust 在單價 query 及部分 p99.9／重傳路徑較低**。尚未替換 Rust 預設，也沒有以此宣告某語言普遍較快。群組／檔案 WAL 未移植，完整產品與極端資源壓力保證仍超出這次範圍。

可稽核資料：[Release 比對](results/oms-parity-validation-2026-09-22.json)、[UBSan 比對](results/oms-parity-ubsan-2026-09-22.json)、[編譯與每輪摘要](results/oms-parity-metadata-2026-09-22.json)、[原始 CSV](results/oms-parity-time-2026-09-22.csv)。

## 對齊內容

| 功能 | 本次 C++ / Rust |
|---|---|
| New / Replace / Cancel | 同版本檢查、數量限制、request ID 唯一性、終結狀態限制 |
| 一單一筆在途 | 已接受的後續意圖覆蓋 desired；Pending 保留原線上請求 |
| 最新意圖、取消鎖定 | 同 superseded／not-needed／unexecutable 規則；Cancel 不能被 Replace 撤回 |
| 等待派送 | 掃描 unsent waiting 集合，包含 blocked／in-flight；ready cancel 優先，再按 OrderId |
| 風控 | 每 Book reserved／uncertain、原／新最大曝險、idle 檢查與 deferred 再驗證；權威 ACK 超限仍記錄，允許降低曝險與取消 |
| 歷史 | 保留 requests、request results、reports、executions、source sequence、最新 intents |
| 回報 | 接受、改單、取消、拒絕／原因、失效、成交、成交更正／撤銷、reconciliation |
| 去重 | 相同 source/sequence 重傳、跨 source 的 request result／execution duplicate、衝突重複拒絕 |
| 回報入口 | `on_report` 發現錯誤時隔離 open orders，避免继续派送 |
| 日誌 | 有容量上限、預先 reserve 的 MemoryJournal，先 append 再提交狀態，append 失敗鎖住 halted |
| 恢復 | 從記憶體事件重播，不派送；對未結束訂單持久化 uncertainty 邊界 |
| 價格查詢 | BookHandle、Level／Member Pool、最多兩個價格 contribution、差異更新、best cache、range、pending 不當 confirmed |
| 後端 | standard：Rust std B-tree / C++ Abseil B-tree；pages：相同 64 格 sparse page + Pool + ordered directory 邏輯 |

新程式為 `cpp/include/aligned_model.hpp`、`aligned_engine.hpp`，不是在舊的簡化 `Flow` 補幾個空函式。Index 改為可接收 Order type，直接讀完整 Order，沒有每次轉換成縮減 snapshot 的額外成本。原有 C++ 索引候選仍可編譯與執行。

單張錯誤分類對齐，C++ 目前以 enum exception 表示內部錯誤；Rust 回傳帶上下文的 `Result<Outcome, Error>`。錯誤訊息文字／完整診斷 payload 及對外 ABI 並非逐位元相同。

## 驗證方式

`tests/data/oms-parity.trace` 是兩邊讀取的同一份文字輸入；包含 1,202 步、定向邊界情境與 80 個固定種子的完整訂單週期。對每一步比較：

1. 成功／錯誤分類和 Outcome（版本、意圖 revision、duplicate、outbound 完整內容）。
2. 所有已知 Order、Request、Execution 的欄位與最新 Intent 狀態。
3. 全部 Book 的 reserved／uncertain／best、排序價格層的六種統計、成員數和排序後 OrderIds。
4. MemoryJournal 長度及每筆事件的標準化內容。

摘要為對固定小端 u64 序列計算 FNV-1a；不讀記憶體 padding、不依賴 HashMap 走訪順序或語言 Debug 格式。摘要比對是大量差異測試，不是對任意輸入的形式證明；C++、Rust 日誌的**邏輯事件**一致，不代表 FileJournal bytes 相容。

兩個後端都逐步一致，Release 與 UBSan 版本皆通過。Rust 原有 98 個測試、C++ 原有 17 組索引測試、parity example 的 Clippy 皆通過。UBSan 另跑縮小 benchmark。ASan 先前在此環境停於初始化，本次沒有宣稱取得 ASan 通過結果。

## 量測方式

- Rust 使用新的 `parity` profile：`opt-level=3`、`lto=false`、`codegen-units=1`。正式 Rust `release` profile 原有 Thin LTO 設定不變。
- C++ 使用 Release `-O3 -DNDEBUG`，沒有 LTO、sanitizer 或 allocator 計數。
- 兩者都沒有 `target-cpu=native` 額外設定；compiler／LLVM 版本與實際 C++ compile/link flags 記在 metadata。
- 每輪 standalone process，按輪次交替 Rust/C++ 及 standard/pages 先後；六輪。
- 每輪每操作 20,000 筆；4／32／128／1,024 張有效訂單，同一個 Book，價格持續近／遠切換。
- 同一次迴圈：價格查詢 → 立即 Replace → 在途 Replace → 再次覆蓋 Replace → first ACK → exact duplicate ACK → dispatch latest → second ACK。各段分開計時；第二次 ACK 為準備下一輪，沒有放進第一筆回報延遲。
- 上述 workload 每次處理一張單，雖然 Book 有多張有效單，waiting 集合並不是同時積壓 1,024 個意圖的壓力測試。拒絕與錯誤路徑在正確性比對涵蓋，沒有額外宣稱其效能尾分位數。
- `guarded_report_visible` 呼叫實際 `on_report`，包含正常入口、去重／序號、reducer、記憶體日誌、歷史、索引更新、settle。沒有省略先前 C++ Flow 缺少的部分。
- `intent_to_command` 包含驗證、記憶體日誌、歷史、風控及 indexed pending；`ready_to_command` 包含相同等待集合扫描與 dispatch 驗證。兩邊讀取 expected version、組裝測試事件都在計時外。
- queued／coalesced 也寫日誌與 request history，兩者量相同語意。
- 另量 20,000 筆獨立成交：新 execution、跨 source business duplicate，以及 exact transport duplicate。
- 為縮小約 42 ns 計時粒度的影響，另量每批 64 次 query；僅以整批 p50 / 64 輔助平均成本分析，不當作單次 p99。
- 每個效能工作負載完成後比對最終狀態與**完整邏輯日誌**摘要。驗證資料結構的維護移到計時迴圈外；測試目標資料準備不計時。
- requests／results／reports／executions／journal 的邏輯容量、reserve 時機及輸入完全相同。物理容量和 allocator 行為依各自容器而異。未計初始化成本，未測新 Book／Pool 擴容風暴，也沒有真實 producer 佇列、socket 或同步磁碟。
- 這次只有時間 pass；CSV 共用格式中配置欄的 0 不代表零配置。

## 還有哪些差異

這是**功能、狀態轉換及工作負載對齊**，不能聲稱控制了只剩語言一個變因：

- Rust std `HashMap` 與 C++ Abseil `flat_hash_map` 都是平坦雜湊容器，但雜湊函式、成長與實作不同；Book registry C++ 仍使用既有 `unordered_map`，Rust 使用 std HashMap。
- Rust std B-tree 與 C++ Abseil B-tree 的節點容量與算法細節不同；page directory 也有這個差異。
- Rust Index 用 enum 做 backend 分派；C++ 用模板靜態分派。
- `Option`、enum、`std::optional`、`std::variant` 的布局不同。實測 Rust/C++：Order 152/176 bytes、Request 40/48、Report 88/96、Event 96/120。MemoryJournal 每筆實際寫入大小因此不同，沒有任意 padding 或裁掉 Rust 欄位來美化結果。
- C++ exceptions 與 Rust Result、邊界／handle 檢查編譯、不同版本 LLVM 都可能影響機器碼。
- C++ 不實作 Group 所有權與排程；這次所有訂單均為單張，Rust Group maps 也為空，但其空 ownership hook 成本仍可能存在。
- 本次沒有新增記憶體耗盡後的 rollback／強例外安全機制。不可把 bounded MemoryJournal append failure 的測試，外推成所有 allocator failure 都可以捕捉後繼續交易。

因此數字回答的是「**本機、本建置設定、這兩份同功能單張實作，哪個工作負載較快**」，不是所有 C++ 一定勝過所有 Rust。若要隔離語言生成碼成本，還需相同容器實作、布局與分派方式的受控 kernel 實驗；不能從這次結果直接推論。

## 重現

```sh
python3 scripts/generate_oms_parity_trace.py
cargo build --offline --profile parity --example oms_parity
cmake -S cpp -B cpp/build -DCMAKE_BUILD_TYPE=Release -DCMAKE_CXX_COMPILER=/usr/bin/clang++
cmake --build cpp/build --target oms_parity index_tests -j4
python3 scripts/check_oms_parity.py
python3 scripts/run_oms_parity.py --tag YOUR_UNIQUE_TAG
python3 scripts/summarize_oms_parity.py docs/results/oms-parity-time-YOUR_UNIQUE_TAG.csv
```

runner 先做兩個後端逐步驗證，再量測；拒絕覆寫既有結果，保存編譯資訊、來源／執行檔雜湊及每輪的 workload digest。

完整數字見 [量測摘要](oms-language-parity-results.md)。
