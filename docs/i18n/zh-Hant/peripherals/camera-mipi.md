# MIPI CSI-2 攝影機

`camera.mipi` 提供者會回報 Modalix ISP 後方的影像感測器。
它會在 `media` 與 `video4linux` uevent 發生時重新掃描。
探索功能以唯讀方式開啟裝置節點，且只發出查詢用的 ioctl；
它從不設定格式或連結，也從不串流。

`simaai-v4l2-vid` 驅動程式的每個 `/dev/mediaN` 都會以
`MEDIA_IOC_G_TOPOLOGY` 讀取，其圖中的每個 `MEDIA_ENT_F_CAM_SENSOR` 實體即為
一台攝影機。其他驅動程式的媒體裝置（例如 `uvcvideo`）會被忽略。

## 識別

`id` 為 `camera:<sensor entity name>`，例如 `camera:imx477 5-001a`：
感測器驅動程式、I2C 匯流排與位址。
libcamera 使用相同的名稱，因此這也是 `CameraInput` 接受的名稱。
名稱相同的兩個感測器（例如位於兩個媒體裝置上）會共用同一個 id，
因此掃描會改為失敗，並回報一個同時列出兩個媒體裝置的錯誤。

## 欄位

| 欄位 | 出現時機 | 意義 |
| --- | --- | --- |
| `type`、`id` | 一律 | `camera`，以及上述的識別碼 |
| `backend` | 一律 | `mipi` |
| `model` | 名稱中含有時 | 實體名稱的第一個單字，例如 `imx477` |
| `availability` | 一律 | `{"state": "unknown", "reason": ...}`：媒體控制器沒有可唯讀查詢的擁有權狀態 |
| `camera_name` | 一律 | 感測器實體名稱，用來傳給 `CameraInput` |
| `media_device` | 一律 | `/dev/mediaN`（僅用於路由；不作為識別） |
| `bus_info` | 有回報時 | 來自 `MEDIA_IOC_DEVICE_INFO` 的媒體裝置匯流排，例如 `platform:csi2video@1` |
| `isp` | 一律 | `{"state": "available", "device_path", "device_paths", "sizing"}`，或 `{"state": "unavailable", "reason"}` 並搭配 `modes: []` |
| `csi_receiver` | 已連結時 | 感測器來源 pad 所連結的實體，例如 `csidev-40c3000.csi` |
| `sensor_timing` | 可讀取時 | `pixel_rate`（像素/秒）、`hblank_min`、`vblank_min`、`width`、`height`，讀取自感測器的 `/dev/v4l-subdevN` |
| `max_fps` | 有 `sensor_timing` 時 | `pixel_rate / ((width + hblank_min) * (height + vblank_min))`，取到小數點後兩位 |

`sensor_timing` 來自感測器的子裝置介面（透過
`/sys/dev/char/<major>:<minor>` 命名）：所連結來源 pad 的作用中格式
（`VIDIOC_SUBDEV_G_FMT`）、目前的 `V4L2_CID_PIXEL_RATE`，以及
`V4L2_CID_HBLANK` 與 `V4L2_CID_VBLANK` 的最小值。只要其中任一項缺少，
就會省略此欄位。

ISP 輸出節點是名稱為
`isp_v4l2-vid-cap-out`、card 為 `arm-isp-out` 的 `video4linux` 項目。若有多個，
只會回報它們共同支援的模式。

`sizing` 說明 ISP 輸出尺寸的來源，依 ISP 列出的尺寸判定，而非平台版本。
依其設定的感測器在執行階段決定尺寸的 ISP（Platform 3.0），在設定前會列出 0x0：
只要任一 ISP 節點列出 0x0 尺寸，`sizing` 即為 `runtime`。否則（Platform 2.1.x），
尺寸是驅動程式內建的表格，`sizing` 為 `fixed`。

## 模式

模式是 ISP 的輸出格式與離散尺寸，也就是
`CameraInput` 能擷取的內容。每個模式都有 `format`（FourCC）、`width`、`height`
與 `isp_output: true`。只有當 ISP 回報該尺寸的影格間隔時，模式才會帶有 `frame_intervals`，
其格式與 USB 攝影機相同。
DevKit 的 ISP 不會回報任何間隔，因此其模式沒有影格率；`max_fps` 與
`sensor_timing` 描述的是感測器的上限。

當 `sizing: "runtime"` 時，ISP 只會列出目前設定的尺寸（閒置時為 0x0），因此不會採用
這些尺寸。此時的模式，是所有 ISP 節點都列出的格式，搭配感測器子裝置在所有媒體匯流排代碼下
回報的每個離散影格尺寸（`VIDIOC_SUBDEV_ENUM_MBUS_CODE`、`VIDIOC_SUBDEV_ENUM_FRAME_SIZE`）。
每個模式都有 `format`、`width`、`height` 與 `sensor_mode: true`；若感測器回報該尺寸的
影格間隔（`VIDIOC_SUBDEV_ENUM_FRAME_INTERVAL`），也會帶有 `frame_intervals`。以範圍表示的
尺寸會略過。若無法列出感測器的尺寸，該攝影機的 `isp` 為不可用，並附上原因。

每個模式也都帶有 `available`，為 false 時另有 `reason`，表示板子是否已為該模式完成設定。
判斷規則請參閱[目錄](../api.md)。

## 錯誤

| 代碼 | 發生時機 |
| --- | --- |
| `io.permission_denied` | 無法開啟媒體裝置（`EACCES`、`EPERM`），或無法列出 `/dev`（`EACCES`） |
| `io.open` | 媒體裝置的任何其他開啟或查詢失敗 |
| `peripherals.discovery_failed` | 未命名的感測器實體，或兩個名稱相同的感測器（其 id 會衝突） |

掃描途中消失的裝置會被略過。ISP 失敗絕不會使掃描失敗；
而是會使 `isp` 變為無法使用，並附上原因。

## 範例

Modalix DevKit 上的 IMX477，模式已節錄（共九個：三種格式、三種尺寸）。
圖與 ISP 尺寸抄錄自 DevKit；
感測器時序數值與 DevKit 回報的相同，
但僅由測試涵蓋。

```json
{"type": "camera", "id": "camera:imx477 5-001a", "model": "imx477",
 "availability": {"state": "unknown", "reason": "The media controller does not expose a reliable read-only ownership state; discovery does not acquire, configure, or stream from the camera."},
 "modes": [{"format": "AR24", "width": 1920, "height": 1080, "isp_output": true},
           {"format": "AR24", "width": 2048, "height": 1080, "isp_output": true}],
 "backend": "mipi", "camera_name": "imx477 5-001a", "media_device": "/dev/media0",
 "bus_info": "platform:csi2video@1",
 "isp": {"state": "available", "device_path": "/dev/video1", "device_paths": ["/dev/video1"],
         "sizing": "fixed"},
 "csi_receiver": "csidev-40c3000.csi",
 "sensor_timing": {"pixel_rate": 840000000, "hblank_min": 9332, "vblank_min": 48,
                   "width": 1920, "height": 1080},
 "max_fps": 66.18}
```
