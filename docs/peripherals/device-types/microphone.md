# Microphones

Audio capture devices that ALSA exposes: USB Audio Class microphones, the
microphone of a composite USB device such as a webcam or headset, capture
PCMs of on-board (platform) sound cards, and capture PCMs of cards with no
parent device (virtual cards). Each capture PCM is one record.
Playback-only devices and the playback side of a headset are not reported,
and a webcam's camera is reported separately by the camera providers.

- **Type token:** `microphone`
- **Providers:** `daemon.audio.alsa` (built-in)
- **Rescan triggers:** `sound`

Discovery reads only kernel text and sysfs: optional `/proc/asound/cards`,
`/proc/asound/cardN/id`, optional `/proc/asound/cardN/pcmMc/info`,
`/proc/asound/cardN/streamM` (USB audio), `/sys/class/sound/pcmCNDMc`,
`/sys/class/sound/cardN/device` and its USB ancestors, and the udev links in
`/dev/snd/by-path` and `/dev/snd/by-id`. It never opens a PCM or control
device, so it cannot take a microphone from an application or change its
mixer. Kernels built without `CONFIG_SND_PROC_FS` omit all of `/proc/asound`;
Sentinel enumerates cards and capture PCMs from sysfs, retains the sysfs card
id and capture selector, and omits the procfs-only name, driver, modes, and
availability metadata with the corresponding issues. Kernels built without
`CONFIG_SND_VERBOSE_PROCFS` omit only `pcmMc/info`; the sysfs class device
still produces a record with unknown availability and a
`peripherals.pcm_info_unreadable` issue.

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
card id and `/dev/snd` node names are routing details and never enter the id
of a card with a device path.
The key and hash are the ones Neat Core's earlier ALSA provider used, so a
microphone keeps its id.

A card registered without a parent device (a virtual card, or a driver that
passes no parent to `snd_card_new`) lives under `/sys/devices/virtual/sound`
and has no `device` link, so it has no device path to key on. Its key is the
card id instead:

```text
alsa-card-id:<card id>:pcm<M>c
alsa-card-id:Loopback:pcm0c
```

The card id is the best attribute available: it survives card renumbering
and reboots, and ALSA keeps it unique among the cards present, so two such
cards never share a key. Its limits: changing the card id (the driver's `id`
module option, or writing `/sys/class/sound/cardN/id`) changes the record id,
and when two cards of one driver are both present the kernel suffixes the
second one's id (`Loopback_1`) in registration order, so they can swap ids
between boots. The kernel never registers a card with an empty id; should the
id read back empty, the key is `alsa-card-index:<N>:pcm<M>c`, where the card
number keeps the records apart but changes between boots.

## Details

