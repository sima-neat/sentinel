//! Tests for the ALSA microphone provider. `/proc/asound`, `/sys` and `/dev`
//! are temporary directories. Every fixture is **synthetic** but laid out as
//! kernel 6.18.3 prints it (`sound/core/init.c` for `cards`,
//! `sound/core/pcm.c` for `pcmMc/info`, `sound/usb/proc.c` for `streamM`) and
//! as `snd_card_new` registers a card in sysfs. The USB ids and descriptors
//! are modelled on a Blue Yeti Nano (b58e:0005) and a Logitech C920
//! (046d:082d) but are not captures from those devices.

use super::*;
use crate::peripherals::sysutil::testing::{on_board, write_file, TempDir};
use crate::peripherals::sysutil::CODE_PERMISSION_DENIED;

use std::os::unix::fs::{symlink, PermissionsExt};

use serde_json::{json, Value};

const XHCI: &str = "devices/platform/soc/xhci-hcd.0.auto/usb1";

/// A Yeti Nano-like stereo microphone with a headphone output: playback and
/// capture share `stream0`, capture offers 16-bit and 24-bit altsets.
const YETI_STREAM: &str = "\
Blue Microphones Yeti Nano at usb-xhci-hcd.0.auto-1.2, full speed : USB Audio

Playback:
  Status: Stop
  Interface 1
    Altset 1
    Format: S16_LE
    Channels: 2
    Endpoint: 0x01 (1 OUT) (ADAPTIVE)
    Rates: 48000
    Bits: 16
    Channel map: FL FR

Capture:
  Status: Stop
  Interface 2
    Altset 1
    Format: S16_LE
    Channels: 2
    Endpoint: 0x82 (2 IN) (ASYNC)
    Rates: 48000
    Bits: 16
    Channel map: FL FR
  Interface 2
    Altset 2
    Format: S24_3LE
    Channels: 2
    Endpoint: 0x82 (2 IN) (ASYNC)
    Rates: 48000
    Bits: 24
    Channel map: FL FR
";

/// A C920-like webcam microphone (high speed, one rate per altset, no
/// channel map); the camera half is the V4L2 provider's.
const C920_STREAM: &str = "\
HD Pro Webcam C920 at usb-xhci-hcd.0.auto-1.3, high speed : USB Audio

Capture:
  Status: Stop
  Interface 3
    Altset 1
    Format: S16_LE
    Channels: 2
    Endpoint: 0x83 (3 IN) (ASYNC)
    Rates: 16000
    Data packet interval: 1000 us
    Bits: 16
  Interface 3
    Altset 2
    Format: S16_LE
    Channels: 2
    Endpoint: 0x83 (3 IN) (ASYNC)
    Rates: 24000
    Data packet interval: 1000 us
    Bits: 16
  Interface 3
    Altset 3
    Format: S16_LE
    Channels: 2
    Endpoint: 0x83 (3 IN) (ASYNC)
    Rates: 32000
    Data packet interval: 1000 us
    Bits: 16
";

/// A mono 16-bit microphone that is recording: the running status block
/// (`Interface = 1`, ...) precedes the formats; its rate list repeats a rate.
const MONO_STREAM: &str = "\
USB PnP Sound Device at usb-xhci-hcd.0.auto-1.4, full speed : USB Audio

Capture:
  Status: Running
    Interface = 1
    Altset = 1
    Packet Size = 96
    Momentary freq = 48000 Hz (0x30.0000)
  Interface 1
    Altset 1
    Format: S16_LE
    Channels: 1
    Endpoint: 0x82 (2 IN) (ADAPTIVE)
    Rates: 48000, 44100, 48000
    Bits: 16
    Channel map: MONO
";

/// A temporary `/proc/asound`, `/sys` and `/dev` with cards added in order.
struct Board {
    root: TempDir,
    cards: String,
}

