# 周邊設備

Sentinel 保存了連接到 Modalix DevKit 的裝置目錄：哪些裝置存在、如何識別它們以及它們能做什麼。Neat Core、Insight、sima-cli2、腳本和代理程式都會讀取相同的目錄，因此在這裡新增的裝置類型會立即在所有地方可見。

這個目錄是為擴展而建立的。相機和麥克風是內建的裝置類型；新增其他類型（例如 IMU、LiDAR 等）不需要對目錄、API、CLI 或客戶端進行更改。

## 它的運作方式

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

**提供者** 是位於 Sentinel 內的 Rust 代碼，它會從核心介面中發現一類設備並返回紀錄。Sentinel 則處理其他所有事情：熱插拔喚醒、去抖動、隔離故障、保留故障提供者的最後有效紀錄、穩定的版本、變更日誌、API 以及 CLI。

發現過程的設計本身很輕量：當沒有變化時，該線程不占用 CPU，運行於比守護程序低 10 個 nice 等級，並將事件爆發和刷新請求合併為一次掃描。

## 頁面

| 頁面 | 給 |
| --- | --- |
| [新增裝置類型](adding-a-device-type.md) | 貢獻者正在為一種新型設備添加支援 |
| [裝置類型](device-types/README.md) | 每種支持類型的記錄格式： [相機](device-types/camera.md), [麥克風](device-types/microphone.md) |
| [本地代理 API](../api.md) | `GET /v1/peripherals`, `POST /v1/peripherals/refresh`，進行民調 `since_revision` |

## 使用目錄

```bash
simaai-sentinel peripherals            # table
simaai-sentinel peripherals --json     # the catalog document
simaai-sentinel peripherals --refresh  # rescan first
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/peripherals
```

從 Neat 應用程式中，`simaai::neat::peripherals::list()`（C++）和 `pyneat.peripherals.list()`（Python）返回相同的目錄。每個裝置都攜帶其詳細資訊作為 JSON（`details_json` / `details`），因此在 Core 為其新增類型化欄位之前，Neat 應用程式就可以使用新裝置類型。

<a id="support-rules"></a>

## 支援規則

每個相機模式都有 `supported` 和 `reason`：安裝的 Neat Core 的 `CameraInput` 是否接受它。Sentinel 不決定這個。Neat Core 將其規則安裝在 `/usr/share/simaai-sentinel/support/neat-core.json`，而外設線程會在比較和寫入目錄之前將它們應用到每個模式，因此 Core 升級會像任何其他變更一樣提升 `revision`。Sentinel 監視該目錄並在不重新掃描硬件的情況下重新應用規則。沒有 Neat Core，每個模式都是 `supported: false`，原因說明 Core 尚未安裝。

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

規則按該順序檢查，首次失敗會成為模式的 `reason`。大小範圍從不標記為支援；當存在 `isp_output` 時，要求該模式為 ISP 輸出大小。目錄的頂層 `support` 報告 `state`（當無效更新導致使用先前規則時為 `applied`、`not_installed`、`invalid` 或 `stale`）、`source` 和 `path`。

Sentinel 創建 `/usr/share/simaai-sentinel/support/`，但從不在其中安裝任何文件，因此 Sentinel 及 Neat Core 從不使用相同路徑，並分別進行安裝、升級或卸載。到目前為止只有格式 1；當新增格式時，Sentinel 會繼續讀取舊的格式。格式新於 Sentinel 的規則文件會保留先前規則的使用，並報告 Sentinel 需要更新。

## 守護程序選項

| 選項 | 預設 | 意義 |
| --- | --- | --- |
| `--peripherals-file PATH` | `/run/simaai-sentinel/peripherals.json` | 目錄的位置，由 `simaai-sentinel peripherals` 進行讀寫 |
| `--support-rules PATH` | `/usr/share/simaai-sentinel/support/neat-core.json` | Neat Core 的相機支援規則 |
| `--no-peripherals` | 關閉 | 在不進行外圍設備發現的情況下運行守護程序 |
