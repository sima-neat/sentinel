# 本機 AI 代理人 API

Sentinel 守護程式透過本機 Unix
通訊端 `/run/simaai-sentinel/api.sock` 提供具版本控制的 HTTP/JSON API，不會在 TCP 連接埠上監聽。
API 與終端使用者介面共用同一個快取與加鎖的檢查點儲存區，因此由
AI 代理人啟動的追蹤會立即顯示在使用者介面與 CLI 中。

```bash
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/health
```

## 端點

| 方法與路徑 | 用途 |
| --- | --- |
| `GET /v1/health` | 版本、資料新鮮度、樣本與指標數量、錯誤，以及作用中的追蹤。 |
| `GET /v1/cache` | 完整的即時快取文件。 |
| `GET /v1/metrics` | 指標定義、單位、說明與閾值。 |
| `GET /v1/samples/latest` | 最新的附時間戳記指標值。 |
| `GET /v1/traces/active` | 作用中的追蹤，或 `null`。 |
| `POST /v1/traces` | 啟動一個具名的追蹤。 |
| `POST /v1/traces/stop` | 停止並保存作用中的追蹤。 |
| `POST /v1/traces/{id}/stop` | 僅在 ID 仍相符時停止作用中的追蹤。 |
| `GET /v1/runs` | 列出進行中與已完成的執行摘要。 |
| `GET /v1/runs/{name-or-id}` | 擷取已儲存的執行及其原始樣本。 |
| `GET /v1/compare?runs=A,B` | 比較兩個以上的執行；第一個為基準。 |
| `GET /v1/peripherals` | 已連接的周邊裝置，取自記憶體中的結果。 |
| `POST /v1/peripherals/refresh` | 重新掃描，然後傳回新的目錄。 |

啟動請求範例：

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -H 'Content-Type: application/json' \
  -d '{"name":"baseline","note":"before optimization","tags":["compiler-v1"]}' \
  http://localhost/v1/traces
```

停止作用中的追蹤：

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -X POST http://localhost/v1/traces/stop
```

先讀取作用中追蹤的客戶端，在停止時應附上其 ID：

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -X POST http://localhost/v1/traces/20261004T120000.000Z-baseline/stop
```

若另一個客戶端已取代該追蹤，條件式形式會傳回 HTTP 409，
且不會停止任何追蹤。ID 比對與停止動作在同一個
執行儲存區鎖定下進行。

名稱必須唯一，且同一時間只能有一個作用中的追蹤。互相衝突的生命週期
作業會傳回 HTTP 409。未知的執行與路由會傳回 HTTP 404。無效的請求
會傳回 HTTP 400。回應以 JSON `null` 表示無法取得的指標。
比較回應預設包含執行中繼資料、統計資料與相對於基準的差值。
僅在需要附時間戳記的樣本時才加上 `raw=1`。

## 周邊裝置

探索執行緒會在守護程式啟動時、核心回報裝置變更時，
以及收到重新整理請求時，掃描開發板的周邊裝置。裝置提供者只讀取
核心介面，從不開啟串流。為了讀取 `board` 區塊的 U-Boot overlay 清單，每次掃描也會以 root
身分執行 `fw_printenv -n dtbos`。它絕不會變更環境，但會取得環境的鎖，因此會建立
`/var/lock/fw_printenv.lock`。`GET /v1/peripherals` 會傳回
最新結果：

```json
{"revision": 1791155282460, "observed_at": "2026-10-04T23:08:02.460Z",
 "board": {"model": "SiMa.ai Modalix SoM 16Gig Board", "configured_cameras": [...], ...},
 "devices": [{"type": "camera", "id": "camera:v4l2:3f2a9c0d41b7e650", ...}],
 "errors": [{"provider": "camera.v4l2", "code": "io.permission_denied", "reason": "..."}]}
