# 雙端排序陣列與自適應價格定位器

2026-09-28。檢查「B-tree 是價格層最快作法」這個判斷，並在同一套 C++ 索引、測試與量測程式中加入三個新候選：

- `SortedDequeLocator`：雙端排序陣列
- `SortedDequeSoaLocator`：同上，但價格獨立成一個陣列（SoA）
- `AdaptiveLocator`：價位少時用雙端陣列，價位多時自動轉成 B-tree

**Rust 預設索引與 C++ 既有後端都沒有修改。** 這些候選只加進 C++ 實驗程式。（2026-09-29 後續：Rust 預設已改為自適應定位器，見 [自適應預設索引](adaptive-default.md)。）

## 先檢查原本的判斷

既有文件其實沒有說 B-tree「最快」。[定案驗證](btree-default.md) 的原文是「目前綜合延遲、價位分布及結構維護成本最合適的預設」，[C++ 比較](cpp-index.md) 的結論則是「保留 B-tree 作為通用基準」。所以要檢查的是：在這個 OMS 的主要情境下，也就是每個 Book 有少量近價價位加上少數遠價，B-tree 是否真的沒有更快的選擇。

既有比較有四個限制，會讓這個問題無法回答：

1. **計時解析度不足**。量測在 Darwin arm64 上進行，計時器每 42 ns 跳一格，多數結果落在 42／84／125。文件本身也寫明「不能計算精確倍率」，所以小 Book 的差異無法排出名次。
2. **排序陣列只測過大 Book**。Rust 的 [Flat sorted Vec](locator-exploration.md) 只在 4,096 個價位的重定價和成長到 20,000 個價位時測過，正好是陣列 O(P) 搬移最吃虧的情況。C++ 的 [linear_prices](cpp-linear.md) 則是**未排序**陣列，有序區間查詢要反覆全掃。兩者都不能代表「少量價位時的排序陣列」。
3. **缺少陣列的最壞情況**。`linear_bench` 的 `updates_*` 是依序號輪流移動訂單，陣列的新增和刪除永遠落在兩端，對陣列非常有利。
4. **ASan 沒有完成**（原因見文末）。

## 候選設計

所有候選都實作既有 locator 介面（`get / insert / remove / working / best / range`），共用同一套 Level/Member Pool、差異更新與最佳價快取。見 [sorted_deque_locator.hpp](../cpp/include/sorted_deque_locator.hpp)。

- **雙端排序陣列（AoS）**
  - 價格遞增排在一段連續緩衝區裡，**兩端都保留空位**。
  - locator 不分買賣方，所以買方最佳價在尾端、賣方最佳價在頭端；單端陣列只有一邊是 O(1)。雙端設計讓兩邊新增最佳價或最差價都是 O(1)。中間的新增和刪除往比較近的一端搬移。
  - 某一端空間用完時：還有至少 1/4 空位就把資料移回中間，否則容量加倍。
  - 沒有價位數或價格範圍上限，Handle 放在 Pool 裡，搬移不影響它。
  - 查找：16 個價位以內線性掃描，超過改用二分搜尋。
  - `best()` 只在最佳價快取失效時才會被呼叫，從最佳端往內跳過只有 pending 的價位。
- **SoA 版本**
  - 價格獨立成 8 bytes 的連續陣列。
  - 64 個價位以內，用「計算有幾個價格小於 p」的方式搜尋，沒有分支、可向量化；超過 64 個改用無分支二分搜尋。
  - 代價是搬移時要動兩個陣列。
- **自適應版本**
  - 超過 1,024 個價位時轉成 Abseil B-tree，低於 256 個時轉回陣列。兩個門檻不同，避免在門檻附近反覆轉換。
  - 轉換時要把所有價位複製一次，只有那一次操作會出現 O(P) 延遲尖峰。
  - 1,024 這個門檻來自本次 `reprice_random` 的交叉點，是實驗值，不是業務容量假設。

