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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PowerRailStatus {
    pub key: String,
    pub label: String,
    pub current_watts: Option<f64>,
    pub samples: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PowerStatus {
    pub profile: String,
    pub sample_interval_ms: u64,
    pub duration_seconds: f64,
    pub valid_samples: u64,
    pub failed_samples: u64,
    pub last_sample_valid: bool,
    pub last_error: Option<String>,
    pub rails: Vec<PowerRailStatus>,
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
    #[serde(default)]
    pub power: Option<PowerStatus>,
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_without_power_field_remains_readable() {
        let cache: CachePayload = serde_json::from_value(serde_json::json!({
            "schema": 1,
            "version": "0.1.0",
            "updated_at": "2026-07-21T00:00:00Z",
            "metrics": [],
            "latest": null,
            "samples": [],
            "errors": []
        }))
        .expect("legacy cache should deserialize");

        assert!(cache.processes.is_empty());
        assert!(cache.power.is_none());
    }
}
