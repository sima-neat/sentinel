# Sentinel 文件

Sentinel 在背景守護程式中收集 Modalix DevKit 的遙測資料，
寫入一個原子性的 JSON 快取，並透過終端報告呈現該快取。
這些頁面說明每個面板回報的內容、每個數值的來源、
計算方式，以及其限制。

## 作業檢視面板

| 面板 | 文件 | 用途 |
| --- | --- | --- |
| 總覽 | [總覽面板](panels/overview.md) | 溫度、電源、CPU、記憶體、MLA 記憶體、儲存空間與網路健康狀態的重點指標。 |
| 熱能 | [熱能面板](panels/thermal.md) | 晶片上的 RTSN/PVT 感測點，以及板級硬體監控讀數。 |
| 電源 | [電源面板](panels/power.md) | PMBus 電源軌功率、總計、工作階段平均值/峰值，以及收集器健康狀態。 |
| 系統 | [系統面板](panels/system.md) | 整體/每核心 CPU、平均負載、Linux 記憶體、MLA 配置、EV74 CMA 記憶體與處理程序。 |
| 儲存/網路 | [儲存與網路面板](panels/storage-network.md) | eMMC/NVMe 容量與 I/O，以及整體網路流量。 |

## 報告與收集行為

- [測量模型](measurement-model.md) 說明收集器的頻率、快取
  歷史記錄、圖表語意、閾值、過時資料、無法取得的值，以及
  重設行為。
- [報告與 JSON 匯出](reports.md) 說明快取結構描述，以及
  `table`、`export`、`sensors` 與 `status` 指令。
- [本機 AI 代理人 API](api.md) 說明透過
  守護程式的 Unix 通訊端進行的即時讀取與追蹤控制。
- [擷取與比較執行](run-comparison.md) 說明持久保存的
  檢查點、「Compare Runs」分頁、保留期限，以及 CSV/JSON 匯出。

## 周邊裝置

Sentinel 也會列出連接到開發板的攝影機與麥克風。
[本機 AI 代理人 API](api.md#peripherals) 說明此目錄及其重新整理方式；
每種裝置類型都有各自的頁面：

- [USB 攝影機](peripherals/camera.md)（V4L2）
- [MIPI CSI-2 攝影機](peripherals/camera-mipi.md)
- [麥克風](peripherals/microphone.md)（ALSA）
- [新增裝置類型](peripherals/adding-a-device-type.md)（供貢獻者參考）