impl Board {
    fn new() -> Self {
        let board = Self {
            root: TempDir::new(),
            cards: String::new(),
        };
        fs::create_dir_all(board.path("sys/class/sound")).unwrap();
        fs::create_dir_all(board.path("proc/asound")).unwrap();
        board.write("proc/asound/cards", "--- no soundcards ---\n");
        board
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    fn write(&self, relative: &str, text: &str) {
        write_file(&self.path(relative), text);
    }

    /// USB device `<XHCI>/<port>` with `attributes` besides its ids.
    fn usb(&self, port: &str, ids: (&str, &str), attributes: &[(&str, &str)]) {
        let device = format!("sys/{XHCI}/{port}");
        let ids = [("idVendor", ids.0), ("idProduct", ids.1)];
        for (name, value) in ids.iter().chain(attributes) {
            self.write(&format!("{device}/{name}"), &format!("{value}\n"));
        }
    }

    /// Card `index` (`id`, `driver`, short `name`) whose sysfs parent is
    /// `device`, registered as `snd_card_new` does, with its udev links.
    fn card(&mut self, index: u32, (id, driver, name): (&str, &str, &str), device: &str) {
        self.cards += &format!("{index:2} [{id:<15}]: {driver} - {name}\n");
        self.cards += &format!(" {name} at {device}\n");
        self.write("proc/asound/cards", &self.cards);
        let card = format!("{device}/sound/card{index}");
        fs::create_dir_all(self.path(&format!("sys/{card}"))).unwrap();
        self.write(&format!("sys/{card}/id"), &format!("{id}\n"));
        symlink("../..", self.path(&format!("sys/{card}/device"))).unwrap();
        let class = self.path(&format!("sys/class/sound/card{index}"));
        symlink(format!("../../{card}"), class).unwrap();
        let control = format!("controlC{index}");
        self.write(&format!("dev/snd/{control}"), "");
        for (directory, name) in [
            ("by-path", "platform-xhci-hcd.0.auto-usb-0"),
            ("by-id", "usb"),
        ] {
            let link = self.path(&format!("dev/snd/{directory}/{name}-{id}-{index}"));
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(format!("../{control}"), link).unwrap();
        }
    }

    /// Capture PCM `pcm` of card `index`, `(count, free)` subdevices, and its
    /// USB audio `stream` file when given.
    fn capture(&self, index: u32, pcm: u32, (count, free): (u32, u32), stream: Option<&str>) {
        let info = format!(
            "card: {index}\ndevice: {pcm}\nsubdevice: 0\nstream: CAPTURE\nid: USB Audio\n\
             name: USB Audio\nsubname: subdevice #0\nclass: 0\nsubclass: 0\n\
             subdevices_count: {count}\nsubdevices_avail: {free}\n"
        );
        self.write(&format!("proc/asound/card{index}/pcm{pcm}c/info"), &info);
        self.write(&format!("sys/class/sound/pcmC{index}D{pcm}c"), "");
        if let Some(stream) = stream {
            self.write(&format!("proc/asound/card{index}/stream{pcm}"), stream);
        }
    }

    fn playback(&self, index: u32, pcm: u32) {
        let info = "stream: PLAYBACK\nsubdevices_count: 1\nsubdevices_avail: 1\n";
        self.write(&format!("proc/asound/card{index}/pcm{pcm}p/info"), info);
    }

    /// A USB microphone at `port` as card `index`, interface `1.0`.
    fn usb_mic(&mut self, index: u32, port: &str, id: &str, stream: &str) {
        self.usb(port, ("b58e", "0005"), &[]);
        self.card(
            index,
            (id, "USB-Audio", "Yeti Nano"),
            &format!("{XHCI}/{port}/{port}:1.0"),
        );
        self.capture(index, 0, (1, 1), Some(stream));
    }

    fn scan(&self) -> Result<Vec<Microphone>, ProviderError> {
        let paths = ["proc/asound", "sys", "dev"].map(|path| self.path(path));
        let mut provider = AlsaProvider::with_roots(&paths[0], &paths[1], &paths[2]);
        microphones(on_board(self.root.path(), provider.discover()))
    }

    fn details(&self) -> Vec<Value> {
        let records = self.scan().unwrap();
        Vec::from_iter(records.iter().map(value))
    }
}

fn microphones(
    result: Result<Vec<Peripheral>, ProviderError>,
) -> Result<Vec<Microphone>, ProviderError> {
    let microphone = |device| match device {
        Peripheral::Microphone(microphone) => microphone,
        other => panic!("not a microphone: {other:?}"),
    };
    Ok(result?.into_iter().map(microphone).collect())
}

/// A microphone as the catalog serializes it.
fn value(microphone: &Microphone) -> Value {
    serde_json::to_value(Peripheral::Microphone(microphone.clone())).unwrap()
}

/// The three reference microphones, in full: their catalog entries are what
/// Insight's microphone page reads. The Yeti's headphone output and the
/// C920's camera are not reported.
#[test]
fn reference_microphones_produce_the_microphone_schema() {
    let mut board = Board::new();
    let yeti = [
        ("manufacturer", "Blue Microphones"),
        ("product", "Yeti Nano"),
    ];
    board.usb("1-1.2", ("b58e", "0005"), &yeti);
    board.write(&format!("sys/{XHCI}/1-1.2/serial"), "REV8\n");
    let device = format!("{XHCI}/1-1.2/1-1.2:1.0");
    board.card(0, ("Nano", "USB-Audio", "Yeti Nano"), &device);
    board.playback(0, 0);
    board.capture(0, 0, (1, 1), Some(YETI_STREAM));

    let c920 = [("product", "HD Pro Webcam C920"), ("serial", "A1B2C3D4")];
    board.usb("1-1.3", ("046d", "082d"), &c920);
    fs::create_dir_all(board.path(&format!("sys/{XHCI}/1-1.3/1-1.3:1.0/video4linux"))).unwrap();
    let device = format!("{XHCI}/1-1.3/1-1.3:1.2");
    board.card(1, ("C920", "USB-Audio", "HD Pro Webcam C920"), &device);
    board.capture(1, 0, (1, 1), Some(C920_STREAM));

    board.usb("1-1.4", ("0c76", "161f"), &[]);
    let device = format!("{XHCI}/1-1.4/1-1.4:1.0");
    board.card(2, ("Device", "USB-Audio", "USB PnP Sound Device"), &device);
    board.capture(2, 0, (1, 0), Some(MONO_STREAM));

    let records = board.scan().unwrap();
    let catalog = Vec::from_iter(records.iter().map(value));
    let by_path = "/dev/snd/by-path/platform-xhci-hcd.0.auto-usb-0";
    let mode = |altset, format, bits, rate| {
        json!({"format": format, "interface": 2, "altset": altset, "channels": 2,
               "sample_bits": bits, "rates_hz": [rate], "channel_map": ["FL", "FR"]})
    };
    let yeti = json!({
        "id": "microphone:alsa:a428cdcba66905a4", "type": "microphone",
        "name": "Yeti Nano", "backend": "alsa", "connection": "usb",
        "capture_target": {"card_id": "Nano", "device": 0, "selector": "plughw:CARD=Nano,DEV=0"},
        "identity": {
            "stable_key": format!("sysfs:{XHCI}/1-1.2/1-1.2:1.0:pcm0c"),
            "card_index": 0, "pcm_node": "/dev/snd/pcmC0D0c",
            "card_id": "Nano", "card_name": "Yeti Nano", "card_driver": "USB-Audio",
            "pcm_name": "USB Audio",
            "by_path": format!("{by_path}-Nano-0"), "by_id": "/dev/snd/by-id/usb-Nano-0",
            "usb": {"vendor_id": "b58e", "product_id": "0005", "bus_path": "1-1.2",
                    "interface": "1-1.2:1.0", "manufacturer": "Blue Microphones",
                    "product": "Yeti Nano", "serial": "REV8"}
        },
        "modes": [mode(1, "S16_LE", 16, 48000), mode(2, "S24_3LE", 24, 48000)],
        "availability": {"state": "available", "subdevices": 1, "subdevices_available": 1}
    });
    let mode = |altset, rate| {
        json!({"format": "S16_LE", "interface": 3, "altset": altset, "channels": 2,
               "sample_bits": 16, "rates_hz": [rate]})
    };
    let c920 = json!({
        "id": "microphone:alsa:ff51de63004c98b6", "type": "microphone",
        "name": "HD Pro Webcam C920", "backend": "alsa", "connection": "usb",
        "capture_target": {"card_id": "C920", "device": 0, "selector": "plughw:CARD=C920,DEV=0"},
        "identity": {
            "stable_key": format!("sysfs:{XHCI}/1-1.3/1-1.3:1.2:pcm0c"),
            "card_index": 1, "pcm_node": "/dev/snd/pcmC1D0c",
            "card_id": "C920", "card_name": "HD Pro Webcam C920", "card_driver": "USB-Audio",
            "pcm_name": "USB Audio",
            "by_path": format!("{by_path}-C920-1"), "by_id": "/dev/snd/by-id/usb-C920-1",
            "usb": {"vendor_id": "046d", "product_id": "082d", "bus_path": "1-1.3",
                    "interface": "1-1.3:1.2", "product": "HD Pro Webcam C920",
                    "serial": "A1B2C3D4"}
        },
        "modes": [mode(1, 16000), mode(2, 24000), mode(3, 32000)],
        "availability": {"state": "available", "subdevices": 1, "subdevices_available": 1}
    });
    let mono = json!({
        "id": "microphone:alsa:9a87b8bf9cfe583c", "type": "microphone",
        "name": "USB PnP Sound Device", "backend": "alsa", "connection": "usb",
        "capture_target": {"card_id": "Device", "device": 0,
                           "selector": "plughw:CARD=Device,DEV=0"},
        "identity": {
            "stable_key": format!("sysfs:{XHCI}/1-1.4/1-1.4:1.0:pcm0c"),
            "card_index": 2, "pcm_node": "/dev/snd/pcmC2D0c",
            "card_id": "Device", "card_name": "USB PnP Sound Device",
            "card_driver": "USB-Audio", "pcm_name": "USB Audio",
            "by_path": format!("{by_path}-Device-2"), "by_id": "/dev/snd/by-id/usb-Device-2",
            "usb": {"vendor_id": "0c76", "product_id": "161f", "bus_path": "1-1.4",
                    "interface": "1-1.4:1.0"}
        },
        "modes": [{"format": "S16_LE", "interface": 1, "altset": 1, "channels": 1,
                   "sample_bits": 16, "rates_hz": [44100, 48000], "channel_map": ["MONO"]}],
        "availability": {"state": "in_use", "subdevices": 1, "subdevices_available": 0}
    });
    let mut expected = [yeti, c920, mono];
    expected.sort_by_key(|record| record["id"].to_string());
    assert_eq!(catalog, expected);
    for value in catalog {
        let device: Peripheral = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(device).unwrap(), value, "round trip");
    }
    assert_eq!(AlsaProvider::new().subsystems(), ["sound"]);
}

