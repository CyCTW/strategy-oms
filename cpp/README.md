# C++ 價格索引實驗

這是針對價格索引的 C++20 移植與效能比較，並包含完整單張 OMS 的語意對齊版本；群組與檔案 WAL 仍留在 Rust。Rust 預設後端固定為 B-tree，定案驗證見[說明](../docs/btree-default.md)。

2026-09-22 另新增 [與 Rust 對齊的單張 OMS](../docs/oms-language-parity.md)：`aligned_engine.hpp` 實作 MemoryJournal、去重／歷史、風控、latest intent、回報與恢復；以相同 trace 驗證 1,202 步，再跑兩個 backend、六輪比較。此版本的 executable 為 `oms_parity`；Group、FileJournal 仍不在移植範圍。舊的 `Flow` 仍只作簡化索引實驗，兩者數字不可混用。

另已完成 [線性搜尋比較](../docs/cpp-linear.md)：新增 `LinearLocator`（掃描價格層目錄）與 `OrderScanIndex`（掃描權威有效訂單），與 Abseil B-tree 進行六輪同語意比較。原始資料及小／大 Book、查詢比例、成長路徑的取捨見 [線性測試結果](../docs/cpp-linear-results.md)。新增 targets 為 `linear_bench`、`linear_alloc`；完整指令見該說明。共用 Flow 模板可替換 Index type，仍屬簡化原型。

## 比較範圍

| 層次 | 已實作 | 未涵蓋 |
|---|---|---|
| Index | 六種數量統計、每價訂單成員、最多兩個價格 contribution、差異更新、BookHandle、最佳確認價、range、跨 Book 共用 pools | 對外交易協定、持久化、完整資源壓力處理 |
| B-tree 後端 | Abseil btree_map 價格目錄＋btree_set 已確認價格 | 樹節點自身的自訂 Pool allocator |
| 分頁後端 | Abseil btree_map 頁目錄＋btree_set 已確認頁、64 格頁面、occupied/working bitmap、共用 PagePool | tick 表轉換、熱區快取、自動調整頁面大小 |
| Flow 原型 | 每單一筆 pending、最新 Replace 意圖合併、版本檢查、關聯 ACK／連續序號檢查、ready dispatch | WAL、report/request 去重及歷史、完整風控、恢復、Group、成交更正、完整拒絕／不確定狀態契約 |

`Flow` 的結果以 `prototype_*` 命名，不能與先前 Rust `engine_*` 的數字直接比較。它只用來比較兩個 C++ 索引後端在同一簡化流程中的影響。

