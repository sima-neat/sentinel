use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{CachePayload, MetricDefinition, Sample};

pub const RUN_SCHEMA: u32 = 1;
pub const EXPORT_SCHEMA: u32 = 1;
pub const DEFAULT_RUNS_DIR: &str = "/var/lib/simaai-sentinel/runs";
const ACTIVE_FILE: &str = "active.json";
const LOCK_FILE: &str = ".lock";
const DEFAULT_RETENTION: usize = 50;
const DEFAULT_MAX_SAMPLES: usize = 21_600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMetadata {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub sample_interval_ms: Option<u64>,
    pub sentinel_version: String,
    #[serde(default)]
    pub system: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedRun {
    pub schema: u32,
    pub metadata: RunMetadata,
    pub metrics: Vec<MetricDefinition>,
    pub samples: Vec<Sample>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub id: String,
    pub name: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: i64,
    pub samples: usize,
    pub energy_joules: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricStatistics {
    pub count: usize,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub mean: Option<f64>,
    pub median: Option<f64>,
    pub p95: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ExportDocument {
    pub schema: u32,
    pub generated_at: DateTime<Utc>,
    pub baseline_id: Option<String>,
    pub runs: Vec<SavedRun>,
    pub summaries: BTreeMap<String, ExportRunSummary>,
    pub baseline_deltas_pct: BTreeMap<String, BTreeMap<String, Option<f64>>>,
}

#[derive(Debug, Serialize)]
pub struct ExportRunSummary {
    pub duration_ms: i64,
    pub samples: usize,
    pub energy_joules: Option<f64>,
    pub metrics: BTreeMap<String, MetricStatistics>,
}

struct StoreLock {
    file: fs::File,
}

impl StoreLock {
    fn acquire(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)
            .with_context(|| format!("create runs directory {}", directory.display()))?;
        let path = directory.join(LOCK_FILE);
        let file = if path.exists() {
            OpenOptions::new().read(true).open(&path)
        } else {
            OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(&path)
        }
        .with_context(|| format!("open run-store lock {}", path.display()))?;
        let status = unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&file), libc::LOCK_EX) };
        if status != 0 {
            return Err(std::io::Error::last_os_error()).context("lock run store");
        }
        Ok(Self { file })
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(std::os::fd::AsRawFd::as_raw_fd(&self.file), libc::LOCK_UN);
        }
    }
}

pub fn start(
    directory: &Path,
    cache: &CachePayload,
    name: &str,
    note: Option<String>,
    tags: Vec<String>,
) -> Result<SavedRun> {
    validate_name(name)?;
    let _lock = StoreLock::acquire(directory)?;
    if active_path(directory).exists() {
        let active = read_run_file(&active_path(directory))?;
        bail!(
            "checkpoint '{}' is already recording; stop it before starting another",
            active.metadata.name
        );
    }
    if find_run_unlocked(directory, name)?.is_some() {
        bail!("a completed run named '{name}' already exists");
    }

    let started_at = Utc::now();
    let id = format!("{}-{}", started_at.format("%Y%m%dT%H%M%S%.3fZ"), slug(name));
    let run = SavedRun {
        schema: RUN_SCHEMA,
        metadata: RunMetadata {
            id,
            name: name.into(),
            note,
            tags,
            started_at,
            ended_at: None,
            sample_interval_ms: cache.samples.windows(2).last().map(|pair| {
                (pair[1].timestamp - pair[0].timestamp)
                    .num_milliseconds()
                    .max(0) as u64
            }),
            sentinel_version: cache.version.clone(),
            system: collect_system_metadata(),
        },
        metrics: cache.metrics.clone(),
        samples: Vec::new(),
    };
    write_json_atomic(&active_path(directory), &run)?;
    Ok(run)
}

pub fn stop(directory: &Path) -> Result<SavedRun> {
    let _lock = StoreLock::acquire(directory)?;
    let path = active_path(directory);
    if !path.exists() {
        bail!("no checkpoint is currently recording");
    }
    let mut run = read_run_file(&path)?;
    run.metadata.ended_at = Some(Utc::now());
    let destination = completed_path(directory, &run.metadata.id);
    write_json_atomic(&destination, &run)?;
    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    enforce_retention_unlocked(directory, retention_limit())?;
    Ok(run)
}

