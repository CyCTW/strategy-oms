# 群組目標與送出排程（v0.3；沿用 v0.2 群組語意）

Controller 已整合進同一個 `Engine`。沒有第二個服務、背景執行緒或隱藏 Gateway 呼叫。策略提供最終目標，Engine 在呼叫端要求送出時重新比較目前狀態，產生最多一筆操作。

## 快速執行

```sh
cargo run --offline --example group_quotes
cargo test --offline
cargo bench --offline --bench groups_latency
```

範例把 10 張拆成兩張各 5 張，模擬延遲回報；策略連續更新 101、102、103，實際只改到 103。之後一般送單額度用完，新的加量目標等待；Stop 仍使用保留的撤單額度清除兩張單，並驗證重播不會重送。

## 單一 Engine 的事件迴圈

```text
建立群組 create_group(group_id, book, policy)
設定限速 configure_scheduler(config)

每次收到策略目標、回報或計時器事件：
    tick(now_ms)
    set_target(...) 或 on_report(...)
    while Gateway 現在能接收，且 dispatch_next() 回傳一筆操作：
        立即交给 Gateway.send()
        若送出結果不明：hold_for_recovery()，停止繼續送出，啟動外部對帳
    發布 group_status()／訂單通知
```

`dispatch_next()` 不是預覽：成功時已寫日誌、保留額度、建立在途請求。回傳的命令必須立即交付 transport；不要預取大量命令放進另一個長佇列。若只想看下一步，使用 `explain_pending_action()`。它的 `next_action` 是可變的預覽，不能保存後直接繞過排程執行。

當 `dispatch_next()` 回傳 `None`，等待回報、目標更新或下一個必要 timer tick；不要 busy-loop 重試。`GroupView.status` 說明目前沒有動作的原因。相同優先級的群組輪流取得送出機會，撤單優先於其他可執行操作。

單張 `apply(Event::New/Cancel/Replace)` 現已接受最新意圖，配合 `dispatch_next_order()` 等待回報後执行，**不走群組限速**，詳見 [single-orders.md](single-orders.md)。群組擁有的子單仍禁止透過單張 Cancel/Replace 直接修改；應修改群組目標或先 Stop。兩者共用嚴格的實體訂單 reducer，不混用子單的意圖所有權。

## 模型與配置政策

- 一個群組固定一個 `Book`（策略、帳戶、場所、商品、方向）和單一目標價格；多檔報價使用多個群組。
- 一張實體單只歸屬一個群組。群組可建立多張子單，每張最多一筆在途，群組最多 `max_inflight` 筆在途。
- 群組目標可以連續更新，只保存最新值，不排隊保存過時改單。每筆已送出的 request 仍保留完整身分與回報關聯。
- 減量優先修改／撤除較新的閒置子單；再修正不合目標價格或超出子單上限的掛單；加量先補既有閒置子單，最後才建立新單。不是交易所排隊順位最佳化。
- `max_child_leaves` 限制每張子單的剩餘量；改單使用包含成交的總量，仍受核心 `Limits.max_order_qty` 約束。若累計成交已用完該實體單的總量空間，可能需要新子單；無可用子單名額時顯示 `ChildCapacity`。
- 已終結子單保留歷史歸屬和成交，但規劃只掃描有效、在途或需對帳的子單。群組累計成交增量更新，成交更正也更新原群組。
- 每次 `dispatch_next()` 選擇動作後立即保留額度，再規劃下一筆，沒有跨子單原子提交。部分接受／拒絕依各自回報更新。

## 數量語意

| 模式 | 行為 |
|---|---|
| `MaintainLeaves` | 維持目標未成交量。成交後會刻意補量，可能建立新子單 |
| `TotalExecution` | 群組所有世代子單共用的累計成交預算；剩餘需求＝目標總量－群組累計成交 |

`TotalExecution` 達標後鎖定該目標版本完成，並清理多餘掛單。後續成交取消不會自動重新啟動已完成的版本；如需再次追求原總量，策略明確提交較新的 revision。改 revision 不會重置歷史累計成交；新的獨立執行任務應使用新群組。

`GroupView` 同時提供三種量：

```text
confirmed_leaves：最近確認的剩餘量
projected_leaves：假設所有在途請求成功後的剩餘量，僅供規劃
reserved_qty：仍可能成交的保守剩餘量
```

例如 A＝4、B＝6，B 正在減為 4：三者分別為 10、8、10。若新目標是 12 且保守上限也是 12，目前最多加 2，不能因預期減量成功就先加 4。原子改單取舊／新量較大者；這個群組規劃器目前不支援把非原子的撤舊下新偽裝成 Replace。

## 目標版本、有效期限與時間

- revision 必須正整數且遞增。相同 revision、相同內容回覆 duplicate；相同 revision 不同內容報錯；舊 revision 不可覆蓋新目標。
- `target=None` 表示清除目前所有子單。它和 Stop 不同：後續新目標仍可在 Running 模式執行。
- `expires_at_ms=None` 明確表示持續有效，直到目標被修改或停止；指定期限則在到期後清除現有掛單。尚未確認的新單／改單須等待結果後再撤單。
- Pause 暫停一般自動調整並保留掛單，但仍遵守目標期限；期限到達時可以排程撤單。
- 引擎不讀取系統時間。呼叫端必須在每次輸入前及無輸入期間驅動 `tick(now_ms)`；數值需非遞減，與目標期限使用相同毫秒時鐘域。正式整合須使用跨重啟一致的時間基準，不可每次重啟歸零。時鐘倒退會被拒絕。
- 每筆群組請求由 `request_timeout_ms` 控制逾時。tick 到期後將訂單標記未知，保留請求與額度，不自動重送。