#[test]
fn card_long_names_cannot_imitate_headers() {
    let text = concat!(
        " 0 [Real           ]: USB-Audio - Real microphone\n",
        " 7 [Fake]: Driver - Name\n",
        " 7 [Actual         ]: USB-Audio - Actual microphone\n",
        " actual long name\n"
    );
    let cards = parse_cards(text, &BTreeSet::from([0, 7]));
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[&0].name, "Real microphone");
    assert!(!cards.contains_key(&7), "ambiguous metadata is omitted");
}

/// Stream parsing across the class: mono to multichannel, 16/24/32-bit,
/// several altsets and interfaces, discrete lists and continuous ranges,
/// several formats on one altset, unknown channel positions, and values the
/// kernel never prints for a working device (dropped, never published).
#[test]
fn stream_formats_cover_the_class() {
    let block = |lines: &str| format!("Card : USB Audio\n\nCapture:\n  Interface 4\n{lines}");
    let cases = [
        (
            "    Altset 3\n    Format: S32_LE\n    Channels: 4\n    Rates: 8000 - 96000 (continuous)\n    Bits: 32\n    Channel map: FL FR -- RR\n",
            json!([{"format": "S32_LE", "interface": 4, "altset": 3, "channels": 4, "sample_bits": 32,
                    "rate_range_hz": {"min": 8000, "max": 96000},
                    "channel_map": ["FL", "FR", "--", "RR"]}]),
        ),
        (
            "    Altset 1\n    Format: S16_LE S24_3LE\n    Channels: 2\n    Rates: 44100, 16000, 0\n",
            json!([{"format": "S16_LE", "interface": 4, "altset": 1, "channels": 2,
                    "rates_hz": [16000, 44100]},
                   {"format": "S24_3LE", "interface": 4, "altset": 1, "channels": 2,
                    "rates_hz": [16000, 44100]}]),
        ),
        (
            "    Altset 1\n    Format: S16_LE\n    Channels: 0\n    Bits: 0\n    Rates: 96000 - 8000 (continuous)\n    Altset 1\n    Format: S16_LE\n    Channels: 0\n",
            json!([{"format": "S16_LE", "interface": 4, "altset": 1}]),
        ),
        ("    Altset 2\n    Format:\n    Channels: 2\n", json!([])),
        ("    Format: S16_LE\n", json!([])),
    ];
    for (lines, expected) in cases {
        assert_eq!(json!(capture_modes(&block(lines))), expected, "{lines}");
    }
    let playback_only =
        "Card : USB Audio\n\nPlayback:\n  Interface 1\n    Altset 1\n    Format: S16_LE\n";
    assert!(capture_modes(playback_only).is_empty());
    let injected_capture = concat!(
        "Device title\nCapture:\n  Interface 9\n    Altset 9\n",
        "    Format: FAKE\n    Channels: 99\n    Rates: 99999\n\n",
        "Capture:\n  Interface 1\n    Altset 1\n    Format: S16_LE\n",
        "    Channels: 1\n    Rates: 48000\n"
    );
    assert_eq!(
        json!(capture_modes(injected_capture)),
        json!([{"format": "S16_LE", "interface": 1, "altset": 1,
                "channels": 1, "rates_hz": [48000]}])
    );
    assert_eq!(keyword_number("Interface = 1", "Interface"), None);
    assert_eq!(
        keyword_number("Interface 99999999999", "Interface"),
        Some(None)
    );
}

