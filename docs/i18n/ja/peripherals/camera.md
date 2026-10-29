# USBカメラ

`camera.v4l2`プロバイダーがV4L2を通じて検出するUSB Video Class（UVC）カメラです。
`video4linux`のueventが発生するたびに再スキャンします。

プロバイダーは`/sys/class/video4linux`を走査し、USBの祖先を持つノードのみを残すため、
プラットフォームノードやISPノードが開かれることはありません。
各候補は`O_RDONLY | O_NONBLOCK`で開かれ、照会用のioctl（`VIDIOC_QUERYCAP`、
`VIDIOC_ENUM_FMT`、`VIDIOC_ENUM_FRAMESIZES`、`VIDIOC_ENUM_FRAMEINTERVALS`）のみを受け取ります。
メタデータ専用、出力専用、およびメモリ間（memory-to-memory）のノードは`VIDIOC_QUERYCAP`の後で除外されるため、
メタデータノードを持つUVCカメラは1回だけ表示されます。

## 識別情報

`id`は`camera:v4l2:<16 hex digits>`で、
`identity.stable_key`（`sysfs:<USB topology>:interface=<bInterfaceNumber>:index=<node index>`）のFNV-1aハッシュです。
同じポートへの再接続や`/dev/videoN`の番号の振り直しがあっても変わりません。
異なるポートに接続すると別のカメラとして扱われ、
異なるポートに接続された同一のカメラにはそれぞれ異なるidが付きます。

`identity.camera_name`は`uvcvideo`ドライバーのノードにのみあり、Neatの`UsbCameraInput`がカメラを開くときの名前
（`UsbCameraInputOptions::camera_name`）です。Neatがlibcameraにならって組み立てるのと同じ方法で組み立てます。
USBデバイス、またはファームウェアノードを持つ最も近いその祖先のファームウェアノード
（`/sys/firmware/devicetree`以下のデバイスツリーのパス、なければACPIの`firmware_node/path`）、
バス番号を除いたインターフェース名、`idVendor:idProduct`からなり、
例は`/base/pci@7000000000-3.1:1.0-1a2b:3c4d`です。いずれかの部分を読み取れない場合は省略されます。

## フィールド

| フィールド | 出現条件 | 意味 |
| --- | --- | --- |
| `type` | 常に | `camera` |
| `id` | 常に | 上記を参照 |
| `backend` | 常に | `v4l2`（USBカメラ） |
| `model` | 判明している場合 | USBの`product`文字列。ない場合はドライバーのカード名 |
| `device_path` | 常に | `/dev/videoN`。ルーティング専用で、識別には使用しません |
| `by_id_path` | udevリンクがある場合 | `device_path`に解決される`/dev/v4l/by-id/...`リンク |
| `identity` | 常に | `stable_key`、`topology`、`interface`、`node_index`、およびUSBデバイスが報告する場合は`vendor_id`、`product_id`、`serial`、`manufacturer`、`speed`（カーネルが出力するとおりのMb/s単位のsysfs速度。例：`"480"`）、ノードのUSBインターフェースにカーネルドライバーがバインドされている場合は`driver`（例：`uvcvideo`）、`camera_name`（上記を参照） |
| `availability` | 常に | `{"state": "unknown", "reason": ...}`：検出処理はストリームを開かないため、カメラが使用中かどうかを判別できません |
| `modes` | 常に | カメラが出力できる内容。下記を参照 |

## モード

フォーマットとフレームサイズごとに1つのモードがあり、フォーマット順、次にサイズ順に並べられます。
同じフォーマットのシングルプレーナーとマルチプレーナーの一覧は統合されます。

| フィールド | 出現条件 | 意味 |
| --- | --- | --- |
| `format` | 常に | V4L2 FourCC。例：`MJPG`、`YUYV`、`NV12` |
| `format_description` | ドライバーが提供する場合 | `VIDIOC_ENUM_FMT`の説明。例：`Motion-JPEG` |
| `width`、`height` | 離散サイズの場合 | フレームサイズ |
| `size_range` | ステップ単位または連続サイズの場合 | `type`（`stepwise`または`continuous`）、`min_width`、`min_height`、`max_width`、`max_height`、`step_width`、`step_height` |
| `frame_intervals` | 常に | プローブした各サイズ（`width`、`height`）について、デバイスが通知するすべての間隔：`{"type": "discrete", "numerator", "denominator"}`または`{"type": "stepwise" or "continuous", "minimum", "maximum", "step"}` |