兩個後端使用相同的 Pool、Level、Member 及 Index delta 更新；頁目錄也使用 Abseil B-tree，不是 std::map。`std::map` 僅作為測試參考模型。Abseil 固定版本與授權見 [DEPENDENCIES.md](DEPENDENCIES.md)，CMake 整合方式參考 [官方文件](https://abseil.io/docs/cpp/quickstart-cmake)。

## 檔案

- `include/oms_index.hpp`：Index、兩個 locator、分段 Pool 與 generation handles。
- `include/flow.hpp`：簡化的 latest Replace 意圖／回報測試流程。
- `include/measure.hpp`：計時、配置計數及分位數輸出。
- `tests.cpp`：獨立重建參考模型、隨機差異測試、頁面／整數邊界、Pool 重用與在途情境。
- `bench.cpp`：近價移動、近遠價並存、跨頁、跳價、多 Book、持續成長與撤近掛遠。

## 建置與測試

初次取得依賴請依 [DEPENDENCIES.md](DEPENDENCIES.md)。以下從專案根目錄執行；下載完成後可離線建置。

```sh
cmake -S cpp -B cpp/build -DCMAKE_BUILD_TYPE=Release -DCMAKE_CXX_COMPILER=/usr/bin/clang++
cmake --build cpp/build --target index_tests clustered_bench clustered_alloc -j 4
ctest --test-dir cpp/build --output-on-failure
```

Release 測試使用自訂 CHECK，不會因為 NDEBUG 而失去斷言。每後端執行 20,000 筆 locator 與 10,000 筆 Index 差異轉換；參考模型由目前所有訂單重新聚合，並獨立列舉成員與最佳價。另驗證極端負／正價格、range 截止、買賣方向、不確定狀態預留量、舊 handle 失效、跨 Index handle、撤單與遠價新單並存、最新意圖覆蓋及過期版本拒絕。

UBSan 檢查：

```sh
cmake -S cpp -B cpp/build-ubsan -DCMAKE_BUILD_TYPE=Debug -DCMAKE_CXX_COMPILER=/usr/bin/clang++ -DOMS_SANITIZE=ON -DOMS_SANITIZERS=undefined
cmake --build cpp/build-ubsan --target index_tests -j 4
UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 ctest --test-dir cpp/build-ubsan --output-on-failure
```

AddressSanitizer＋UBSan 可將 OMS_SANITIZERS 改為 `address,undefined` 並使用另一個 build 目錄。Sanitizer 目標使用 -O1 -g，效能量測只用不含 sanitizer 的 Release。

## 效能量測

```sh
python3 scripts/run_cpp_bench.py --tag 2026-09-22
python3 scripts/summarize_cpp_index.py > docs/cpp-index-results.md
```

預設每操作 20,000 筆、五輪交替後端順序。再次執行需使用不同 `--tag`，避免覆寫資料，並將新 time/alloc CSV 路徑傳給摘要腳本。runner 記錄平台、編譯旗標、依賴 commit、來源／執行檔雜湊。

- `clustered_bench` 使用原始系統 allocator，沒有覆寫 new/delete。
- `clustered_alloc` 是另外編譯的執行檔，只計數不計時；覆寫 new/delete，包括 aligned 與 sized 形式，以 header 記錄 requested bytes。header 和 allocator 額外成本不計入 requested bytes，也不會進入計時執行檔。
- 不做 timer 扣除。保留 p50、p99、p99.9、max 及樣本數；表格列的是各輪分位數中位數，不是合併分位數。
- `index_*` 只含價格索引更新。撤近掛遠也屬 Index 測試，New ACK 在舊 Cancel ACK 之前，逐輪驗證風控預留 500→600→500；未聲稱已實作完整 Cancel/New API。
- `prototype_*` 才包含簡化 Flow 的 ID 查找、pending／desired、序號與 ready 集合處理，沒有 WAL 等完整 OMS 成本。
- 索引工作負載與 Rust clustered benchmark 採相同中心移動、價格距離與 old/pending/ACK 批次安排。但容器實作、編譯方式、物件布局、HashMap 成長及 dispatch 方式不同，不能將差值全歸因於程式語言。
- 1 原始價格單位等於 1 tick 僅是此次輸入設定；沒有接真實商品 tick 規則或漲跌停驗證。
- benchmark 的樣本數防呆上限 1,000,000 只限制一次實驗，並不是索引的價格／訂單容量上限。

## 使用界限

此元件為單一寫入者，借用指標／range callback 期間不能重入修改索引。Pool 擴充保持物件位址，generation 防止回收槽被舊 handle 存取；範例不向策略公開 raw pointer。

`Index::update` 是內部轉換介面，呼叫者必須持有一致的 old/new Order 與其 Memberships，不能更換同一筆訂單的 Book。並未實作每次更新的 bad_alloc 回滾；配置失敗或內部不變量失敗不保證強例外安全，不能捕捉錯誤後直接繼續交易。

每頁 64 格與每 Pool block 64 頁是不同粒度。頁塊保留到 Index 銷毀，頁目錄與 ID maps 仍可能配置。沒有宣稱無限資源、整套零配置或固定最壞延遲。

本次執行結果與 sanitizer 狀態見 [C++ 比較紀錄](../docs/cpp-index.md)。
