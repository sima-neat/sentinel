# マイクロフォン

ALSAが公開するオーディオキャプチャデバイス: USBオーディオクラスのマイク、ウェブカメラやヘッドセットなどの複合USBデバイスのマイク、本体（プラットフォーム）サウンドカードのPCMキャプチャ、および親デバイスを持たないカード（仮想カード）のPCMキャプチャ。各キャプチャPCMは1つのレコードです。再生専用のデバイスやヘッドセットの再生側は報告されず、ウェブカメラのカメラはカメラプロバイダによって別途報告されます。

- **タイプトークン:** `microphone`
- **プロバイダー:** `daemon.audio.alsa`（組み込み）
- **再スキャンのトリガー:** `sound`

Discoveryはカーネルテキストとsysfsのみを読み取ります：オプションの`/proc/asound/cards`、`/proc/asound/cardN/id`、オプションの`/proc/asound/cardN/pcmMc/info`、`/proc/asound/cardN/streamM`（USBオーディオ）、`/sys/class/sound/pcmCNDMc`、`/sys/class/sound/cardN/device`およびそのUSB先祖、さらに`/dev/snd/by-path`および`/dev/snd/by-id`のudevリンクを読み取ります。PCMやコントロールデバイスを開くことはないため、アプリケーションからマイクを取得したり、ミキサーを変更したりすることはできません。`CONFIG_SND_PROC_FS`なしで構築されたカーネルは、`/proc/asound`のすべてを省略します。SentinelはsysfsからカードとキャプチャPCMを列挙し、sysfsカードIDとキャプチャセレクターを保持し、同対応する問題によりprocfs専用の名前、ドライバー、モード、および可用性メタデータを省略します。`CONFIG_SND_VERBOSE_PROCFS`なしで構築されたカーネルは、`pcmMc/info`のみを省略します。それでもsysfsクラスデバイスは、未知の可用性と`peripherals.pcm_info_unreadable`問題を伴うレコードを生成します。

## アイデンティティ

`id` は `microphone:alsa:<16 hex digits>` であり、`identity.stable_key` の64ビットFNV-1aハッシュです:

```text
sysfs:<card's sysfs device, relative to /sys>:pcm<M>c
sysfs:devices/platform/soc/xhci-hcd.0.auto/usb1/1-1.2/1-1.2:1.0:pcm0c
```

USBデバイスの場合、sysfsデバイスはそのポート上のオーディオコントロールインターフェースであるため、同じポートに再接続した場合や再起動、ALSAのカード番号変更時にもIDは同じままであり、異なるポートにある同一のマイクには異なるIDが割り当てられる。異なるポートは異なるマイクである。カード番号、カードID、`/dev/snd`ノード名はルーティングの詳細であり、デバイスパスを持つカードのIDには決して含まれない。キーとハッシュはNeat Coreの以前のALSAプロバイダーが使用していたものであり、マイクはそのIDを保持する。

親デバイスを持たないカード（仮想カード、または`snd_card_new`に親を渡さないドライバ）が登録された場合は、`/sys/devices/virtual/sound`の下に存在し、`device`リンクを持たないため、キーにするデバイスパスを持たない。そのキーはカードIDになる:

```text
alsa-card-id:<card id>:pcm<M>c
alsa-card-id:Loopback:pcm0c
```

カードIDは最も優れた利用可能な属性であり、カードの番号変更や再起動にも耐え、ALSAは存在するカード間で一意性を保持するため、これらのカードがキーを共有することはありません。その制限としては、カードID（ドライバの`id`モジュールオプション、または`/sys/class/sound/cardN/id`への書き込み）を変更すると、レコードIDが変わることがあります。また、同じドライバのカードが2枚ある場合、カーネルは登録順に2枚目のIDにサフィックスを付けます（`Loopback_1`）、そのため起動間でIDが入れ替わる可能性があります。カーネルは空のIDを持つカードを登録することはありません。IDが空で読み戻された場合、キーは`alsa-card-index:<N>:pcm<M>c`であり、カード番号がレコードを区別しますが、起動ごとに変わります。

## 詳細

