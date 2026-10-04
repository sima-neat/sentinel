# 相機

Neat 應用程式可擷取影像的感測器：位於 Modalix ISP 後方的 MIPI CSI-2
感測器，以及 USB Video Class (UVC) 相機。

- **類型權杖：** `camera`
- **提供者：** `daemon.camera.mipi`（內建）、`daemon.camera.v4l2`（內建，USB）
- **重新掃描觸發條件：** `media`、`video4linux`

## 識別

| 相機 | `id` | 組成來源 |
| --- | --- | --- |
| MIPI | `camera:<sensor entity name>`，例如 `camera:imx477 5-001a` | 感測器的媒體控制器實體名稱：驅動程式、I2C 匯流排和位址。libcamera 使用相同名稱，因此也是 `CameraInput` 接受的名稱。 |
| USB | `camera:v4l2:<16 hex digits>` | USB 拓撲的雜湊（匯流排路徑、介面、介面中的節點索引）。重新插入相同連接埠及 `/dev/videoN` 重新編號時不變；不同連接埠視為不同相機。 |

## 詳細資料：兩種類型

| 欄位 | 類型 | 永遠存在 | 含義 |
| --- | --- | --- | --- |
| `backend` | 字串 | 是 | `mipi` 或 `v4l2` |
| `connection` | 字串 | 是 | `mipi-csi2` 或 `usb` |
| `model` | 字串 | 否 | 感測器或產品型號 |
| `availability` | 物件 | 是 | `{"state": "unknown", "reason": ...}`。探索不會開啟串流，因此無法判斷相機是否正在使用。 |
| `modes` | 陣列 | 是 | 相機可輸出的內容；見下文 |

## 詳細資料：僅 MIPI

| 欄位 | 類型 | 永遠存在 | 含義 | 來源 |
| --- | --- | --- | --- | --- |
| `camera_name` | 字串 | 是 | 傳給 `CameraInput` 的名稱 | `simaai-v4l2-vid` 媒體裝置上的感測器實體名稱 |
| `media_device` | 字串 | 是 | 媒體裝置節點，例如 `/dev/media0`（僅供路由，不作為識別） | `/dev/media*` |
| `bus_info` | 字串 | 否 | 媒體裝置匯流排，例如 `platform:csi2video@1` | `MEDIA_IOC_DEVICE_INFO` |
| `isp` | 物件 | 是 | `{"state": "available", "device_path", "device_paths"}`，或 `{"state": "unavailable", "reason"}` 且 `modes: []` | ISP 輸出節點（`isp_v4l2-vid-cap-out`，卡片 `arm-isp-out`） |
| `csi_receiver` | 字串 | 否 | 感測器所連接的 CSI-2 接收器實體，例如 `csidev-40c3000.csi` | 從感測器來源 pad 的資料連結另一端取得（先選啟用的連結，再選最低 pad 索引），來源為 `MEDIA_IOC_G_TOPOLOGY` |
| `sensor_timing` | 物件 | 否 | `pixel_rate`（像素/秒）、`hblank_min`、`vblank_min`、`width`、`height` | 感測器的 `/dev/v4l-subdevN`（介面連結，透過 `/sys/dev/char/<major>:<minor>` 命名），以唯讀方式開啟：已連結來源 pad 的有效格式（`VIDIOC_SUBDEV_G_FMT`）、目前的 `V4L2_CID_PIXEL_RATE`（`VIDIOC_G_EXT_CTRLS`），以及 `V4L2_CID_HBLANK` 和 `V4L2_CID_VBLANK` 的最小值（`VIDIOC_QUERY_EXT_CTRL`）。缺少子裝置、格式或任何控制項時省略 |
| `max_fps` | 數值 | 與 `sensor_timing` 一起 | 感測器在有效格式下的影格率上限，取至小數點後兩位 | `pixel_rate / ((width + hblank_min) * (height + vblank_min))` |

## 詳細資料：僅 USB

