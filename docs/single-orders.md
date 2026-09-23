# 單張訂單最新意圖（v0.3）

單張訂單直接使用 `apply(Event::New/Replace/Cancel)`；不必為了等待回報而建立 Group。`Replace` 就是修改價格／總量的操作。Group 用來協調多張子單的總目標、拆單與補量；兩者共用同一個實體訂單狀態機，但群組子單仍由 Group 獨占管理。

## 接受、送出與確認

每張單分成三份資訊：

- `order(id)` 的價格、總量、剩餘量：交易所最後確認的狀態；新單未確認前價格／總量仍是初始請求值。
- `order(id).pending`：已承諾交付 transport 的一筆在途請求。
- `order_intent(id).intent`：策略最新的期望價格／總量，或 Cancel。

`apply` 成功代表本機接受意圖，不代表交易所接受。`Outcome.intent_revision` 是這張單的意圖版本；`Outcome.version` 是包含意圖及訂單狀態變動的訂單版本。

| 情況 | 行為 |
|---|---|
| New 合法且額度足夠 | 原子記錄初始意圖與送出決策，回傳 `outbound` |
| 訂單已確認、可執行的 Replace／Cancel | 原子記錄意圖與送出決策，回傳 `outbound` |
| 有在途請求 | 保存最新意圖，回傳 `outbound=None` |
| 訂單結果未知 | 可以保存合法意圖，等對帳完成才允許執行 |
| Replace 已符合確認狀態 | 保存意圖並標記 NotNeeded，不送無效改單 |
| 終結單且沒有在途請求 | 拒絕新的操作，不重新開單 |

這個版本仍保留 New／可立即執行操作的同步 `outbound` 行為，沒有「尚未承諾送出的 New」佇列。一旦 `outbound` 回傳，就必須立即交付 Gateway；不能因為還沒呼叫 `send` 就在本機丟棄。若不確定是否送達，進入隔離並對帳。

## 事件迴圈

```text
收到策略命令：
    outcome = oms.apply(New / Replace / Cancel)
    outcome.outbound 若存在，立即交給 Gateway

收到回報：
    oms.on_report(report)

每次輸入或 transport 再次可用後：
    while transport 可接收：
        outcome = oms.dispatch_next_order()
        若 None：離開迴圈，等待下一個事件
        立即傳送 outcome.outbound
        傳送結果不明：hold_for_recovery，停止送出，啟動外部對帳

    若同時使用 Group，再依上層排程驅動 dispatch_next()
```

`dispatch_next_order()` 自行尋找可執行的單張訂單意圖，不需要策略保存失敗命令或重新提交。它每次最多產生一筆操作，ready Cancel 優先於 ready Replace；暫時被風控或未知狀態阻擋的單不阻擋其他可執行訂單。沒有隱藏背景執行緒，也不會在處理回報時偷偷呼叫 Gateway。

`None` 表示目前沒有可送出的操作，並不表示所有意圖都已完成。禁止在 None 時持續忙迴圈。意圖仍待執行時，後续更新會替換舊意圖；已送出的請求不被覆蓋。

可執行範例：`cargo run --offline --example single_order`。流程是 New 100 → 在途期間修改 101／102／103 → New 確認後只送 103 → 改單途中要求 Cancel 並成交 3 → 改單確認後撤剩餘 7 → 重播驗證。

## 數量、取消與拒絕

- Replace 的 `total_qty` 包含已成交量，不代表要持續維持的剩餘量。成交 3、總量 10，剩餘需求是 7。
- 尚未送出的修改若因成交而使 `total_qty <= cum_filled`，在結果明確且沒有在途請求後標記 `Unexecutable`，不自動撤單，也不建立新單。策略可查詢後明確 Cancel 或提出合法的新修改。
- 終結單不因未送出的加量意圖而建立另一张實體單。成交更正也不會自動重啟已結束的意圖。
- Cancel 一旦接受即鎖定取消方向，後續 Replace 回傳 `CancelRequested`。想再次掛單須明確 New，並確認舊單處理結果。
- Cancel 遇在途操作先等待；若訂單已全成、被拒或失效，未送的 Cancel 會標記 `NotNeeded`。查詢訂單生命週期可分辨原因，不能把 NotNeeded 當撤單成功。
- 已送請求被拒後不自動重試。若最新意圖就是該請求，狀態為 Rejected；若另有較新意圖，會依最新意圖重新規劃。明確提交新 request ID 的 Cancel 可重試拒絕的撤單，但不解除取消鎖定。
- 每個接受的意圖都使用全域唯一、不可重用的 `request_id`，包括未送出、被取代或不需送出的意圖。重複 ID 仍回傳 DuplicateId，並非冪等重送 API。
- `expected_version` 必須等於目前 `order(id).version`。接受新意圖也會增加此版本；實際延後送出時由 OMS 使用當下版本，不要求策略重送。

## 查詢

`order_intent(id)` 回傳最新期望、意圖版本、對應 request ID、request_state 與狀態。

| 狀態 | 意義 |
|---|---|
| Ready | 等待事件迴圈取得並送出 |
| WaitingForReport | 有在途請求，暫時不能再送 |
| NeedsReconciliation | 結果未知，必須對帳 |
| RiskBlocked | 延後操作目前無法通過數量風控 |
| Rejected | 最新已送請求被拒，未啟動自動重試 |
| Unexecutable | 最新未送修改因成交等原因無法執行 |
| Resolved | 最新操作已確認或不需送出；若交易所調整條件，仍以確認狀態為準 |
| Closed(lifecycle) | 訂單已終結，生命週期指出成交／撤單／拒絕等原因 |
| Halted | 日誌失敗等原因使 Engine 停止 |

`request(id)` 保留個別操作歷史。新增 `Queued`、`NotNeeded`、`Unexecutable`；尚未送出的舊意圖被替換時為 `Superseded`，真正送出後才是 `Pending`。

現有價格索引仍只計算已確認與已承諾送出的在途數量，不把未送 desired 視為市場掛單；`reserved_qty` 也不替尚未送出的期望預扣額度。提交時檢查單筆數量上限，真正送出時再按當下價格簿風控檢查。已知閒置訂單的立即操作若超限仍直接回傳 RiskLimit；先前接受的延後操作則可顯示 RiskBlocked。

## 恢復與邊界

新的 WAL 事件把「意圖接受」與「延後送出」分開，立即送出則在同一筆紀錄內提交。追加失敗不更新意圖／在途／預留量，且 Engine halt。重播不產生 outbound；重啟後仍需要補足請求結果、成交與 Reconciled 快照，才可送出保存的最新意圖。

v0.1／v0.2 的原始訂單與 Group 紀錄仍依舊語意重播，不能把歷史送出操作重新解讀成待處理意圖。舊的非 Group 訂單在第一次接受 v0.3 命令時開始建立意圖版本。舊版程式不能讀取含新事件的 WAL，回滾需另行安排。

`on_report` 遇序號缺口／矛盾等錯誤，會隔離 Engine 的所有有效或在途訂單及 Group；`apply(Event::Report)` 仍由呼叫端負責錯誤後的隔離。單張單可用明確 `Timeout` 標記未知；目前 `tick` 自動逾時政策仍僅用於 Group。

單張送出路徑維持既有的 Book 數量限制，不套用 Group 的固定窗口限速、TTL 或自動重試。所有單張／Group 共用 transport 的節流與公平性由整合端負責。每張單最多一筆在途、原子改單、市場 Adapter 契約及歷史容量上限均維持不變。
