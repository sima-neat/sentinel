# Sentinel 文件

Sentinel 會在後台程序中收集 Modalix DevKit 的遙測資料，並寫入一個原子化的 JSON 快取，然後透過終端報告呈現該快取。這些頁面會說明每個面板報告的內容、每個數值來自何處、如何計算，以及其限制為何。

## 作業檢視面板

| 面板 | 文件 | 目的 |
| --- | --- | --- |
| 總覽 | [概覽面板](panels/overview.md) | 顯示處理器、電源、CPU、記憶體、MLA 記憶體、儲存裝置和網路的健康狀態。 |
| 熱能 | [熱板](panels/thermal.md) | 晶片上的 RTSN/PVT 感測點，以及板級硬體監測讀數。 |
| 力量 | [電源面板](panels/power.md) | PMBus 電源軌總量、會話平均值/峰值，以及收集器狀態。 |
| 系統 | [系統面板](panels/system.md) | 彙總／每個核心的 CPU 使用率、平均負載、Linux 記憶體、MLA 分配、EV74 CMA 記憶體和程序。 |
| 儲存空間／網路 | [儲存和網路面板](panels/storage-network.md) | eMMC/NVMe 的儲存容量和 I/O 效能，以及總網路流量。 |

## 報告和收集行為

- [測量模型](measurement-model.md)，說明收集器的頻率和快取。
  歷史記錄、圖表語義、閾值、過時資料、無法取得的值，以及重設行為。
- [報告和JSON匯出功能，記錄快取結構和](reports.md)文件。
  `table`、`export`、`sensors` 和 `status` 指令。
- [本機代理程式 API](api.md)文件會即時讀取並追蹤對其的控制。
  守護程式的 Unix 網路插座。
- [擷取和比較執行結果](run-comparison.md)文件會永久保存。
  檢查點、比較執行結果標籤、保留設定，以及 CSV/JSON 匯出功能。
