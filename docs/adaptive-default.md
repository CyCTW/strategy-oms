# 自適應預設價格索引

> **2026-09-29 更新：預設已再改為 `IndexBackend::SlidingWindow`**，見 [滑動視窗價格定位器](sliding-window.md)。`Adaptive` 仍可明確選擇，也作為滑動視窗的外部定位器使用。以下是改為 Adaptive 當時的紀錄。

2026-09-29 起，`IndexBackend::default()`、`Engine::new` 和 `Engine::recover` 改用 `IndexBackend::Adaptive`。這不受 `pooled-index`／`paged-index` 編譯 feature 影響。原本的 B-tree 仍可用 `Engine::new_with_index(..., IndexBackend::Standard)` 明確選擇。**WAL 格式、交易語意與公開查詢 API 都沒有改變**，同一份日誌可以用任何後端重播。

## 設計

[`src/price_deque.rs`](../src/price_deque.rs) 替每個 Book 保存一個有序價格定位器：

- **價位少時（Small）**：`(price, level handle, working)` 依價格遞增存在 `VecDeque`。
  - 定位器不分買賣方，所以買方最佳價在尾端、賣方最佳價在頭端。環形緩衝區讓兩邊新增最佳價或最差價都是 O(1)。
  - 中間價位的新增和刪除由 `VecDeque::insert`／`remove` 往比較短的一側搬移。
  - 16 價以內線性掃描，超過改用二分搜尋。
  - `best()` 只在最佳價快取失效時才被呼叫，從最佳端往內跳過只有 pending 的價位。
- **價位多時（Large）**：超過 1,024 價位時，整個 Book 轉成與 `Standard` 相同的 `BTreeMap<Price, Handle>`＋`BTreeSet<Price>`。
  - 低於 256 價位時轉回陣列。兩個門檻不同，避免在門檻附近反覆轉換。
  - 轉換時把所有價位複製一次，只有觸發轉換的那一次操作會出現 O(P) 延遲尖峰。
  - 1,024 取自 C++ 隨機重定價量測的交叉點（見 [sorted-deque.md](sorted-deque.md)），是實驗值，不是容量上限。沒有價位數或價格範圍限制。

Level、Member 與訂單仍在原本的分段 Pool 中，同價差異更新、成員連結和最佳價快取都沒有改動。這次只換了「價格 → Level handle」的有序查找。

## 驗證

- `price_deque` 單元測試：
  - 預設門檻下 30,000 步隨機操作，對照 `BTreeMap` 參考，包含 `i64` 兩端極值與各種區間。
  - 極小門檻（16／4）下的轉換測試。
  - 40,000 步門檻振盪測試：working 旗標混合，升級和降級都超過 100 次。
  - 頭尾兩端成長與刪除測試。
- 既有 Engine／索引測試把 `Adaptive` 加入後端清單：10,000 步成員差異對照舊索引、在途改單雙價位、極端價格、513 個遠價位、恢復。
- [`tests/default_index.rs`](../tests/default_index.rs)（由 `default_btree.rs` 改名）：
  - 驗證新預設，並沿用在途遠價不提前成為最佳價、最新意圖、重複與突發回報、恢復後舊 BookHandle 失效等測試。
  - 新增 Engine 層跨門檻測試：1,100 個稀疏價位，外加 3 個只有 pending 的更高價位，先轉成 B-tree；再從最佳價往下撤單到 200 價，轉回陣列。每一步檢查最佳價、價位彙總、區間與恢復結果。
  - 刻意把轉換時的 working 旗標改壞（降級時全設為 working、升級時遺失 confirmed），這個測試都會失敗。
- 以下驗證指令全部通過：

```sh
cargo fmt --all -- --check
cargo test --offline
cargo test --offline --features pooled-index
cargo test --offline --features paged-index
cargo test --offline --all-features
cargo clippy --offline --all-targets --all-features -- -D warnings
```

