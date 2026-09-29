# 分層環形滑動視窗價格定位器（新預設）

2026-09-29 起，`IndexBackend::default()`、`Engine::new` 與 `Engine::recover` 改用 `IndexBackend::SlidingWindow`，不受 `pooled-index`／`paged-index` feature 影響。前一個預設 `IndexBackend::Adaptive`（[自適應預設索引](adaptive-default.md)）和 B-tree（`IndexBackend::Standard`）仍可明確選擇。**WAL 格式、交易語意與公開查詢 API 都沒有改變**，同一份日誌可以用任何後端重播。

## 結論

目標是在 Rust 和 C++ 中找出目前最快的「價格 → Level handle」有序定位器。受測候選包括 B-tree、Pool 分頁、雙端排序陣列（VecDeque／平面陣列、AoS／SoA）、陣列↔B-tree 自適應、固定視窗，以及分層環形滑動視窗。

在本專案的目標情境（近價群 16–128 價、隨市價移動，外加少數遠價）中，**分層環形滑動視窗（128 格）在 Rust 與 C++ 都是整體最快**：

| 操作（每次 ns，批次中位數） | B-tree | 分頁 | 自適應 | **視窗 128** |
|---|---:|---:|---:|---:|
| C++ w32 get | 24.0 | 9.4 | 28.6 | **5.6** |
| C++ w32 set_working | 32.9 | 12.5 | 34.2 | **4.2** |
| C++ w32 reprice_to_top | 146 | 51.4 | 97.0 | **13.5** |
| C++ w32 roll | 89.7 | 38.0 | 40.5 | **9.7** |
| C++ 索引 rolling_128 query | 44.4 | 16.1 | 58.2 | **9.9** |
| Rust w32 get | 26.1 | 9.4 | 15.3 | **7.3** |
| Rust w32 set_working | 36.0 | 10.0 | 15.9 | **5.6** |
| Rust w32 reprice_to_top | 130.8 | 44.1 | 81.5 | **17.1** |
| Rust w32 roll | 108 | 29.3 | 60.2 | **13.1** |
| Rust 索引 rolling_128 query | 51.8 | 25.7 | 38.3 | **21.8** |

以 C++ `clustered_bench` 的逐次 p50 幾何平均（B-tree = 1.000）比較：分頁 1.044、排序陣列 1.046、自適應 1.112，**視窗 128 為 0.941**；p99 在多數情境最低或並列最低。

