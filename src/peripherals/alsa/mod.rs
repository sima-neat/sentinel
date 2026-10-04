//! Read-only discovery of ALSA capture devices (`daemon.audio.alsa`).
//!
//! Every capture PCM (`/sys/class/sound/pcmCNDMc`) becomes one `microphone`
//! record; playback-only cards and playback PCMs are not reported, and a
//! composite USB device (a webcam or headset) contributes only its capture
//! PCMs. Only kernel text is read: `/proc/asound/cards`, each card's `id`,
//! optional `pcmMc/info` and, for USB audio, `streamM`, plus the card's sysfs
//! device and USB ancestry for identity and the udev `/dev/snd/by-*` links.
//! No PCM or control device is opened, so discovery never takes or configures
//! a microphone. Ids hash the card's sysfs device path and the PCM device
//! number, never the card number, which changes between replugs; a card with
//! no parent device (a virtual card) is keyed by its card id instead.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::model::{Provider, ProviderError, Record};
use super::sysutil::{errno_of, io_error, read_text_file, CODE_DISCOVERY_FAILED, CODE_IO_OPEN};
use super::videodev2::MAX_ENUMERATION_ENTRIES;

pub const PROVIDER_NAME: &str = "daemon.audio.alsa";

/// Reads a USB device attribute; tests substitute one that unplugs the
/// device between the `idVendor` and `idProduct` reads.
type AttributeReader = fn(&Path) -> Option<String>;

/// The `daemon.audio.alsa` provider.
pub struct AlsaProvider {
    asound: PathBuf,
    sys_root: PathBuf,
    dev_root: PathBuf,
    subsystems: Vec<String>,
    read_usb_attribute: AttributeReader,
}

impl AlsaProvider {
    /// Scan the live system (`/proc/asound`, `/sys`, `/dev`).
    pub fn new() -> Self {
        Self::with_roots("/proc/asound", "/sys", "/dev")
    }

    fn with_roots(
        asound: impl Into<PathBuf>,
        sys_root: impl Into<PathBuf>,
        dev_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            asound: asound.into(),
            sys_root: sys_root.into(),
            dev_root: dev_root.into(),
            subsystems: vec!["sound".to_string()],
            read_usb_attribute: read_text_file,
        }
    }
}

impl Provider for AlsaProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn subsystems(&self) -> &[String] {
        &self.subsystems
    }

    fn discover(&mut self) -> Result<Vec<Record>, ProviderError> {
        self.scan()
    }
}

/// A card's line in `/proc/asound/cards`.
#[derive(Debug, Clone, Default, PartialEq)]
struct Card {
    id: String,
    driver: String,
    name: String,
}

/// A decimal number as the kernel prints it (`%d`, `%u`).
fn number(text: &str) -> Option<u32> {
    let text = text.trim();
    let digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    digits.then(|| text.parse().ok()).flatten()
}

/// `/proc/asound/cards`: `"%2i [%-15s]: %s - %s"` (index, id, driver, short
/// name), each followed by an indented long-name line, which is skipped. An
/// empty short name leaves the line ending in `" - "`.
fn parse_cards(text: &str) -> BTreeMap<u32, Card> {
    let parse = |line: &str| {
        let (index, rest) = line.trim_start().split_once([' ', '\t'])?;
        let (_, rest) = rest.split_once('[')?;
        let (id, rest) = rest.split_once(']')?;
        let (_, metadata) = rest.split_once(':')?;
        let (driver, name) = metadata.split_once(" - ")?;
        let card = Card {
            id: id.trim().into(),
            driver: driver.trim().into(),
            name: name.trim().into(),
        };
        Some((number(index)?, card))
    };
    text.lines().filter_map(parse).collect()
}

/// `key: value` lines, as in `pcmMc/info`; a repeated key keeps its last value.
fn parse_key_values(text: &str) -> BTreeMap<String, String> {
    let pair = |line: &str| {
        let (key, value) = line.split_once(':')?;
        Some((key.trim().to_string(), value.trim().to_string()))
    };
    text.lines().filter_map(pair).collect()
}

/// One `Altset` of a USB audio stream's capture section.
#[derive(Default)]
struct StreamFormat {
    interface: Option<u32>,
    altset: Option<u32>,
    formats: Vec<String>,
    channels: Option<u32>,
    sample_bits: Option<u32>,
    rates_hz: Vec<u32>,
    rate_range_hz: Option<(u32, u32)>,
    channel_map: Vec<String>,
}

