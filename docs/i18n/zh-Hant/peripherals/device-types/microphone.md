# 麥克風

ALSA 所提供的音頻擷取裝置：USB 音訊類麥克風、複合 USB 裝置例如網路攝影機或耳機的麥克風、擷取內建（平台）音效卡的 PCM，以及擷取沒有母裝置的音效卡（虛擬音效卡）的 PCM。每個擷取的 PCM 記錄為一個記錄。僅播放裝置和耳機的播放端不會被報告，而網路攝影機的攝影功能會由攝影機供應者單獨報告。

- **類型標記:** `microphone`
- **提供者：** `daemon.audio.alsa`（內建）
- **重新掃描觸發器：** `sound`

Discovery 只讀取核心文本和 sysfs：可選的 `/proc/asound/cards`、可選的 `/proc/asound/cardN/pcmMc/info`、`/proc/asound/cardN/streamM`（USB 音頻）、`/sys/class/sound/pcmCNDMc`、`/sys/class/sound/cardN/device` 及其 USB 上層設備，以及 `/dev/snd/by-path` 和 `/dev/snd/by-id` 中的 udev 連結。它從不打開 PCM 或控制設備，因此無法從應用程式獲取麥克風或更改其混音器。未啟用 `CONFIG_SND_PROC_FS` 的核心會省略所有 `/proc/asound`；Sentinel 從 sysfs 枚舉卡和捕獲 PCM，保留 sysfs 卡 ID 和捕獲選擇器，並省略僅在 procfs 中的名稱、驅動程式、模式和可用性元資料及相應問題。未啟用 `CONFIG_SND_VERBOSE_PROCFS` 的核心僅省略 `pcmMc/info`；sysfs 類設備仍會生成具有未知可用性和 `peripherals.pcm_info_unreadable` 問題的紀錄。

## 身份

`id` 是 `microphone:alsa:<16 hex digits>`，一個 `identity.stable_key` 的 64 位 FNV-1a 哈希值：

```text
sysfs:<card's sysfs device, relative to /sys>:pcm<M>c
sysfs:devices/platform/soc/xhci-hcd.0.auto/usb1/1-1.2/1-1.2:1.0:pcm0c
```

對於 USB 設備而言，sysfs 設備是其埠上的音頻控制介面，因此無論重新插入同一埠、重新啟動還是 ALSA 音卡重新編號，其 id 都保持不變，而且兩個不同埠的相同麥克風將獲得不同的 id。不同的埠就是不同的麥克風。音卡編號、音卡 id 以及 `/dev/snd` 節點名稱都是路由細節，從不進入具有設備路徑的音卡 id。

key 和 hash 是 Neat Core 早期 ALSA 提供者使用的值，因此麥克風能保持其 id。

未註冊父設備的音卡（虛擬音卡，或傳遞給 `snd_card_new` 的驅動不提供父設備）位於 `/sys/devices/virtual/sound` 下，且沒有 `device` 連結，因此沒有可用於定位的設備路徑。它的 key 是音卡 id，而非其他:

```text
alsa-card-id:<card id>:pcm<M>c
alsa-card-id:Loopback:pcm0c
```

卡片 ID 是最好的可用屬性：它可以在卡片重新編號和重啟後存活下來，並且 ALSA 可以保持它在現有卡片中的唯一性，因此兩張這樣的卡片永遠不會共享同一個鍵。其限制是：更改卡片 ID（驅動程序的 `id` 模組選項，或寫入 `/sys/class/sound/cardN/id`）會改變記錄 ID，且當同一驅動程序的兩張卡片同時存在時，內核會在註冊順序中對第二張卡片的 ID 添加後綴（`Loopback_1`），因此它們可能在重啟之間交換 ID。內核從不註冊 ID 為空的卡片。如果即時 sysfs ID 為空或無法讀取，Sentinel 會將該快照視為暫時狀態，並跳過該卡片直到稍後重新掃描。

## 詳細資訊

