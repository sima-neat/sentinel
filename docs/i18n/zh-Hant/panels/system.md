# 系統面板

「系統」面板整合了 Linux CPU 計數器、系統負載平均值、記憶體使用量、SiMa MLA 與 EV74 記憶體遙測、連續記憶體（CMA）使用量，以及週期性的程序快照。

## CPU 欄位

| 欄位 | 來源與計算方式 |
| --- | --- |
| CPU 使用量 | 系統採樣之間，彙總的 CPU 時間變更量為 `/proc/stat`。忙碌百分比為 `1 - idle_delta / total_delta`；閒置狀態包含閒置和 I/O 等待狀態。 |
| CPU 核心使用量 | 針對 `/proc/stat` 中的每個 `cpuN` 行，執行相同的差異值計算。 |
| CPU 負載（過去 1 分鐘） | `/proc/loadavg` 中的第一個值；Linux 以指數方式計算的可執行任務和處於不可中斷等待狀態中的任務的平均數量。 |
| CPU 負載量（過去 1 分鐘的百分比） | `load1 / logical_core_count × 100`。這是一種用於呈現的正規化方法，且可以超過 100%。 |

CPU 使用率是指在最新採樣時間範圍內，CPU 立即忙碌的時間。負載是指在約一分鐘內排隊或等待處理的工作。高負載但 CPU 使用率低，可能表示存在 I/O 等待或工作被阻擋，而非 CPU 達到飽和狀態。

第一個 CPU 採樣值沒有先前的計數器基準值，因此無法使用。

## 每個核心和流程面板

每個核心的圖表顯示最新的 `cpuN` 使用率。向上/向下按鈕用於選擇核心。

程序表格欄位如下：

| 欄位 | 意義 |
| --- | --- |
| 程序識別碼 | Linux 程序識別碼。 |
| 中央處理器 | `/proc/<pid>/stat` 中記錄的最後一個處理器編號；這表示該程序上次是在哪個處理器上執行的，並非保證該程序會一直與該處理器關聯。 |
| CPU 使用率 | 計算使用者和系統 CPU 週期差異，然後除以經過的時間。大約 100% 代表一個完全使用的邏輯 CPU。多執行緒程序可能會超過 100%。 |
| RSS MB | 從 `/proc/<pid>/stat` 取得的常駐頁面數量，乘以系統頁面大小，然後轉換為 MiB（兆位元組）。 |
| 姓名 | 從 `/proc/<pid>/stat` 取得程序指令名稱。 |

系統會每隔五秒重新整理資料。Sentinel 只會保留全球前 64 個程序，並依 CPU 使用率和 RSS 值排序。當選定的核心沒有任何保留的程序時，使用者介面會切換為顯示全球保留的程序清單。因此，此面板更偏向於診斷用途，而非提供詳盡的程序統計報告。

## Linux 記憶體

```text
used_bytes = MemTotal - MemAvailable
used_pct   = used_bytes / MemTotal × 100
```

`MemAvailable` 的數值來自 `/proc/meminfo`，並估算可供新工作使用的記憶體量，且不包含交換記憶體。這表示 Linux 可以回收的快取記憶體不會自動被視為不可用記憶體。

`MB` 顯示的數值是使用 1024 × 1024 位元組計算得出，因此從技術上來說，它實際上是 MiB（二進位兆位元組），即使使用者介面標籤顯示的是 MB（兆位元組）。

## MLA 記憶體

在 Platform 3.0 上，若存在 `/dev/dma_heap/simaai,dms`，便會選用 DMA-BUF 遙測。Sentinel 讀取 `/sys/kernel/debug/dma_buf/bufinfo`，將由 `mla` 或 `simaai,dms` 匯出的物件與附加至 MLA 裝置的物件取聯集，並依 inode 去除重複後計算。每個 DMA-BUF 物件最多只會計算一次。

在沒有此 heap 的舊版上，Sentinel 讀取 `/dev/simaai-mem`，找到 `Total allocated size:`，解析其後的十六進位位元組數，並換算為 MiB。

這是目前配置給 MLA 的記憶體。它不是：

- MLA 計算資源使用率；
- 模型中的每秒畫面數 (FPS) 或延遲時間；
- 總體實體 DRAM 使用量；
- 一種測量 `MLA 0.68V` 功率的方法。

