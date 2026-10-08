//! Read-only discovery of ALSA capture devices (`microphone.alsa`).
//!
//! Every capture PCM (`/sys/class/sound/pcmCNDMc`) becomes one `microphone`
//! record; playback-only cards and playback PCMs are not reported, and a
//! composite USB device (a webcam or headset) contributes only its capture
//! PCMs. Cards and PCMs are listed from `/sys/class/sound`; only kernel text
//! is read: each card's sysfs `id`, `/proc/asound/cards`, `pcmMc/info` and,
//! for USB audio, `streamM`, plus the card's sysfs device and USB ancestry
//! for identity and the udev `/dev/snd/by-*` links.
//! No PCM or control device is opened, so discovery never takes or configures
//! a microphone. Ids hash the card's sysfs device path and the PCM device
//! number, never the card number, which changes between replugs; a card with
//! no parent device (a virtual card) is keyed by its card id instead.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::microphone::{
    Backend, CaptureTarget, Connection, Identity, Issue, Microphone, Mode, RateRange, UsbIdentity,
};
use super::sysutil::{errno_of, io_error, read_text_file, vanished, CODE_IO_OPEN};
use super::{Availability, AvailabilityState, Peripheral, Provider, ProviderError};

pub const PROVIDER_NAME: &str = "microphone.alsa";

/// The `microphone.alsa` provider.
pub struct AlsaProvider {
    asound: PathBuf,
    sys_root: PathBuf,
    dev_root: PathBuf,
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
        }
    }
}

