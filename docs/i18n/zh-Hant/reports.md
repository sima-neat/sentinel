# 報告和匯出 JSON

所有 Sentinel 指令都會讀取快取，除非 `daemon` 指令正在執行收集器。

## 指令

| 指令 | 輸出 |
| --- | --- |
| `simaai-sentinel` | 互動式五面板終端作業檢視畫面。 |
| `simaai-sentinel table` | 持續更新的指標表格，顯示目前的數值、單位和臨界值狀態。 |
| `simaai-sentinel table --once` | 一張適合用於記錄的表格快照。 |
| `simaai-sentinel export` | 將快取文件完整地格式化為 JSON。 |
| `simaai-sentinel sensors` | 指標名稱、群組、單位、閾值，以及收集器提供的描述。 |
| `simaai-sentinel status` | Sentinel 版本、快取時間戳、最新樣本時間／年齡、歷史記錄大小、指標數量，以及頂層守護程式錯誤。 |

使用 `--cache PATH` 來讀取或寫入非預設的快取。UI/表格重新整理間隔只會影響顯示。`daemon --interval SEC` 會改變系統/快取的正常週期；PMBus、溫度感測器和處理器收集器則會維持各自的週期。

## 快取文件

`simaai-sentinel export` 會輸出這個頂層結構：

| 欄位 | 意義 |
| --- | --- |
| `schema` | 快取結構定義版本。目前為 `1`。 |
| `version` | 撰寫快取時使用的 Sentinel 套件版本。 |
| `updated_at` | 快取資料寫入時的 UTC 時間。 |
| `metrics` | 當守護程式啟動時，所有可用指標的定義。 |
| `latest` | 最新的快取樣本，或者在建立樣本之前使用 `null`。 |
| `samples` | 保留依時間順序排列的環形緩衝區歷史記錄。 |
| `processes` | 最新的已儲存流程快照，每五秒更新一次。 |
| `power` | 電源收集器狀態和每個通道的計數器，或針對較舊/不相容的快取使用 `null`。 |
| `errors` | 最上層的背景程式訊息。電源軌線錯誤通常會顯示在 `power.last_error` 中。 |

## 指標定義

每個 `metrics` 條目包含：

| 欄位 | 意義 |
| --- | --- |
| `key` | 在 `Sample.values` 中使用的穩定且可供機器讀取的名稱。 |
| `label` | 長且易於人類閱讀的名稱。 |
| `short` | 簡潔的 UI/表格標籤。 |
| `group` | 子系統類別。 |
| `unit` | 顯示單元。 |
| `description` | 由收集者提供的測量摘要。 |
| `warn` | 可選的警告閾值。 |
| `critical` | 可選的臨界值。 |

消費者應該透過 `key` 將範例值與定義連結，而不是透過標籤或陣列位置來連結。

## 樣本

每個樣本都包含：

- `timestamp`：UTC 時間的資料收集/快取時間戳記；
- `values`：將指標鍵對應到一個數字或`null`。

`null` 表示收集器沒有產生一個確定的數值。它不應被解讀為零。當子系統未配置時，也可能缺少一個金鑰，例如，當 `/media/nvme` 在守護程式啟動時未掛載時。

## 處理記錄

每個程序記錄包含 `pid`、`name`、`cpu_pct`、`rss_mb`，以及可選的 `cpu_core`。請參閱 [系統面板](panels/system.md)，以了解計算和選擇的限制。

## 電源狀態

`power` 物件包含：

| 欄位 | 意義 |
| --- | --- |
| `profile` | 偵測到軌道剖面，例如 `modalix_som` 或 `modalix_dvt`。 |
| `sample_interval_ms` | 已設定 PMBus 的採樣間隔。 |
| `duration_seconds` | 自開始累積電力以來經過的時間。 |
| `valid_samples` | 至少有一個軌道成功通過。 |
| `failed_samples` | 沒有任何成功的鐵軌通過。 |
| `last_sample_valid` | 最新的 PMBus 傳輸是否已讀取至少一條匯流排。 |
| `last_error` | 將最新一次執行時失敗的 Rails 框架產生的錯誤訊息合併顯示，或顯示 `null`。 |
| `rails` | 每個軌道的標籤、度量關鍵字、上次成功採樣時的瓦數、成功採樣次數和錯誤次數。 |

在使用這些欄位進行自動比較之前，請參閱「[Power panel](panels/power.md)」；部分軌道讀取會影響總體的可比性。

## 原子性和保留性

這個守護程式會寫入一個臨時檔案，然後將其刷新，並將其重新命名為快取檔案，
因此讀取者應該會看到先前完整的負載或新的完整負載。快取是一個當前狀態和短期歷史記錄報告，而不是一個持久的審計日誌。當需要持久的證據時，請將其匯出到外部。