pub fn record(directory: &Path, cache: &CachePayload) -> Result<bool> {
    let _lock = StoreLock::acquire(directory)?;
    let path = active_path(directory);
    if !path.exists() {
        return Ok(false);
    }
    let Some(sample) = cache.latest.as_ref() else {
        return Ok(false);
    };
    let mut run = read_run_file(&path)?;
    if run
        .samples
        .last()
        .is_some_and(|previous| previous.timestamp >= sample.timestamp)
    {
        return Ok(false);
    }
    if run.metrics.is_empty() {
        run.metrics = cache.metrics.clone();
    }
    if run.samples.len() >= sample_limit() {
        bail!(
            "active checkpoint '{}' reached the {}-sample safety limit; stop it to save the captured data",
            run.metadata.name,
            sample_limit()
        );
    }
    run.samples.push(sample.clone());
    write_json_atomic(&path, &run)?;
    Ok(true)
}

pub fn active(directory: &Path) -> Result<Option<SavedRun>> {
    let _lock = StoreLock::acquire(directory)?;
    let path = active_path(directory);
    path.exists().then(|| read_run_file(&path)).transpose()
}

pub fn list(directory: &Path) -> Result<Vec<RunSummary>> {
    let _lock = StoreLock::acquire(directory)?;
    let mut runs = completed_runs_unlocked(directory)?;
    if let Some(active) = active_path(directory)
        .exists()
        .then(|| read_run_file(&active_path(directory)))
        .transpose()?
    {
        runs.push(active);
    }
    runs.sort_by_key(|run| run.metadata.started_at);
    runs.reverse();
    Ok(runs.iter().map(summary).collect())
}

pub fn load(directory: &Path, selector: &str) -> Result<SavedRun> {
    let _lock = StoreLock::acquire(directory)?;
    find_run_unlocked(directory, selector)?.with_context(|| format!("unknown run '{selector}'"))
}

pub fn load_many(directory: &Path, selectors: &[String]) -> Result<Vec<SavedRun>> {
    let _lock = StoreLock::acquire(directory)?;
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for selector in selectors {
        let run = find_run_unlocked(directory, selector)?
            .with_context(|| format!("unknown run '{selector}'"))?;
        if seen.insert(run.metadata.id.clone()) {
            result.push(run);
        }
    }
    Ok(result)
}

pub fn delete(directory: &Path, selector: &str) -> Result<SavedRun> {
    let _lock = StoreLock::acquire(directory)?;
    if let Some(active) = active_path(directory)
        .exists()
        .then(|| read_run_file(&active_path(directory)))
        .transpose()?
    {
        if active.metadata.id == selector || active.metadata.name == selector {
            bail!(
                "cannot delete active run '{}'; stop it first",
                active.metadata.name
            );
        }
    }
    let run = find_completed_unlocked(directory, selector)?
        .with_context(|| format!("unknown completed run '{selector}'"))?;
    let path = completed_path(directory, &run.metadata.id);
    fs::remove_file(&path).with_context(|| format!("delete {}", path.display()))?;
    Ok(run)
}

pub fn clear_completed(directory: &Path) -> Result<usize> {
    let _lock = StoreLock::acquire(directory)?;
    let completed = completed_runs_unlocked(directory)?;
    for run in &completed {
        let path = completed_path(directory, &run.metadata.id);
        fs::remove_file(&path).with_context(|| format!("delete {}", path.display()))?;
    }
    Ok(completed.len())
}

pub fn summary(run: &SavedRun) -> RunSummary {
    let ended = run
        .metadata
        .ended_at
        .or_else(|| run.samples.last().map(|sample| sample.timestamp))
        .unwrap_or(run.metadata.started_at);
    RunSummary {
        id: run.metadata.id.clone(),
        name: run.metadata.name.clone(),
        started_at: run.metadata.started_at,
        ended_at: run.metadata.ended_at,
        duration_ms: (ended - run.metadata.started_at).num_milliseconds().max(0),
        samples: run.samples.len(),
        energy_joules: integrate_energy(run, "power_current_watts"),
    }
}

pub fn statistics(run: &SavedRun, metric: &str) -> MetricStatistics {
    let mut values: Vec<f64> = run
        .samples
        .iter()
        .filter_map(|sample| sample.values.get(metric).copied().flatten())
        .filter(|value| value.is_finite())
        .collect();
    values.sort_by(f64::total_cmp);
    if values.is_empty() {
        return MetricStatistics {
            count: 0,
            minimum: None,
            maximum: None,
            mean: None,
            median: None,
            p95: None,
        };
    }
    let percentile = |fraction: f64| {
        let index = ((values.len() - 1) as f64 * fraction).round() as usize;
        values[index]
    };
    MetricStatistics {
        count: values.len(),
        minimum: values.first().copied(),
        maximum: values.last().copied(),
        mean: Some(values.iter().sum::<f64>() / values.len() as f64),
        median: Some(percentile(0.5)),
        p95: Some(percentile(0.95)),
    }
}