## 驗證

- `index_tests` 從 21 組增加到 38 組。每個新候選都跑既有的：
  - 20,000 步 locator 對照 `std::map`
  - 10,000 步 Index 對照重新聚合的參考模型
  - 撤單後重掛（cancel/rehang）、最新意圖（latest intent）
- 另外新增兩個測試：
  - **兩端壓力測試**：在頭尾交替新增、兩端和中間隨機刪除，涵蓋 recenter 和擴容。
  - **自適應門檻振盪測試**：40,000 步裡讓價位數在兩個門檻之間來回，working 旗標混合，並檢查升級和降級都真的發生超過 100 次。
- Release、ASan、UBSan 全部通過。
- **故意改錯檢查**：在新程式裡注入 10 種錯誤，其中 6 種被測試抓到，包括刪除搬移少一格、`best()` 忽略 working、計數搜尋誤用 `<=`、二分搜尋漏掉最後一步修正，以及自適應升級或降級時遺失 working 旗標。
  - 自適應降級遺失 working 旗標這一種，原本的測試抓不到，因此新增了門檻振盪測試，現在會失敗。
  - 沒抓到的 4 種經分析是等價變更，不會改變結果：
    - 清空時不移回中間：只影響效能。
    - 永遠只 recenter、不擴容：只影響效能。
    - 空位為奇數時的偏向：在「至少 1/4 空位才 recenter」的規則下不會被用到。
    - 無分支二分搜尋改用另一種同樣正確的寫法：已改採這個常見寫法。

## 本次量測

- 環境：Linux x86-64 雲端 VM（Intel Xeon 2.1 GHz，4 vCPU），clang 18，CMake Release（`-O3 -DNDEBUG`），計時程式綁在 CPU 2 上執行。
- 計時器解析度 1 ns，空操作基線 p50／p99 為 29／31 ns，沒有扣除。
- 共 12 輪，每輪輪換 backend 的執行順序，每 6 輪反轉一次。
- 沿用既有兩個量測程式，只新增 backend，以及 `linear_bench` 的 `reprice_random_*`（隨機訂單改到隨機稀疏價格，也就是陣列的最壞情況）。

下表是 12 輪 p99 的中位數（ns）。完整的輪間範圍、p50／p99.9、配置次數見 [量測摘要](sorted-deque-results.md)。

| 情境（每 Book 價位數） | B-tree | 分頁 | 雙端陣列 | 雙端 SoA | 自適應 |
|---|---:|---:|---:|---:|---:|
| 近價群移動 32／ACK 更新 | 190 | 136 | **128** | 140 | 133 |
| 近價群移動 128／ACK 更新 | 184 | 134 | 126 | **124** | 128 |
| 近價群＋2 遠價 128／ACK 更新 | 198 | 133 | **126** | 130 | 140 |
| 跳價 128／改單送出 | 298 | **152** | 225 | 224 | 218 |
| 近價群移動 32／單價查詢 | 76 | **60** | 74 | 80 | 78 |
| 近價群移動 32／前 8 層區間 | 97 | 94 | **84** | 98 | 96 |
| 簡化流程 32 近價＋2 遠價／回報可見 | 288 | 197 | **170** | 190 | 189 |
| 4096 Book 各 6 價／單價查詢 | 480 | 946 | **458** | 528 | 502 |
| 32 價／64 次隨機查詢整批 | 3,173 | — | 5,638 | **2,514** | 6,077 |
| 128 價／隨機單價查詢 | **120** | — | 130 | 123 | 138 |
| 4096 價／隨機單價查詢 | **368** | — | 409 | 404 | 420 |
| 隨機重定價 128／ACK | 484 | — | **298** | 300 | 306 |
| 隨機重定價 1024／送出 | **487** | — | 530 | 548 | 548 |
| 隨機重定價 4096／ACK | **706** | — | 1,696 | 1,694 | 711 |
| 隨機重定價 4096／送出 | **648** | — | 2,719 | 2,913 | 674 |
| 成長至 20,000 價／新增 | 3,798 | — | **2,658** | 2,710 | 3,899 |