/// The id is the card's sysfs device and the PCM number: unchanged when the
/// card is renumbered or a temporary card id changes, different for identical
/// microphones on two ports and for a second capture PCM. Missing USB strings
/// are omitted. The key and its hash are pinned, so a microphone keeps its id
/// from one release to the next.
#[test]
fn ids_follow_the_sysfs_device_not_the_card_number() {
    let scan_as = |index: u32, id: &str| {
        let mut board = Board::new();
        board.usb_mic(index, "1-3.2", id, MONO_STREAM);
        board.scan().unwrap().remove(0)
    };
    let (first, renumbered) = (scan_as(2, "Nano"), scan_as(7, "Nano_1"));
    assert_eq!(first.id, renumbered.id);
    assert_eq!(renumbered.identity.card_index, 7);
    assert_eq!(
        value(&renumbered)["capture_target"]["selector"],
        "plughw:CARD=Nano_1,DEV=0"
    );
    let usb = json!({"vendor_id": "b58e", "product_id": "0005", "bus_path": "1-3.2",
                     "interface": "1-3.2:1.0"});
    assert_eq!(value(&first)["identity"]["usb"], usb);
    let key = "sysfs:devices/pci0000:00/usb1/1-3.2/1-3.2:1.0:pcm0c";
    assert_eq!(
        format!("{:016x}", fnv1a(key)),
        "3ff3d77bf791d455",
        "pinned id"
    );

    let mut board = Board::new();
    board.usb_mic(0, "1-3.2", "Nano", YETI_STREAM);
    board.usb_mic(1, "1-3.3", "Nano_1", YETI_STREAM);
    board.capture(1, 1, (1, 1), Some(MONO_STREAM));
    let records = board.scan().unwrap();
    let keys = Vec::from_iter(records.iter().map(|r| json!(r.identity.stable_key)));
    let mut expected = [
        "1-3.2/1-3.2:1.0:pcm0c",
        "1-3.3/1-3.3:1.0:pcm0c",
        "1-3.3/1-3.3:1.0:pcm1c",
    ]
    .map(|key| json!(format!("sysfs:{XHCI}/{key}")));
    let id_order = |key: &Value| fnv1a(key.as_str().unwrap());
    expected.sort_by_key(id_order);
    assert_eq!(keys, expected, "distinct and sorted by id");
}