pub fn export_json(runs: Vec<SavedRun>) -> ExportDocument {
    let baseline_id = runs.first().map(|run| run.metadata.id.clone());
    let baseline_means: BTreeMap<String, Option<f64>> = runs
        .first()
        .map(|run| {
            run.metrics
                .iter()
                .map(|metric| (metric.key.clone(), statistics(run, &metric.key).mean))
                .collect()
        })
        .unwrap_or_default();
    let summaries: BTreeMap<String, ExportRunSummary> = runs
        .iter()
        .map(|run| {
            let metrics = run
                .metrics
                .iter()
                .map(|metric| (metric.key.clone(), statistics(run, &metric.key)))
                .collect();
            let run_summary = summary(run);
            (
                run.metadata.id.clone(),
                ExportRunSummary {
                    duration_ms: run_summary.duration_ms,
                    samples: run_summary.samples,
                    energy_joules: run_summary.energy_joules,
                    metrics,
                },
            )
        })
        .collect();
    let baseline_deltas_pct = summaries
        .iter()
        .map(|(run_id, summary)| {
            let deltas = summary
                .metrics
                .iter()
                .map(|(key, statistics)| {
                    let delta = match (statistics.mean, baseline_means.get(key).copied().flatten())
                    {
                        (Some(value), Some(baseline)) if baseline.abs() > f64::EPSILON => {
                            Some((value - baseline) / baseline * 100.0)
                        }
                        _ => None,
                    };
                    (key.clone(), delta)
                })
                .collect();
            (run_id.clone(), deltas)
        })
        .collect();
    ExportDocument {
        schema: EXPORT_SCHEMA,
        generated_at: Utc::now(),
        baseline_id,
        runs,
        summaries,
        baseline_deltas_pct,
    }
}

pub fn export_csv(runs: &[SavedRun]) -> String {
    let mut output =
        String::from("run_id,run_name,elapsed_ms,timestamp,category,metric,unit,value,status\n");
    for run in runs {
        let definitions: BTreeMap<&str, &MetricDefinition> = run
            .metrics
            .iter()
            .map(|metric| (metric.key.as_str(), metric))
            .collect();
        for sample in &run.samples {
            let elapsed = (sample.timestamp - run.metadata.started_at)
                .num_milliseconds()
                .max(0);
            let keys: BTreeSet<&str> = definitions
                .keys()
                .copied()
                .chain(sample.values.keys().map(String::as_str))
                .collect();
            for key in keys {
                let definition = definitions.get(key).copied();
                let value = sample.values.get(key).copied().flatten();
                let status = match sample.values.get(key) {
                    None => "missing",
                    Some(None) => "unavailable",
                    Some(Some(value)) if value.is_finite() => metric_status(*value, definition),
                    Some(Some(_)) => "unavailable",
                };
                let row = [
                    run.metadata.id.clone(),
                    run.metadata.name.clone(),
                    elapsed.to_string(),
                    sample
                        .timestamp
                        .to_rfc3339_opts(SecondsFormat::Millis, true),
                    definition
                        .map(|metric| metric.group.clone())
                        .unwrap_or_default(),
                    key.into(),
                    definition
                        .map(|metric| metric.unit.clone())
                        .unwrap_or_default(),
                    value.map(|value| value.to_string()).unwrap_or_default(),
                    status.into(),
                ];
                output.push_str(
                    &row.iter()
                        .map(|field| csv_field(field))
                        .collect::<Vec<_>>()
                        .join(","),
                );
                output.push('\n');
            }
        }
    }
    output
}

pub fn integrate_energy(run: &SavedRun, metric: &str) -> Option<f64> {
    integrate_energy_until(run, metric, None)
}

pub fn integrate_energy_until(
    run: &SavedRun,
    metric: &str,
    max_elapsed_seconds: Option<f64>,
) -> Option<f64> {
    let mut energy = 0.0;
    let mut segments = 0usize;
    for pair in run.samples.windows(2) {
        let right_elapsed = (pair[1].timestamp - run.metadata.started_at)
            .num_microseconds()
            .unwrap_or(0) as f64
            / 1_000_000.0;
        if max_elapsed_seconds.is_some_and(|limit| right_elapsed > limit) {
            continue;
        }
        let left = pair[0].values.get(metric).copied().flatten();
        let right = pair[1].values.get(metric).copied().flatten();
        if let (Some(left), Some(right)) = (left, right) {
            let seconds = (pair[1].timestamp - pair[0].timestamp)
                .num_microseconds()
                .unwrap_or(0) as f64
                / 1_000_000.0;
            if left.is_finite() && right.is_finite() && seconds > 0.0 {
                energy += (left + right) * 0.5 * seconds;
                segments += 1;
            }
        }
    }
    (segments > 0).then_some(energy)
}

