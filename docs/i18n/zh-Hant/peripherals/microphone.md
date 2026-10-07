# 麥克風

`microphone` 是一個 ALSA 擷取 PCM：可以是 USB Audio Class 麥克風、
網路攝影機或耳麥內建的麥克風、板載（平台）音效卡的擷取 PCM，
或沒有父裝置之音效卡（例如 `snd-aloop` 這類虛擬音效卡）的擷取 PCM。
僅播放的音效卡與播放 PCM 不會回報；
網路攝影機的攝影機部分是另一個獨立的 `camera` 裝置。

- **提供者：** `microphone.alsa`
- **觸發重新掃描的 uevent 來源：** `sound`

探索只讀取文字與連結：
從 `/sys/class/sound` 讀取音效卡與擷取 PCM，以及每張音效卡的 `id`、`device` 連結與 USB 上層裝置；
從 `/proc/asound/cards` 讀取音效卡名稱與驅動程式（僅在該行的 id 與音效卡的 sysfs id 相符時使用）；
每張音效卡的 `pcmMc/info`，以及 USB 音訊的 `streamM`；
還有 `/dev/snd/by-path` 與 `/dev/snd/by-id` 中的 udev 連結。
它從不開啟 PCM 或控制裝置，因此不會從應用程式搶走麥克風，
也不會變更其混音器。需要 ALSA procfs（`CONFIG_SND_PROC_FS`）；
若沒有 `CONFIG_SND_VERBOSE_PROCFS`，則只會缺少 `pcmMc/info`。

## 識別

`id` 為 `microphone:alsa:<16 hex digits>`，即
`identity.stable_key` 的 64 位元 FNV-1a 雜湊值：

```text
sysfs:<card's sysfs device, relative to /sys>:pcm<M>c
sysfs:devices/platform/soc/xhci-hcd.0.auto/usb1/1-1.2/1-1.2:1.0:pcm0c
```

對 USB 麥克風而言，sysfs 裝置是其連接埠上的音訊介面，
因此 id 在重新插拔至同一連接埠、重新開機及 ALSA 音效卡重新編號後都保持不變，
而插在不同連接埠的相同麥克風會得到不同的 id。
音效卡編號、音效卡 id 與 `/dev/snd` 名稱都不會納入其中。

沒有父裝置的音效卡（位於 `/sys/devices/virtual/sound` 下，且沒有 `device`
連結）沒有裝置路徑，因此其鍵值是即時的音效卡 id：
`alsa-card-id:<card id>:pcm<M>c`。它在重新編號後保持不變，
但會隨音效卡 id 變更（驅動程式的 `id` 選項）而改變，
且同一驅動程式的兩張音效卡可能在不同次開機之間互換其帶後綴的 id（`Loopback`、`Loopback_1`）。
這類記錄會帶有 `peripherals.sysfs_device_missing` 問題。
即時 sysfs id 遺失或為空的音效卡會被略過，直到之後的掃描；
無法讀取的則會使掃描失敗。

## 欄位

| 欄位 | 意義 |
| --- | --- |
| `type`、`id` | `microphone` 與上述 id。 |
| `name` | 音效卡的簡稱，否則為 PCM 名稱，再否則為音效卡 id。 |
| `backend` | `alsa`. |
| `connection` | `usb`（音效卡的裝置有 USB 上層裝置）、`platform` 或 `unknown`（沒有父裝置）。 |
| `capture_target` | `card_id`、`device`（PCM 編號），以及當音效卡 id 只包含字母、數字、`_` 與 `-`，且不是一位或兩位數字時（ALSA 會將 `CARD=7` 解讀為音效卡索引 7）的 `selector`：`plughw:CARD=<card_id>,DEV=<M>`。僅在目前這次開機期間有效。 |
| `identity` | `stable_key`、`card_index`（重新插拔之間會改變）、`pcm_node`（`/dev/snd/pcmC<N>D<M>c`）、`card_id`；已知時還有 `card_name`、`card_driver`、`pcm_name`、`by_path` 與 `by_id`（依名稱排序，指向該音效卡 `controlC<N>` 的第一個 udev 連結），以及 USB 裝置的 `usb`：`vendor_id`、`product_id`、`bus_path`（連接埠，例如 `1-1.2`），存在時還有 `interface`、`manufacturer`、`product`、`serial`。 |
| `modes` | `streamM` 中每個擷取 altset 的每種格式各一筆，依介面與 altset 排序，不含重複項目：`format`（例如 `S16_LE`），以及在核心有印出時的 `interface`、`altset`、`channels`、`sample_bits`、`rates_hz`（已排序）或 `rate_range_hz`（`{"min", "max"}`），和 `channel_map`（未知位置以 `--` 表示）。零值或反轉的值會被捨棄。當驅動程式在未開啟 PCM 的情況下不公布任何格式時（任何非 USB 驅動程式皆是如此），此欄位為空。 |
| `availability` | `state`：`available`、`in_use`（沒有可用的擷取子裝置）或 `unknown`；已知時附帶 `subdevices` 與 `subdevices_available`。 |
| `issues` | 每個無法讀取的部分各一個 `{"code", "reason"}`；為空時省略。該記錄仍會發布。 |

問題代碼：`peripherals.pcm_info_unreadable`、
`peripherals.capture_selector_unavailable`、
`peripherals.capabilities_unavailable`、`peripherals.availability_unknown`、
`peripherals.sysfs_device_missing`。

## 可用性只是快照

`availability` 是 ALSA 在最近一次掃描時回報的狀態。
Sentinel 會在熱插拔與重新整理時重新掃描，但應用程式開啟或關閉 PCM 時，沒有任何機制會通知它。
PulseAudio 等音效伺服器會短暫佔用新插入的麥克風，
因此熱插拔後立即進行的掃描可能會回報 `in_use`，直到下一次重新整理。
擷取前請即時檢查（開啟 PCM 並處理 `EBUSY`），
且不要將任一狀態視為保證。

## 範例

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