## 暫停、停止與重試

| 控制 | 行為 |
|---|---|
| `Pause` | 保存最新目標，暫停一般調整，保留掛單；目標過期仍撤單 |
| `Stop` | 停止新增，逐筆清除所有子單；新的策略目標不能解除停止 |
| `Resume` | 明確恢復 Running；有未知訂單時拒絕。也會解除 recovery hold |
| `Retry` | 人工解除拒絕／重試阻擋，不解除停止、Pause 或 recovery hold |
| `ReleaseRecovery` | 完成對帳後解除 recovery hold，保留原本 Running／Paused／Stopping 模式 |

停止期間若有在途操作，先等待；回報後只做必要撤單。全部子單確認終結且没有未知狀態時，狀態是 `Stopped`。反覆 Stop 不會清除已拒絕撤單的阻擋；Pause 也不能解除 Stop，只有明確 Resume 才會再次依原目標掛單。

## 拒絕政策

`RejectedWithReason` 提供 Permanent、Transient、RateLimited、Risk；原有不帶原因的 `Rejected` 預設按 Permanent 處理。

- Permanent／Risk：阻擋自動重送，等待不同交易條件的目標或明確 Retry。
- Transient／RateLimited：使用 policy 的固定等待時間和有限次重試。重試額度按群組目標計算，不會因中間某一張子單成功就歸零。
- 僅提高 revision 或續期、但價格／數量／模式相同，不會重置拒絕阻擋，避免上游心跳造成拒單風暴。
- 舊目標的拒絕若已不適用最新交易條件，保留診斷資訊但不阻擋新條件；相同交易條件仍適用阻擋。
- Stop 撤單失敗不因策略改價而自動清除阻擋，須 Retry 或外部處理。
- 逾時是未知結果，必須對帳，不能走一般重試。

## 限速與曝險

`SchedulerConfig` 是本 Engine 管理的所有群組共用的固定窗口：每個新／改／撤命令計一次。一般操作最多使用 `max_actions - cancel_reserve`，撤單可使用剩餘總額度。額度耗盡時回傳下一窗口時間，沒有睡眠或背景重試。

此為本地發送嘗試限速，不能代替交易所原生的多窗口、加權、IP／帳戶／session 限速，也不是任意滑動區間的精確上限。固定窗口邊界可能連續釋放兩批額度。Gateway 的真實限制仍需介接。

停止撤單依然受單筆在途、未知狀態、群組在途上限和總限速約束。未實作場所原生批量撤單、斷線撤單或可繞過連線故障的緊急通道。

風險同時檢查群組 `max_reserved_qty` 和原有 Book 上限，並可只補部分缺口來利用現有額度。這不是帳戶持倉、資金或名目金額風控。

## 恢復與錯誤

所有目標、控制、時間、限速設定及實際發送決策均寫入既有 WAL。命令與群組歸屬用同一筆 Dispatch 記錄，避免重啟後出現本地有委託卻沒有群組的中間狀態。重播驗證原始決策，不執行 Gateway，也不自動根據目標產生額外命令。

恢復後所有群組都有 recovery hold，包括當時尚未產生實體單的目標。先更新時間、補足請求／成交並對帳，再 `ReleaseRecovery` 保留原模式繼續；如刻意要恢復交易才用 `Resume`。重啟不重置限速窗口已消耗的額度或拒絕重試計數。

推薦使用 `on_report` 接入回報。回報序號缺口、矛盾、未知訂單等錯誤會保守隔離本 Engine 所有 managed 群組及有效／在途單張訂單，因為目前沒有完整的來源→帳戶範圍映射。原始錯誤回傳呼叫者記錄與處理，不會默默略過。`hold_for_recovery()` 可由連線失敗主動觸發。

直接使用 `apply(Event::Report)` 時，回報錯誤仍由呼叫者管理隔離。Journal 寫入失敗則整個 Engine 自行 halt。

## 效能與目前範圍

- 一個群組一個價格，尚無跨群組自成交檢查、策略 ownership epoch、人工訂單接管、多場所路由或完整市場規則。
- 群組限額、重試與配置政策建立後固定。核心 Limits 仍須在重播時提供相容設定；SchedulerConfig 與 GroupPolicy 已寫入日誌。
- `group_status` 會掃描該群組有效／在途／未知子單並預覽一次動作；`dispatch_next` 目前掃描全部群組挑選下一筆，不是 O(1) 或零配置實作。大量群組需先量測，再考慮 dirty-set／ready-queue 優化。
- 所有歷史訂單、請求、群組與事件仍受既有容量限制，沒有快照壓縮或輪替。
- `FaultGateway` 是有界的腳本測試器，可排程延遲／重複／缺口回報、模擬送出結果不明；不會自動假定交易所接受委託。
