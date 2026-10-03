use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_CATALOG_PATH: &str = "/run/simaai-sentinel/peripherals.json";

/// One peripheral as reported by a provider. `details` is type-specific and is
/// published under a key named after `kind` (for example `"camera": {...}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub provider: String,
    pub details: Value,
}

impl Record {
    pub fn to_catalog_value(&self) -> Value {
        let mut value = json!({
            "id": self.id,
            "type": self.kind,
            "provider": self.provider,
        });
        value[self.kind.as_str()] = self.details.clone();
        value
    }
}

/// A provider that could not be refreshed in the latest scan.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Issue {
    pub provider: String,
    pub code: String,
    pub reason: String,
    pub retained_last_good: bool,
}

/// A structured provider failure, for example `io.permission_denied`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Stable provider name, for example `daemon.camera.v4l2`.
    fn name(&self) -> &str;
    /// Kernel uevent subsystems whose events should rerun this provider.
    fn subsystems(&self) -> &[String];
    /// Run one complete, read-only scan.
    fn discover(&mut self) -> Result<Vec<Record>, ProviderError>;
}

/// One entry of the bounded change log published with the catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub sequence: u64,
    pub revision: u64,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

/// The published catalog document (`peripherals.json`, `GET /v1/peripherals`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogDocument {
    pub schema_version: u32,
    pub instance_id: String,
    pub state: String,
    pub ready: bool,
    pub stale: bool,
    pub revision: u64,
    pub sequence: u64,
    pub scan_sequence: u64,
    pub last_success_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub error: Option<Value>,
    pub issues: Vec<Issue>,
    #[serde(default)]
    pub changes: Vec<Change>,
    /// Which Neat Core rules classified `supported`, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support: Option<super::support::SupportStatus>,
    pub devices: Vec<Value>,
}