fn metric_status(value: f64, definition: Option<&MetricDefinition>) -> &'static str {
    match definition {
        Some(metric) if metric.critical.is_some_and(|limit| value >= limit) => "critical",
        Some(metric) if metric.warn.is_some_and(|limit| value >= limit) => "warning",
        _ => "ok",
    }
}

fn collect_system_metadata() -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();
    for (key, path) in [
        ("hostname", "/etc/hostname"),
        ("sdk_release", "/etc/sdk-release"),
        ("os_release", "/etc/os-release"),
        ("device_tree_model", "/proc/device-tree/model"),
    ] {
        if let Ok(value) = fs::read_to_string(path) {
            metadata.insert(key.into(), value.trim_matches(char::from(0)).trim().into());
        }
    }
    metadata.insert("architecture".into(), std::env::consts::ARCH.into());
    metadata
}

fn validate_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("checkpoint name must not be empty");
    }
    if trimmed.len() > 80 {
        bail!("checkpoint name must be at most 80 bytes");
    }
    if trimmed.chars().any(char::is_control) {
        bail!("checkpoint name must not contain control characters");
    }
    Ok(())
}

fn slug(name: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            result.push(character);
            separator = false;
        } else if !separator && !result.is_empty() {
            result.push('-');
            separator = true;
        }
    }
    while result.ends_with('-') {
        result.pop();
    }
    if result.is_empty() {
        "run".into()
    } else {
        result
    }
}

fn retention_limit() -> usize {
    std::env::var("SIMA_SENTINEL_RUN_RETENTION")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_RETENTION)
        .max(1)
}

fn sample_limit() -> usize {
    std::env::var("SIMA_SENTINEL_RUN_MAX_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_MAX_SAMPLES)
        .max(2)
}

fn enforce_retention_unlocked(directory: &Path, limit: usize) -> Result<()> {
    let mut runs = completed_runs_unlocked(directory)?;
    runs.sort_by_key(|run| run.metadata.started_at);
    let remove_count = runs.len().saturating_sub(limit);
    for run in runs.into_iter().take(remove_count) {
        let path = completed_path(directory, &run.metadata.id);
        fs::remove_file(&path).with_context(|| format!("remove expired run {}", path.display()))?;
    }
    Ok(())
}

fn completed_runs_unlocked(directory: &Path) -> Result<Vec<SavedRun>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut runs = Vec::new();
    for entry in fs::read_dir(directory)
        .with_context(|| format!("read runs directory {}", directory.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json")
            || path.file_name().and_then(|name| name.to_str()) == Some(ACTIVE_FILE)
        {
            continue;
        }
        match read_run_file(&path) {
            Ok(run) => runs.push(run),
            Err(error) => eprintln!(
                "Warning: ignoring corrupt run {}: {error:#}",
                path.display()
            ),
        }
    }
    Ok(runs)
}

fn find_run_unlocked(directory: &Path, selector: &str) -> Result<Option<SavedRun>> {
    if active_path(directory).exists() {
        let active = read_run_file(&active_path(directory))?;
        if active.metadata.id == selector || active.metadata.name == selector {
            return Ok(Some(active));
        }
    }
    find_completed_unlocked(directory, selector)
}

fn find_completed_unlocked(directory: &Path, selector: &str) -> Result<Option<SavedRun>> {
    let matches: Vec<SavedRun> = completed_runs_unlocked(directory)?
        .into_iter()
        .filter(|run| run.metadata.id == selector || run.metadata.name == selector)
        .collect();
    if matches.len() > 1 {
        bail!("run name '{selector}' is ambiguous; use a run ID");
    }
    Ok(matches.into_iter().next())
}

fn read_run_file(path: &Path) -> Result<SavedRun> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let run: SavedRun =
        serde_json::from_slice(&data).with_context(|| format!("parse {}", path.display()))?;
    if run.schema != RUN_SCHEMA {
        bail!(
            "unsupported run schema {} in {} (expected {})",
            run.schema,
            path.display(),
            RUN_SCHEMA
        );
    }
    Ok(run)
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("run file has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create runs directory {}", parent.display()))?;
    let temporary = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(value).context("serialize run")?;
    {
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        file.write_all(&data)
            .with_context(|| format!("write {}", temporary.display()))?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)
        .with_context(|| format!("replace {} atomically", path.display()))?;
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.into()
    }
}