「32 價／64 次隨機查詢整批」是整批耗時的 p99，不是單次查詢的 p99。分頁後端只在 `clustered_bench` 裡有，所以其他列是「—」。

**配置次數**（獨立 allocation pass）：雙端陣列在穩態更新時為 0 次；B-tree 在節點分裂時會配置。例如 20,000 次操作中，`jump_128` 改單送出 B-tree 配置 254 次、雙端陣列 7 次；簡化流程 `prototype_32` 派送 B-tree 938 次、雙端陣列 0 次。

## 結論

1. **B-tree 不是每種規模都最快，但它最不容易出現極端的差結果。**
   - 在這個 OMS 主要的每 Book 4–128 價情境裡，雙端陣列的索引更新 p99 大多比 B-tree 低 30–50%（ACK 128 vs 190、回報可見 170 vs 288、128 價改單 ACK 226 vs 458），而且輪間範圍多半不重疊。
   - 分頁在單價查詢與跳價最快。但就像先前的結論，它在多 Book（946 vs 480）和成長（11,634 vs 2,789）時明顯較慢。
   - B-tree 只在約 1,000 個以上價位、而且價位隨機新增刪除時明顯勝出（4096 價重定價 706 vs 1,696）。另外在 128–4096 價的隨機單價查詢也略快（約 5–10%）。
2. **最快的通用作法是「少量價位用雙端陣列、大量價位用 B-tree」**，也就是 `AdaptiveLocator`。
   - 小 Book 的延遲接近雙端陣列，4096 價時接近 B-tree。
   - 代價有兩個：每次呼叫多一次分派（約 5–10 ns），以及跨門檻那一次的 O(P) 轉換尖峰。
   - 轉換尖峰落在成長情境的最大值裡。但本次各 backend 的成長最大值都在數十到數百 µs，而且輪間波動很大（B-tree 46–554 µs），在這個 VM 上無法精確分離出轉換本身的成本。
3. **SoA 的無分支搜尋是查詢專用的取捨**。
   - 32 價的批次查詢最快：2,514，B-tree 3,173，AoS 5,638。
   - 但更新路徑比 AoS 慢：簡化流程 32 價派送 312 vs 222，因為搜尋每次都掃全部價格，搬移也要動兩個陣列。
   - 只有在策略查詢遠多於回報更新時才值得採用。
4. **AoS 的單價查詢仍比 B-tree 慢一些**（32 價 108 vs 98、128 價 130 vs 120）。從 SoA 的結果推測原因是二分搜尋的分支預測失敗，但本次沒有量硬體計數器。下一步可以試「AoS 配置＋無分支搜尋」，看能不能兼顧查詢與更新。

**建議**：C++ 版本可以把 `AdaptiveLocator` 列為預設候選。在把 Rust 預設從 B-tree 換掉之前，還需要做到：

- 移植到 Rust
- 在目標正式硬體上重跑三段延遲量測
- 用正式工作負載量出每個 Book 的價位數分布與查詢／更新比例

本次沒有修改 Rust `IndexBackend` 或 Engine 預設。

> **2026-09-29 後續**：已移植到 Rust，並在 Rust 量測中確認索引層同樣較快；Rust 預設已改為 `IndexBackend::Adaptive`，見 [自適應預設索引](adaptive-default.md)。目標正式硬體與正式工作負載的量測仍未完成。

## 限制

- 雲端 VM，不是正式交易機，也沒有 NUMA、磁碟同步或網路。有綁 CPU，但沒有隔離核心，也沒有關閉頻率調整。
- 計時器開銷約 30 ns，沒有扣除。所有候選都包含同樣的開銷，比較相對差異時不受影響。
- 最大值受配置與排程干擾，不是固定的最壞延遲保證。表格只列各輪 p99 的中位數。
- `reprice_random` 用的是均勻隨機稀疏價格，比真實近價集中的改單更不利於陣列。`updates_*` 則偏向兩端操作、對陣列有利。兩者分別代表兩個極端。
- 沒有移植到 Rust，也沒有在完整 Rust Engine 上量測。