/// Playback-only cards contribute nothing. A capture PCM is still reported,
/// with an issue saying why, when its card is not on USB and publishes no
/// formats, when ALSA's metadata is unreadable or reports no valid
/// availability, or when the card id cannot form a safe selector.
#[test]
fn incomplete_captures_degrade_and_playback_only_cards_are_skipped() {
    let mut board = Board::new();
    board.usb("1-1.5", ("1234", "5678"), &[]);
    board.card(
        0,
        ("Speaker", "USB-Audio", "Speaker"),
        &format!("{XHCI}/1-1.5/1-1.5:1.0"),
    );
    board.playback(0, 0);
    board.card(
        1,
        ("Codec", "simple-card", "Board Codec"),
        "devices/platform/sound",
    );
    board.capture(1, 0, (1, 1), None);
    board.write(
        "proc/asound/card1/pcm1c/info",
        "name: Line In\nsubdevices_count: 0\n",
    );
    board.write("sys/class/sound/pcmC1D1c", "");
    board.card(
        2,
        ("Mic", "USB-Audio", ""),
        &format!("{XHCI}/1-1.6/1-1.6:1.0"),
    );
    fs::create_dir_all(board.path("proc/asound/card2/pcm0c")).unwrap();
    board.write("sys/class/sound/pcmC2D0c", "");
    board.write(
        &format!("sys/{XHCI}/1-1.6/1-1.6:1.0/sound/card2/id"),
        "bad id\n",
    );
    board.write("proc/asound/card2/stream0", MONO_STREAM);

    let details = board.details();
    let summary = |details: &Value| {
        let codes = details["issues"].as_array().map(|issues| {
            Vec::from_iter(issues.iter().map(|issue| issue["code"].as_str().unwrap()))
        });
        json!({"name": details["name"], "connection": details["connection"],
               "modes": details["modes"].as_array().unwrap().len(),
               "state": details["availability"]["state"],
               "selector": details["capture_target"]["selector"], "issues": codes})
    };
    let unknown = "peripherals.availability_unknown";
    let expected = json!([
        {"name": "Board Codec", "connection": "platform", "modes": 0, "state": "unknown",
         "selector": "plughw:CARD=Codec,DEV=1",
         "issues": ["peripherals.capabilities_unavailable", unknown]},
        {"name": "Board Codec", "connection": "platform", "modes": 0, "state": "available",
         "selector": "plughw:CARD=Codec,DEV=0",
         "issues": ["peripherals.capabilities_unavailable"]},
        {"name": "bad id", "connection": "platform", "modes": 1, "state": "unknown",
         "selector": null,
         "issues": ["peripherals.pcm_info_unreadable",
                    "peripherals.capture_selector_unavailable", unknown]}
    ]);
    let mut actual = Vec::from_iter(details.iter().map(summary));
    actual.sort_by_key(|summary| summary.to_string());
    assert_eq!(Value::from(actual), expected);
}

