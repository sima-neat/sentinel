# 開發板攝影機設定

目錄中的 `board` 區塊說明開發板針對 MIPI
攝影機的設定方式：這是哪一塊開發板、U-Boot 套用哪些 overlay、開機所用的裝置樹
描述了哪些攝影機感測器，以及已安裝的 overlay
能描述哪些感測器。每個已設定的攝影機都會指出作為其感測器的 `camera.mipi` 攝影機，
因此用戶端可以找到某台攝影機的 `compatible`，並在
`supported_sensors` 中找到設定它的 overlay。與裝置記錄一樣，
它只回報事實，從不變更設定。

此區塊在每次掃描中於提供者之後讀取，因此 `camera_id` 一律
指向同一份目錄的 `devices`，且 `revision` 會在
區塊變更時改變。它只在第一次掃描完成前不存在；當探索功能停用時，
API 會如同以往傳回 HTTP 503。

## 欄位

| 欄位 | 出現時機 | 意義 |
| --- | --- | --- |
| `model` | 裝置樹中有此值時 | `/sys/firmware/devicetree/base/model`，取到第一個 NUL 為止並去除前後空白，例如 `SiMa.ai Modalix SoM 16Gig Board` |
| `firmware` | `/etc/buildinfo` 中有此值時 | `/etc/buildinfo` 的 `DISTRO_VERSION`，即韌體版本，例如 `2.1.3`。僅用於說明錯誤，Sentinel 從不依版本決定行為 |
| `overlays` | `fw_printenv -n dtbos` 曾經成功後 | U-Boot `dtbos` 變數中以 `.dtbo` 結尾的項目，保持原本順序：U-Boot 套用的每一個 overlay，而不只是攝影機 overlay（PCIe、安全開機與快閃記憶體的 overlay 也使用同一個變數）。Sentinel 不會指出哪個項目設定了攝影機；`supported_sensors` 會列出設定某個感測器的 overlay。若之後的執行失敗，會保留最後一次成功執行的清單，並回報該失敗 |
| `configured_cameras` | 一律 | 即時裝置樹中位於 I2C 上的 MIPI CSI-2 感測器；可能為空 |
| `supported_sensors` | 一律 | `/boot` 下的 overlay 檔案所設定的感測器；可能為空 |

每個 `configured_cameras` 項目：

| 欄位 | 意義 |
| --- | --- |
| `compatible` | 節點的第一個 `compatible` 字串，例如 `sony,imx477` |
| `dt_node` | 節點從裝置樹根節點起算的路徑，例如 `/i2cmux@0/i2c@0/imx477@1a` |
| `i2c_device` | I2C 裝置，即 sysfs 所命名的 `<bus>-<address>`，例如 `5-001a` |
| `data_lanes` | 感測器端點的 `data-lanes` 中的 cell 數量 |
| `camera_id` | 作為此感測器的 `camera.mipi` 攝影機的 `id`；目錄中沒有此攝影機時不存在 |

每個 `supported_sensors` 項目都有 `compatible` 與 `overlays`，後者是設定該感測器之
overlay 的檔名，已排序。項目依
`compatible` 排序。此清單合併了 `/boot` 本身及其下的每個槽位目錄，因此可能
包含僅安裝在未開機槽位中的 overlay。

## 各項事實的來源

- **即時裝置樹**是核心開機時所使用的裝置樹，也就是在 U-Boot
  套用 overlay 之後的結果。它是實際生效的設定，因此型號與
  已設定的攝影機都取自它，而非取自 overlay 檔案。