fn active_path(directory: &Path) -> PathBuf {
    directory.join(ACTIVE_FILE)
}

fn completed_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sentinel-runs-{}-{suffix}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn cache() -> CachePayload {
        let metric = MetricDefinition::new(
            "power_current_watts",
            "Power",
            "Power",
            "Power",
            "W",
            "Power",
            None,
            None,
        );
        let timestamp = Utc.with_ymd_and_hms(2026, 7, 31, 1, 2, 3).unwrap();
        CachePayload {
            schema: 1,
            version: "0.1.0".into(),
            updated_at: timestamp,
            metrics: vec![metric],
            latest: Some(Sample {
                timestamp,
                values: BTreeMap::from([("power_current_watts".into(), Some(2.0))]),
            }),
            samples: Vec::new(),
            processes: Vec::new(),
            power: None,
            errors: Vec::new(),
        }
    }

    #[test]
    fn lifecycle_persists_samples_and_exports() {
        let directory = temp_dir();
        let mut cache = cache();
        let started = start(&directory, &cache, "baseline", Some("note".into()), vec![]).unwrap();
        assert_eq!(started.metadata.name, "baseline");
        assert!(record(&directory, &cache).unwrap());
        cache.latest.as_mut().unwrap().timestamp += chrono::Duration::seconds(2);
        cache
            .latest
            .as_mut()
            .unwrap()
            .values
            .insert("power_current_watts".into(), Some(4.0));
        assert!(record(&directory, &cache).unwrap());
        assert!(!record(&directory, &cache).unwrap());
        let stopped = stop(&directory).unwrap();
        assert_eq!(stopped.samples.len(), 2);
        assert_eq!(integrate_energy(&stopped, "power_current_watts"), Some(6.0));
        assert!(
            export_csv(std::slice::from_ref(&stopped)).contains(",Power,power_current_watts,W,")
        );
        let export = export_json(vec![stopped.clone()]);
        assert_eq!(
            export.baseline_id.as_deref(),
            Some(stopped.metadata.id.as_str())
        );
        assert_eq!(
            export
                .summaries
                .get(&stopped.metadata.id)
                .and_then(|summary| summary.energy_joules),
            Some(6.0)
        );
        assert_eq!(
            export
                .baseline_deltas_pct
                .get(&stopped.metadata.id)
                .and_then(|metrics| metrics.get("power_current_watts"))
                .copied()
                .flatten(),
            Some(0.0)
        );
        assert_eq!(list(&directory).unwrap().len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn duplicate_and_active_operations_are_rejected() {
        let directory = temp_dir();
        let cache = cache();
        start(&directory, &cache, "same", None, vec![]).unwrap();
        assert!(start(&directory, &cache, "other", None, vec![]).is_err());
        assert!(delete(&directory, "same").is_err());
        stop(&directory).unwrap();
        assert!(start(&directory, &cache, "same", None, vec![]).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupt_completed_run_does_not_hide_valid_runs() {
        let directory = temp_dir();
        let cache = cache();
        start(&directory, &cache, "valid", None, vec![]).unwrap();
        stop(&directory).unwrap();
        fs::write(directory.join("corrupt.json"), b"not json").unwrap();
        assert_eq!(list(&directory).unwrap().len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn clear_removes_completed_runs_but_preserves_active_capture() {
        let directory = temp_dir();
        let cache = cache();
        start(&directory, &cache, "complete", None, vec![]).unwrap();
        stop(&directory).unwrap();
        start(&directory, &cache, "recording", None, vec![]).unwrap();

        assert_eq!(clear_completed(&directory).unwrap(), 1);
        let remaining = list(&directory).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "recording");
        assert!(remaining[0].ended_at.is_none());
        stop(&directory).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn csv_quotes_names_and_marks_missing_values() {
        let directory = temp_dir();
        let cache = cache();
        start(&directory, &cache, "a,b", None, vec![]).unwrap();
        record(&directory, &cache).unwrap();
        let run = stop(&directory).unwrap();
        let csv = export_csv(&[run]);
        assert!(csv.contains("\"a,b\""));
        fs::remove_dir_all(directory).unwrap();
    }
}