```

| 欄位 | 意義 |
| --- | --- |
| `revision` | 每當 `board`、`devices` 或 `errors` 變更時就會改變。守護程式啟動時它從隨機值開始，因此重新啟動或時鐘變更幾乎不可能讓它重複出現相同的值；只能比較是否相等。其值小於 2^52，因此在以 double 讀取數值的 JSON 讀取器中也能精確表示。 |
| `observed_at` | 產生此結果的那次掃描開始的時間；第一次掃描完成前為 `null`。 |
| `board` | 開發板針對攝影機的設定方式：型號、U-Boot 套用的 overlay、裝置樹所設定的攝影機（目錄中有對應項目時，各附上作為其感測器的 `camera.mipi` 攝影機的 `id`），以及已安裝的 overlay 所支援的感測器。與 `devices` 在同一次掃描中讀取；第一次掃描完成前不存在。請參閱 [開發板攝影機設定](peripherals/board.md)。 |
| `devices` | 每個裝置一個物件，以 `type` 標記類型。`id` 在重新插拔至同一連接埠後保持不變，且絕不會是 `/dev/videoN` 名稱。 |
| `errors` | 在最近一次掃描中失敗的提供者，以及 `board` 區塊中無法完整讀取的欄位（提供者 `board.<field>`，例如 `board.overlays`）。失敗的提供者在上次成功掃描中找到的裝置會保留在 `devices` 中，而失敗的 `fw_printenv` 會保留最後的 overlay 清單。`hotplug.unavailable` 表示無法接收核心 uevent，因此只有在重新整理時才會重新掃描。 |

Sentinel 只回報硬體事實。應用程式是否支援某個
裝置或模式，由該應用程式決定。

`POST /v1/peripherals/refresh` 會等待一次在該請求之後開始的掃描，
並以 HTTP 200 傳回該目錄，與 `GET` 的文件相同。
同時發出的重新整理請求會共用掃描。探索執行緒在每次掃描後會暫停 1 秒，因此重新整理最多可能多等這段時間才開始掃描，重複的重新整理請求也無法讓探索持續執行。若掃描未在 10 秒內完成，會傳回 HTTP 504；
若已有 8 個重新整理請求在等待，則傳回 HTTP 429。
當探索功能已停用（`--no-peripherals`）或已停止時，
兩個路由都會傳回 HTTP 503。

`simaai-sentinel peripherals` 會以表格印出相同的結果；加上 `--json`
可取得原始文件，加上 `--refresh` 則會先重新掃描。

各類型的裝置記錄分別說明於：[USB 攝影機](peripherals/camera.md)、
[MIPI CSI-2 攝影機](peripherals/camera-mipi.md)、
[麥克風](peripherals/microphone.md)。`board` 區塊說明於
[開發板攝影機設定](peripherals/board.md)。

[`peripherals/catalog-example.json`](peripherals/catalog-example.json) 是一份
來自 DevKit 的完整回應，其中 USB 攝影機已精簡為每種格式一個模式。
其 `board` 區塊抄錄自 DevKit，而非
實際擷取。Sentinel 的測試會依據結構描述檢查它，因此用戶端可以
拿它來測試。

## 安全性與並行性

此通訊端僅限 DevKit 本機使用，Sentinel 絕不會將它對外公開。
它刻意開放給本機使用者存取，讓 Insight 與 CLI 等用戶端無需 root 權限即可運作。
控制作業會啟動與停止遙測追蹤，以及要求重新掃描周邊裝置；API 不會刪除執行、
執行工作負載或修改硬體。重新掃描只會讀取，且每次掃描後 1 秒的暫停限制了
重新整理請求可造成的工作量。若本機使用者持續讓 8 個重新整理請求處於等待狀態，
其他重新整理會傳回 HTTP 429，而 `GET` 仍可正常運作。遠端存取應由經過驗證的
Kerrigan/Fleet Manager 代理伺服器提供，而不是轉送此通訊端或
新增未經驗證的 TCP 監聽程式。

快取寫入採用原子性重新命名。執行作業使用與 CLI、TUI 及守護程式記錄器
相同的獨占檔案鎖定。
當客戶端不得停止取代其所觀察之追蹤的另一個追蹤時，請使用帶有 ID 的停止路由。

## AI 代理人技能

安裝期間，產生的 Sentinel 安裝指令碼會執行 `sima-cli
playbooks install`，並使用建置該套件時所用的確切 Sentinel Git 提交。
如此可讓執行階段與技能的修訂版本保持一致，而不必在 Vulcan 構件中
加入巢狀的技能資源。
playbook 管理程式會為每個支援的 AI 代理人安裝該技能，並在本機
登錄檔中記錄其來源提交。

在 DevKit 上安裝 Sentinel 一律從 `sima-cli neat install
sentinel` 開始，因此在那裡通常不需要另外執行技能安裝步驟。
若要在未安裝 DevKit 套件的環境中使用 Sentinel 技能，
例如 SDK 容器或開發人員工作站，
請使用該環境的 `sima-cli` 直接從 GitHub 安裝技能：

```bash
sima-cli playbooks install gh:sima-neat/sentinel/skills/use-sentinel
```

若要在修訂版本進入 `main` 之前先行測試，請附加 Git 參照：

```bash
sima-cli playbooks install --force \
  gh:sima-neat/sentinel/skills/use-sentinel@feature/checkpoint-run-comparison
```

技能的更新或移除仍由 `sima-cli playbooks` 管理。