如果無法讀取所選介面，或其文字格式無法識別，則該值將為「無法取得」，而非零。選用 DMA-BUF 遙測後，Sentinel 不會退回使用舊版配置器。

## EV74 記憶體

在 Platform 3.0 上，EV74（CVU）使用 DMA-BUF 記憶體運作。EV74 使用某個緩衝區時，核心會在 `/sys/kernel/debug/dma_buf/bufinfo` 中，將 `cvu` 裝置列為該緩衝區的已附加裝置。Sentinel 會加總附加到 `cvu` 或名為 `<address>.cvu` 之裝置的物件，每個 DMA-BUF inode 只計算一次：

```text
ev74_mem_allocated_mb = sum of sizes of unique inodes attached to cvu
```

每個 EV74 用戶端都會個別附加到緩衝區，因此同一個緩衝區常會多次列出 `cvu`，但仍只計算一次。Sentinel 每次取樣只讀取 `bufinfo` 一次，同時用於 MLA、EV74 與 `linux,cma` 記憶體。System 分頁會以圖表顯示此值，Compare Runs 分頁可為選取的執行疊加 `EV74 memory`。

閱讀此值時請留意以下限制：

- EV74 與 MLA 共用的緩衝區（例如交給模型的前處理張量）會同時計入 EV74 記憶體與 MLA 記憶體。請勿將兩者相加。
- 緩衝區在 EV74 使用後才會計入。已為 EV74 配置但 EV74 尚未處理的張量屬於 `linux,cma` 記憶體，而非 EV74 記憶體。
- DMA-BUF 名稱會被忽略。系統常駐程式持有名為 `EV74` 的小型緩衝區，但 EV74 並未使用它們。
- EV74 固定的韌體保留區與核心內部配置不是 DMA-BUF，因此不包含在內。
- CVU 驅動程式每個用戶端最多快取 64 個緩衝區。超過此數量的管線可能使使用中的緩衝區被逐出，導致數值偏低；逐出次數可在 `/sys/kernel/debug/simaai-cvu/stats` 查看。

當 `/dev/cvu` 不存在，或 `bufinfo` 無法讀取或解析時，此值會顯示為無法取得，而非零。

## linux,cma 記憶體

`linux_cma_used_mb` 會加總由 `linux,cma` 堆積匯出的 DMA-BUF 物件，每個 inode 只計算一次。EV74、視訊、ISP 與 CPU 緩衝區共用此記憶體池，因此這是整個記憶體池已配置的記憶體，而不只是 EV74 的記憶體。當 `/dev/dma_heap/linux,cma` 不存在，或 `bufinfo` 無法讀取或解析時，此值會顯示為無法取得。

## CMA 記憶體池

Sentinel 也會在每次一般常駐程式取樣時，從 `/proc/meminfo` 讀取 `CmaTotal` 與 `CmaFree`，並回報：

```text
CMA used = CmaTotal - CmaFree
```

`CmaTotal` 涵蓋開機時保留的所有 CMA 區域。`CmaFree` 不包含 Linux 放入 CMA、並在需要時移出的可移動頁面（例如頁面快取）。因此 CMA used 會高估已配置的記憶體，也不是 EV74 記憶體：在閒置的 Modalix Platform 3.0 DevKit 上，它約為 90 MiB，而 `linux,cma` DMA-BUF 總計僅 3 MiB。EV74 請使用 `ev74_mem_allocated_mb`，記憶體池的配置請使用 `linux_cma_used_mb`。

鍵名 `ev74_cma_total_mb`、`ev74_cma_free_mb` 與 `ev74_cma_used_mb` 保持不變，讓現有用戶端與已記錄的執行仍可讀取；其標籤為 `CMA reserved`、`CMA free` 與 `CMA used`。檢查點記錄會保留這三個值。Linux CMA 是共用的核心功能，Sentinel 無法僅憑 `/proc/meminfo` 將這些頁面歸屬到個別程序。若任一欄位不存在，推算出的使用量會回報為無法取得而非零；存在的欄位仍可各自取得。

開機時的保留區可另外使用以下指令檢查：

```bash
dmesg | grep -i cma
```
