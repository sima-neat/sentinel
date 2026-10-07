# Microphones

A `microphone` is one ALSA capture PCM: a USB Audio Class microphone, the
microphone of a webcam or headset, a capture PCM of an on-board (platform)
sound card, or one of a card with no parent device (a virtual card such as
`snd-aloop`). Playback-only cards and playback PCMs are not reported; a
webcam's camera is a separate `camera` device.

- **Provider:** `microphone.alsa`
- **Rescans on uevents from:** `sound`

Discovery reads only text and links: cards and capture PCMs from
`/sys/class/sound`, with each card's `id`, `device` link and USB ancestors;
card names and drivers from `/proc/asound/cards` (used only when the line's
id matches the card's sysfs id); each card's `pcmMc/info` and, for USB audio,
`streamM`; and the udev links in `/dev/snd/by-path` and `/dev/snd/by-id`. It
never opens a PCM or control device, so it cannot take a microphone from an
application or change its mixer. ALSA procfs (`CONFIG_SND_PROC_FS`) is
required; without `CONFIG_SND_VERBOSE_PROCFS` only `pcmMc/info` is missing.

## Identity

`id` is `microphone:alsa:<16 hex digits>`, the 64-bit FNV-1a hash of
`identity.stable_key`:

```text
sysfs:<card's sysfs device, relative to /sys>:pcm<M>c
sysfs:devices/platform/soc/xhci-hcd.0.auto/usb1/1-1.2/1-1.2:1.0:pcm0c
```

For a USB microphone the sysfs device is the audio interface on its port, so
the id survives replugs into the same port, reboots and ALSA card
renumbering, and identical microphones on different ports get different ids.
The card number, card id and `/dev/snd` names never enter it.

A card with no parent device (under `/sys/devices/virtual/sound`, no `device`
link) has no device path, so its key is the live card id:
`alsa-card-id:<card id>:pcm<M>c`. It survives renumbering, but changes if
the card id changes (the driver's `id` option), and two cards of one driver
can swap their suffixed ids (`Loopback`, `Loopback_1`) between boots. Such a
record carries the `peripherals.sysfs_device_missing` issue. A card whose
live sysfs id is missing or empty is skipped until a later scan; one that
cannot be read fails the scan.

## Fields

| Field | Meaning |
| --- | --- |
| `type`, `id` | `microphone` and the id above. |
| `name` | The card's short name, else the PCM name, else the card id. |
| `backend` | `alsa`. |
| `connection` | `usb` (the card's device has a USB ancestor), `platform`, or `unknown` (no parent device). |
| `capture_target` | `card_id`, `device` (PCM number) and, when the card id holds only letters, digits, `_` and `-` and is not a one- or two-digit number (ALSA reads `CARD=7` as card index 7), `selector`: `plughw:CARD=<card_id>,DEV=<M>`. Valid for the current boot only. |
| `identity` | `stable_key`, `card_index` (changes between replugs), `pcm_node` (`/dev/snd/pcmC<N>D<M>c`), `card_id`; when known, `card_name`, `card_driver`, `pcm_name`, `by_path` and `by_id` (the first udev link, in name order, to the card's `controlC<N>`), and for USB `usb`: `vendor_id`, `product_id`, `bus_path` (the port, e.g. `1-1.2`) and, when present, `interface`, `manufacturer`, `product`, `serial`. |
| `modes` | One entry per format of each capture altset in `streamM`, sorted by interface and altset, without duplicates: `format` (e.g. `S16_LE`) and, when the kernel prints them, `interface`, `altset`, `channels`, `sample_bits`, `rates_hz` (sorted) or `rate_range_hz` (`{"min", "max"}`), and `channel_map` (`--` for an unknown position). Zero or inverted values are dropped. Empty when the driver publishes no formats without opening the PCM (any non-USB driver). |
| `availability` | `state`: `available`, `in_use` (no capture subdevice free) or `unknown`; with `subdevices` and `subdevices_available` when known. |
| `issues` | `{"code", "reason"}` for each part that could not be read; omitted when empty. The record is still published. |

Issue codes: `peripherals.pcm_info_unreadable`,
`peripherals.capture_selector_unavailable`,
`peripherals.capabilities_unavailable`, `peripherals.availability_unknown`,
`peripherals.sysfs_device_missing`.

## Availability is a snapshot

`availability` is what ALSA reported at the latest scan. Sentinel rescans on
hot-plug and on refresh, but nothing tells it when an application opens or
closes a PCM. Sound servers such as PulseAudio briefly hold a newly plugged
microphone, so a scan right after hot-plug can report `in_use` until the next
refresh. Check live before capturing (open the PCM and handle `EBUSY`) and do
not treat either state as a guarantee.

## Example

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