- **I2C `of_node` 連結**是找到並比對已設定攝影機的方式。
  `/sys/bus/i2c/devices` 中的每個 I2C 用戶端（名稱為 `<bus>-<address>`；
  名稱為 `i2c-N` 的轉接器會被略過），只要其 `of_node` 連結解析到
  即時裝置樹中，就是候選項目。當其節點
  具有 `compatible`，且其下方有一個不在另一個
  裝置節點內的 `endpoint` 節點具有 `data-lanes`（這標示 MIPI CSI-2 來源）時，它就是已設定的攝影機。V4L2 將
  I2C 感測器的子裝置命名為 `<driver> <bus>-<address>`，有時後面還會接
  另一個單字（`ccs 5-0010 pixel_array`），因此 `camera_name` 中
  有某個以空白分隔的完整單字為 `<i2c_device>` 的攝影機，就是此感測器。這種方式
  比對的是確切的裝置，而不需比較廠商或感測器名稱。
- **overlay 清單**是平台本身的紀錄，保存在 U-Boot
  環境中。`fw_printenv` 知道該環境在開發板上的
  儲存位置，因此 Sentinel 會詢問它，而不是自行讀取快閃記憶體。其值是
  U-Boot 在下次開機時套用的清單；除非開機後
  曾經變更，否則它就是本次開機所用的清單。這是探索功能中唯一執行程式、
  而非讀取核心介面的部分。`fw_printenv` 以 root 身分執行，絕不會變更環境，
  但會取得環境的鎖：DevKit 隨附的 libubootenv 會建立
  `/var/lock/fw_printenv.lock` 並等待取得其獨占 `flock`，因此同時執行的
  `fw_setenv` 可能使掃描一直等到逾時。
- **overlay 檔案**是平台隨附的 overlay。每個 `*.dtbo`
  檔案，只要直接位於 `/boot` 中（Platform 3.0 的安裝位置），或位於其正下方的目錄中
  （例如 Platform 2.1 的 A/B 槽位 `/boot/boot-0/` 與
  `/boot/boot-1/`），都會被解析。感測器是指 overlay 加到 I2C 匯流排上、
  具有 `compatible` 與 CSI-2 端點（判斷方式同上）的節點：
  位於名稱以 `i2c` 開頭的節點之下，或位於目標為此類節點的 fragment 中；
  目標可由 `target-path` 指定，或由其 `target` 所參照、
  位於 `__fixups__` 中的標籤指定。在這些目錄中的多處出現的同名檔案只會列出一次。

下方還有另一個 CSI-2 來源的 CSI-2 來源屬於橋接器，例如
I2C 多工器，或 GMSL 解串器與串列器；它在兩份清單中都會被排除，
因此只會出現感測器本身。

攝影機的解析度與格式仍來自 ISP，列於攝影機的
`modes` 中；overlay 不會描述它們。

## 讀取上限

探索功能最多讀取 1024 個 I2C 裝置；每個裝置的子樹最多讀取 256 個節點、每個節點的
256 個項目，以及含裝置本身節點在內的 32 層節點；`/boot` 及其每個目錄的 4096 個項目；
以及 512 個 overlay 檔案，每個檔案最大 1 MiB。在這些上限處被截斷的清單，會作為其欄位的
錯誤回報，已讀取的部分仍會發布。overlay 檔案只解析一次，只有在其大小、修改時間、變更時間或 inode
改變時才會重新解析；無法讀取的檔案
會在下一次掃描中重新讀取。`fw_printenv` 在 2 秒後
會被終止。掃描最多再等待 0.5 秒讓它結束，之後便交由背景回收。在它結束之前，之後的掃描不會執行 `fw_printenv`，
而是回報錯誤並保留最後的清單。最多只讀取其 64 KiB 的輸出與 4 KiB 的錯誤
輸出。

## 錯誤

每個錯誤都會指出它使哪個欄位不完整：`provider` 為
`board.model`、`board.overlays`、`board.configured_cameras` 或
`board.supported_sensors`。區塊的其餘部分仍會照常發布。

