use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const DEFAULT_CACHE_PATH: &str = "/run/simaai-sentinel/cache.json";
pub const DEFAULT_CACHE_DIR: &str = "/run/simaai-sentinel";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricDefinition {
    pub key: String,
    pub label: String,
    pub short: String,
    pub group: String,
    pub unit: String,
    pub description: String,
    pub warn: Option<f64>,
    pub critical: Option<f64>,
}

impl MetricDefinition {
    pub fn new(
        key: &str,
        label: &str,
        short: &str,
        group: &str,
        unit: &str,
        description: &str,
        warn: Option<f64>,
        critical: Option<f64>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            short: short.into(),
            group: group.into(),
            unit: unit.into(),
            description: description.into(),
            warn,
            critical,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub timestamp: DateTime<Utc>,
    pub values: std::collections::BTreeMap<String, Option<f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_pct: f64,
    pub rss_mb: f64,
    pub cpu_core: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachePayload {
    pub schema: u32,
    pub version: String,
    pub updated_at: DateTime<Utc>,
    pub metrics: Vec<MetricDefinition>,
    pub latest: Option<Sample>,
    pub samples: Vec<Sample>,
    #[serde(default)]
    pub processes: Vec<ProcessInfo>,
    pub errors: Vec<String>,
}
