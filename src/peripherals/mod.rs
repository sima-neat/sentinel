//! Peripheral inventory. One worker thread runs the built-in providers at
//! startup, on kernel uevents and on request, and replaces an in-memory
//! catalog that the API serves. Sentinel reports hardware facts only; whether
//! an application supports a device is decided by that application.

mod alsa;
pub mod camera;
pub mod cli;
pub mod microphone;
mod mipi;
mod sysutil;
mod uevent;
mod v4l2;
mod videodev2;
mod worker;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use worker::{start, Peripherals, RefreshError, Worker};

/// One connected device, serialized flat with a `type` tag. Each device type
/// is added as a variant together with the provider that discovers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
// Only the test build has a second, small variant to compare against.
#[cfg_attr(test, allow(clippy::large_enum_variant))]
pub enum Peripheral {
    Camera(camera::Camera),
    Microphone(microphone::Microphone),
    #[cfg(test)]
    Test(tests::TestDevice),
}

impl Peripheral {
    /// Stable identity; never an enumeration index such as `/dev/videoN`.
    pub fn id(&self) -> &str {
        match *self {
            Peripheral::Camera(ref camera) => &camera.id,
            Peripheral::Microphone(ref microphone) => &microphone.id,
            #[cfg(test)]
            Peripheral::Test(ref device) => &device.id,
        }
    }

    /// The `type` tag.
    pub fn kind(&self) -> &'static str {
        match *self {
            Peripheral::Camera(_) => "camera",
            Peripheral::Microphone(_) => "microphone",
            #[cfg(test)]
            Peripheral::Test(_) => "test",
        }
    }

    /// One line for `simaai-sentinel peripherals`.
    pub fn describe(&self) -> String {
        match *self {
            Peripheral::Camera(ref camera) => camera.describe(),
            Peripheral::Microphone(ref microphone) => microphone.describe(),
            #[cfg(test)]
            Peripheral::Test(ref device) => device.id.clone(),
        }
    }
}

/// Whether the device is free, as the kernel last reported it. Discovery
/// never opens a stream, so this is a snapshot from the latest scan, or
/// `unknown` with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Availability {
    pub state: AvailabilityState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Capture subdevices, and how many of them are free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdevices: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdevices_available: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilityState {
    Unknown,
    Available,
    InUse,
}

/// The catalog served by `GET /v1/peripherals`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// Changes whenever `devices` or `errors` change. Seeded from the daemon
    /// start time, so a value is never reused after a restart; compare it
    /// for equality only.
    pub revision: u64,
    /// When the scan that produced this catalog started; `null` until the
    /// first scan completes.
    pub observed_at: Option<DateTime<Utc>>,
    pub devices: Vec<Peripheral>,
    pub errors: Vec<CatalogError>,
}

/// A provider that failed in the latest scan. Its devices from its last
/// successful scan stay in `devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogError {
    pub provider: String,
    pub code: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub code: String,
    pub reason: String,
}

impl ProviderError {
    pub fn new(code: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            reason: reason.into(),
        }
    }
}

/// A discovery backend. Providers only observe devices: they never acquire,
/// configure, or stream from them.
pub trait Provider: Send {
    /// Stable provider name, for example `camera.v4l2`.
    fn name(&self) -> &'static str;
    /// Kernel uevent subsystems whose events should trigger a rescan.
    fn subsystems(&self) -> &'static [&'static str];
    /// Run one complete, read-only scan.
    fn discover(&mut self) -> Result<Vec<Peripheral>, ProviderError>;
}

/// Every built-in provider. Add a device type's provider here.
pub fn builtin_providers() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(mipi::MipiProvider::new()),
        Box::new(v4l2::V4l2Provider::new()),
        Box::new(alsa::AlsaProvider::new()),
    ]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct TestDevice {
        pub id: String,
    }

    pub fn device(id: &str) -> Peripheral {
        Peripheral::Test(TestDevice { id: id.into() })
    }

    #[test]
    fn devices_serialize_flat_with_a_type_tag() {
        let catalog = Catalog {
            revision: 7,
            observed_at: None,
            devices: vec![device("test:a")],
            errors: Vec::new(),
        };
        let value = serde_json::to_value(&catalog).unwrap();
        assert_eq!(value["devices"][0]["type"], "test");
        assert_eq!(value["devices"][0]["id"], "test:a");
        assert_eq!(serde_json::from_value::<Catalog>(value).unwrap(), catalog);
    }

    /// `docs/peripherals/catalog-example.json` is the published contract that
    /// consumers test against: a DevKit capture (IMX477, Logitech C920 camera
    /// and microphone) with the USB camera trimmed to one mode per format. It
    /// must round-trip unchanged, so changing the schema fails here until
    /// the example, and the consumers using it, are updated.
    #[test]
    fn the_published_example_matches_the_schema() {
        let text = include_str!("../../docs/peripherals/catalog-example.json");
        let value: serde_json::Value = serde_json::from_str(text).unwrap();
        let catalog: Catalog = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&catalog).unwrap(), value);
        let types: Vec<_> = catalog.devices.iter().map(Peripheral::kind).collect();
        assert_eq!(types, ["camera", "camera", "microphone"]);
    }
}