impl Provider for AlsaProvider {
    fn name(&self) -> &'static str {
        PROVIDER_NAME
    }

    fn subsystems(&self) -> &'static [&'static str] {
        &["sound"]
    }

    fn discover(&mut self) -> Result<Vec<Peripheral>, ProviderError> {
        let microphones = self.scan()?;
        Ok(microphones
            .into_iter()
            .map(Peripheral::Microphone)
            .collect())
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
/// name), each followed by `" %s"` (the long name). Device-provided names may
/// contain newlines, including text that looks like another header, so accept
/// only registered indices and discard ambiguous metadata. An empty short
/// name leaves the header ending in `" - "`.
fn parse_cards(text: &str, registered: &BTreeSet<u32>) -> BTreeMap<u32, Card> {
    let parse = |line: &str| {
        // The kernel right-aligns a one-digit index in a two-character field.
        let line = line.strip_prefix(' ').unwrap_or(line);
        let (index, rest) = line.split_once(' ')?;
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
    let mut cards = BTreeMap::new();
    let mut ambiguous = BTreeSet::new();
    for (index, card) in text.lines().filter_map(parse) {
        if registered.contains(&index) && cards.insert(index, card).is_some() {
            ambiguous.insert(index);
        }
    }
    cards.retain(|index, _| !ambiguous.contains(index));
    cards
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
/// format of each altset, sorted by interface and altset and without
/// duplicates. Zero channels, bits or rates are omitted rather than published.
fn capture_modes(text: &str) -> Vec<Mode> {
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
            // The device-controlled stream title can contain newlines and
            // imitate an earlier capture section. The kernel's real section
            // is the final top-level `Capture:` block, so discard anything
            // accumulated before it.
            if capture {
                parsed.clear();
            }
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
            modes.push(Mode {
                format: format.clone(),
                interface: mode.interface,
                altset: mode.altset,
                channels: mode.channels,
                sample_bits: mode.sample_bits,
                rates_hz: mode.rates_hz.clone(),
                rate_range_hz: mode.rate_range_hz.map(|(min, max)| RateRange { min, max }),
                channel_map: mode.channel_map.clone(),
            });
        }
    }
    modes.sort_by(|left, right| {
        let position = |mode: &Mode| (mode.interface, mode.altset);
        position(left)
            .cmp(&position(right))
            .then_with(|| left.cmp(right))
    });
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
    for entry in fs::read_dir(directory).map_err(failed)? {
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
/// it and the card's device. `None` for a card that is not on USB; an error
/// for an ancestor with one of `idVendor` and `idProduct`, or an attribute
/// that exists but cannot be read.
fn usb_identity(sys: &Path, device: &Path) -> Result<Option<UsbIdentity>, ProviderError> {
    for path in device.ancestors().take_while(|path| *path != sys) {
        let attribute = |name: &str| {
            let file = path.join(name);
            read_text_file(&file).map_err(|error| {
                io_error("failed to read ALSA sysfs attribute", &file, &error, false)
            })
        };
        let (vendor, product) = match (attribute("idVendor")?, attribute("idProduct")?) {
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
        let prefix = format!("{bus_path}:");
        let interface = device
            .ancestors()
            .take_while(|child| *child != path)
            .filter_map(|child| child.file_name()?.to_str())
            .find(|name| name.starts_with(&prefix));
        let text = |key: &str| attribute(key).map(|text| text.filter(|text| !text.is_empty()));
        return Ok(Some(UsbIdentity {
            vendor_id: vendor,
            product_id: product,
            bus_path: bus_path.into_owned(),
            interface: interface.map(str::to_owned),
            manufacturer: text("manufacturer")?,
            product: text("product")?,
            serial: text("serial")?,
        }));
    }
    Ok(None)
}

fn issue(issues: &mut Vec<Issue>, code: &str, reason: &str) {
    issues.push(Issue {
        code: code.into(),
        reason: reason.into(),
    });
}

/// `in_use` when no capture subdevice is free, from `pcmMc/info`.
fn availability(info: &BTreeMap<&str, &str>, issues: &mut Vec<Issue>) -> Availability {
    let field = |key: &str| info.get(key).copied().and_then(number);
    match (field("subdevices_count"), field("subdevices_avail")) {
        (Some(count), Some(free)) if count > 0 && free <= count => Availability {
            state: if free == 0 {
                AvailabilityState::InUse
            } else {
                AvailabilityState::Available
            },
            reason: None,
            subdevices: Some(count),
            subdevices_available: Some(free),
        },
        _ => {
            issue(
                issues,
                "peripherals.availability_unknown",
                "ALSA did not report valid capture subdevice availability. Refresh after the \
                 device finishes initializing.",
            );
            Availability {
                state: AvailabilityState::Unknown,
                reason: None,
                subdevices: None,
                subdevices_available: None,
            }
        }
    }
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

/// 64-bit FNV-1a, the hash behind the microphone ids.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(14_695_981_039_346_656_037, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211)
    })
}

impl AlsaProvider {
    fn scan(&self) -> Result<Vec<Microphone>, ProviderError> {
        let sound_class = self.sys_root.join("class/sound");
        // A kernel without ALSA has no sound class and no microphones; a
        // class that cannot be checked fails the scan.
        let inspect = "failed to inspect ALSA sysfs class";
        let present = sound_class
            .try_exists()
            .map_err(|error| io_error(inspect, &sound_class, &error, false))?;
        if !present {
            return Ok(Vec::new());
        }
        let cards = numbered(&sound_class, "card", "")?;
        if cards.is_empty() {
            return Ok(Vec::new());
        }
        let cards_path = self.asound.join("cards");
        let text = fs::read(&cards_path).map_err(|error| {
            io_error("failed to read ALSA card list", &cards_path, &error, false)
        })?;
        let registered = cards.keys().copied().collect();
        let mut listed = parse_cards(&String::from_utf8_lossy(&text), &registered);
        let sys = fs::canonicalize(&self.sys_root).map_err(|error| {
            io_error(
                "failed to resolve ALSA sysfs root",
                &self.sys_root,
                &error,
                false,
            )
        })?;
        let mut records = Vec::new();
        for (&index, entry) in &cards {
            records.extend(self.card_records(&sys, index, entry, listed.remove(&index))?);
        }
        records.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(records)
    }

    /// One microphone per capture PCM of card `index`, whose class entry is
    /// `entry` and whose `/proc/asound/cards` line is `listed`.
    fn card_records(
        &self,
        sys: &Path,
        index: u32,
        entry: &Path,
        listed: Option<Card>,
    ) -> Result<Vec<Microphone>, ProviderError> {
        // The kernel never registers a card without an id; a missing or empty
        // one is skipped until a later scan, and a read error fails the scan.
        let id = entry.join("id");
        let card_id = read_text_file(&id)
            .map_err(|error| io_error("failed to read ALSA card id", &id, &error, false))?;
        let Some(card_id) = card_id.filter(|id| !id.is_empty()) else {
            return Ok(Vec::new());
        };
        // The global list's name and driver describe this card only when its
        // id matches the live one; card indices are reused.
        let listed = listed.filter(|card| card.id == card_id).unwrap_or_default();
        let link = entry.join("device");
        let device = match fs::canonicalize(&link) {
            Ok(device) => Some(device),
            Err(_) if vanished(entry) => return Ok(Vec::new()),
            Err(_) if parentless(sys, entry, &link) => None,
            Err(error) => {
                let action = "failed to resolve ALSA sysfs device";
                return Err(io_error(action, &link, &error, false));
            }
        };
        let (key_base, usb) = match &device {
            Some(device) => {
                let topology = device.strip_prefix(sys).unwrap_or(device);
                let key_base = format!("sysfs:{}", topology.to_string_lossy());
                (key_base, usb_identity(sys, device)?)
            }
            // No device path to key on: the live card id is the only
            // attribute that survives renumbering, and ALSA keeps it unique
            // among the registered cards.
            None => (format!("alsa-card-id:{card_id}"), None),
        };
        let connection = match (&device, &usb) {
            (None, _) => Connection::Unknown,
            (_, Some(_)) => Connection::Usb,
            _ => Connection::Platform,
        };
        let selector_unavailable = if !card_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            Some(
                "ALSA did not report a safe stable card ID. Refresh after the device \
                 finishes initializing.",
            )
        // alsa-lib's `snd_card_get_index` reads a one- or two-digit `CARD=`
        // value as a card index before trying it as an id.
        } else if card_id.len() <= 2 && card_id.bytes().all(|byte| byte.is_ascii_digit()) {
            Some(
                "The ALSA card ID is a one- or two-digit number, which ALSA reads as a \
                 card index in a CARD= selector, so it could open a different card. Give \
                 the card an ID that is not a number (the driver's id module option).",
            )
        } else {
            None
        };
        let control = format!("controlC{index}");
        let snd = self.dev_root.join("snd");
        let by_path = link_to(&snd.join("by-path"), &control);
        let by_id = link_to(&snd.join("by-id"), &control);
        let directory = self.asound.join(format!("card{index}"));
        let non_empty = |text: &str| (!text.is_empty()).then(|| text.to_string());

        let mut records = Vec::new();
        let sound_class = self.sys_root.join("class/sound");
        for pcm in numbered(&sound_class, &format!("pcmC{index}D"), "c")?.into_keys() {
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
            let info = read_text_file(&directory.join(format!("pcm{pcm}c/info")))
                .ok()
                .flatten();
            if info.is_none() {
                issue(
                    &mut issues,
                    "peripherals.pcm_info_unreadable",
                    "ALSA capture metadata could not be read. Check /proc/asound permissions \
                     and refresh the catalog.",
                );
            }
            // `key: value` lines; a repeated key keeps its last value.
            let info: BTreeMap<_, _> = info
                .as_deref()
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.split_once(':'))
                .map(|(key, value)| (key.trim(), value.trim()))
                .collect();
            let pcm_name = info.get("name").copied().unwrap_or_default();
            let name = [listed.name.as_str(), pcm_name, &card_id]
                .into_iter()
                .find(|name| !name.is_empty())
                .unwrap_or_default()
                .to_string();

            let selector = match selector_unavailable {
                None => Some(format!("plughw:CARD={card_id},DEV={pcm}")),
                Some(reason) => {
                    issue(
                        &mut issues,
                        "peripherals.capture_selector_unavailable",
                        reason,
                    );
                    None
                }
            };

            let stable_key = format!("{key_base}:pcm{pcm}c");
            let pcm_node = snd.join(format!("pcmC{index}D{pcm}c"));
            let identity = Identity {
                stable_key: stable_key.clone(),
                card_index: index,
                pcm_node: pcm_node.to_string_lossy().into_owned(),
                card_id: card_id.clone(),
                card_name: non_empty(&listed.name),
                card_driver: non_empty(&listed.driver),
                pcm_name: non_empty(pcm_name),
                by_path: by_path.clone(),
                by_id: by_id.clone(),
                usb: usb.clone(),
            };

            let stream = read_text_file(&directory.join(format!("stream{pcm}")))
                .ok()
                .flatten();
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

            let availability = availability(&info, &mut issues);
            records.push(Microphone {
                id: format!("microphone:alsa:{:016x}", fnv1a(&stable_key)),
                name,
                backend: Backend::Alsa,
                connection,
                capture_target: CaptureTarget {
                    card_id: card_id.clone(),
                    device: pcm,
                    selector,
                },
                identity,
                modes,
                availability,
                issues,
            });
        }
        Ok(records)
    }
}