| 欄位 | 類型 | 永遠存在 | 含義 |
| --- | --- | --- | --- |
| `device_path` | 字串 | 否 | `/dev/videoN`（僅供路由，不作為識別） |
| `identity` | 物件 | 是 | `stable_key`、`topology`、`interface`、`node_index`、`vendor_id`、`product_id`，以及裝置有回報時的 `serial`、`manufacturer` 和 `speed`（核心在 sysfs 中列印的 USB 裝置速度，單位 Mb/s，例如 `"480"`、`"5000"`） |
| `by_id_path` | 字串 | 否 | udev `/dev/v4l/by-id/...` 連結，會解析到 `device_path`；沒有 udev 連結時不存在（僅供路由，不作為識別） |

## 模式

| 欄位 | 類型 | 出現條件 | 含義 |
| --- | --- | --- | --- |
| `format` | 字串 | 永遠 | V4L2 FourCC，例如 `NV12`、`RGB3`、`AR24`、`MJPG`、`YUYV` |
| `format_description` | 字串 | USB，且驅動程式有提供 | 驅動程式的 `VIDIOC_ENUM_FMT` 描述，例如 `Motion-JPEG`、`YUYV 4:2:2` |
| `width`, `height` | 整數 | 離散尺寸 | 影格尺寸 |
| `size_range` | 物件 | 範圍（USB） | `min_width`、`min_height`、`max_width`、`max_height`、`step_width`、`step_height` |
| `framerate_num`, `framerate_den` | 整數 | 永遠 | 影格率；USB 使用公告的最快間隔 |
| `frame_intervals` | 陣列 | USB | 裝置公告的每個間隔 |
| `isp_output` | 布林值 | MIPI | `true`：ISP 輸出尺寸 |
| `framerate_source` | 字串 | MIPI | `isp`（離散 ISP 影格間隔，或間隔範圍中最快的有效影格率）、`sensor_timing`（不超過 `max_fps` 的影格率），或 `nominal`（兩者皆未知時為 30/1） |
| `supported`, `reason` | 布林值、字串 | 永遠 | 由 Neat Core 規則的支援階段加入 |

MIPI 模式是 ISP 輸出節點的格式與離散尺寸，也就是 `CameraInput` 實際可
擷取的內容。libcamera 會公告更長的清單，其中包含 ISP 無法產生的尺寸
（見 sima-neat/core#883）。

各尺寸的 MIPI 影格率：若 ISP 列出離散影格間隔，就使用該清單；若列出步進／連續間隔範圍，則使用其中最快的有效影格率。否則，
若已知 `max_fps`，就為四捨五入至最接近整數的 `max_fps`（至少為 1），以及
所有低於它的標準影格率 60、30、25、20、15、10 和 5 各建立一個模式，
由快至慢排列（66.18 會產生 66、60、30、25、20、15、10、5）。若仍未知，
則使用一個名義上的 30/1 模式。`max_fps` 是感測器有效格式的上限，適用於
每個 ISP 尺寸；需要另一個感測器模式的尺寸可能較慢。

## 記錄範例

```json
{"id": "camera:imx477 5-001a", "type": "camera", "provider": "daemon.camera.mipi",
 "camera": {"camera_name": "imx477 5-001a", "model": "imx477", "backend": "mipi",
            "connection": "mipi-csi2", "media_device": "/dev/media0",
            "bus_info": "platform:csi2video@1", "csi_receiver": "csidev-40c3000.csi",
            "sensor_timing": {"pixel_rate": 840000000, "hblank_min": 9332, "vblank_min": 48,
                              "width": 1920, "height": 1080},
            "max_fps": 66.18,
            "availability": {"state": "unknown", "reason": "..."},
            "isp": {"state": "available", "device_path": "/dev/video1", "device_paths": ["/dev/video1"]},
            "modes": [{"format": "NV12", "width": 1920, "height": 1080,
                       "framerate_num": 66, "framerate_den": 1, "framerate_source": "sensor_timing",
                       "isp_output": true, "supported": false, "reason": "..."},
                      {"format": "NV12", "width": 1920, "height": 1080,
                       "framerate_num": 30, "framerate_den": 1, "framerate_source": "sensor_timing",
                       "isp_output": true, "supported": true, "reason": ""}]}}
```