## Rust 量測

- 平台：Linux x86-64 KVM VM（Intel Xeon 2.8 GHz，4 vCPU，kernel 6.18.44），Rust 1.94.1。
- 用 `cargo bench` release profile（thin LTO），以 `taskset -c 2` 綁核心，每情境每輪 20,000 筆，共 12 輪，後端順序交替。
- 計時器沒有扣除。空操作基線 p50／p99：`index_comparison` 為 32／34 ns，`clustered_index` 為 19／20 ns。
- 下表是 12 輪 p99 的中位數（括號內是輪間最小–最大），單位 ns。

| 情境／操作 | B-tree（Standard） | 自適應（Adaptive） |
|---|---:|---:|
| 128 價持續換價／索引更新 | 757 (591–1075) | **368** (277–765) |
| 128 Book 各 8 價／單價查詢 | 192 (129–518) | **156** (128–344) |
| 128 Book 各 8 價／前 8 層區間 | 292 (157–462) | **238** (125–462) |
| 32 近價＋2 遠價／ACK | 173 (145–418) | **128** (121–224) |
| 32 近價＋2 遠價／前 8 層區間 | 188 (112–333) | **124** (95–268) |
| 近價群移動 32／ACK | 155 (137–303) | **128** (116–227) |
| 邊界往返 128／ACK | 270 (252–441) | **180** (149–275) |
| 跳價 128／ACK | 303 (290–508) | **222** (204–345) |
| 跳價 128／改單送出 | 246 (221–410) | **197** (162–292) |
| 跳價 128／單價查詢 | **106** (85–175) | 127 (78–199) |
| 邊界往返 128／單價查詢 | **82** (70–162) | 104 (80–231) |
| 4,096 價／單價查詢（自適應已是 B-tree） | 526 (396–727) | 478 (306–726) |
| 成長至 20,000 價／索引更新 | 2,032 (1890–5152) | 2,117 (1945–2933) |
| 新增 Book／索引插入 | 4,072 (3443–8934) | **3,066** (2769–4349) |
| 完整 Engine／立即意圖到命令 | 2,312 (1783–4335) | 2,114 (1705–2644) |
| 完整 Engine／改單回報可見 | 4,380 (3520–6237) | 4,338 (3587–6437) |
| 完整 Engine 32 近價＋2 遠價／回報可見 | 4,429 (3832–6178) | 4,392 (3924–5862) |
| 完整 Engine 成長至 20,000 單 | 13,103 (12812–18842) | 13,188 (12638–13522) |

**配置次數**（獨立 allocation pass，每 20,000 次操作）：

| 情境／操作 | B-tree | 自適應 |
|---|---:|---:|
| 128 價持續換價／索引更新 | 5,696 | 0 |
| 跳價 32／改單送出 | 468 | 1 |
| 邊界往返 128／ACK | 475 | 0 |
| 完整 Engine 跳價 32／派送 | 647 | 0 |
| 新增 Book／索引插入 | 40,977 | 20,977 |

自適應只有每個 Book 第一次建立 `VecDeque` 時會配置，之後由容量加倍攤銷。

## 解讀與取捨

