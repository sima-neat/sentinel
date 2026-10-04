# カメラ

Neat アプリケーションがキャプチャできる画像センサー、つまり Modalix ISP の背後にある
MIPI CSI-2 センサーと USB Video Class（UVC）カメラを対象にします。

- **タイプトークン:** `camera`
- **プロバイダー:** `daemon.camera.mipi`（組み込み）、`daemon.camera.v4l2`（組み込み、USB）
- **再スキャントリガー:** `media`、`video4linux`

## 識別子

| カメラ | `id` | 構成要素 |
| --- | --- | --- |
| MIPI | `camera:<sensor entity name>`、例: `camera:imx477 5-001a` | センサーの media-controller エンティティ名（ドライバー、I2C バス、アドレス）。libcamera も同じ名前を使うため、`CameraInput` が受け入れる名前でもあります。 |
| USB | `camera:v4l2:<16 hex digits>` | USB トポロジー（バスパス、インターフェース、インターフェース内のノード索引）のハッシュ。同じポートへの再接続や `/dev/videoN` の番号変更では変わらず、別ポートでは別カメラになります。 |

## 詳細: 両方の種類

| フィールド | 型 | 常に存在 | 意味 |
| --- | --- | --- | --- |
| `backend` | string | yes | `mipi` または `v4l2` |
| `connection` | string | yes | `mipi-csi2` または `usb` |
| `model` | string | no | センサーまたは製品モデル |
| `availability` | object | yes | `{"state": "unknown", "reason": ...}`。検出処理はストリームを開かないため、カメラが使用中かどうかは判定できません。 |
| `modes` | array | yes | カメラが出力できる内容。下記参照 |

## 詳細: MIPI のみ

| フィールド | 型 | 常に存在 | 意味 | 情報源 |
| --- | --- | --- | --- | --- |
| `camera_name` | string | yes | `CameraInput` に渡す名前 | `simaai-v4l2-vid` メディアデバイス上のセンサーエンティティ名 |
| `media_device` | string | yes | メディアデバイスノード。例: `/dev/media0`（経路のみで識別子ではない） | `/dev/media*` |
| `bus_info` | string | no | メディアデバイスのバス。例: `platform:csi2video@1` | `MEDIA_IOC_DEVICE_INFO` |
| `isp` | object | yes | `{"state": "available", "device_path", "device_paths"}`、または `{"state": "unavailable", "reason"}` と `modes: []` | ISP 出力ノード（`isp_v4l2-vid-cap-out`、カード `arm-isp-out`） |
| `csi_receiver` | string | no | センサーが接続する CSI-2 受信エンティティ。例: `csidev-40c3000.csi` | `MEDIA_IOC_G_TOPOLOGY` にある、センサーのソースパッドからのデータリンクの反対側（有効なリンクを優先し、次に最小パッド索引） |
| `sensor_timing` | object | no | `pixel_rate`（pixels/s）、`hblank_min`、`vblank_min`、`width`、`height` | センサーの `/dev/v4l-subdevN`（`/sys/dev/char/<major>:<minor>` から名前を得るインターフェースリンク）を読み取り専用で開き、リンク済みソースパッドの有効形式（`VIDIOC_SUBDEV_G_FMT`）、現在の `V4L2_CID_PIXEL_RATE`（`VIDIOC_G_EXT_CTRLS`）、`V4L2_CID_HBLANK` と `V4L2_CID_VBLANK` の最小値（`VIDIOC_QUERY_EXT_CTRL`）を取得。サブデバイス、形式、コントロールのいずれかがない場合は省略 |
| `max_fps` | number | `sensor_timing` と共に | 有効形式におけるセンサーのフレームレート上限（小数第 2 位まで） | `pixel_rate / ((width + hblank_min) * (height + vblank_min))` |

## 詳細: USB のみ

| フィールド | 型 | 常に存在 | 意味 |
| --- | --- | --- | --- |
| `device_path` | string | no | `/dev/videoN`（経路のみで識別子ではない） |
| `identity` | object | yes | `stable_key`、`topology`、`interface`、`node_index`、`vendor_id`、`product_id`。デバイスが報告する場合は `serial`、`manufacturer`、`speed` も含む（この値はカーネルが出力する USB の sysfs 速度 Mb/s。例: `"480"`、`"5000"`） |
| `by_id_path` | string | no | udev の `/dev/v4l/by-id/...` リンクで、`device_path` に解決される。udev リンクがなければ省略（経路のみで識別子ではない） |

## モード