它不是每一格都贏，弱點如下（詳見[取捨](#取捨與何時改用其他後端)）：

- 4,096 個很小的 Book 查詢時，B-tree 略快。
- 很寬的 Book（≥512 價），或移動範圍比視窗寬時，分頁較快。
- `best` 需重算時要 4–6 ns，B-tree 與陣列約 2 ns。這只在最佳價快取失效時才會發生。
- Rust 的 `range` 迭代器開銷較大：w32 range8 為 39.4 ns，陣列為 14.9 ns。C++ 同一結構為 12.3 ns，比陣列（15.6–16.1 ns）快。

完整表格見 [量測摘要](sliding-window-results.md)。

## 設計

Rust 實作在 [`src/price_slide.rs`](../src/price_slide.rs)，C++ 在 [`cpp/include/slide_locator.hpp`](../cpp/include/slide_locator.hpp)，兩者結構相同。

- **視窗**：N = 128 格，以 `u128` 當 occupied／working 兩張位元圖，另有 `[Handle; N]` 槽位。
  - 槽位以 `price mod N` 定址，視窗涵蓋 `[base, base + N)`。
  - 滑動時只改 `base`，不搬移槽位。
  - 不變式：價格在視窗範圍內時，就一定在視窗中；外部定位器永遠不會存放這個範圍內的價格。
- **滑動**：新價格落在視窗外但距離 ≤ N/2 時，視窗移動到讓新價格那側保留 N/4 空間。
  - 移出視窗的價位逐一移到外部定位器。
  - 外部定位器中落入新範圍的價位被拉進視窗。
  - 距離以無號差值、`i128` 和飽和運算計算，`i64` 兩端極值也不會溢位。
- **遠價（outlier）**：距離超過 N/2 的價格直接放入外部定位器，不移動視窗。
  - Rust 外部定位器為 `AdaptiveLocator<FlatDeque>`：平面雙端排序陣列，超過 1,024 價轉 B-tree。C++ 為 `AdaptiveLocator<>`。
  - 回補時刪單再下單到很高或很低的價格，只會進入外部定位器，不會把視窗拉走。
- **查詢**：
  - 視窗內的 get／insert／remove／set_working 都是 O(1) 的位元與槽位操作。
  - `best` 把 working 位元圖旋轉（`rotate_right(base mod N)`）成價格順序後取 `lz`／`tz`，再與外部定位器的最佳價比較。
  - `range` 依序走「視窗下方的外部段 → 視窗 → 視窗上方的外部段」。外部段只在 `outside.bounds()` 可能相交時才走訪。
- **分層（tier）**：Book 先只有外部陣列，價位超過 8 個時才建立視窗，並以觸發建立的價格為中心。
  - 小 Book 和多 Book 工作負載因此不必為每個 Book 付出約 2 KB 的視窗記憶體和初始化成本。
  - 加入分層前，new_books 第一次插入為 1,902 ns，加入後約 589 ns。
- **單次搜尋路徑**：`Index::update` 新增價位時改用 `find_or_insert`，並在建立時就帶入 working 旗標，取代原本「get → insert → set_working」的三次查找。
  - 這個改動套用到所有後端：Standard 用 `BTreeMap` entry API，Adaptive／SlidingWindow 各自實作，其他後端回退為 get＋insert。
  - 新建價位時只更新最佳價快取（`note_working`）。

Level、Member、訂單 Pool、同價差異更新與最佳價快取都沒有改動。

### 注意：tick > 1

價格是原始單位。若商品 tick 大於 1，128 格只涵蓋 128 / tick 個 tick。目前沒有依 tick 縮放；對 tick 很大的商品，視窗等效寬度會變窄，更多價位會落到外部陣列。結果仍然正確，只是較慢。

## 量測方法

- **環境**：Intel Xeon 2.8 GHz KVM VM，4 vCPU，kernel 6.18.44-fc-v49，rustc 1.94.1，clang 18.1.3，Abseil `d9e4955`。
  - 計時程式以 `taskset -c 2` 固定 CPU，在閒置容器中依序執行。
  - 建置與雜湊資訊見 [metadata.json](results/sliding-window-2026-09-29-linux/metadata.json)。
- **批次 micro**（新增）：Rust `benches/locator_micro.rs`、`benches/index_micro.rs` 與 C++ `cpp/micro.cpp`。
  - 每批 256 次操作計一次時，共 160 批，取「每次操作 ns」的中位數與 p90。
  - 各後端每輪輪替並反轉執行順序，C++ 12 輪、Rust 12 輪。
  - 這樣能攤平計時器開銷與單次中斷，看出幾 ns 的差異。
- **逐次分位數**：沿用 C++ `clustered_bench`／`linear_bench`（14 輪），以及 Rust `index_comparison`／`clustered_index`，用來觀察 p99 尾端延遲。
- **為何需要批次量測**：在 VM 上，Rust 逐次 p50 的差距常小於雜訊。同一份程式只因程式碼對齊不同，p50 就能差 10–20%。
  - 因此本文件以批次中位數判斷排名，逐次 p99 只用來確認尾端延遲沒有變差。
  - 原專案在 Mac 上的量測使用 24 MHz 計數器，每 42 ns 才前進一格，所以無法區分小於約 40 ns 的差異。這是先前結論「B-tree 最快」無法細分排名的原因之一。

## 量測結果

主要數字見上方[結論](#結論)，完整表格見 [量測摘要](sliding-window-results.md)。重點：

- **C++ 定位器層**：
  - 16–128 價的 get、set_working、reprice、roll，視窗 128 在所有候選中最快或並列最快。唯一例外是 w128 roll：分頁 37.5 ns，視窗 41.7 ns。
  - aggressive_in_out（遠方新最佳價進出）為 35.5 ns，和自適應（37.3 ns）同級，比 B-tree（93 ns）和分頁（212 ns）快。
- **C++ 索引層**：
  - rolling_32 update：55.3 ns（B-tree 78.6、分頁 57.3、自適應 103.6）。
  - rolling_32 query：9.4 ns（B-tree 33.1）。
  - 4096books query：89.6 ns，B-tree 74.6 ns 最快。
  - new_books：658 ns，與 B-tree（684 ns）相當。
- **Rust 定位器層**：趨勢與 C++ 相同。w32 get 為 7.3 ns（B-tree 26.1），reprice 為 17.1 ns（B-tree 130.8）。
- **Rust 索引層**：
  - query 全面最快：rolling_32 為 17.1 ns，B-tree 41.2 ns。
  - update 差距較小：在安靜時段量到 rolling_32 視窗 64.6 ns、B-tree 68.0 ns、自適應 75.4 ns。定稿那輪 update 整體偏高（108–121 ns），排名相同但差距在雜訊內。
  - Rust `clustered_index` 逐次 p50 幾何平均為：B-tree 1.000、分頁 1.087、自適應 1.287、視窗 1.129。這組數字受對齊與雜訊支配。**我們不宣稱 Rust 索引更新一定比 B-tree 快**，只能說同級，且查詢明顯較快。

## 試過但沒有勝出的方案

| 方案 | 結果 |
|---|---|
| 固定（不滑動）視窗 64／256 | 近價群一旦漂移出視窗就全部落入外部，退化成外部陣列，已刪除 |
| 視窗 64 格（`u64`） | w16–w32 與 128 格同級，但 w128 get 為 24.7 ns，128 格為 7.3 ns；128 格整體較好 |
| `VecDeque` vs 平面雙端陣列 | 互有勝負；Rust 外部定位器採平面陣列（連續記憶體、`partition_point`） |
| 陣列線性搜尋門檻 16／32／64 | 64 較差；維持 16，超過改二分搜尋 |
| 手寫無分支二分搜尋 | 512 價時 49.8 ns，`partition_point` 為 18.3 ns，已放棄 |
| Pool 分頁 | 寬 Book 和 roll 很強，但遠價要分配新頁，多 Book 與成長成本高（4096books query 145 ns） |

## 取捨與何時改用其他後端

- **大量小 Book**（每 Book ≤8 價）：視窗不會建立，行為等同外部平面陣列，與 B-tree 同級。若查詢極多，可考慮 `Standard`。
- **很寬的 Book**（≥512 連續價），或近價群移動範圍大於 128 個原始價格單位：`PooledPages` 較快。
- **需要大量有序走訪**的 Rust 使用者：`range` 迭代器每次多約 20 ns，但 Engine 只在查詢 API 走訪。
- **記憶體**：每個建立了視窗的 Book 多約 2 KB（128 × 16 B handle＋兩張位元圖），外加外部陣列。

## 驗證

- **Rust 單元測試**：
  - `price_slide` 以 64 與 128 格、4 種設定做隨機漫步，對照 `BTreeMap` 參考模型，並斷言視窗與外部定位器都有被使用。
  - rolling book 測試確認移動的近價群一直留在視窗內。
  - `price_deque` 的測試改為對 `VecDeque` 與 `FlatDeque` 泛型化，`find_or_insert` 也對照參考模型。
- **Engine 測試**：
  - `tests/price_index.rs`、`tests/default_index.rs` 的後端清單加入 `SlidingWindow`。
  - 預設後端測試在所有 feature 組合下斷言預設為 `SlidingWindow`。
  - 新增 `default_index_follows_a_drifting_cluster`：40 張單加上兩個遠價（10 與 -1,000,000,000），跑 3,000 步。每步把最上方的單改到最下方減一並 ACK，途中會跳過價格 10。每步都檢查最佳價、摘要、完整區間與部分區間，最後驗證恢復。
- **C++ 測試**：`slide_walk<64/128>` 隨機漫步對照 `std::map`，並把 `SlideLocator` 加入 locator／index／cancel_rehang／flow／sorted_deque_edges 各組，共 45 組全部通過。
- **變異測試**：
  - Rust 滑動邏輯的 4 個核心變異都被單元測試與 Engine 測試抓到。「多移出一格」和「關閉分層」兩個變異是等價變異，不影響正確性。
  - C++ 5 個變異全數被抓到。
  - `FlatDeque` 4 個變異被抓到，另 2 個為等價變異。
- **Sanitizer**：C++ 全部測試在 ASan（g++）與 UBSan（clang trap 模式）下通過。Sanitizer 建置下隨機漫步縮短為 3,000 步。

### Sanitizer 建置備註

專案的 `OMS_SANITIZE` 只對本專案 target 加上 instrumentation。Abseil `raw_hash_set` 因而出現 ABI 不一致，在 ASan 下 SEGV。要檢查 Abseil 以外的程式碼，請改用全域旗標：

```sh
# ASan
CXX=g++ cmake -S cpp -B cpp/build-asan -DCMAKE_BUILD_TYPE=Debug \
  -DCMAKE_CXX_FLAGS="-fsanitize=address -fno-omit-frame-pointer"
cmake --build cpp/build-asan --target index_tests && ./cpp/build-asan/index_tests
# UBSan（不需 runtime）
CXX=clang++ cmake -S cpp -B cpp/build-ubsan -DCMAKE_BUILD_TYPE=Debug \
  -DCMAKE_CXX_FLAGS="-fsanitize=undefined -fsanitize-trap=undefined"
cmake --build cpp/build-ubsan --target index_tests && ./cpp/build-ubsan/index_tests
```

## 過程中修正的錯誤

- **雙端陣列中間插入**（Rust `FlatDeque` 與 C++ `SortedDeque` 都有）：較短一側已滿時，原本會觸發搬移長側。512 價 reprice 因而為 2,781 ns。
  - 改為先 `make_room` 再重算插入位置，修正後 512 價 reprice 約 106–118 ns。
  - 這個錯誤會影響先前 [sorted-deque.md](sorted-deque.md) 的寬 Book 數字，但不影響正確性。

## 限制

- 所有數字都來自單一 KVM VM，在其他 CPU 上排名可能不同，特別是 `best` 這類幾 ns 的差距。
- Rust 逐次 p50 受程式碼對齊影響，排名以批次 micro 為準。
- Rust `range` 迭代器開銷尚未最佳化。
- 視窗寬度以原始價格單位計，沒有依 tick 縮放（見上文）。

## 重現

```sh
T=$(date +%F)-linux
# Rust
OMS_BENCH_ROUNDS=12 taskset -c 2 cargo bench --bench locator_micro > rust-locator-micro.csv
OMS_BENCH_ROUNDS=12 taskset -c 2 cargo bench --bench index_micro   > rust-index-micro.csv
OMS_BENCH_ROUNDS=10 taskset -c 2 cargo bench --bench index_comparison > rust-index-comparison.csv
OMS_BENCH_ROUNDS=12 taskset -c 2 cargo bench --bench clustered_index  > rust-clustered-index.csv
# C++
cmake -S cpp -B cpp/build -DCMAKE_BUILD_TYPE=Release && cmake --build cpp/build -j
OMS_BENCH_ROUNDS=12 taskset -c 2 ./cpp/build/micro > cpp-micro.csv
python3 scripts/run_cpp_bench.py   # clustered_bench / clustered_alloc，預設 14 輪
python3 scripts/run_cpp_linear.py  # linear_bench / linear_alloc，預設 14 輪
# 摘要
python3 scripts/summarize_micro.py \
  "C++ micro" cpp-micro.csv absl_btree,pool_pages,sorted_deque,adaptive,window64,window128 \
  "Rust 定位器 micro" rust-locator-micro.csv btree,pages,vecdeque,flat,window64,window128 \
  "Rust 索引 micro" rust-index-micro.csv standard,pages,adaptive,window
```

本次原始資料在 [results/sliding-window-2026-09-29-linux](results/sliding-window-2026-09-29-linux/)。