| 欄位 | 類型 | 永遠存在 | 意義 | 來源 |
| --- | --- | --- | --- | --- |
| `name` | 字串 | 是 | 卡片簡稱，否則為 PCM 名稱，否則為卡片 ID，否則 `ALSA capture PCM <M>` | `/proc/asound/cards`，`pcmMc/info` |
| `backend` | 字串 | 是 | `alsa` | |
| `connection` | 字串 | 是 | `usb` 當該卡的設備有一個 USB 上層設備時， `unknown` 當卡沒有父裝置時，否則 `platform` | sysfs |
| `capture_target` | 物件 | 是 | `card_id` (非空字串)， `device` (PCM 編號)，而且 `selector` (`plughw:CARD=<card_id>,DEV=<M>`) 當卡片 ID 只包含字母、數字時， `_` 和 `-` 並且不是一位數或兩位數的數字（ALSA 讀取 `CARD=7` 如卡片索引 7)。僅針對當前啟動的路由 | `/sys/class/sound/cardN/id` |
| `identity` | 物件 | 是 | 見下文 | |
| `modes` | 陣列 | 是 | 捕獲格式；當驅動程式未發佈任何格式時為空（見 `issues`) | `streamM` |
| `availability` | 物件 | 是 | `state`: `available`, `in_use`（沒有捕捉子設備可用）或 `unknown`; 當已知時，含有 `subdevices` 和 `subdevices_available`。來自上次掃描的快照；請參閱 [可用性](#availability) | `pcmMc/info` |
| `issues` | 陣列 | 沒有 | `{"code", "reason"}` 對於每個無法讀取的部分；該記錄仍然發布 | |

### `identity`

| 欄位 | 類型 | 永遠存在 | 意思 |
| --- | --- | --- | --- |
| `stable_key` | 字串 | 是 | 這個鍵 `id` 的雜湊值（上方） |
| `card_index` | 整數 | 是 | 目前的 ALSA 卡號（重新插拔時會改變） |
| `pcm_node` | 字串 | 是 | `/dev/snd/pcmC<N>D<M>c`（僅路由） |
| `card_id`、`card_name`、`card_driver` | 字串 | `card_id` 永遠存在；其他欄位在非空時 | ID 來自即時 sysfs；名稱和驅動程式來自 `/proc/asound/cards`，例如 `Nano`、`Yeti Nano`、`USB-Audio` |
| `pcm_name` | 字串 | 當非空 | PCM的名稱，例如 `USB Audio` |
| `by_path`，`by_id` | 字串 | 與 udev | 第一個 `/dev/snd/by-path` / `/dev/snd/by-id` 連結，按照名稱順序，至卡片的 `controlC<N>` |
| `usb` | 對象 | 僅限 USB | `vendor_id`、`product_id`、`bus_path`（USB 埠，例如 `1-1.2`），以及當存在時 `interface`（例如 `1-1.2:1.0`）、`manufacturer`、`product`、`serial` |

### 模式

USB 音頻 `streamM` 文件中每個擷取替代集的每種格式僅一種模式。模式已排序並移除重複項。

| 欄位 | 類型 | 當前 | 含義 |
| --- | --- | --- | --- |
| `format` | 字串 | 永遠 | ALSA 範例格式，例如 `S16_LE`、`S24_3LE`、`S32_LE` |
| `interface`, `altset` | 整數 | 列印時 | USB 介面和替代設定 |
| `channels` | 整數 | 當為正值時 | 通道數 |
| `sample_bits` | 整數 | 當為正數時 | 每個樣本的有效位元 |
| `rates_hz` | 整數陣列 | 離散頻率 | 已排序，無重複或零值 |
| `rate_range_hz` | 對象 | 連續速率 | `{"min", "max"}`；一個模式有 `rates_hz` 或 `rate_range_hz`，從不兩者兼有 |
| `channel_map` | 字串陣列 | 當列印時 | 聲道位置，例如 `["FL", "FR"]`、`["MONO"]`； `--` 對於未知位置 |

### 可用性

`availability` 是 ALSA 在上次掃描時報告的狀態，而非即時狀態。Sentinel 會在熱插拔事件和刷新請求時重新掃描，但沒有任何機制告訴它當應用程序打開或關閉 PCM 時的情況。像 PulseAudio 這樣的聲音伺服器會短暫打開新連接的麥克風（例如 C920 在 DevKit 上需 5-8 秒），所以在熱插拔後立即掃描可能會報告 `in_use`，並且該紀錄會保持 `in_use`，直到下一次刷新或熱插拔。客戶端在捕捉前必須檢查即時狀態（例如，打開 PCM 並處理 `EBUSY`），或先請求刷新，並且不能將 `in_use` 或 `available` 視為保證。

### 問題代碼

| 代碼 | 當 |
| --- | --- |
| `peripherals.pcm_info_unreadable` | `pcmMc/info` 無法讀取，包括當核心因為 `CONFIG_SND_VERBOSE_PROCFS` 被禁用而省略它時 |
| `peripherals.capture_selector_unavailable` | 該卡 ID 無法形成安全的 `selector`，或是 ALSA 會讀取為卡索引的一位或兩位數字 |
| `peripherals.capabilities_unavailable` | 無擷取格式：非 USB 驅動程式（格式僅以 USB 音訊發布，無需開啟 PCM）或 USB 串流，|
| `peripherals.availability_unknown` | `subdevices_count` / `subdevices_avail` 缺失或無效 |
| `peripherals.sysfs_device_missing` | 該卡在 sysfs 中沒有父設備：沒有總線或 USB 身份，且 ID 跟隨卡 ID（見 [身份](#identity)） |

## 範例記錄

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

## 涵蓋的變體

- 單聲道、立體聲和多聲道；16、24 和 32 位元格式；多種格式
  在一個替代字型集；多個替代字型集和介面。
- 離散費率列表（已排序、去重）和連續範圍；零或
  反轉的值會被丟棄，而不是發布。
- 僅播放的卡片（無錄音）和耳機（僅捕捉）；網路攝影機的
  麥克風（它自己的錄音在攝影機旁）。
- 多個相同的麥克風在不同的端口；多個捕獲的 PCM 在
  一張卡；在重新插入後會改變的卡號。
- 缺少製造商、產品或序號（已遺漏）；缺少 udev 連結；
  在選擇器中不安全的或 ALSA 會讀取為卡索引的卡 ID；不可讀的 `pcmMc/info`；沒有公開格式的非 USB 卡；正在錄製的流（跳過運行狀態行，使用中可用性 `in_use`）。
- 沒有 ALSA 的核心，或者沒有聲卡：沒有記錄。
- 沒有 `CONFIG_SND_VERBOSE_PROCFS` 的核心：捕獲的 PCM 已列舉
  從 sysfs 讀取並發布，具體可用性未知。
- 一張沒有母裝置的卡片（在 `/sys/devices/virtual/sound` 下，無
  `device` 鏈接）：其捕獲的 PCM 已列出 `connection: unknown`未檢測到 USB 身份以及一個 `peripherals.sysfs_device_missing` 問題，除了其他麥克風之外。
- 一張卡片被添加或移除（卡片列表和卡片目錄）
  不同意)，一張卡片的父設備其 `device` 連結遺失（僅在卡片被移除時看到）或無法解析，且存在的 USB 祖先只有 `idVendor` 和 `idProduct` 中的一個，掃描將失敗，因此目錄會保留最後的良好記錄並報告提供者的問題，直到下一次掃描。掃描期間其 sysfs 條目或設備消失的卡片（與拔出競爭）將被跳過。

## 支援規則

無。麥克風錄音不含有任何 `supported` 或 `reason`。

## 驗證

| 行為 | 實體硬體（哪個設備） | 只有固定裝置 |
| --- | --- | --- |
| 網路攝影機麥克風：在攝影機旁錄音一個，`connection: usb`，`plughw:CARD=C920,DEV=0`，S16_LE 2 聲道於 16/24/32 kHz（替代集 1-3），`by_id` 連結，無問題；攝影機未受影響 | Logitech C920 內建麥克風，DevKit，2026-10-04 | 類 C920 合成裝置 |
| 在拔掉並重新插入後相同 `id`（USB `authorized` 0 然後 1）：移除，然後重新添加 | Logitech C920，DevKit，2026-10-04 | 卡重新編號：synthetic |
| `availability` 是 `in_use` 當…的時候 `arecord` 記錄和 `available` 之後；在 6 秒的錄音期間進行 20 次刷新並未干擾它 | 羅技 C920, DevKit, 2026-10-04 | |
| `availability` 熱插拔後保持 `in_use` 當 PulseAudio 保留新設備時，直到下一次掃描（參見 [可用性](#availability)) | 羅技 C920, DevKit, 2026-10-04 | |
| 立體聲 24 位元、單聲道 32 位元、連續速率，每個替代集多種格式、耳機、相同的麥克風、平台卡、無父設備的卡、部分快照、缺失的 USB 字串 | 尚未 | 人工 `/proc/asound` 文字在內核 6.18.3 的佈局中 (`sound/core/init.c`、`sound/core/pcm.c`、`sound/usb/proc.c`) 以及人工 sysfs；類似 Yeti Nano 的單聲道和平台裝置並非真實設備的擷取 |
