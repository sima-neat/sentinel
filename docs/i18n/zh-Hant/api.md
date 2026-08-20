# 本地代理 API

Sentinel 守護程式透過本機 Unix 介面公開一個帶有版本號的 HTTP/JSON API，即 `/run/simaai-sentinel/api.sock`。它不會在 TCP 連接埠上監聽。API 和終端使用者介面使用相同的快取和鎖定檢查點儲存，因此由代理程式啟動的追蹤資訊會立即顯示在使用者介面和命令列介面中。

```bash
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/health
```

## 端點

| 方法與途徑 | 目的 |
| --- | --- |
| `GET /v1/health` | 版本、更新時間、樣本數量和指標數量、錯誤，以及目前正在追蹤的資料。 |
| `GET /v1/cache` | 完成即時快取文件。 |
| `GET /v1/metrics` | 指標定義、單位、描述和閾值。 |
| `GET /v1/samples/latest` | 最新的帶有時間戳的指標值。 |
| `GET /v1/traces/active` | 啟用追蹤或 `null`。 |
| `POST /v1/traces` | 啟動一個具名稱的追蹤。 |
| `POST /v1/traces/stop` | 停止並繼續目前的追蹤。 |
| `GET /v1/runs` | 列出目前正在執行的任務和已完成任務的摘要。 |
| `GET /v1/runs/{name-or-id}` | 檢索已儲存的執行結果和原始樣本。 |
| `GET /v1/compare?runs=A,B` | 比較兩次或多次的執行結果；第一次的結果作為基準。 |

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

## 安全性與並行性

這個通訊端是 DevKit 的本機通訊端，且永遠不會透過 Sentinel 進行遠端存取。

它會刻意讓本機使用者可以存取，因為支援的控制作業僅限於啟動和停止遙測追蹤；API 不會刪除執行、執行工作負載或修改硬體。遠端存取應由經過驗證的 Kerrigan/Fleet Manager 代理伺服器提供，而不是透過轉發此通訊端或新增未經驗證的 TCP 監聽程式來提供。

快取寫入使用原子重新命名。執行作業使用與 CLI、TUI 和守護程式記錄器相同的獨佔檔案鎖定。因此，透過 API 啟動的追蹤可以安全地透過任何支援的介面進行檢查或停止。

## 客服人員技能

在安裝過程中，產生的 Sentinel 安裝指令碼會執行 `sima-cli
playbooks install`，並使用與建立套件時完全相同的 Sentinel Git 提交版本。 這樣可以確保執行階段和技能版本保持一致，而無需將巢狀技能資源新增到 Vulcan 構件中。 指令碼管理程式會為每個受支援的代理程式安裝技能，並將其來源提交記錄在本地登錄中。

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