USB 相機詳細資料的縮寫範例：

```json
{"model": "HD Pro Webcam C920", "backend": "v4l2", "connection": "usb",
 "device_path": "/dev/video0",
 "by_id_path": "/dev/v4l/by-id/usb-046d_HD_Pro_Webcam_C920_A1B2C3D4-video-index0",
 "identity": {"stable_key": "sysfs:devices/platform/.../usb1/1-1:interface=00:index=0",
              "topology": "devices/platform/.../usb1/1-1", "interface": "00", "node_index": "0",
              "vendor_id": "046d", "product_id": "082d", "serial": "A1B2C3D4",
              "manufacturer": "Logitech", "speed": "480"},
 "modes": [{"format": "MJPG", "format_description": "Motion-JPEG",
            "width": 1920, "height": 1080, "framerate_num": 30, "framerate_den": 1,
            "frame_intervals": [...], "supported": false, "reason": "..."}]}
```

## 涵蓋的變化

- 一個媒體裝置上有多個感測器，以及多個 SiMa 媒體裝置，每個都有自己的
  接收器和時序。
- 感測器沒有子裝置節點、可讀取的有效格式，或任何像素率與消隱控制項
  （沒有 `sensor_timing`，使用名義影格率）；圖形沒有連結、連結數超過上限，
  或媒體 API 早於 4.19（沒有 pad 索引，因此沒有 `sensor_timing`）；感測器
  有多個來源 pad（使用已連結且啟用的 pad）。
- 沒有感測器的媒體裝置及其他驅動程式的媒體裝置（忽略）。
- ISP 節點遺失、無法讀取、回報影格間隔，或存在多個節點（只回報所有 ISP
  節點共有的模式）。
- USB 相機有或沒有製造商、速度和 `/dev/v4l/by-id` 連結，by-id 連結沒有目標，
  以及格式有或沒有驅動程式描述。
- 多個相同 USB 相機、沒有序號的相機、複合裝置（相機加麥克風）、僅中繼資料
  和僅輸出的視訊節點（排除），以及離散、步進與連續的尺寸和間隔。
- 裝置節點在掃描之間重新編號。

## 支援規則

Neat Core 會安裝 `/usr/share/simaai-sentinel/support/neat-core.json`。相機規則
依序檢查 `backend`、`format`、影格率、尺寸範圍（永遠不支援）和
`isp_output`。若沒有 Neat Core，每個模式都是 `supported: false`，原因為
"Neat Core is not installed"。請參閱[支援規則](../README.md#support-rules)。

## 驗證

| 行為 | 真實硬體 | 僅固定資料 |
| --- | --- | --- |
| IMX477 名稱、媒體圖形和 ISP 尺寸 | 從 DevKit 擷取資料轉錄（2.1.3） | |
| 透過 `/dev/media*` 和 ISP 節點即時探索：名稱符合 libcamera、16 個 ISP 節點、9 個模式、支援 NV12 | DevKit，2026-10-03 | 合成的核心介面替身 |
| Logitech C920：一筆記錄、排除中繼資料節點、17 個 MJPG / 18 個 YUYV 模式符合 `v4l2-ctl`，拔除後重新插入仍為相同 id | DevKit，2026-10-03 | 合成，符合 v1 目錄固定資料 |
| 在 1920x1080 串流期間探索不會遺失影格 | DevKit，2026-10-03 | |
| 熱插拔、多部相機、重新編號 | | 合成 |
| `csi_receiver`、`sensor_timing`、`max_fps` 和感測器時序影格率 | 尚未。測試中的 IMX477 數值（840 MHz、HBLANK 9332、VBLANK 48、1920x1080、66.18 fps）來自 DevKit，實測為 65–66 fps；圖形連結轉錄自 `media-ctl -p` | 合成的子裝置和控制項；合成的裝置編號與圖形 id |
| USB `manufacturer`、`speed`、`by_id_path`、`format_description` | 尚未 | 合成 sysfs、udev 連結和驅動程式描述 |
