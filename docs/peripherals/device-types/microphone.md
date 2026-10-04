# Microphones

Audio capture devices that ALSA exposes: USB Audio Class microphones, the
microphone of a composite USB device such as a webcam or headset, and capture
PCMs of on-board (platform) sound cards. Each capture PCM is one record.
Playback-only devices and the playback side of a headset are not reported,
and a webcam's camera is reported separately by the camera providers.

- **Type token:** `microphone`
- **Providers:** `daemon.audio.alsa` (built-in)
- **Rescan triggers:** `sound`

Discovery reads only kernel text and sysfs: `/proc/asound/cards`,
`/proc/asound/cardN/id`, `/proc/asound/cardN/pcmMc/info`,
`/proc/asound/cardN/streamM` (USB audio), `/sys/class/sound/cardN/device` and
its USB ancestors, and the udev links in `/dev/snd/by-path` and
`/dev/snd/by-id`. It never opens a PCM or control device, so it cannot take a
microphone from an application or change its mixer.

## Identity

`id` is `microphone:alsa:<16 hex digits>`, a 64-bit FNV-1a hash of
`identity.stable_key`:

```text
sysfs:<card's sysfs device, relative to /sys>:pcm<M>c
sysfs:devices/platform/soc/xhci-hcd.0.auto/usb1/1-1.2/1-1.2:1.0:pcm0c
```

For a USB device the sysfs device is the audio control interface on its port,
so the id stays the same across replugs into the same port, reboots and ALSA
card renumbering, and two identical microphones on different ports get
different ids. A different port is a different microphone. The card number,
card id and `/dev/snd` node names are routing details and never enter the id.
The key and hash are the ones Neat Core's earlier ALSA provider used, so a
microphone keeps its id.

## Details

| Field | Type | Always present | Meaning | Source |
| --- | --- | --- | --- | --- |
| `name` | string | yes | Card short name, else the PCM name, else the card id, else `ALSA capture PCM <M>` | `/proc/asound/cards`, `pcmMc/info` |
| `backend` | string | yes | `alsa` | |
| `connection` | string | yes | `usb` when the card's device has a USB ancestor, else `platform` | sysfs |
| `capture_target` | object | yes | `card_id` (string, may be empty), `device` (PCM number), and `selector` (`plughw:CARD=<card_id>,DEV=<M>`) when the card id holds only letters, digits, `_` and `-`. Routing for the current boot only | `/proc/asound/cardN/id` |
| `identity` | object | yes | See below | |
| `modes` | array | yes | Capture formats; empty when the driver publishes none (see `issues`) | `streamM` |
| `availability` | object | yes | `state`: `available`, `in_use` (no capture subdevice free) or `unknown`; with `subdevices` and `subdevices_available` when known | `pcmMc/info` |
| `issues` | array | no | `{"code", "reason"}` for each part that could not be read; the record is still published | |

### `identity`

| Field | Type | Always present | Meaning |
| --- | --- | --- | --- |
| `stable_key` | string | yes | The key `id` hashes (above) |
| `card_index` | integer | yes | Current ALSA card number (changes between replugs) |
| `pcm_node` | string | yes | `/dev/snd/pcmC<N>D<M>c` (routing only) |
| `card_id`, `card_name`, `card_driver` | string | when non-empty | From `/proc/asound/cards` and `cardN/id`, e.g. `Nano`, `Yeti Nano`, `USB-Audio` |
| `pcm_name` | string | when non-empty | The PCM's name, e.g. `USB Audio` |
| `by_path`, `by_id` | string | with udev | The first `/dev/snd/by-path` / `/dev/snd/by-id` link, in name order, to the card's `controlC<N>` |
| `usb` | object | USB only | `vendor_id`, `product_id`, `bus_path` (the USB port, e.g. `1-1.2`), and when present `interface` (e.g. `1-1.2:1.0`), `manufacturer`, `product`, `serial` |

### Modes

One mode per format of each capture altset in the USB audio `streamM` file.
Modes are sorted and duplicates removed.

| Field | Type | Present | Meaning |
| --- | --- | --- | --- |
| `format` | string | always | ALSA sample format, e.g. `S16_LE`, `S24_3LE`, `S32_LE` |
| `interface`, `altset` | integer | when printed | USB interface and alternate setting |
| `channels` | integer | when positive | Channel count |
| `sample_bits` | integer | when positive | Valid bits per sample |
| `rates_hz` | array of integers | discrete rates | Sorted, without duplicates or zeros |
| `rate_range_hz` | object | continuous rates | `{"min", "max"}`; a mode has `rates_hz` or `rate_range_hz`, never both |
| `channel_map` | array of strings | when printed | Channel positions, e.g. `["FL", "FR"]`, `["MONO"]`; `--` for an unknown position |

### Issue codes

| Code | When |
| --- | --- |
| `peripherals.pcm_info_unreadable` | `pcmMc/info` could not be read |
| `peripherals.capture_selector_unavailable` | The card id cannot form a safe `selector` |
| `peripherals.capabilities_unavailable` | No capture formats: a non-USB driver (formats are only published by USB audio without opening the PCM) or a USB stream without them |
| `peripherals.availability_unknown` | `subdevices_count` / `subdevices_avail` missing or invalid |

## Example record

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

## Variation covered

- Mono, stereo and multichannel; 16-, 24- and 32-bit formats; several formats
  on one altset; several altsets and interfaces.
- Discrete rate lists (sorted, deduplicated) and continuous ranges; zero or
  inverted values are dropped rather than published.
- Playback-only cards (no record) and headsets (capture only); a webcam's
  microphone (its own record beside the camera's).
- Several identical microphones on different ports; several capture PCMs on
  one card; card numbers that change between replugs.
- Missing manufacturer, product or serial (omitted); missing udev links;
  card ids that are not safe in a selector; unreadable `pcmMc/info`;
  non-USB cards without published formats; a stream that is recording
  (running status lines are skipped, availability `in_use`).
- A kernel without ALSA, or without sound cards: no records.
- A card being added or removed (the card list and the card directories
  disagree, or the card's sysfs device is gone) and an incomplete USB
  ancestor fail the scan, so the catalog keeps the last good records and
  reports the provider issue until the next scan.

## Support rules

None. Microphone records carry no `supported` or `reason`.

## Verification

| Behaviour | Real hardware (which device) | Fixtures only |
| --- | --- | --- |
| Record schema, identity, modes, availability | Not yet | Synthetic `/proc/asound` text in kernel 6.18.3's layout (`sound/core/init.c`, `sound/core/pcm.c`, `sound/usb/proc.c`) and synthetic sysfs; USB ids and descriptors modelled on a Yeti Nano and a C920, not captured from them |
| Webcam microphone, headset, identical microphones, renumbering, partial snapshots | Not yet | Synthetic |
| Discovery does not disturb a recording | Not yet | |
