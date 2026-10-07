# USB 攝影機

USB Video Class（UVC）攝影機，由 `camera.v4l2`
提供者透過 V4L2 探索。每個 `video4linux` uevent 之後都會重新掃描。

提供者會走訪 `/sys/class/video4linux`，只保留具有 USB 上層裝置的節點，
因此絕不會開啟平台與 ISP 節點。每個候選節點都以
`O_RDONLY | O_NONBLOCK` 開啟，且只接收查詢用的 ioctl（`VIDIOC_QUERYCAP`、
`VIDIOC_ENUM_FMT`、`VIDIOC_ENUM_FRAMESIZES`、`VIDIOC_ENUM_FRAMEINTERVALS`）。
僅含中繼資料、僅輸出及記憶體對記憶體的節點會在
`VIDIOC_QUERYCAP` 之後被捨棄，因此具有中繼資料節點的 UVC 攝影機只會出現一次。

## 識別

`id` 為 `camera:v4l2:<16 hex digits>`，即 `identity.stable_key`
（`sysfs:<USB topology>:interface=<bInterfaceNumber>:index=<node index>`）的 FNV-1a 雜湊值。
它在重新插拔至同一連接埠以及 `/dev/videoN` 重新編號後保持不變。
不同的連接埠就是不同的攝影機，
插在不同連接埠的相同攝影機也會有不同的 id。

## 欄位

| 欄位 | 出現時機 | 意義 |
| --- | --- | --- |
| `type` | 一律 | `camera` |
| `id` | 一律 | 見上文 |
| `backend` | 一律 | `v4l2`（USB 攝影機） |
| `model` | 已知時 | USB 的 `product` 字串，否則為驅動程式回報的 card 名稱 |
| `device_path` | 一律 | `/dev/videoN`；僅用於路由，不作為識別 |
| `by_id_path` | 有 udev 連結時 | 解析至 `device_path` 的 `/dev/v4l/by-id/...` 連結 |
| `identity` | 一律 | `stable_key`、`topology`、`interface`、`node_index`，以及在 USB 裝置有回報時的 `vendor_id`、`product_id`、`serial`、`manufacturer` 與 `speed`（核心所印出的 sysfs 速度，單位為 Mb/s，例如 `"480"`） |
| `availability` | 一律 | `{"state": "unknown", "reason": ...}`：探索從不開啟串流，因此無法判斷攝影機是否正在使用中 |
| `modes` | 一律 | 攝影機可輸出的內容；見下文 |

## 模式

每種格式與影格尺寸各一個模式，先依格式、再依尺寸排序。
同一格式的單平面與多平面清單會合併。

| 欄位 | 出現時機 | 意義 |
| --- | --- | --- |
| `format` | 一律 | V4L2 FourCC，例如 `MJPG`、`YUYV`、`NV12` |
| `format_description` | 驅動程式有提供時 | `VIDIOC_ENUM_FMT` 的說明，例如 `Motion-JPEG` |
| `width`、`height` | 離散尺寸 | 影格尺寸 |
| `size_range` | 階梯式或連續尺寸 | `type`（`stepwise` 或 `continuous`）、`min_width`、`min_height`、`max_width`、`max_height`、`step_width`、`step_height` |
| `frame_intervals` | 一律 | 針對每個探測的尺寸（`width`、`height`），列出裝置宣告的所有間隔：`{"type": "discrete", "numerator", "denominator"}` 或 `{"type": "stepwise" or "continuous", "minimum", "maximum", "step"}` |

尺寸範圍會以其最小與最大尺寸進行探測。
沒有影格間隔的尺寸不構成模式。Sentinel 不判斷模式是否受支援；
由使用攝影機的應用程式做此判斷。

## 限制與錯誤

任何失敗都會使該提供者的掃描失敗，並回報於目錄的
`errors` 中：

- `io.permission_denied`：節點或 sysfs 傳回的 `EACCES`，或 `EPERM`（來自
  開啟節點）。
- `io.open`：任何其他驅動程式或 sysfs 錯誤；超過 1024 筆項目的清單；
  單一裝置上超過 4096 次列舉查詢；
  格式錯誤的尺寸或間隔。
- `peripherals.discovery_failed`：USB 攝影機缺少其介面編號或
  節點索引。

掃描期間被拔除的攝影機會被略過，而不會使掃描失敗；
最遲在其移除所觸發的重新掃描中就會被排除。

## 範例

```json
{"type": "camera", "id": "camera:v4l2:295faa7ac0d61654", "backend": "v4l2",
 "model": "HD Pro Webcam C920", "device_path": "/dev/video97",
 "by_id_path": "/dev/v4l/by-id/usb-046d_HD_Pro_Webcam_C920_A1B2-video-index0",
 "identity": {"stable_key": "sysfs:devices/pci0000:00/usb1/1-2.3:interface=00:index=0",
              "topology": "devices/pci0000:00/usb1/1-2.3", "interface": "00",
              "node_index": "0", "vendor_id": "046d", "product_id": "082d",
              "serial": "A1B2", "manufacturer": "Logitech", "speed": "480"},
 "availability": {"state": "unknown", "reason": "V4L2 does not expose a reliable read-only ownership state; discovery does not acquire, configure, or stream from the camera."},
 "modes": [{"format": "MJPG", "format_description": "Motion-JPEG",
            "width": 1920, "height": 1080,
            "frame_intervals": [{"width": 1920, "height": 1080, "intervals": [
              {"type": "discrete", "numerator": 1, "denominator": 30}]}]}]}
```
