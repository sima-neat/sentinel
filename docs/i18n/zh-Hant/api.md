# 本機 AI 代理人 API

Sentinel 守護程式透過本機 Unix 介面公開一個帶有版本號的 HTTP/JSON API，即 `/run/simaai-sentinel/api.sock`。它不會在 TCP 連接埠上監聽。API 和終端使用者介面使用相同的快取和鎖定檢查點儲存，因此由 AI 代理人啟動的追蹤資訊會立即顯示在使用者介面和命令列介面中。

```bash
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/health
```

## 端點

| 方法與途徑 | 目的 |
| --- | --- |
| `GET /v1/health` | 版本、更新時間、樣本與指標數量、錯誤、目前追蹤，以及 `peripherals` 摘要。 |
| `GET /v1/cache` | 完成即時快取文件。 |
| `GET /v1/metrics` | 指標定義、單位、描述和閾值。 |
| `GET /v1/samples/latest` | 最新的帶有時間戳的指標值。 |
| `GET /v1/traces/active` | 啟用追蹤或 `null`。 |
| `POST /v1/traces` | 啟動一個具名稱的追蹤。 |
| `POST /v1/traces/stop` | 停止並繼續目前的追蹤。 |
| `GET /v1/runs` | 列出目前正在執行的任務和已完成任務的摘要。 |
| `GET /v1/runs/{name-or-id}` | 檢索已儲存的執行結果和原始樣本。 |
| `GET /v1/compare?runs=A,B` | 比較兩次或多次的執行結果；第一次的結果作為基準。 |
| `GET /v1/peripherals` | 目前的周邊裝置目錄。若沒有變更，加入 `since_revision=N` 可取得簡短的 `unchanged` 回應。 |
| `POST /v1/peripherals/refresh` | 要求重新掃描並傳回 `target_scan_sequence`。 |

開始請求範例：

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -H 'Content-Type: application/json' \
  -d '{"name":"baseline","note":"before optimization","tags":["compiler-v1"]}' \
  http://localhost/v1/traces
```

停止即時追蹤：

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -X POST http://localhost/v1/traces/stop
```

名稱必須是唯一的，且僅能有一個追蹤記錄處於作用狀態。如果生命週期作業發生衝突，則會傳回 HTTP 409。未知的執行和路由會傳回 HTTP 404。無效的請求會傳回 HTTP 400。回應會使用 JSON `null` 來表示無法取得的指標。

比較回應預設會包含執行中繼資料、統計資料和基準差異。只有在需要帶有時間戳記的樣本時，才新增 `raw=1`。

## 周邊裝置目錄

`GET /v1/peripherals` 會原樣傳回周邊裝置執行緒寫入
`/run/simaai-sentinel/peripherals.json` 的已連接裝置目錄。探索方式、每種
裝置類型的記錄格式，以及新增類型的方法，請參閱[周邊裝置](peripherals/README.md)。

| 欄位 | 含義 |
| --- | --- |
| `instance_id` | 每次啟動守護程式時都會更新。不同的值表示守護程式已重新啟動。 |
| `state`, `ready` | 第一次掃描前為 `starting`，之後為 `ready`；當提供者、事件監視器或支援規則回報問題時為 `degraded`。已停止的守護程式會留下 `degraded` 和錯誤 `peripherals.stopped`。 |
| `revision` | 每當用戶端可見的內容改變時增加：裝置、問題、錯誤或支援規則狀態。 |
| `scan_sequence` | 每次完成掃描後增加，包括沒有變更的掃描。 |
| `stale`, `issues`, `error` | 失敗的提供者會保留上次正確的記錄並標記 `retained_last_good`；其他提供者不受影響。`error` 描述守護程式本身的問題，例如無法使用熱插拔監視。 |
| `changes` | 最近 256 筆變更：`added`、`removed`、`changed`、`error`、`recovered`，每筆都有 `sequence` 和 `revision`。 |
| `support` | 用來分類相機模式的 Neat Core 規則；請參閱[支援規則](peripherals/README.md#support-rules)。 |
| `devices` | `{id, type, provider, <type>: {...}}`。`id` 在重新插入相同連接埠後仍保持不變，且絕不會是 `/dev/videoN` 名稱。 |

若要低成本輪詢，請傳送上次看到的 `revision` 和 `instance_id`：
`GET /v1/peripherals?since_revision=7&instance_id=<id>`。如果用戶端可見的內容
沒有變更，回應為
`{"unchanged": true, "revision": 7, "scan_sequence": ..., "instance_id": ...}`；
否則為完整目錄。修訂版會隨每次啟動守護程式重新開始，因此缺少
`instance_id` 的 `since_revision` 會以 HTTP 400 拒絕。

`POST /v1/peripherals/refresh` 會排定掃描並傳回
`{"accepted": true, "target_scan_sequence": N, "instance_id": ...}`；當具有
該 `instance_id` 的目錄到達 `scan_sequence` `N` 時，重新整理即告完成。
同時到達的要求會共用一次掃描。HTTP 503 表示周邊裝置探索未執行或已停止，
或者目錄檔案無法寫入（例如 `/run` 已滿）而過時；`error` 會說明原因，日誌中
有詳細資訊。在這些情況下，`/v1/health` 會回報 `"peripherals": null`。
Sentinel 每秒重試失敗的寫入，成功後恢復提供目錄。`GET /v1/peripherals`
傳回 HTTP 503 也可能表示無法讀取目錄。

## 安全性與並行性

這個通訊端是 DevKit 的本機通訊端，且永遠不會透過 Sentinel 進行遠端存取。

它會刻意讓本機使用者可以存取，因為支援的控制作業僅限於啟動和停止遙測追蹤；API 不會刪除執行、執行工作負載或修改硬體。遠端存取應由經過驗證的 Kerrigan/Fleet Manager 代理伺服器提供，而不是透過轉發此通訊端或新增未經驗證的 TCP 監聽程式來提供。

快取寫入使用原子重新命名。執行作業使用與 CLI、TUI 和守護程式記錄器相同的獨佔檔案鎖定。因此，透過 API 啟動的追蹤可以安全地透過任何支援的介面進行檢查或停止。

## AI 代理人技能

在安裝過程中，產生的 Sentinel 安裝指令碼會執行 `sima-cli
playbooks install`，並使用與建立套件時完全相同的 Sentinel Git 提交版本。 這樣可以確保執行階段和技能版本保持一致，而無需將巢狀技能資源新增到 Vulcan 構件中。 指令碼管理程式會為每個受支援的 AI 代理人安裝技能，並將其來源提交記錄在本地登錄中。

在 DevKit 上安裝 Sentinel 時，始終從 `sima-cli neat install
sentinel` 開始，因此通常不需要單獨的技能安裝步驟。 若要在一個不安裝 DevKit 套件的環境中使用 Sentinel 技能（例如，SDK 容器或開發人員工作站），請直接從 GitHub 使用該環境的 `sima-cli` 安裝技能。

```bash
sima-cli playbooks install gh:sima-neat/sentinel/skills/use-sentinel
```

在將修訂版本發布到 `main` 之前，請先進行測試，方法是附加 Git 參考：

```bash
sima-cli playbooks install --force \
  gh:sima-neat/sentinel/skills/use-sentinel@feature/checkpoint-run-comparison
```

更新或移除技能仍由 `sima-cli playbooks` 管理。