| 代碼 | 發生時機 |
| --- | --- |
| `io.open` | `model`、`/sys/bus/i2c/devices`、某個裝置的節點、`/boot`、其中某個目錄，或某個 overlay 檔案存在但無法讀取 |
| `io.permission_denied` | 同上，但以 `EACCES` 失敗；或 `fw_printenv` 存在但無法執行（`EACCES` 或 `EPERM`） |
| `peripherals.discovery_failed` | `fw_printenv` 因其他原因無法啟動、無法讀取其輸出、等待它時失敗、未及時完成，或因 `dtbos` 未設定以外的原因而以失敗結束（原因會列出它印到 stderr 的第一行）；overlay 檔案格式錯誤或大於 1 MiB；某份清單在上述上限之一處被截斷 |
| `platform.unsupported` | 提供者 `board.firmware`：`firmware` 早於 2.1.2，即 Sentinel 支援 MIPI 攝影機的最舊版本。該韌體沒有 SiMa 攝影機媒體驅動程式，因此 `camera.mipi` 不會列出攝影機；USB 攝影機與麥克風仍會列出 |

每份清單的問題彙整為一個錯誤回報，附上第一個問題的代碼與原因，以及其他問題的數量。
對 `supported_sensors` 而言，`/boot` 或其中某個目錄的清單無法讀取或被截斷，會與被略過的
overlay 檔案一併計算；對 `configured_cameras` 而言，I2C 裝置清單被截斷，會與無法讀取或
被截斷的裝置一併計算。若缺少 `fw_printenv`，
或它以失敗結束並表示變數 `not defined`（如同
u-boot-tools 在 `dtbos` 未設定時的行為），則會省略 `overlays` 且不回報
錯誤。DevKit 隨附的 libubootenv 對未設定的變數會印出空值，因此此時 `overlays` 為空清單。
任何其他失敗都會保留最後的清單並回報錯誤；在任何一次執行成功之前，會省略 `overlays`。沒有裝置樹、I2C 裝置或 `/boot` 的
開發板會回報空清單，且不回報
錯誤。

## 限制

- CSI-2 規則是結構性的。具有自己的 CSI-2 端點、且下方沒有
  感測器的裝置，即使不是攝影機也會被列出，例如
  HDMI 轉 CSI-2 橋接器，或未描述其感測器的
  GMSL 解串器。
- 若 overlay 將感測器加到某個 I2C 匯流排，而該匯流排的標籤或路徑不是
  以 `i2c` 開頭，則不會被辨識。
- `data_lanes` 取自依節點順序的第一個端點；具有
  多個不同寬度端點的感測器只會回報其中一個。
- 端點沒有 `data-lanes` 的感測器不會被辨識為 CSI-2
  來源，因此不會出現在 `configured_cameras` 與 `supported_sensors` 中。
- 具有自己 CSI-2 端點的輔助晶片會被列為已設定的
  攝影機，例如 METOAK-DUAL overlay 中的 `Metoak,xc9080`。沒有任何
  `camera.mipi` 攝影機以它為感測器，因此它永遠不會有 `camera_id`。
- 沒有任何機制監看 `/boot` 或 U-Boot 環境。以 `fw_setenv` 所做的變更，或安裝、移除的
  overlay 檔案，會在下一次掃描時出現；下一次掃描會在攝影機或音訊裝置事件之後，或重新整理時執行。

## 範例

Modalix DevKit 上的 IMX477，如
[`catalog-example.json`](catalog-example.json) 中所示。型號、裝置樹路徑、
I2C 裝置與 overlay 名稱抄錄自 DevKit，而非從
Sentinel 擷取；此區塊由合成的測試資料涵蓋。安裝了更多
overlay 的開發板會在 `supported_sensors` 中列出更多 overlay。

```json
"board": {
  "model": "SiMa.ai Modalix SoM 16Gig Board",
  "overlays": ["modalix-som-waveshare-ARDU-IMX477-1CAM.dtbo"],
  "configured_cameras": [{
    "compatible": "sony,imx477",
    "dt_node": "/i2cmux@0/i2c@0/imx477@1a",
    "i2c_device": "5-001a",
    "data_lanes": 2,
    "camera_id": "camera:imx477 5-001a"
  }],
  "supported_sensors": [
    {"compatible": "sony,imx477",
     "overlays": ["modalix-som-waveshare-ARDU-IMX477-1CAM.dtbo"]}
  ]
}
```
