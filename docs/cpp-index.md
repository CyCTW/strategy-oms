# C++ 價格索引比較紀錄

2026-09-22。已建立 C++20 的 B-tree＋Pool 與 Pool 稀疏分頁測試，實際編譯、驗證並執行五輪量測。程式位於 [cpp](../cpp/README.md)，Rust OMS 並未被替換。

## 完成範圍

移植的是價格索引：共用的 Book/Level/Member pools、generation handle、六種數量統計、同價差異更新、在途最多兩價 contribution、價格層成員、best 快取及有序 range。以重新聚合所有訂單的獨立參考模型驗證更新結果。

價格基準使用 `absl::btree_map<Price, Handle>` 與已確認價格 `btree_set`。分頁使用同一套 Abseil B-tree 保存 page_id，頁面由共用 Pool 管理；每頁 64 價格位置，每 Pool 區塊 64 頁。std::map 僅用於測試參考模型，不當作 B-tree benchmark。

另提供 `Flow` 簡化原型，量測 latest Replace 意圖、單筆 pending、版本／ACK 關聯／序號檢查與派送。**它未移植完整 OMS：沒有 WAL、request/report 去重及歷史、完整風控、恢復、Group 與全部拒絕／成交更正語意。** 因此結果分成 `index_*` 與 `prototype_*`，不能把後者與 Rust 的完整 `engine_*` 直接相比。

## 主要結果

下表為五輪 p99 中位數，單位 ns；完整輪間範圍、p99.9 與記憶體見[量測摘要](cpp-index-results.md)。

| 價格索引情境 | Abseil B-tree＋Pool | Pool 分頁 |
|---|---:|---:|
| 32 近價＋2 遠價：精確查詢 | 42 | 42 |
| 32 近價＋2 遠價：ACK 引起的索引更新 | 84 | 42 |
| 32 近價：跨頁提交子集 | 83 | 42 |
| 32 近價：跳價提交子集 | 125 | 125 |
| 4096 Book、各 4 近價＋2 遠價：隨機查詢 | 42 | 250 |
| 持續新增 Book：索引插入 | 584 | 2500 |
| 撤近掛遠：建立新單價格紀錄 | 84 | 125 |

**結論仍是保留 B-tree 作為通用基準。** Pool 分頁在近價密集的部分更新有優勢，但沒有全面勝出。只有幾個價格、Book 很多時，其記憶體與查詢代價值得優先處理。

計時器基準 p99 是 42 ns，單次量測有 0/41/42/83/84 等量化值；不能由 84→42 宣稱精確加速兩倍。多 Book 查詢的 B-tree 各輪 p99 為 42–167 ns、分頁為 250–500 ns，原始變異均保留。未取得硬體效能計數器資料，不能將差異直接斷言為 cache miss。

簡化 Flow 的 32 近價＋2 遠價情境：

| 操作 | Abseil B-tree＋Pool | Pool 分頁 |
|---|---:|---:|
| 意圖立即轉命令 | 84 | 83 |
| pending 時保存最新意圖 | 42 | 42 |
| ACK 處理（原型） | 167 | 84 |
| 已就緒意圖派送 | 125 | 84 |

這些較小的數字不代表完整 OMS 已達到此延遲，也不是 C++ 對 Rust 的語言效能比較。四個近價的簡化流程還出現分頁較慢：立即命令 42→84 ns、派送 84→125 ns；不能僅展示 32 近價的收益。

## 記憶體與配置

兩個後端都有 Pool，並保留高水位區塊；差異主要是有序價格節點與頁面布局，而非「有 Pool」對「沒有 Pool」。

| Index 分布 | Abseil B-tree＋Pool | Pool 分頁 |
|---|---:|---:|
| 1 Book、4 近價＋2 遠價 | 20,088 B | 88,064 B |
| 128 Book、4 近價＋2 遠價 | 216,104 B | 744,552 B |
| 128 Book、32 近價＋2 遠價 | 1,235,752 B | 1,463,144 B |
| 128 Book、128 近價＋2 遠價 | 4,180,776 B | 4,079,528 B |

這是 Index 自身及容器／Pools 的存活 requested bytes，不是 RSS；不含外部 Order/Memberships、整個 Flow、allocator metadata。每頁仍保留 64 個位置，極小 Book 的低占用與首次 64 頁配置成本並未消失。

32 近價的 index_submit 在 20,000 次操作中，配置由 57 次降到 0；簡化流程 ready_to_command 由 227 次降到 0。但是成長情境頁面 Pool 仍需配置，page directory 與 ID map 也可能配置，不能推導整套零配置。

新 Book 成長另外抽出兩個後端相同的第 0、64、128…筆操作比較：p99 中位數為 B-tree 1,250 ns、分頁 3,708 ns。分頁的這些操作同時觸發 PagePool 成長，也可能觸發其他 Pool／目錄成長，不把所有成本歸因於單一配置。該 subset 每輪 313 筆，極高分位數的可靠性有限。

## 測試、編譯與限制

- 平台：Darwin arm64，Apple Clang 17.0.0（clang-1700.3.19.1）、libc++、C++20、Release `-O3 -DNDEBUG`，未加 LTO 或綁核。
- Abseil：20250127.1，commit `d9e4955c65cd4367dd6bf46f4ccb8cd3d100540b`。依賴來源與授權保留於 `cpp/third_party/abseil-cpp`，詳見[依賴紀錄](../cpp/DEPENDENCIES.md)。
- 一般操作每輪 20,000 筆，五輪交替後端順序；索引情境沿用近價移動、遠價、跨頁與跳價輸入。行情最佳價由合成中心軌跡模擬，沒有市場連線。
- 時間與配置計數分成兩個執行檔。時間檔不覆寫 allocator；配置檔覆寫 C++ new/delete，包含 aligned 與 sized 形式，只統計不計時。沒有扣除 timer。
- 計時前建立輸入／樣本容器；steady 情境在計時外建構起始索引，但沒有刻意移除前幾筆慢樣本。new_book_growth 的初次配置與所有成長均在計時內。
- Release：10 組測試通過，每後端含 20,000 步 locator 及 10,000 步 Index 隨機差異測試；CHECK 在 NDEBUG 下仍有效。
- UBSan：同 10 組測試通過，以 `UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1` 執行，未報錯。
- **ASan 未完成**：本機 address sanitizer 連只輸出一行文字的最小程式也卡在初始化，verbose 最後停在 `AddressSanitizer: libc interceptors initialized`；專案測試在 sandbox 外嘗試仍未完成。已停止停滯的程序。這不是 automatic approval review 拒絕，也不能列為記憶體安全檢查通過；根本原因尚未確認。
- 自有 C++ 檔案通過 `-Wall -Wextra -Wpedantic -Werror` 與 clang-format 檢查；第三方 headers 作為 SYSTEM include。
- 沒有磁碟同步、網路、並行 producer 或排隊延遲量測。沒有宣稱 ASan 已通過、沒有以這次數字宣稱 C++ 比完整 Rust OMS 快。

程式、重跑命令及內部介面限制見 [C++ README](../cpp/README.md)。[metadata JSON](results/cpp-index-metadata-2026-09-22.json) 保存實際時間、編譯旗標、依賴 commit 與來源／執行檔雜湊；[原始 time CSV](results/cpp-index-time-2026-09-22.csv)、[alloc CSV](results/cpp-index-alloc-2026-09-22.csv) 保留完整樣本數、p50/p99/p99.9/max 與配置統計。