サイズ範囲は、その最小サイズと最大サイズでプローブされます。
フレーム間隔のないサイズにはモードがありません。各モードは`available`も持ち、falseの場合は
`reason`も持ちます（[カタログ](../api.md)を参照）。Neatの`UsbCameraInput`は
`uvcvideo`カメラを`camera_name`で開き、カメラが列挙するサイズとフレーム間隔でMJPEGをキャプチャするため、
そのようなカメラの`MJPG`モードは、カメラの先のノードが列挙していない限り利用可能です（下記を参照）。
その他のフォーマットは利用不可で、他のドライバーのカメラのモードもすべて利用不可です。
`camera_name`のない`uvcvideo`カメラの`MJPG`モードは、`UsbCameraInput`がそのカメラを開けないため利用不可です。
`driver`のないノード（スキャン中にインターフェースのバインドまたはアンバインドが進行していたもの）のモードには判定が付きません。

複数のストリーミングインターフェースを持つカメラは、VideoControlインターフェース上に複数のキャプチャノードを持ち、
それらは同じ`camera_name`を共有します。複数のVideoControlインターフェースを持つデバイスは、それぞれに`camera_name`を持ちます。
`UsbCameraInput`はカメラのノードをデバイスパスの短い順（`/dev/video10`より先に`/dev/video2`）に試し、
要求されたMJPEGのサイズとレートを列挙する最初のノードを開きます。離散間隔は完全一致で、範囲はその境界内で一致します。
したがって`MJPG`モードは、カメラの先のノードがそのサイズをすべてのフレーム間隔で列挙している場合は利用不可で、
一部の間隔でのみ列挙している場合は判定が付きません。

## 制限とエラー

エラーが発生するとプロバイダーのスキャンが失敗し、その内容はカタログの
`errors`で報告されます：

- `io.permission_denied`：ノードまたはsysfsから`EACCES`が返された場合、または`EPERM`が
  ノードを開く際に返された場合。
- `io.open`：その他のドライバーまたはsysfsのエラー、1024エントリを超えるリスト、
  1つのデバイスに対する4096回を超える列挙クエリ、
  不正な形式のサイズまたは間隔。
- `peripherals.discovery_failed`：インターフェース番号または
  ノードインデックスを持たないUSBカメラ。

スキャン中に取り外されたカメラは、スキャンを失敗させずに結果から除外されます。
遅くとも、その取り外しによって発生する再スキャンの時点で除外されます。

## 例

説明用のレコードです。sysfsのトポロジーとACPIパスは簡略化しています。

```json
{"type": "camera", "id": "camera:v4l2:295faa7ac0d61654", "backend": "v4l2",
 "model": "UVC Webcam", "device_path": "/dev/video97",
 "by_id_path": "/dev/v4l/by-id/usb-1a2b_UVC_Webcam_A1B2-video-index0",
 "identity": {"stable_key": "sysfs:devices/pci0000:00/usb1/1-2.3:interface=00:index=0",
              "topology": "devices/pci0000:00/usb1/1-2.3", "interface": "00",
              "node_index": "0", "vendor_id": "1a2b", "product_id": "3c4d",
              "serial": "A1B2", "manufacturer": "Example Vendor", "speed": "480",
              "driver": "uvcvideo", "camera_name": "\\_SB_.PCI0-2.3:1.0-1a2b:3c4d"},
 "availability": {"state": "unknown", "reason": "V4L2 does not expose a reliable read-only ownership state; discovery does not acquire, configure, or stream from the camera."},
 "modes": [{"format": "MJPG", "format_description": "Motion-JPEG",
            "width": 1920, "height": 1080,
            "frame_intervals": [{"width": 1920, "height": 1080, "intervals": [
              {"type": "discrete", "numerator": 1, "denominator": 30}]}],
            "available": true}]}
```