| Field | Type | Always present | Meaning | Source |
| --- | --- | --- | --- | --- |
| `name` | string | yes | Card short name, else the PCM name, else the card id, else `ALSA capture PCM <M>` | `/proc/asound/cards`, `pcmMc/info` |
| `backend` | string | yes | `alsa` | |
| `connection` | string | yes | `usb` when the card's device has a USB ancestor, `unknown` when the card has no parent device, else `platform` | sysfs |
| `capture_target` | object | yes | `card_id` (string, may be empty), `device` (PCM number), and `selector` (`plughw:CARD=<card_id>,DEV=<M>`) when the card id holds only letters, digits, `_` and `-` and is not a one- or two-digit number (ALSA reads `CARD=7` as card index 7). Routing for the current boot only | `/sys/class/sound/cardN/id`, `/proc/asound/cardN/id` |
| `identity` | object | yes | See below | |
| `modes` | array | yes | Capture formats; empty when the driver publishes none (see `issues`) | `streamM` |
| `availability` | object | yes | `state`: `available`, `in_use` (no capture subdevice free) or `unknown`; with `subdevices` and `subdevices_available` when known. A snapshot from the last scan; see [Availability](#availability) | `pcmMc/info` |
| `issues` | array | no | `{"code", "reason"}` for each part that could not be read; the record is still published | |

### `identity`

| Field | Type | Always present | Meaning |
| --- | --- | --- | --- |
| `stable_key` | string | yes | The key `id` hashes (above) |
| `card_index` | integer | yes | Current ALSA card number (changes between replugs) |
| `pcm_node` | string | yes | `/dev/snd/pcmC<N>D<M>c` (routing only) |
| `card_id`, `card_name`, `card_driver` | string | when non-empty | The id comes from sysfs (or procfs); name and driver come from `/proc/asound/cards`, e.g. `Nano`, `Yeti Nano`, `USB-Audio` |
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

### Availability

`availability` is what ALSA reported at the last scan, not a live state.
Sentinel rescans on hot-plug events and refresh requests, but nothing tells
it when an application opens or closes a PCM. Sound servers such as
PulseAudio briefly open a newly connected microphone (5-8 s for a C920 on a
DevKit), so a scan right after hot-plug can report `in_use`, and the record
stays `in_use` until the next refresh or hot-plug. Clients must check live
before capturing (for example, open the PCM and handle `EBUSY`) or request a
refresh first, and must not treat `in_use` or `available` as a guarantee.

### Issue codes

| Code | When |
| --- | --- |
| `peripherals.pcm_info_unreadable` | `pcmMc/info` could not be read, including when the kernel omits it because `CONFIG_SND_VERBOSE_PROCFS` is disabled |
| `peripherals.capture_selector_unavailable` | The card id cannot form a safe `selector`, or is a one- or two-digit number that ALSA would read as a card index |
| `peripherals.capabilities_unavailable` | No capture formats: a non-USB driver (formats are only published by USB audio without opening the PCM) or a USB stream without them |
| `peripherals.availability_unknown` | `subdevices_count` / `subdevices_avail` missing or invalid |
| `peripherals.sysfs_device_missing` | The card has no parent device in sysfs: no bus or USB identity, and the id follows the card id (see [Identity](#identity)) |

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
  card ids that are not safe in a selector or that ALSA would read as a card
  index; unreadable `pcmMc/info`;
  non-USB cards without published formats; a stream that is recording
  (running status lines are skipped, availability `in_use`).
- A kernel without ALSA, or without sound cards: no records.
- A kernel without `CONFIG_SND_VERBOSE_PROCFS`: capture PCMs are enumerated
  from sysfs and published with unknown availability.
- A card with no parent device (under `/sys/devices/virtual/sound`, no
  `device` link): its capture PCMs are listed with `connection: unknown`,
  no USB identity and a `peripherals.sysfs_device_missing` issue, beside the
  other microphones.
- A card being added or removed (the card list and the card directories
  disagree), a card with a parent device whose `device` link is missing
  (seen only while the card is being removed) or cannot be resolved, and a
  present USB ancestor with only one of `idVendor` and `idProduct` fail the
  scan, so the catalog keeps the last good records and reports the provider
  issue until the next scan. A card whose sysfs entry or device disappears
  during the scan (an unplug racing it) is skipped instead.

## Support rules

None. Microphone records carry no `supported` or `reason`.

## Verification

| Behaviour | Real hardware (which device) | Fixtures only |
| --- | --- | --- |
| Webcam microphone: one record beside the camera, `connection: usb`, `plughw:CARD=C920,DEV=0`, S16_LE 2 ch at 16/24/32 kHz (altsets 1-3), `by_id` link, no issues; cameras unaffected | Logitech C920 built-in microphone, DevKit, 2026-10-04 | Synthetic C920-like fixture |
| Same `id` after unplug and replug (USB `authorized` 0 then 1): removed, then re-added | Logitech C920, DevKit, 2026-10-04 | Card renumbering: synthetic |
| `availability` is `in_use` while `arecord` records and `available` afterwards; 20 refreshes during a 6 s recording did not disturb it | Logitech C920, DevKit, 2026-10-04 | |
| `availability` after hot-plug stays `in_use` while PulseAudio holds the new device, until the next scan (see [Availability](#availability)) | Logitech C920, DevKit, 2026-10-04 | |
| Stereo 24-bit, mono, 32-bit, continuous rates, several formats per altset, headsets, identical microphones, platform cards, cards with no parent device, partial snapshots, missing USB strings | Not yet | Synthetic `/proc/asound` text in kernel 6.18.3's layout (`sound/core/init.c`, `sound/core/pcm.c`, `sound/usb/proc.c`) and synthetic sysfs; the Yeti Nano-like, mono and platform fixtures are not captures of real devices |
