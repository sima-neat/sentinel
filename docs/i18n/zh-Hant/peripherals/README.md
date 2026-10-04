# 周邊裝置

Sentinel 會維護連接至 Modalix DevKit 的裝置目錄：有哪些裝置、如何識別
它們，以及它們具備哪些功能。Neat Core、Insight、sima-cli2、指令碼和
代理程式都讀取相同的目錄，因此在此加入的裝置類型會立即顯示在所有位置。

此目錄是為擴充而設計。相機是第一種裝置類型；加入其他類型（麥克風、
IMU、LiDAR 等）不需要變更目錄、API、CLI 或用戶端。

## 運作方式

```text
kernel hot-plug event ─┐
refresh request ───────┤
                       ▼
            peripherals thread (one per daemon, sleeps until woken)
              1. run every provider (read-only)
              2. apply Neat Core's support rules (cameras)
              3. compare with the last catalog; revision +1 if changed
              4. write /run/simaai-sentinel/peripherals.json
                       │
                       ▼
            GET /v1/peripherals  ·  simaai-sentinel peripherals
```

**提供者**是 Sentinel 內的 Rust 程式碼，從核心介面探索一類裝置並傳回
記錄。其餘工作全由 Sentinel 處理：在熱插拔時喚醒、去彈跳、隔離故障、
保留失敗提供者上次正確的記錄、穩定的修訂版、變更記錄、API 和 CLI。

探索作業刻意設計得很輕量：沒有變更時執行緒不使用 CPU，以低於守護程式
10 個 nice 等級執行，並將成批事件與重新整理要求合併為一次掃描。

## 頁面

| 頁面 | 適用對象 |
| --- | --- |
| [加入裝置類型](adding-a-device-type.md) | 為新種類裝置加入支援的貢獻者 |
| [裝置類型](device-types/README.md) | 每種支援類型的記錄格式，從[相機](device-types/camera.md)開始 |
| [本機代理程式 API](../api.md) | `GET /v1/peripherals`、`POST /v1/peripherals/refresh`，以及使用 `since_revision` 輪詢 |

## 使用目錄

```bash
simaai-sentinel peripherals            # table
simaai-sentinel peripherals --json     # the catalog document
simaai-sentinel peripherals --refresh  # rescan first
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/peripherals
```

在 Neat 應用程式中，`simaai::neat::peripherals::list()`（C++）和
`pyneat.peripherals.list()`（Python）會傳回相同目錄。每個裝置都以 JSON
攜帶其詳細資料（`details_json` / `details`），所以在 Core 加入強型別欄位
之前，Neat 應用程式已能使用新的裝置類型。

<a id="support-rules"></a>

## 支援規則

每個相機模式都有 `supported` 和 `reason`：表示已安裝 Neat Core 的
`CameraInput` 是否接受該模式。Sentinel 不會自行判定。Neat Core 將規則
安裝至 `/usr/share/simaai-sentinel/support/neat-core.json`，周邊裝置執行緒
在比較與寫入目錄前，會將規則套用至每個模式，因此 Core 升級會像其他變更
一樣遞增 `revision`。Sentinel 監看該目錄，且不需重新掃描硬體就會重新套用
規則。若未安裝 Neat Core，每個模式都是 `supported: false`，原因會說明
Core 未安裝。

```json
{
  "format": 1,
  "source": "neat-core 0.4.0",
  "camera": {
    "backends": {"accept": ["mipi"], "reason": "..."},
    "formats": {"accept": ["NV12"], "reason": "..."},
    "framerates": {"accept": [{"num": 30, "den": 1}], "reason": "..."},
    "isp_output": {"reason": "..."}
  }
}
```

規則依該順序檢查，第一個失敗項目會成為模式的 `reason`。尺寸範圍永遠不會
標示為支援；若有 `isp_output`，模式必須是 ISP 輸出尺寸。目錄最上層的
`support` 會回報 `state`（`applied`、`not_installed`、`invalid`，或無效更新
仍沿用先前規則時的 `stale`）、`source` 和 `path`。

Sentinel 會建立 `/usr/share/simaai-sentinel/support/`，但不會在其中安裝檔案，
所以 Sentinel 與 Neat Core 不會宣告同一路徑，且能獨立安裝、升級或解除安裝。
目前只有格式 1；加入新格式時，Sentinel 仍會讀取舊格式。若規則檔格式比
Sentinel 已知的更新，系統會繼續使用先前規則並回報 Sentinel 需要更新。

## 守護程式選項

| 選項 | 預設值 | 含義 |
| --- | --- | --- |
| `--peripherals-file PATH` | `/run/simaai-sentinel/peripherals.json` | 寫入目錄的位置，也是 `simaai-sentinel peripherals` 讀取的位置 |
| `--support-rules PATH` | `/usr/share/simaai-sentinel/support/neat-core.json` | Neat Core 的相機支援規則 |
| `--no-peripherals` | 關閉 | 執行守護程式但不探索周邊裝置 |