/// CONFIG_SND_VERBOSE_PROCFS controls the per-card `pcmMc` directories, but
/// not PCM class devices. Capture enumeration therefore still works without
/// the optional procfs directories; only their name and availability degrade.
#[test]
fn capture_devices_do_not_require_verbose_procfs() {
    let mut board = Board::new();
    board.usb_mic(0, "1-1.2", "Nano", YETI_STREAM);
    fs::remove_dir_all(board.path("proc/asound/card0/pcm0c")).unwrap();

    let records = board.scan().unwrap();
    assert_eq!(records.len(), 1);
    let details = &value(&records[0]);
    assert_eq!(
        details["capture_target"]["selector"],
        "plughw:CARD=Nano,DEV=0"
    );
    assert_eq!(details["modes"].as_array().unwrap().len(), 2);
    assert_eq!(details["availability"]["state"], "unknown");
    let codes = Vec::from_iter(
        details["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["code"].as_str().unwrap()),
    );
    assert_eq!(
        codes,
        [
            "peripherals.pcm_info_unreadable",
            "peripherals.availability_unknown"
        ]
    );
}

/// The global card list's name and driver are used only when its id matches
/// the live sysfs id: card indices are reused.
#[test]
fn reused_card_index_discards_stale_global_metadata() {
    let mut board = Board::new();
    let port = "1-1.2";
    board.usb_mic(0, port, "OldMic", MONO_STREAM);
    board.write(
        &format!("sys/{XHCI}/{port}/{port}:1.0/sound/card0/id"),
        "NewMic\n",
    );

    let records = board.scan().unwrap();
    assert_eq!(records.len(), 1);
    let details = &value(&records[0]);
    assert_eq!(details["name"], "USB Audio");
    assert_eq!(details["capture_target"]["card_id"], "NewMic");
    assert_eq!(
        details["capture_target"]["selector"],
        "plughw:CARD=NewMic,DEV=0"
    );
    assert_eq!(details["identity"]["card_id"], "NewMic");
    assert!(details["identity"].get("card_name").is_none());
    assert!(details["identity"].get("card_driver").is_none());
}

/// A one- or two-digit card id gets no selector: ALSA would read `CARD=7` as
/// card index 7, not the card whose id is `7`. Longer numeric ids and ids
/// with a letter are looked up by id and keep their selector.
#[test]
fn index_like_card_ids_get_no_selector() {
    let mut board = Board::new();
    for (index, id) in [(0, "7"), (1, "12"), (2, "123"), (3, "D7")] {
        board.usb_mic(index, &format!("1-1.{index}"), id, MONO_STREAM);
    }
    let mut targets = Vec::from_iter(board.details().into_iter().map(|details| {
        let target = &details["capture_target"];
        json!([target["card_id"], target["selector"], details["issues"]])
    }));
    targets.sort_by_key(|target| target[0].to_string());
    let ambiguous = json!([{
        "code": "peripherals.capture_selector_unavailable",
        "reason": "The ALSA card ID is a one- or two-digit number, which ALSA reads as a card \
                   index in a CARD= selector, so it could open a different card. Give the \
                   card an ID that is not a number (the driver's id module option).",
    }]);
    let expected = [
        json!(["12", null, ambiguous]),
        json!(["123", "plughw:CARD=123,DEV=0", null]),
        json!(["7", null, ambiguous]),
        json!(["D7", "plughw:CARD=D7,DEV=0", null]),
    ];
    assert_eq!(targets, expected);
}

/// A card registered without a parent device (a loopback or other virtual
/// card) sits under `devices/virtual/sound` with no `device` link. It does not
/// fail the scan: its capture PCMs are listed beside the USB microphone with
/// what ALSA reports, `connection: unknown`, no USB identity, an id keyed on
/// the card id (so it survives renumbering), and an issue saying why.
#[test]
fn card_without_a_parent_device_is_listed_beside_usb_microphones() {
    let scan_as = |index: u32| {
        let mut board = Board::new();
        board.usb_mic(0, "1-3.2", "Nano", MONO_STREAM);
        board.card(
            index,
            ("Loopback", "Loopback", "Loopback"),
            "devices/virtual",
        );
        let card = format!("sys/devices/virtual/sound/card{index}");
        fs::remove_file(board.path(&format!("{card}/device"))).unwrap();
        board.capture(index, 0, (8, 8), None);
        board.capture(index, 1, (8, 7), None);
        board.scan().unwrap()
    };
    let records = scan_as(1);
    let summary = Vec::from_iter(records.iter().map(|record| {
        let details = &value(record);
        let codes = details["issues"].as_array().map(|issues| {
            Vec::from_iter(issues.iter().map(|issue| issue["code"].as_str().unwrap()))
        });
        json!([
            details["connection"],
            details["identity"]["stable_key"],
            details["identity"].get("usb").is_some(),
            details["capture_target"]["selector"],
            details["availability"]["state"],
            codes
        ])
    }));
    let missing = [
        "peripherals.sysfs_device_missing",
        "peripherals.capabilities_unavailable",
    ];
    let virtual_pcm = |pcm| {
        let key = format!("alsa-card-id:Loopback:pcm{pcm}c");
        let selector = format!("plughw:CARD=Loopback,DEV={pcm}");
        json!(["unknown", key, false, selector, "available", missing])
    };
    let usb_key = format!("sysfs:{XHCI}/1-3.2/1-3.2:1.0:pcm0c");
    let usb = json!([
        "usb",
        usb_key,
        true,
        "plughw:CARD=Nano,DEV=0",
        "available",
        null
    ]);
    let mut expected = [usb, virtual_pcm(0), virtual_pcm(1)];
    expected.sort_by_key(|row| fnv1a(row[1].as_str().unwrap()));
    assert_eq!(summary, expected);
    let loopback = &records
        .iter()
        .find(|r| r.connection == Connection::Unknown)
        .unwrap();
    assert_eq!(loopback.identity.card_driver.as_deref(), Some("Loopback"));
    let ids = |records: &[Microphone]| Vec::from_iter(records.iter().map(|r| r.id.clone()));
    assert_eq!(ids(&records), ids(&scan_as(5)), "renumbering keeps the ids");

    // The kernel never registers a card without an id. An empty live id is a
    // transient/invalid snapshot and is skipped rather than keyed by index.
    let mut board = Board::new();
    for index in [0, 1] {
        board.card(index, ("", "Dummy", "Dummy"), "devices/virtual");
        let link = format!("sys/devices/virtual/sound/card{index}/device");
        fs::remove_file(board.path(&link)).unwrap();
        board.capture(index, 0, (1, 1), None);
    }
    assert!(board.details().is_empty());
}

/// A card whose live sysfs id is missing is skipped rather than published
/// with an id from the global list; one whose id cannot be read fails the scan.
#[test]
fn card_without_a_live_id_is_skipped() {
    let mut board = Board::new();
    let port = "1-3.2";
    board.usb_mic(2, port, "Departed", MONO_STREAM);
    let id = board.path(&format!("sys/{XHCI}/{port}/{port}:1.0/sound/card2/id"));
    fs::remove_file(&id).unwrap();
    assert!(board.details().is_empty());
    // An id that exists but cannot be read fails the scan instead.
    fs::create_dir(&id).unwrap();
    let error = board.scan().unwrap_err();
    assert_eq!(error.code, "io.open");
    assert!(
        error.reason.starts_with("failed to read ALSA card id"),
        "{}",
        error.reason
    );
}

/// A USB attribute that exists but cannot be read fails the scan rather than
/// reading as absent.
#[test]
fn unreadable_usb_attributes_fail_the_scan() {
    let mut board = Board::new();
    let port = "1-3.2";
    board.usb_mic(2, port, "Yeti", MONO_STREAM);
    fs::create_dir_all(board.path(&format!("sys/{XHCI}/{port}/serial"))).unwrap();
    let error = board.scan().unwrap_err();
    assert_eq!(error.code, "io.open");
    let read = "failed to read ALSA sysfs attribute";
    assert!(error.reason.starts_with(read), "{}", error.reason);
}

/// A kernel without ALSA, or without sound cards, has no microphones. A
/// stale or unparsable global card list only loses name and driver. An
/// unreadable list, a sound class that cannot be checked, a present card with
/// no `device` link, and an incomplete USB ancestor fail the scan. A card already removed is skipped.
#[test]
fn absent_alsa_and_partial_snapshots_are_handled() {
    let board = Board::new();
    assert!(board.scan().unwrap().is_empty(), "no soundcards");
    let empty = TempDir::new();
    let mut provider = AlsaProvider::with_roots(empty.path().join("asound"), empty.path(), "/dev");
    assert!(
        provider.discover().unwrap().is_empty(),
        "no ALSA in the kernel"
    );
    // A sound class that cannot be checked is an error, not "no ALSA".
    let broken = TempDir::new();
    fs::write(broken.path().join("class"), "").unwrap();
    let mut provider =
        AlsaProvider::with_roots(broken.path().join("asound"), broken.path(), "/dev");
    let error = provider.discover().unwrap_err();
    assert_eq!(error.code, "io.open");
    assert!(
        error
            .reason
            .starts_with("failed to inspect ALSA sysfs class"),
        "{}",
        error.reason
    );

    let fail = |board: &Board| {
        let error = board.scan().unwrap_err();
        format!("{} {}", error.code, error.reason)
    };
    let mut board = Board::new();
    board.usb_mic(2, "1-3.2", "Nano", MONO_STREAM);
    board.write(
        "proc/asound/cards",
        &format!(
            "{}{}",
            board.cards, " 3 [New            ]: USB-Audio - New\n"
        ),
    );
    board.scan().unwrap();
    board.write("proc/asound/cards", "--- no soundcards ---\n");
    board.scan().unwrap();
    board.write("proc/asound/cards", "garbage\n");
    board.scan().unwrap();
    board.write("proc/asound/cards", &board.cards);
    board.scan().unwrap();

    let link = board.path(&format!("sys/{XHCI}/1-3.2/1-3.2:1.0/sound/card2/device"));
    fs::remove_file(&link).unwrap();
    assert!(fail(&board).starts_with("io.open failed to resolve ALSA sysfs device"));
    symlink("../..", &link).unwrap();
    let device = board.path("sys/class/sound/card2");
    fs::rename(&device, board.path("sys/class/sound/hidden")).unwrap();
    assert!(
        board.scan().unwrap().is_empty(),
        "a removed card is skipped"
    );
    fs::rename(board.path("sys/class/sound/hidden"), &device).unwrap();
    fs::remove_file(board.path(&format!("sys/{XHCI}/1-3.2/idProduct"))).unwrap();
    assert!(fail(&board).contains("incomplete vendor/product attributes"));
    board.usb("1-3.2", ("b58e", "0005"), &[]);

    let asound = board.path("proc/asound");
    fs::set_permissions(&asound, fs::Permissions::from_mode(0o000)).unwrap();
    let denied = board.scan();
    fs::set_permissions(&asound, fs::Permissions::from_mode(0o755)).unwrap();
    if unsafe { libc::geteuid() } == 0 {
        return; // Root ignores the permission bits.
    }
    assert_eq!(denied.unwrap_err().code, CODE_PERMISSION_DENIED);
}