/// `Interface N` or `Altset N` (not the running status's `Interface = N`).
fn keyword_number(line: &str, keyword: &str) -> Option<Option<u32>> {
    let mut words = line.split_whitespace();
    let matches = words.next() == Some(keyword);
    let value = words
        .next()
        .filter(|word| word.bytes().all(|b| b.is_ascii_digit()))?;
    (matches && words.next().is_none()).then(|| number(value))
}

/// The capture modes of a USB audio `streamM` file, in the layout of the
/// kernel's `sound/usb/proc.c`: a `Capture:` section of `Interface N` /
/// `Altset N` blocks with `Format:`, `Channels:`, `Rates:` (a list, or
/// `min - max (continuous)`), `Bits:` and `Channel map:` lines. One mode per
/// format of each altset, sorted and without duplicates. Zero channels, bits
/// or rates are omitted rather than published.
fn capture_modes(text: &str) -> Vec<Value> {
    let mut parsed: Vec<StreamFormat> = Vec::new();
    let (mut capture, mut interface, mut current) = (false, None, None);
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            parsed.extend(current.take());
            capture = stripped == "Capture:";
            interface = None;
            continue;
        }
        if !capture {
            continue;
        }
        if let Some(value) = keyword_number(stripped, "Interface") {
            interface = value;
            continue;
        }
        if let Some(altset) = keyword_number(stripped, "Altset") {
            parsed.extend(current.take());
            current = Some(StreamFormat {
                interface,
                altset,
                ..StreamFormat::default()
            });
            continue;
        }
        let (Some(mode), Some((key, value))) = (current.as_mut(), stripped.split_once(':')) else {
            continue;
        };
        let positive = |text: &str| number(text).filter(|&value| value > 0);
        let words = |text: &str| Vec::from_iter(text.split_whitespace().map(String::from));
        let value = value.trim();
        match key.trim() {
            "Format" => mode.formats = words(value),
            "Channels" => mode.channels = positive(value),
            "Bits" => mode.sample_bits = positive(value),
            "Channel map" => mode.channel_map = words(value),
            "Rates" => match value.strip_suffix("(continuous)") {
                Some(range) => {
                    let bounds = range.split_once('-');
                    let bounds =
                        bounds.and_then(|(low, high)| Some((positive(low)?, number(high)?)));
                    mode.rate_range_hz = bounds.filter(|(low, high)| low <= high);
                }
                None => {
                    let rates = value
                        .split(|c: char| !c.is_ascii_digit())
                        .filter_map(positive);
                    mode.rates_hz = rates.collect();
                    mode.rates_hz.sort_unstable();
                    mode.rates_hz.dedup();
                }
            },
            _ => {}
        }
    }
    parsed.extend(current);

    let mut modes = Vec::new();
    for mode in &parsed {
        for format in &mode.formats {
            let mut value = json!({"format": format});
            let optional = [
                ("interface", mode.interface),
                ("altset", mode.altset),
                ("channels", mode.channels),
                ("sample_bits", mode.sample_bits),
            ];
            for (key, field) in optional {
                if let Some(field) = field {
                    value[key] = json!(field);
                }
            }
            if !mode.rates_hz.is_empty() {
                value["rates_hz"] = json!(mode.rates_hz);
            }
            if let Some((min, max)) = mode.rate_range_hz {
                value["rate_range_hz"] = json!({"min": min, "max": max});
            }
            if !mode.channel_map.is_empty() {
                value["channel_map"] = json!(mode.channel_map);
            }
            modes.push(value);
        }
    }
    modes.sort_by_cached_key(Value::to_string);
    modes.dedup();
    modes
}

/// The entries of `directory` named `<prefix><number><suffix>`, by number.
fn numbered(
    directory: &Path,
    prefix: &str,
    suffix: &str,
) -> Result<BTreeMap<u32, PathBuf>, ProviderError> {
    let failed = |error: io::Error| io_error("failed to list", directory, &error, false);
    let mut entries = BTreeMap::new();
    for entry in fs::read_dir(directory)
        .map_err(failed)?
        .take(MAX_ENUMERATION_ENTRIES as usize)
    {
        let name = entry.map_err(failed)?.file_name();
        let name = name.to_string_lossy();
        let middle = name
            .strip_prefix(prefix)
            .and_then(|n| n.strip_suffix(suffix));
        if let Some(index) = middle.and_then(number) {
            entries.insert(index, directory.join(&*name));
        }
    }
    Ok(entries)
}