1. **索引層更快，而且幾乎不配置記憶體**。每 Book 4–128 價的近價群、多 Book、持續換價情境，更新 p99 多數低 20–50%。穩態更新不配置記憶體，B-tree 則在節點分裂和合併時反覆配置。
2. **單價查詢與 B-tree 相當**。第一次量測中，部分 128 價情境看起來略慢（跳價 127 vs 106、邊界往返 104 vs 82），但兩者輪間範圍大幅重疊，重跑時沒有重現（75 vs 82、74 vs 72，見下方「重跑」）。C++ 實驗中，AoS 陣列的單價查詢在 128 價以上確實略慢於 B-tree；若正式工作負載以查詢為主，可參考 [sorted-deque.md](sorted-deque.md) 的無分支搜尋，但它會讓更新變慢。
3. **完整 Engine 端到端延遲沒有可量測的變化**。這台 VM 上一次完整 Engine 操作約 2–4 µs，索引只佔其中數十到一百多 ns，差異落在輪間雜訊內。所以這次切換的主要收益在索引本身的延遲與配置次數；要改善端到端延遲，下一個瓶頸不在價格索引。
4. **大 Book 由 B-tree 處理**。超過 1,024 價位後行為與 `Standard` 相同。跨越門檻的那一次操作會有 O(P) 複製，這台 VM 上各後端的成長最大值本來就波動很大（數十到數百 µs），無法分離出這個尖峰的精確大小。
5. **這次量測所在的 VM 與 C++ 量測那次不同**（容器已遷移，CPU 從 2.1 GHz 變成 2.8 GHz），而且輪間波動比 C++ 那次大。所以只做同一次執行、交錯輪次內的相對比較，不跨次比較絕對數字。

## 何時改回 B-tree

以下情況可以用 `IndexBackend::Standard`：

- 單一 Book 常態有數百到上千個隨機分布的價位。
- 策略以單價查詢為主，幾乎不更新。

正式採用前，仍建議在目標正式硬體上用實際交易量重跑以下兩個量測：

```sh
OMS_BENCH_ROUNDS=12 taskset -c 2 cargo bench --offline --bench index_comparison > docs/results/adaptive-default-index-YOUR_TAG.csv
OMS_BENCH_ROUNDS=12 taskset -c 2 cargo bench --offline --bench clustered_index  > docs/results/adaptive-default-clustered-YOUR_TAG.csv
```

本次原始資料：[index_comparison](results/adaptive-default-index-2026-09-29-linux.csv)、[clustered_index](results/adaptive-default-clustered-2026-09-29-linux.csv)。兩個檔案都保留 p50／p99／p99.9／max 與配置次數。

## 重跑

2026-09-29 在同型號 CPU（Intel Xeon 2.8 GHz，kernel 6.18.44-fc-v49，系統負載 0）以相同設定重跑一次（12 輪、綁核心、後端交錯）。下表是 12 輪 p99 的中位數，單位 ns。

| 情境／操作 | B-tree 第一次 | 自適應 第一次 | B-tree 重跑 | 自適應 重跑 |
|---|---:|---:|---:|---:|
| 128 價持續換價／索引更新 | 757 | 368 | 738 | **389** |
| 128 Book 各 8 價／前 8 層區間 | 292 | 238 | 291 | **240** |
| 近價群移動 32／ACK | 155 | 128 | 160 | **125** |
| 32 近價＋2 遠價／ACK | 173 | 128 | 176 | **142** |
| 邊界往返 128／ACK | 270 | 180 | 280 | **180** |
| 跳價 128／ACK | 303 | 222 | 318 | **223** |
| 跳價 128／改單送出 | 246 | 197 | 256 | **198** |
| 跳價 128／單價查詢 | 106 | 127 | 82 | 75 |
| 邊界往返 128／單價查詢 | 82 | 104 | 72 | 74 |
| 新增 Book／索引插入 | 4,072 | 3,066 | 3,602 | **2,662** |
| 完整 Engine／改單回報可見 | 4,380 | 4,338 | 4,244 | 4,208 |
| 完整 Engine 成長至 20,000 單 | 13,103 | 13,188 | 13,048 | 13,430 |

結論：

- 索引更新較快的結論重現了，數值差距在 ±10% 內。
- 完整 Engine 延遲仍然相當。
- 第一次看到的「128 價單價查詢較慢」沒有重現，已修正上方「解讀與取捨」第 2 點。

重跑原始資料：[index_comparison](results/adaptive-default-index-2026-09-29-linux-rerun.csv)、[clustered_index](results/adaptive-default-clustered-2026-09-29-linux-rerun.csv)。
