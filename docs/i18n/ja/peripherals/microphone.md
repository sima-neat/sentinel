# マイク

`microphone`は1つのALSAキャプチャPCMです。USB Audio Classマイク、
Webカメラやヘッドセットのマイク、オンボード（プラットフォーム）サウンドカードのキャプチャPCM、
または親デバイスを持たないカード（`snd-aloop`などの仮想カード）のキャプチャPCMが該当します。
再生専用のカードと再生PCMは報告されません。
Webカメラのカメラ部分は別の`camera`デバイスになります。

- **プロバイダー：** `microphone.alsa`
- **ueventで再スキャンするサブシステム：** `sound`

検出処理はテキストとリンクのみを読み取ります。カードとキャプチャPCMは
`/sys/class/sound`から、各カードの`id`、`device`リンク、USBの祖先とともに読み取ります。
カード名とドライバーは`/proc/asound/cards`から読み取ります（行のidがカードのsysfs idと一致する場合のみ使用）。
さらに、各カードの`pcmMc/info`と、USBオーディオの場合は`streamM`、
そして`/dev/snd/by-path`と`/dev/snd/by-id`内のudevリンクを読み取ります。
PCMやコントロールデバイスを開くことはないため、アプリケーションからマイクを奪ったり、ミキサーを変更したりすることはありません。
ALSA procfs（`CONFIG_SND_PROC_FS`）が必要です。
`CONFIG_SND_VERBOSE_PROCFS`がない場合は、`pcmMc/info`のみが欠けます。

## 識別情報

`id`は`microphone:alsa:<16 hex digits>`で、
`identity.stable_key`の64ビットFNV-1aハッシュです：

```text
sysfs:<card's sysfs device, relative to /sys>:pcm<M>c
sysfs:devices/platform/soc/xhci-hcd.0.auto/usb1/1-1.2/1-1.2:1.0:pcm0c
```

USBマイクの場合、sysfsデバイスはそのポート上のオーディオインターフェースであるため、
idは同じポートへの再接続、再起動、ALSAカード番号の振り直しを経ても変わらず、
異なるポートに接続された同一のマイクには異なるidが付きます。
カード番号、カードid、`/dev/snd`の名前がidに含まれることはありません。
これらは、Neat Coreの以前のALSAプロバイダーが生成していたidと同じです。

親デバイスを持たないカード（`/sys/devices/virtual/sound`の下にあり、`device`リンクがない）には
デバイスパスがないため、そのキーは現在のカードidになります：
`alsa-card-id:<card id>:pcm<M>c`。これは番号の振り直しでは変わりませんが、
カードidが変わる（ドライバーの`id`オプション）と変わります。また、同じドライバーの2枚のカードは、
再起動をまたいでサフィックス付きのid（`Loopback`、`Loopback_1`）が入れ替わることがあります。
このようなレコードには`peripherals.sysfs_device_missing` issueが付きます。
現在のsysfs idが欠けているか空のカードは、以降のスキャンまでスキップされます。
読み取れないカードがあるとスキャンは失敗します。

## フィールド

| フィールド | 意味 |
| --- | --- |
| `type`、`id` | `microphone`と上記のid。 |
| `name` | カードの短い名前。ない場合はPCM名、それもない場合はカードid。 |
| `backend` | `alsa`. |
| `connection` | `usb`（カードのデバイスにUSBの祖先がある）、`platform`、または`unknown`（親デバイスなし）。 |
| `capture_target` | `card_id`、`device`（PCM番号）、およびカードidが英字、数字、`_`、`-`のみで構成され、1桁または2桁の数字ではない場合は`selector`：`plughw:CARD=<card_id>,DEV=<M>`（ALSAは`CARD=7`をカードインデックス7として解釈するため）。現在のブート中のみ有効です。 |
| `identity` | `stable_key`、`card_index`（再接続のたびに変わる）、`pcm_node`（`/dev/snd/pcmC<N>D<M>c`）、`card_id`。判明している場合は`card_name`、`card_driver`、`pcm_name`、`by_path`、`by_id`（カードの`controlC<N>`への、名前順で最初のudevリンク）。USBの場合は`usb`：`vendor_id`、`product_id`、`bus_path`（ポート。例：`1-1.2`）、および存在する場合は`interface`、`manufacturer`、`product`、`serial`。 |
| `modes` | `streamM`内の各キャプチャaltsetのフォーマットごとに1エントリ。インターフェースとaltsetの順に並べ、重複は除きます：`format`（例：`S16_LE`）、およびカーネルが出力する場合は`interface`、`altset`、`channels`、`sample_bits`、`rates_hz`（ソート済み）または`rate_range_hz`（`{"min", "max"}`）、`channel_map`（不明な位置には`--`）。0または逆転した値は除外されます。ドライバーがPCMを開かずにフォーマットを公開しない場合（USB以外のすべてのドライバー）は空になります。 |
| `availability` | `state`：`available`、`in_use`（空いているキャプチャサブデバイスがない）、または`unknown`。判明している場合は`subdevices`と`subdevices_available`も含みます。 |
| `issues` | 読み取れなかった各部分の`{"code", "reason"}`。空の場合は省略されます。レコード自体は引き続き公開されます。 |

issueコード：`peripherals.pcm_info_unreadable`、
`peripherals.capture_selector_unavailable`、
`peripherals.capabilities_unavailable`、`peripherals.availability_unknown`、
`peripherals.sysfs_device_missing`。

## 利用可否はスナップショットです

`availability`は、最新のスキャン時点でALSAが報告した内容です。
Sentinelはホットプラグ時とリフレッシュ時に再スキャンしますが、アプリケーションがPCMを開いたり閉じたりしたことは通知されません。
PulseAudioなどのサウンドサーバーは新しく接続されたマイクを一時的に保持するため、
ホットプラグ直後のスキャンでは、次のリフレッシュまで`in_use`と報告されることがあります。
キャプチャの前に実際の状態を確認し（PCMを開いて`EBUSY`を処理します）、
どちらの状態も保証として扱わないでください。

## 例

```json
{"type": "microphone", "id": "microphone:alsa:a428cdcba66905a4",
 "name": "Yeti Nano", "backend": "alsa", "connection": "usb",
 "capture_target": {"card_id": "Nano", "device": 0, "selector": "plughw:CARD=Nano,DEV=0"},
 "identity": {"stable_key": "sysfs:devices/platform/soc/xhci-hcd.0.auto/usb1/1-1.2/1-1.2:1.0:pcm0c",
              "card_index": 0, "pcm_node": "/dev/snd/pcmC0D0c", "card_id": "Nano",
              "card_name": "Yeti Nano", "card_driver": "USB-Audio", "pcm_name": "USB Audio",
              "by_id": "/dev/snd/by-id/usb-Blue_Microphones_Yeti_Nano_REV8-00",
              "usb": {"vendor_id": "b58e", "product_id": "0005", "bus_path": "1-1.2",
                      "interface": "1-1.2:1.0", "manufacturer": "Blue Microphones",
                      "product": "Yeti Nano", "serial": "REV8"}},
 "modes": [{"format": "S16_LE", "interface": 2, "altset": 1, "channels": 2, "sample_bits": 16,
            "rates_hz": [48000], "channel_map": ["FL", "FR"]},
           {"format": "S24_3LE", "interface": 2, "altset": 2, "channels": 2, "sample_bits": 24,
            "rates_hz": [48000], "channel_map": ["FL", "FR"]}],
 "availability": {"state": "available", "subdevices": 1, "subdevices_available": 1}}
```