/// The first link, in name order, in `directory` that points at `target`
/// (udev's `/dev/snd/by-path` and `/dev/snd/by-id` links to `controlCN`).
fn link_to(directory: &Path, target: &str) -> Option<String> {
    let entries = fs::read_dir(directory).ok()?;
    let mut names: Vec<_> = entries
        .take(MAX_ENUMERATION_ENTRIES as usize)
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let link = fs::read_link(entry.path()).ok()?;
            (link.file_name()? == target).then(|| entry.file_name())
        })
        .collect();
    names.sort();
    let first = names.first()?;
    Some(directory.join(first).to_string_lossy().into_owned())
}

/// A USB card's `identity.usb`: from the nearest sysfs ancestor with
/// `idVendor` and `idProduct` (the USB device) and the USB interface between
/// it and the card's device. `None` for a card that is not on USB; the only
/// error is an ancestor with one of `idVendor` and `idProduct`.
fn usb_identity(
    sys: &Path,
    device: &Path,
    read: AttributeReader,
) -> Result<Option<Value>, ProviderError> {
    for path in device.ancestors().take_while(|path| *path != sys) {
        let attribute = |name: &str| read(&path.join(name));
        let (vendor, product) = match (attribute("idVendor"), attribute("idProduct")) {
            (Some(vendor), Some(product)) => (vendor, product),
            (None, None) => continue,
            _ => {
                let reason = format!(
                    "ALSA USB ancestor has incomplete vendor/product attributes: {}",
                    path.display()
                );
                return Err(ProviderError::new(CODE_IO_OPEN, reason));
            }
        };
        let bus_path = path.file_name().unwrap_or_default().to_string_lossy();
        let mut value = json!({"vendor_id": vendor, "product_id": product, "bus_path": bus_path});
        let prefix = format!("{bus_path}:");
        let interface = device
            .ancestors()
            .take_while(|child| *child != path)
            .filter_map(|child| child.file_name()?.to_str())
            .find(|name| name.starts_with(&prefix));
        if let Some(interface) = interface {
            value["interface"] = json!(interface);
        }
        for key in ["manufacturer", "product", "serial"] {
            if let Some(text) = attribute(key).filter(|text| !text.is_empty()) {
                value[key] = json!(text);
            }
        }
        return Ok(Some(value));
    }
    Ok(None)
}

fn issue(issues: &mut Vec<Value>, code: &str, reason: &str) {
    issues.push(json!({"code": code, "reason": reason}));
}

/// `in_use` when no capture subdevice is free, from `pcmMc/info`.
fn availability(info: &BTreeMap<String, String>, issues: &mut Vec<Value>) -> Value {
    let field = |key: &str| info.get(key).and_then(|value| number(value));
    match (field("subdevices_count"), field("subdevices_avail")) {
        (Some(count), Some(free)) if count > 0 && free <= count => json!({
            "state": if free == 0 { "in_use" } else { "available" },
            "subdevices": count,
            "subdevices_available": free,
        }),
        _ => {
            issue(
                issues,
                "peripherals.availability_unknown",
                "ALSA did not report valid capture subdevice availability. Refresh after the \
                 device finishes initializing.",
            );
            json!({"state": "unknown"})
        }
    }
}

/// Whether `path` is gone (ENOENT or ENOTDIR); any other error is not taken
/// as a removal.
fn vanished(path: &Path) -> bool {
    fs::metadata(path).is_err_and(|error| matches!(errno_of(&error), libc::ENOENT | libc::ENOTDIR))
}

/// Whether a present card was registered without a parent device: the
/// kernel then places it under `devices/virtual/sound` and creates no
/// `device` link (`get_device_parent` and `device_add_class_symlinks` in
/// `drivers/base/core.c`). A card with a parent that lacks the link is being
/// removed, which is not this.
fn parentless(sys: &Path, entry: &Path, link: &Path) -> bool {
    let no_link = fs::symlink_metadata(link).is_err_and(|error| errno_of(&error) == libc::ENOENT);
    let virtual_card = fs::canonicalize(entry)
        .is_ok_and(|card| card.parent() == Some(&sys.join("devices/virtual/sound")));
    no_link && virtual_card
}

/// ALSA card ids are safe in a `plughw:CARD=<id>` selector when they hold
/// only letters, digits, `_` and `-`.
fn valid_card_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Whether a card id is one or two decimal digits: alsa-lib's
/// `snd_card_get_index`, which resolves `CARD=` in `hw:` and `plughw:`, reads
/// such a string as a card index before trying it as an id.
fn index_like_card_id(id: &str) -> bool {
    matches!(id.len(), 1 | 2) && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// 64-bit FNV-1a, the hash behind the record ids.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(14_695_981_039_346_656_037, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211)
    })
}