| フィールド | タイプ | 常に存在 | 意味 | ソース |
| --- | --- | --- | --- | --- |
| `name` | 文字列 | はい | カードの短い名前、存在しなければ PCM 名、それもなければカード ID、それもなければ `ALSA capture PCM <M>` | `/proc/asound/cards`, `pcmMc/info` |
| `backend` | 文字列 | はい | `alsa` | |
| `connection` | 文字列 | はい | `usb` カードのデバイスにUSBの祖先がある場合、 `unknown` カードに親デバイスがない場合、そうでなければ `platform` | sysfs |
| `capture_target` | オブジェクト | はい | `card_id` （文字列、空の可能性あり）、`device` （PCM番号）、および `selector` (`plughw:CARD=<card_id>,DEV=<M>`)。カードIDが文字、数字、`_` および `-` のみを保持しており、1桁または2桁の番号でない場合（ALSAは `CARD=7` をカードインデックス7として読み取ります）。現在のブートのみのルーティング | `/sys/class/sound/cardN/id`、`/proc/asound/cardN/id` |
| `identity` | オブジェクト | はい | 下記を参照 | |
| `modes` | 配列 | はい | キャプチャ形式；ドライバーが公開していない場合は空（参照） `issues`) | `streamM` |
| `availability` | オブジェクト | はい | `state`: `available`, `in_use` （キャプチャサブデバイスなしでフリー）または `unknown`；と `subdevices` そして `subdevices_available` 既知の場合。最後のスキャンからのスナップショット; 参照 [利用可能性](#availability) | `pcmMc/info` |
| `issues` | 配列 | 読み取れなかった各部分に対する | `{"code", "reason"}`; 記録はそれでも公開されます | |

### `identity`

| フィールド | タイプ | 常に存在 | 意味 |
| --- | --- | --- | --- |
| `stable_key` | 文字列 | はい | キー `id` のハッシュ (上記) |
| `card_index` | 整数 | はい | 現在のALSAカード番号（再接続時に変わります） |
| `pcm_node` | 文字列 | はい | `/dev/snd/pcmC<N>D<M>c`（ルーティングのみ） |
| `card_id`、`card_name`、`card_driver` | 文字列 | 空でないとき | ID は sysfs（または procfs）から取得されます；名前とドライバーは `/proc/asound/cards` から取得されます。例えば `Nano`、`Yeti Nano`、`USB-Audio` |
| `pcm_name` | 文字列 | 空でない場合 | PCMの名前、例：`USB Audio` |
| `by_path`、`by_id` | 文字列 | udev がある場合 | カードの `controlC<N>` を指す最初の `/dev/snd/by-path` / `/dev/snd/by-id` リンク（名前順） |
| `usb` | オブジェクト | USBのみ | `vendor_id`, `product_id`, `bus_path`（USBポート、例: `1-1.2`）、および存在する場合は `interface`（例: `1-1.2:1.0`）、`manufacturer`, `product`, `serial` |

### モード

USBオーディオ`streamM`ファイル内の各キャプチャオルトセットのフォーマットごとに1つのモード。モードはソートされ、重複は削除されます。

| フィールド | タイプ | 存在 | 意味 |
| --- | --- | --- | --- |
| `format` | 文字列 | 常に | ALSA サンプルフォーマット、例：`S16_LE`、`S24_3LE`、`S32_LE` |
| `interface`, `altset` | 整数 | 印刷するとき | USBインターフェースと代替設定 |
| `channels` | 整数 | ポジティブなとき | チャンネル数 |
| `sample_bits` | 整数 | 正の時 | サンプルごとの有効ビット |
| `rates_hz` | 整数の配列 | 離散的なレート | 重複やゼロなしでソート済み |
| `rate_range_hz` | オブジェクト | 連続レート | `{"min", "max"}`; モードは `rates_hz` または `rate_range_hz` を持つが、両方は持たない |
| `channel_map` | 文字列の配列 | 印刷されたときに | チャンネルの位置、例: `["FL", "FR"]`, `["MONO"]`; `--` 不明な位置の場合 |

### 利用可能性

`availability` は、最後のスキャン時にALSAが報告したものであり、ライブ状態ではありません。
Sentinel はホットプラグイベントやリフレッシュ要求時に再スキャンしますが、アプリケーションがPCMを開閉したときには通知されません。PulseAudioのようなサウンドサーバーは、新たに接続されたマイク（DevKit のC920で5〜8秒）を一時的に開くことがあるため、ホットプラグ直後のスキャンでは `in_use` と報告されることがあり、その記録は次のリフレッシュやホットプラグまで `in_use` のままです。クライアントはキャプチャ前にライブで確認する必要があります（例えば、PCMを開いて `EBUSY` を処理する）か、まずリフレッシュを要求する必要があり、 `in_use` や `available` を保証として扱ってはいけません。

### 問題コード

| コード | 時に |
| --- | --- |
| `peripherals.pcm_info_unreadable` | `pcmMc/info` は読み取れませんでした。カーネルが `CONFIG_SND_VERBOSE_PROCFS` を無効にしているため省略する場合も含まれます |
| `peripherals.capture_selector_unavailable` | カードIDは安全な`selector`を構成できないか、あるいはALSAがカードインデックスとして読み取る1桁または2桁の数字です |
| `peripherals.capabilities_unavailable` | キャプチャ形式がありません：非USBドライバー（形式はPCMを開かずにUSBオーディオのみが公開します）またはそれらのないUSBストリーム |
| `peripherals.availability_unknown` | `subdevices_count` / `subdevices_avail` 欠落または無効 |
| `peripherals.sysfs_device_missing` | このカードはsysfsに親デバイスがありません：バスやUSBの識別情報がなく、IDはカードIDに続きます（[アイデンティティ](#identity)を参照） |

## 例の記録

```json
{"id": "microphone:alsa:a428cdcba66905a4", "type": "microphone", "provider": "daemon.audio.alsa",
 "microphone": {
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
   "availability": {"state": "available", "subdevices": 1, "subdevices_available": 1}}}
```

## 変動をカバー

- モノラル、ステレオ、マルチチャンネル；16ビット、24ビット、32ビットのフォーマット；いくつかのフォーマット
  1つのアルトセットで；いくつかのアルトセットとインターフェース。
- 離散レートリスト（ソート済み、重複排除済み）および連続範囲；ゼロまたは
  反転した値は公開されず、破棄されます。
- 再生専用カード（録画不可）およびヘッドセット（キャプチャ専用）；ウェブカメラの
  マイク（カメラのものとは別に自分用の録音）。
- 異なるポートに複数の同一マイク; 複数のPCMをキャプチャ
  1枚のカード；差し替えごとに変わるカード番号。
- 製造元、製品、またはシリアルが不足しています（省略されました）；udevリンクが不足しています；
  セレクターで安全でない、またはALSAがカードインデックスとして読み取るカードID；読み取れない `pcmMc/info`；公開フォーマットのない非USBカード；録音中のストリーム（実行中のステータスラインはスキップされ、使用中の場合 `in_use`）。
- ALSAなし、またはサウンドカードなしのカーネル：レコードなし。
- カーネルに `CONFIG_SND_VERBOSE_PROCFS` がない場合：キャプチャ PCM が列挙されます
  sysfs から取得され、利用可能性は不明のまま公開されました。
- `/sys/devices/virtual/sound` の下に親デバイスがないカード、なし
  `device` のリンク): そのキャプチャ PCM は `connection: unknown` と共にリストされており、USB 識別情報がなく、他のマイクの横に `peripherals.sysfs_device_missing` の問題があります。
- カードが追加または削除されること（カードリストおよびカードディレクトリ）
  同意しない）、親デバイスの `device` リンクが欠落しているカード（カードが取り外されているときにのみ表示される）、または解決できないカード、さらに `idVendor` と `idProduct` のうちどちらか一方しか持たない現存の USB 先祖はスキャンに失敗するため、カタログは最後の正常な記録を保持し、次回のスキャンまでプロバイダの問題を報告する。スキャン中に sysfs エントリやデバイスが消えるカード（取り外しによる競合）はスキップされる。

## サポート規則

なし。マイクの録音には`supported`や`reason`は含まれません。

## 確認

| 動作 | 実ハードウェア（どのデバイスか） | 固定具のみ |
| --- | --- | --- |
| ウェブカメラのマイク：カメラ横に1つの録音、`connection: usb`、`plughw:CARD=C920,DEV=0`、S16_LE 2チャンネル 16/24/32 kHz（altsets 1-3）、`by_id` リンク、問題なし；カメラには影響なし | Logitech C920 内蔵マイク、DevKit、2026-10-04 | C920風合成装置 |
| 同じ `id` を抜き差し後に再接続（USB `authorized` 0 から 1 に）：削除後、再追加 | Logitech C920、DevKit、2026-10-04 | カードの番号付け直し：合成 |
| `availability` です `in_use` 〜の間 `arecord` 記録と `available` その後；6秒間の記録中に20回更新しても、それを妨げることはなかった | ロジテック C920 DevKit, 2026-10-04 | |
| ホットプラグ後の `availability` は、PulseAudio が新しいデバイスを保持している間 `in_use` のままで、次のスキャンまで続きます（[利用可能性](#availability) を参照）。 | Logitech C920、DevKit、2026-10-04 | |
| ステレオ24ビット、モノラル32ビット、連続レート、各オルトセットごとの複数フォーマット、ヘッドセット、同一マイク、プラットフォームカード、親デバイスのないカード、部分的スナップショット、USB文字列の欠如 | まだ | 合成 `/proc/asound` カーネル6.18.3のレイアウトのテキスト (`sound/core/init.c`、`sound/core/pcm.c`、`sound/usb/proc.c`) および合成sysfs; Yeti Nanoのようなモノラルおよびプラットフォームの装置は実際のデバイスのキャプチャではない |
