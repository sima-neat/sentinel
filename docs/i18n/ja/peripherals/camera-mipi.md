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
| `isp` | 常に | `{"state": "available", "device_path", "device_paths", "sizing"}`、または`{"state": "unavailable", "reason"}`（この場合は`modes: []`） |
| `modes_error` | `isp`が利用可能だがモードを列挙できない場合 | `modes`が`[]`である理由：センサーのサイズを列挙できなかった、または数が多すぎる |
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

`sizing`は、ISPの出力サイズの由来を示します。プラットフォームのバージョンではなく、
ISPが列挙するサイズで判定します。設定されたセンサーに合わせて実行時にサイズを決めるISP
（Platform 3.0）は、設定されるまで0x0を列挙します。いずれかのISPノードが0x0のサイズを
列挙する場合、`sizing`は`runtime`です。それ以外（Platform 2.1.x）では、サイズはドライバーに
組み込まれたテーブルであり、`sizing`は`fixed`です。

## モード

モードはISPの出力フォーマットと離散サイズであり、
これは`CameraInput`がキャプチャできるものです。各モードには`format`（FourCC）、`width`、`height`、
および`isp_output: true`があります。モードは、ISPがそのサイズの間隔を報告する場合にのみ、
USBカメラと同じ形式の`frame_intervals`を持ちます。
DevKitのISPは間隔を報告しないため、そのモードにはフレームレートがありません。
`max_fps`と`sensor_timing`がセンサーの上限を示します。

`sizing: "runtime"`の場合、ISPは現在設定されているサイズ（待機中は0x0）のみを列挙するため、
それらのサイズは使用しません。モードは、すべてのISPノードが列挙するフォーマットと、
センサーのサブデバイスがすべてのメディアバスコードについて報告する離散フレームサイズ
（`VIDIOC_SUBDEV_ENUM_MBUS_CODE`、`VIDIOC_SUBDEV_ENUM_FRAME_SIZE`）の組み合わせです。
各モードには`format`、`width`、`height`、`sensor_mode: true`、`isp_input`があり、センサーがその
サイズの間隔を報告する場合（`VIDIOC_SUBDEV_ENUM_FRAME_INTERVAL`）は`frame_intervals`も
持ちます。範囲で示されるサイズはスキップされます。センサーのサイズを列挙できない場合、
そのカメラは`modes: []`となり、`modes_error`がその理由を示します。`isp`は引き続きISPを報告します。
Sentinelがカメラごとに公開するフォーマットとサイズの組み合わせは最大1,024個です。それを超える場合は、
無制限のカタログを割り当てる代わりに同じ方法で報告されます。

`isp_input`は、センサーがそのサイズを12ビットBayerのメディアバスコード
（`MEDIA_BUS_FMT_S{BGGR,GBRG,GRBG,RGGB}12_1X12`）で列挙し、幅が64の倍数、高さが偶数で、
4096x3072以下である場合に`true`になります。SiMaのlibcameraハンドラーはセンサーを12ビットRAWで設定し、
ISPのRAW DMAは32バイトのラインストライド（64ピクセルの倍数の12ビットライン）を必要とし、
ISPにはスケーラーがないため、ISPはそのようなモードをそのサイズのまま出力します。これらの制限を満たさない
サイズはlibcameraが縮小します（IMX477の4056x3040は、Platform 3.0では4032x3040でストリーミングされます）。

固定ISPの各モードは`available`と、それがfalseの場合は`reason`も持ち、ボードがその正確な
出力用に設定されているかどうかを示します。NV12以外のセンサーモードも利用不可と判断できます。
`isp_input: true`を持つNV12の`sensor_mode`は利用可能です。
それ以外のものは、それ以上の大きさの`isp_input`モードがない場合は利用不可となり、ある場合は判定が付きません。
libcameraがその大きいモードをこのサイズに切り抜く可能性がありますが、Sentinelの読み取り専用検出は
カメラを設定したりストリーミングしたりせずにそれを確認できないためです。Platform 3.0上のIMX477では、
これにより1280x990、1920x1080、2048x1080、3840x2160、3968x3040、4032x3040が利用可能、
幅4056の2つのサイズが利用不可、1332x990、2028x1080、2028x1520が未判定になります。
完全な規則は[カタログ](../api.md)にあります。

## エラー

| コード | 発生条件 |
| --- | --- |
| `io.permission_denied` | メディアデバイスを開けない（`EACCES`、`EPERM`）、または`/dev`を一覧表示できない（`EACCES`） |
| `io.open` | メディアデバイスのその他のオープンまたは照会の失敗 |
| `peripherals.discovery_failed` | 名前のないセンサーエンティティ、または同じ名前を持つ2つのセンサー（idが衝突するため） |

スキャン中に消えたデバイスはスキップされます。ISPの失敗によってスキャンが失敗することはなく、
理由とともに`isp`が利用不可になります。サイズを列挙できないセンサーには、
代わりに`modes_error`が付きます。

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
 "isp": {"state": "available", "device_path": "/dev/video1", "device_paths": ["/dev/video1"],
         "sizing": "fixed"},
 "csi_receiver": "csidev-40c3000.csi",
 "sensor_timing": {"pixel_rate": 840000000, "hblank_min": 9332, "vblank_min": 48,
                   "width": 1920, "height": 1080},
 "max_fps": 66.18}
```