## Sanitizer 建置備註

`OMS_SANITIZE` 只對專案自己的 target 加上 sanitizer 旗標，Abseil 的已編譯部分沒有加。Abseil 的 `raw_hash_set` 在 ASan 下的物件布局不同，兩邊不一致，所以 `index_tests` 會在 `flat_hash_map` 裡 SEGV。**這是既有建置設定的問題，與本次新增的 locator 無關。** 先前 Darwin 上 ASan 卡住，可能是不同的原因，本次沒有驗證。

本次改用全域旗標建置，讓 Abseil 也一起加上 sanitizer：

```sh
# ASan（g++ 13；此環境的 clang 沒有 ASan runtime）
cmake -S cpp -B cpp/build-asan -DCMAKE_BUILD_TYPE=Debug -DCMAKE_CXX_COMPILER=g++ \
  "-DCMAKE_CXX_FLAGS=-fsanitize=address -fno-omit-frame-pointer" -DCMAKE_EXE_LINKER_FLAGS=-fsanitize=address
# UBSan（clang trap 模式，不需要 runtime；g++ 的 UBSan 會在 Abseil flat_hash 觸發 constexpr 編譯錯誤）
cmake -S cpp -B cpp/build-ubsan -DCMAKE_BUILD_TYPE=Debug -DCMAKE_CXX_COMPILER=clang++ \
  "-DCMAKE_CXX_FLAGS=-fsanitize=undefined -fsanitize-trap=undefined"
```

38 組測試在 Release、ASan、UBSan 下都通過。

## 重現

```sh
git clone --depth 1 --branch 20250127.1 https://github.com/abseil/abseil-cpp.git cpp/third_party/abseil-cpp
cmake -S cpp -B cpp/build -DCMAKE_BUILD_TYPE=Release -DCMAKE_CXX_COMPILER=clang++
cmake --build cpp/build --target index_tests clustered_bench clustered_alloc linear_bench linear_alloc -j4
./cpp/build/index_tests
T=YOUR_UNIQUE_TAG
taskset -c 2 ./cpp/build/clustered_bench > docs/results/sorted-deque-clustered-time-$T.csv
taskset -c 2 ./cpp/build/linear_bench    > docs/results/sorted-deque-linear-time-$T.csv
./cpp/build/clustered_alloc > docs/results/sorted-deque-clustered-alloc-$T.csv
./cpp/build/linear_alloc    > docs/results/sorted-deque-linear-alloc-$T.csv
python3 scripts/summarize_sorted_deque.py docs/results/sorted-deque-{clustered,linear}-time-$T.csv \
  docs/results/sorted-deque-{clustered,linear}-alloc-$T.csv > docs/sorted-deque-results.md
```

本次原始資料：

- [clustered time](results/sorted-deque-clustered-time-2026-09-28-linux.csv)
- [linear time](results/sorted-deque-linear-time-2026-09-28-linux.csv)
- [clustered alloc](results/sorted-deque-clustered-alloc-2026-09-28-linux.csv)
- [linear alloc](results/sorted-deque-linear-alloc-2026-09-28-linux.csv)
- [metadata](results/sorted-deque-metadata-2026-09-28-linux.json)：平台、編譯器、Abseil commit、原始碼與執行檔雜湊

## 後續

2026-09-29 發現雙端陣列在「較短一側已滿」時的中間插入會搬移長側，512 價隨機重定價因此被高估；已在 Rust 與 C++ 修正，見 [滑動視窗價格定位器](sliding-window.md#過程中修正的錯誤)。本文數字保留為當時紀錄。之後的預設改為 `IndexBackend::SlidingWindow`。