| フィールド | 型 | 存在条件 | 意味 |
| --- | --- | --- | --- |
| `format` | string | always | V4L2 FourCC。例: `NV12`、`RGB3`、`AR24`、`MJPG`、`YUYV` |
| `format_description` | string | USB、ドライバーが返す場合 | ドライバーの `VIDIOC_ENUM_FMT` 説明。例: `Motion-JPEG`、`YUYV 4:2:2` |
| `width`, `height` | integer | 離散サイズ | フレームサイズ |
| `size_range` | object | 範囲（USB） | `min_width`、`min_height`、`max_width`、`max_height`、`step_width`、`step_height` |
| `framerate_num`, `framerate_den` | integer | always | フレームレート。USB は広告された最速の間隔を使用 |
| `frame_intervals` | array | USB | デバイスが広告するすべての間隔 |
| `isp_output` | bool | MIPI | `true`: ISP 出力サイズ |
| `framerate_source` | string | MIPI | `isp`（ISP の離散フレーム間隔、または間隔範囲の最速の有効レート）、`sensor_timing`（`max_fps` までのレート）、または `nominal`（30/1: どちらも不明） |
| `supported`, `reason` | bool, string | always | Neat Core のルールに基づきサポート段階が追加 |

MIPI モードは ISP 出力ノードの形式と離散サイズであり、`CameraInput` が実際にキャプチャできる内容です。
libcamera は ISP が生成できないサイズを含む、より長い一覧を広告します（sima-neat/core#883 参照）。

サイズごとの MIPI フレームレートは、ISP が離散フレーム間隔を示す場合はその値、またはステップワイズ／連続の間隔範囲で最速の有効レートです。それ以外で
`max_fps` が分かる場合は、`max_fps` を最も近い整数（最低 1）に丸めたレートと、それ以下の
標準レート 60、30、25、20、15、10、5 を速い順に生成します（66.18 なら 66、60、30、25、
20、15、10、5）。どちらも不明なら公称 30/1 モードを 1 つ生成します。`max_fps` はセンサーの
有効形式での上限で、すべての ISP サイズに適用されます。別のセンサーモードが必要なサイズは
さらに遅い場合があります。

## レコード例

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

USB カメラの詳細（省略形）:

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

## 対応する変動

- 1 つのメディアデバイス上の複数センサー、および複数の SiMa メディアデバイス。それぞれ独自の
  受信機とタイミングを持つ。
- サブデバイスノード、読み取り可能な有効形式、pixel-rate や blanking コントロールがないセンサー
  （`sensor_timing` なし、公称レート）。リンクがない、上限を超えるリンクがある、media API が
  4.19 より古い（パッド索引がなく `sensor_timing` なし）グラフ。複数ソースパッドを持つセンサー
  （リンク済みの有効パッドを使用）。
- センサーのないメディアデバイスと、別ドライバーのメディアデバイス（無視）。
- ISP ノードがない、読めない、フレーム間隔を報告する、または複数存在する場合
  （すべての ISP ノードが共有するモードのみ報告）。
- manufacturer、speed、`/dev/v4l/by-id` リンクがある／ない USB カメラ、解決できない by-id リンク、
  ドライバー説明がある／ない形式。
- 複数の同一 USB カメラ、シリアルのないカメラ、複合デバイス（カメラとマイク）、メタデータ専用・
  出力専用ビデオノード（除外）、離散・段階・連続のサイズと間隔。
- スキャン間でデバイスノード番号が変わる場合。

## サポートルール

Neat Core は `/usr/share/simaai-sentinel/support/neat-core.json` をインストールします。
カメラルールは `backend`、`format`、フレームレート、サイズ範囲（常に未サポート）、
`isp_output` の順に検査します。Neat Core がなければ、すべてのモードは `supported: false` で、
理由は「Neat Core is not installed」になります。[サポートルール](../README.md#support-rules)を参照してください。

## 検証

| 動作 | 実機 | フィクスチャのみ |
| --- | --- | --- |
| IMX477 の名前、メディアグラフ、ISP サイズ | DevKit キャプチャ（2.1.3）から転記 | |
| `/dev/media*` と ISP ノードによるライブ検出: 名前が libcamera と一致、ISP ノード 16、モード 9、NV12 をサポート | DevKit、2026-10-03 | 合成カーネルインターフェース |
| Logitech C920: 1 レコード、メタデータノードを除外、`v4l2-ctl` と一致する 17 MJPG / 18 YUYV モード、再接続後も同じ id | DevKit、2026-10-03 | 合成、v1 カタログフィクスチャと一致 |
| 1920x1080 ストリーム中の検出でフレームを落とさない | DevKit、2026-10-03 | |
| ホットプラグ、複数カメラ、番号変更 | | 合成 |
| `csi_receiver`、`sensor_timing`、`max_fps`、sensor-timing レート | 未実施。テストの IMX477 値（840 MHz、HBLANK 9332、VBLANK 48、1920x1080、66.18 fps）は DevKit の報告値で、65～66 fps を測定。グラフのリンクは `media-ctl -p` から転記 | 合成サブデバイスとコントロール、合成デバイス番号とグラフ id |
| USB の `manufacturer`、`speed`、`by_id_path`、`format_description` | 未実施 | 合成 sysfs、udev リンク、ドライバー説明 |
