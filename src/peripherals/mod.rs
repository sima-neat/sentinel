//! Peripheral inventory. One worker thread runs the built-in providers at
//! startup, on kernel uevents and on request, and replaces an in-memory
//! catalog that the API serves. Sentinel reports hardware facts only; whether
//! an application supports a device is decided by that application.

pub mod camera;
pub mod cli;
mod sysutil;
mod uevent;
mod v4l2;
mod videodev2;
mod worker;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use worker::{start, Peripherals, Worker};

/// One connected device, serialized flat with a `type` tag. Each device type
/// is added as a variant together with the provider that discovers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
// Only the test build has a second, small variant to compare against.
#[cfg_attr(test, allow(clippy::large_enum_variant))]
pub enum Peripheral {
    Camera(camera::Camera),
    #[cfg(test)]
    Test(tests::TestDevice),
}

impl Peripheral {
    /// Stable identity; never an enumeration index such as `/dev/videoN`.
    pub fn id(&self) -> &str {
        match *self {
            Peripheral::Camera(ref camera) => &camera.id,
            #[cfg(test)]
            Peripheral::Test(ref device) => &device.id,
        }
    }

    /// The `type` tag.
    pub fn kind(&self) -> &'static str {
        match *self {
            Peripheral::Camera(_) => "camera",
            #[cfg(test)]
            Peripheral::Test(_) => "test",
        }
    }

    /// One line for `simaai-sentinel peripherals`.
    pub fn describe(&self) -> String {
        match *self {
            Peripheral::Camera(ref camera) => camera.describe(),
            #[cfg(test)]
            Peripheral::Test(ref device) => device.id.clone(),
        }
    }
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
    vec![Box::new(v4l2::V4l2Provider::new())]
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
}
