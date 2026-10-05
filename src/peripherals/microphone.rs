//! The `microphone` device type: audio capture devices. Each capture PCM is
//! one microphone.

use serde::{Deserialize, Serialize};

use super::Availability;

/// One capture PCM, serialized flat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Microphone {
    pub id: String,
    /// Card short name, else the PCM name, else the card id.
    pub name: String,
    pub backend: Backend,
    pub connection: Connection,
    pub capture_target: CaptureTarget,
    pub identity: Identity,
    /// Capture formats; empty when the driver publishes none (see `issues`).
    pub modes: Vec<Mode>,
    pub availability: Availability,
    /// Parts of the record that could not be read; the record is still
    /// published.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<Issue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Alsa,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    /// The card's sysfs device has a USB ancestor.
    Usb,
    /// The card has a parent device that is not on USB.
    Platform,
    /// The card has no parent device (a virtual card).
    Unknown,
}

/// Routing for the current boot only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureTarget {
    pub card_id: String,
    /// The PCM device number.
    pub device: u32,
    /// `plughw:CARD=<card_id>,DEV=<device>`, when the card id is safe in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// The key `id` hashes.
    pub stable_key: String,
    /// Current ALSA card number; changes between replugs.
    pub card_index: u32,
    /// `/dev/snd/pcmC<N>D<M>c`, routing only.
    pub pcm_node: String,
    pub card_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_driver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pcm_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb: Option<UsbIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbIdentity {
    pub vendor_id: String,
    pub product_id: String,
    /// The USB port, e.g. `1-1.2`.
    pub bus_path: String,
    /// The audio interface, e.g. `1-1.2:1.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
}

/// One format of one capture altset.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Mode {
    /// ALSA sample format, e.g. `S16_LE`.
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altset: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u32>,
    /// Valid bits per sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_bits: Option<u32>,
    /// Discrete rates, sorted and without duplicates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rates_hz: Vec<u32>,
    /// Continuous rates; a mode has `rates_hz` or this, never both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_range_hz: Option<RateRange>,
    /// Channel positions, e.g. `FL FR`; `--` for an unknown position.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channel_map: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RateRange {
    pub min: u32,
    pub max: u32,
}

/// Why part of a record is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub code: String,
    pub reason: String,
}

impl Microphone {
    /// One line for `simaai-sentinel peripherals`.
    pub fn describe(&self) -> String {
        let backend = match self.backend {
            Backend::Alsa => "alsa",
        };
        format!("{}  ({backend}, {} modes)", self.name, self.modes.len())
    }
}