impl AlsaProvider {
    fn scan(&self) -> Result<Vec<Record>, ProviderError> {
        let cards_path = self.asound.join("cards");
        let text = match fs::read(&cards_path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            // A kernel without ALSA has no sound class and no microphones.
            Err(error)
                if errno_of(&error) == libc::ENOENT
                    && !self.sys_root.join("class/sound").exists() =>
            {
                return Ok(Vec::new())
            }
            Err(error) => {
                return Err(io_error(
                    "failed to read ALSA card list",
                    &cards_path,
                    &error,
                    false,
                ))
            }
        };
        let directories = numbered(&self.asound, "card", "")?;
        let no_cards = text.contains("no soundcards");
        let cards = if no_cards {
            BTreeMap::new()
        } else {
            parse_cards(&text)
        };
        if !no_cards && cards.is_empty() {
            let reason = format!(
                "could not parse any ALSA card from {}; refresh after the sound subsystem \
                 finishes initializing.",
                cards_path.display()
            );
            return Err(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
        }
        // A card being added or removed can be listed without its directory,
        // or the reverse; publishing half of it would churn ids.
        if !cards.keys().eq(directories.keys()) {
            let reason = format!(
                "ALSA card list {} and its card directories disagree (a card is being added or \
                 removed); refresh after the sound subsystem finishes updating.",
                cards_path.display()
            );
            return Err(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
        }
        if cards.is_empty() {
            return Ok(Vec::new());
        }
        let sys = fs::canonicalize(&self.sys_root).map_err(|error| {
            io_error(
                "failed to resolve ALSA sysfs root",
                &self.sys_root,
                &error,
                false,
            )
        })?;
        let mut records = Vec::new();
        for ((&index, card), directory) in cards.iter().zip(directories.values()) {
            records.extend(self.card_records(&sys, index, card.clone(), directory)?);
        }
        records.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(records)
    }

    /// One record per capture PCM of card `index`.
    fn card_records(
        &self,
        sys: &Path,
        index: u32,
        mut card: Card,
        directory: &Path,
    ) -> Result<Vec<Record>, ProviderError> {
        if let Some(id) = read_text_file(&directory.join("id")).filter(|id| !id.is_empty()) {
            card.id = id;
        }
        let entry = self.sys_root.join(format!("class/sound/card{index}"));
        let link = entry.join("device");
        let device = match fs::canonicalize(&link) {
            Ok(device) => Some(device),
            // A card removed after the card list was read has no class entry.
            Err(_) if vanished(&entry) => return Ok(Vec::new()),
            Err(_) if parentless(sys, &entry, &link) => None,
            Err(error) => {
                let action = "failed to resolve ALSA sysfs device";
                return Err(io_error(action, &link, &error, false));
            }
        };
        let (key_base, usb) = match &device {
            Some(device) => {
                let Ok(topology) = device.strip_prefix(sys) else {
                    let reason = format!(
                        "ALSA sysfs device escaped the configured sysfs root: {}",
                        device.display()
                    );
                    return Err(ProviderError::new(CODE_IO_OPEN, reason));
                };
                let usb = match usb_identity(sys, device, self.read_usb_attribute) {
                    // An unplug removes the attributes one at a time: if the
                    // card's sysfs device (below the USB ancestor) is gone
                    // too, the card vanished mid-scan and is skipped.
                    Err(_) if vanished(device) => return Ok(Vec::new()),
                    usb => usb?,
                };
                // If the whole device disappeared before the first USB
                // attribute read, identity returns `None` rather than an
                // incomplete-pair error. Do not misclassify that stale path
                // as a platform microphone.
                if usb.is_none() && vanished(device) {
                    return Ok(Vec::new());
                }
                (format!("sysfs:{}", topology.to_string_lossy()), usb)
            }
            // No device path to key on: the card id is the only attribute
            // that survives renumbering, and ALSA keeps it unique among the
            // registered cards. The kernel never registers a card without
            // one; should the id read back empty, the card number keeps the
            // records apart.
            None if card.id.is_empty() => (format!("alsa-card-index:{index}"), None),
            None => (format!("alsa-card-id:{}", card.id), None),
        };
        let control = format!("controlC{index}");
        let snd = self.dev_root.join("snd");
        let links = [
            ("by_path", link_to(&snd.join("by-path"), &control)),
            ("by_id", link_to(&snd.join("by-id"), &control)),
        ];

        let mut records = Vec::new();
        // The per-card `pcmMc` procfs directories exist only with
        // CONFIG_SND_VERBOSE_PROCFS. PCM class devices exist independently,
        // so use them to enumerate captures and treat procfs metadata as
        // optional below.
        let sound_class = self.sys_root.join("class/sound");
        let prefix = format!("pcmC{index}D");
        for (pcm, _) in numbered(&sound_class, &prefix, "c")? {
            let mut issues = Vec::new();
            if device.is_none() {
                issue(
                    &mut issues,
                    "peripherals.sysfs_device_missing",
                    "The ALSA card has no parent device in sysfs (a virtual card, or a driver \
                     that registers its card without one), so no bus or USB identity is known \
                     and the record id follows the card ID: it changes if the card ID changes.",
                );
            }
            let info = read_text_file(&directory.join(format!("pcm{pcm}c/info")));
            if info.is_none() {
                issue(
                    &mut issues,
                    "peripherals.pcm_info_unreadable",
                    "ALSA capture metadata could not be read. Check /proc/asound permissions \
                     and refresh the catalog.",
                );
            }
            let info = parse_key_values(info.as_deref().unwrap_or_default());
            let pcm_name = info.get("name").cloned().unwrap_or_default();
            let name = [&card.name, &pcm_name, &card.id]
                .into_iter()
                .find(|name| !name.is_empty())
                .cloned()
                .unwrap_or_else(|| format!("ALSA capture PCM {pcm}"));

            let mut capture_target = json!({"card_id": card.id, "device": pcm});
            let unavailable = if !valid_card_id(&card.id) {
                Some(
                    "ALSA did not report a safe stable card ID. Refresh after the device \
                     finishes initializing.",
                )
            } else if index_like_card_id(&card.id) {
                Some(
                    "The ALSA card ID is a one- or two-digit number, which ALSA reads as a \
                     card index in a CARD= selector, so it could open a different card. Give \
                     the card an ID that is not a number (the driver's id module option).",
                )
            } else {
                None
            };
            match unavailable {
                None => {
                    let selector = format!("plughw:CARD={},DEV={pcm}", card.id);
                    capture_target["selector"] = json!(selector);
                }
                Some(reason) => issue(
                    &mut issues,
                    "peripherals.capture_selector_unavailable",
                    reason,
                ),
            }

            let stable_key = format!("{key_base}:pcm{pcm}c");
            let pcm_node = snd.join(format!("pcmC{index}D{pcm}c"));
            let mut identity = json!({
                "stable_key": stable_key,
                "card_index": index,
                "pcm_node": pcm_node.to_string_lossy(),
            });
            let optional = [
                ("card_id", &card.id),
                ("card_name", &card.name),
                ("card_driver", &card.driver),
                ("pcm_name", &pcm_name),
            ];
            for (key, value) in optional {
                if !value.is_empty() {
                    identity[key] = json!(value);
                }
            }
            for (key, link) in &links {
                if let Some(link) = link {
                    identity[*key] = json!(link);
                }
            }
            if let Some(usb) = &usb {
                identity["usb"] = usb.clone();
            }

            let stream = read_text_file(&directory.join(format!("stream{pcm}")));
            let modes = stream.as_deref().map(capture_modes).unwrap_or_default();
            if modes.is_empty() {
                let reason = if usb.is_some() {
                    "The USB audio stream did not publish capture formats. Reconnect the device \
                     and refresh the catalog."
                } else {
                    "This ALSA driver does not publish read-only capture formats. The daemon \
                     does not open the PCM during discovery."
                };
                issue(&mut issues, "peripherals.capabilities_unavailable", reason);
            }

            let mut details = json!({
                "name": name,
                "backend": "alsa",
                "connection": match (&device, &usb) {
                    (None, _) => "unknown",
                    (_, Some(_)) => "usb",
                    _ => "platform",
                },
                "capture_target": capture_target,
                "identity": identity,
                "modes": modes,
                "availability": availability(&info, &mut issues),
            });
            if !issues.is_empty() {
                details["issues"] = json!(issues);
            }
            records.push(Record {
                id: format!("microphone:alsa:{:016x}", fnv1a(&stable_key)),
                kind: "microphone".to_string(),
                provider: PROVIDER_NAME.to_string(),
                details,
            });
        }
        Ok(records)
    }
}
