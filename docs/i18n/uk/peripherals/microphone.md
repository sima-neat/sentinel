# Мікрофони

`microphone` — це один PCM захоплення ALSA: мікрофон USB Audio Class,
мікрофон вебкамери чи гарнітури, PCM захоплення вбудованої (платформної)
звукової карти або PCM карти без батьківського пристрою (віртуальної карти, як-от
`snd-aloop`). Карти лише для відтворення та PCM відтворення не повідомляються;
камера вебкамери є окремим пристроєм `camera`.

- **Провайдер:** `microphone.alsa`
- **Повторне сканування за uevent від:** `sound`

Виявлення читає лише текст і посилання: карти та PCM захоплення з
`/sys/class/sound`, разом з `id` кожної карти, посиланням `device` і предками USB;
імена карт і драйвери з `/proc/asound/cards` (використовуються лише тоді, коли
id у рядку збігається з sysfs id карти); `pcmMc/info` кожної карти і, для USB-аудіо,
`streamM`; а також посилання udev у `/dev/snd/by-path` і `/dev/snd/by-id`. Воно
ніколи не відкриває PCM чи керувальний пристрій, тому не може забрати мікрофон у
застосунку чи змінити його мікшер. Потрібен procfs ALSA
(`CONFIG_SND_PROC_FS`); без `CONFIG_SND_VERBOSE_PROCFS` бракує лише `pcmMc/info`.

## Ідентифікація

`id` — це `microphone:alsa:<16 hex digits>`, 64-бітний хеш FNV-1a від
`identity.stable_key`:

```text
sysfs:<card's sysfs device, relative to /sys>:pcm<M>c
sysfs:devices/platform/soc/xhci-hcd.0.auto/usb1/1-1.2/1-1.2:1.0:pcm0c
```

Для USB-мікрофона пристроєм sysfs є аудіоінтерфейс на його порту, тому
id зберігається після повторних підключень до того самого порту, перезавантажень і перенумерації
карт ALSA, а однакові мікрофони на різних портах отримують різні id.
Номер карти, id карти та імена `/dev/snd` ніколи до нього не входять.

Карта без батьківського пристрою (у `/sys/devices/virtual/sound`, без посилання
`device`) не має шляху пристрою, тому її ключем є поточний id карти:
`alsa-card-id:<card id>:pcm<M>c`. Він зберігається після перенумерації, але змінюється, якщо
змінюється id карти (опція драйвера `id`), а дві карти одного драйвера
можуть між завантаженнями обмінятися своїми id із суфіксами (`Loopback`, `Loopback_1`). Такий
запис містить проблему `peripherals.sysfs_device_missing`. Карту, чий
поточний sysfs id відсутній або порожній, пропускають до наступного сканування; карта, яку
неможливо прочитати, спричиняє збій сканування.

## Поля

| Поле | Значення |
| --- | --- |
| `type`, `id` | `microphone` та id, описаний вище. |
| `name` | Коротке ім’я карти, інакше ім’я PCM, інакше id карти. |
| `backend` | `alsa`. |
| `connection` | `usb` (пристрій карти має предка USB), `platform` або `unknown` (немає батьківського пристрою). |
| `capture_target` | `card_id`, `device` (номер PCM) і, якщо id карти містить лише літери, цифри, `_` та `-` і не є одно- чи двозначним числом (ALSA читає `CARD=7` як індекс карти 7), `selector`: `plughw:CARD=<card_id>,DEV=<M>`. Дійсне лише для поточного завантаження. |
| `identity` | `stable_key`, `card_index` (змінюється між повторними підключеннями), `pcm_node` (`/dev/snd/pcmC<N>D<M>c`), `card_id`; якщо відомі — `card_name`, `card_driver`, `pcm_name`, `by_path` і `by_id` (перше в порядку імен посилання udev на `controlC<N>` карти), а для USB — `usb`: `vendor_id`, `product_id`, `bus_path` (порт, наприклад `1-1.2`) і, за наявності, `interface`, `manufacturer`, `product`, `serial`. |
| `modes` | Один запис на кожен формат кожного altset захоплення в `streamM`, відсортовані за інтерфейсом і altset, без дублікатів: `format` (наприклад, `S16_LE`) і, якщо ядро їх виводить, `interface`, `altset`, `channels`, `sample_bits`, `rates_hz` (відсортовані) або `rate_range_hz` (`{"min", "max"}`), а також `channel_map` (`--` для невідомої позиції). Нульові або перевернуті значення відкидаються. Порожній, якщо драйвер не публікує формати без відкриття PCM (будь-який драйвер, крім USB). |
| `availability` | `state`: `available`, `in_use` (немає вільного субпристрою захоплення) або `unknown`; з `subdevices` і `subdevices_available`, якщо вони відомі. |
| `issues` | `{"code", "reason"}` для кожної частини, яку не вдалося прочитати; пропускається, якщо порожнє. Запис усе одно публікується. |

Коди проблем: `peripherals.pcm_info_unreadable`,
`peripherals.capture_selector_unavailable`,
`peripherals.capabilities_unavailable`, `peripherals.availability_unknown`,
`peripherals.sysfs_device_missing`.

## Доступність — це знімок стану

`availability` — це те, що ALSA повідомила під час останнього сканування. Sentinel виконує повторне сканування при
гарячому підключенні та за запитом на оновлення, але ніщо не повідомляє його, коли застосунок відкриває або
закриває PCM. Звукові сервери, як-от PulseAudio, ненадовго утримують щойно підключений
мікрофон, тому сканування одразу після гарячого підключення може повідомляти `in_use` до наступного
оновлення. Перевіряйте стан безпосередньо перед захопленням (відкрийте PCM і обробіть `EBUSY`) і
не вважайте жоден зі станів гарантією.

## Приклад

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
