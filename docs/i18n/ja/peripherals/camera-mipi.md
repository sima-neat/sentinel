# MIPI CSI-2カメラ

`camera.mipi`プロバイダーは、Modalix ISPの背後にあるイメージセンサーを報告します。
`media`および`video4linux`のueventで再スキャンします。
検出処理はデバイスノードを読み取り専用で開き、照会用のioctlのみを発行します。
フォーマットやリンクを設定することはなく、ストリーミングも行いません。

`simaai-v4l2-vid`ドライバーの各`/dev/mediaN`は
`MEDIA_IOC_G_TOPOLOGY`で読み取られ、そのグラフ内の各`MEDIA_ENT_F_CAM_SENSOR`エンティティが1台のカメラになります。
他のドライバーのメディアデバイス（たとえば`uvcvideo`）は無視されます。

## 識別情報

`id`は`camera:<sensor entity name>`で、たとえば`camera:imx477 5-001a`のように、
センサードライバー、I2Cバス、アドレスで構成されます。libcameraも同じ名前を使用するため、
これは`CameraInput`が受け付ける名前でもあります。
同じ名前を持つ2つのセンサー（たとえば2つのメディアデバイス上にある場合）はidが重複するため、
その場合、スキャンは両方のメディアデバイスを示すエラーで失敗します。

## フィールド

| フィールド | 出現条件 | 意味 |
| --- | --- | --- |
| `type`、`id` | 常に | `camera`、および上記の識別情報 |
| `backend` | 常に | `mipi` |
| `model` | 名前に含まれる場合 | エンティティ名の最初の単語。例：`imx477` |
| `availability` | 常に | `{"state": "unknown", "reason": ...}`：メディアコントローラーには読み取り専用で取得できる所有状態がありません |
| `camera_name` | 常に | センサーエンティティ名。`CameraInput`に渡します |
| `media_device` | 常に | `/dev/mediaN`（ルーティング専用。識別には使用しません） |
| `bus_info` | 報告される場合 | `MEDIA_IOC_DEVICE_INFO`から取得したメディアデバイスのバス。例：`platform:csi2video@1` |
| `isp` | 常に | `{"state": "available", "device_path", "device_paths"}`、または`{"state": "unavailable", "reason"}`（この場合は`modes: []`） |
| `csi_receiver` | リンクされている場合 | センサーのソースパッドのリンク先エンティティ。例：`csidev-40c3000.csi` |
| `sensor_timing` | 読み取れる場合 | `pixel_rate`（ピクセル/秒）、`hblank_min`、`vblank_min`、`width`、`height`。センサーの`/dev/v4l-subdevN`から読み取ります |
| `max_fps` | `sensor_timing`がある場合 | `pixel_rate / ((width + hblank_min) * (height + vblank_min))`。小数点以下2桁 |

`sensor_timing`は、センサーのサブデバイスインターフェース（`/sys/dev/char/<major>:<minor>`で名前を解決）から取得します。
内容は、リンクされたソースパッドのアクティブなフォーマット（`VIDIOC_SUBDEV_G_FMT`）、
現在の`V4L2_CID_PIXEL_RATE`、
および`V4L2_CID_HBLANK`と`V4L2_CID_VBLANK`の最小値です。
いずれかが欠けている場合は省略されます。

ISP出力ノードは、`isp_v4l2-vid-cap-out`という名前で、
カードが`arm-isp-out`である`video4linux`エントリです。
複数存在する場合は、すべてに共通するモードのみが報告されます。

## モード

モードはISPの出力フォーマットと離散サイズであり、
これは`CameraInput`がキャプチャできるものです。各モードには`format`（FourCC）、`width`、`height`、
および`isp_output: true`があります。モードは、ISPがそのサイズの間隔を報告する場合にのみ、
USBカメラと同じ形式の`frame_intervals`を持ちます。
DevKitのISPは間隔を報告しないため、そのモードにはフレームレートがありません。
`max_fps`と`sensor_timing`がセンサーの上限を示します。

## エラー

| コード | 発生条件 |
| --- | --- |
| `io.permission_denied` | メディアデバイスを開けない（`EACCES`、`EPERM`）、または`/dev`を一覧表示できない（`EACCES`） |
| `io.open` | メディアデバイスのその他のオープンまたは照会の失敗 |
| `peripherals.discovery_failed` | 名前のないセンサーエンティティ、または同じ名前を持つ2つのセンサー（idが衝突するため） |

スキャン中に消えたデバイスはスキップされます。ISPの失敗によってスキャンが失敗することはなく、
理由とともに`isp`が利用不可になります。

## 例

Modalix DevKit上のIMX477の例です（モードは省略しています。全部で9つ：
3つのフォーマット×3つのサイズ）。グラフとISPのサイズは
DevKitから書き写したものです。センサータイミングの値はDevKitが報告するとおりですが、
テストでのみ検証されています。

```json
{"type": "camera", "id": "camera:imx477 5-001a", "model": "imx477",
 "availability": {"state": "unknown", "reason": "The media controller does not expose a reliable read-only ownership state; discovery does not acquire, configure, or stream from the camera."},
 "modes": [{"format": "AR24", "width": 1920, "height": 1080, "isp_output": true},
           {"format": "AR24", "width": 2048, "height": 1080, "isp_output": true}],
 "backend": "mipi", "camera_name": "imx477 5-001a", "media_device": "/dev/media0",
 "bus_info": "platform:csi2video@1",
 "isp": {"state": "available", "device_path": "/dev/video1", "device_paths": ["/dev/video1"]},
 "csi_receiver": "csidev-40c3000.csi",
 "sensor_timing": {"pixel_rate": 840000000, "hblank_min": 9332, "vblank_min": 48,
                   "width": 1920, "height": 1080},
 "max_fps": 66.18}
```
