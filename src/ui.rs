use std::io;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;

use crate::cache::read_cache;
use crate::model::{CachePayload, MetricDefinition};

const RESET: &str = "\x1b[0m";
const BOLD_RED: &str = "\x1b[1;31m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";
const CLEAR: &str = "\x1b[2J\x1b[H";

pub fn run_table(cache_path: &str, interval: Duration, once: bool) -> Result<()> {
    loop {
        let cache = read_cache(cache_path)?;
        let output = render_table(&cache);
        if once {
            println!("{output}");
            return Ok(());
        }
        print!("{CLEAR}{output}\n");
        thread::sleep(interval);
    }
}

pub fn run_ops(cache_path: &str, interval: Duration, once: bool) -> Result<()> {
    loop {
        let cache = read_cache(cache_path)?;
        let output = render_ops(&cache);
        if once {
            println!("{output}");
            return Ok(());
        }
        print!("{CLEAR}{output}\n");
        thread::sleep(interval);
    }
}

pub fn print_sensors(cache_path: &str) -> Result<()> {
    let cache = read_cache(cache_path)?;
    println!("{}", render_help(&cache));
    Ok(())
}

pub fn print_status(cache_path: &str) -> Result<()> {
    let cache = read_cache(cache_path)?;
    println!("Sentinel version: {}", cache.version);
    println!("Cache updated:    {}", cache.updated_at);
    println!(
        "Latest sample:    {}",
        cache
            .latest
            .as_ref()
            .map(|s| s.timestamp.to_rfc3339())
            .unwrap_or_else(|| "-".into())
    );
    if let Some(latest) = &cache.latest {
        let age = Utc::now()
            .signed_duration_since(latest.timestamp)
            .num_milliseconds() as f64
            / 1000.0;
        println!("Sample age:       {age:.1}s");
    }
    println!("Cached samples:   {}", cache.samples.len());
    println!("Metric count:     {}", cache.metrics.len());
    if !cache.errors.is_empty() {
        println!("Errors:           {}", cache.errors.join("; "));
    }
    Ok(())
}

pub fn export_json(cache_path: &str) -> Result<()> {
    let cache = read_cache(cache_path)?;
    serde_json::to_writer_pretty(io::stdout(), &cache)?;
    println!();
    Ok(())
}

fn render_table(cache: &CachePayload) -> String {
    let width = max_metric_width(&cache.metrics);
    let mut out = Vec::new();
    out.push(format!(
        "{:<width$}  {:<8}  {:>10}  {:<5}  Status",
        "Metric",
        "Group",
        "Value",
        "Unit",
        width = width
    ));
    out.push("-".repeat(width + 42));
    let values = cache.latest.as_ref().map(|s| &s.values);
    for metric in &cache.metrics {
        let value = values
            .and_then(|v| v.get(&metric.key).copied().flatten())
            .unwrap_or(f64::NAN);
        out.push(format!(
            "{:<width$}  {:<8}  {:>10}  {:<5}  {}",
            metric.short,
            metric.group,
            color_value(value, metric),
            metric.unit,
            status(value, metric),
            width = width
        ));
    }
    out.push(String::new());
    if let Some(latest) = &cache.latest {
        out.push(format!("Updated: {}", latest.timestamp));
    }
    if !cache.errors.is_empty() {
        out.push(format!(
            "{RED}Daemon errors: {}{RESET}",
            cache.errors.join("; ")
        ));
    }
    out.join("\n")
}

fn render_ops(cache: &CachePayload) -> String {
    let width = max_metric_width(&cache.metrics);
    let graph_width = 64usize;
    let mut out = vec!["Sentinel Ops View".into(), "-".repeat(96)];
    for metric in &cache.metrics {
        let series: Vec<f64> = cache
            .samples
            .iter()
            .filter_map(|s| s.values.get(&metric.key).copied().flatten())
            .collect();
        let latest = series
            .iter()
            .rev()
            .copied()
            .find(|v| v.is_finite())
            .unwrap_or(f64::NAN);
        out.push(format!(
            "{:<width$}  {:>8} {:<4}  {}",
            metric.short,
            color_value(latest, metric),
            metric.unit,
            sparkline(&series, graph_width),
            width = width
        ));
    }
    out.push(String::new());
    out.push(format!("Samples: {}", cache.samples.len()));
    out.join("\n")
}

fn render_help(cache: &CachePayload) -> String {
    let width = max_metric_width(&cache.metrics);
    let mut out = Vec::new();
    out.push(format!(
        "{:<width$}  {:<8}  {:<5}  {:>8}  {:>8}  Description",
        "Metric",
        "Group",
        "Unit",
        "Warn",
        "Critical",
        width = width
    ));
    out.push("-".repeat(width + 76));
    for metric in &cache.metrics {
        out.push(format!(
            "{:<width$}  {:<8}  {:<5}  {:>8}  {:>8}  {}",
            metric.short,
            metric.group,
            metric.unit,
            threshold(metric.warn),
            threshold(metric.critical),
            metric.description,
            width = width
        ));
    }
    out.join("\n")
}

fn max_metric_width(metrics: &[MetricDefinition]) -> usize {
    metrics.iter().map(|m| m.short.len()).max().unwrap_or(8)
}

fn color_value(value: f64, metric: &MetricDefinition) -> String {
    if !value.is_finite() {
        return "nan".into();
    }
    let text = match metric.unit.as_str() {
        "%" | "MB" | "MB/s" => format!("{value:.1}"),
        _ => format!("{value:.2}"),
    };
    match (metric.warn, metric.critical) {
        (_, Some(critical)) if value >= critical => format!("{BOLD_RED}{text}{RESET}"),
        (Some(warn), _) if value >= warn => format!("{RED}{text}{RESET}"),
        _ if metric.unit == "C" && value < 45.0 => format!("{CYAN}{text}{RESET}"),
        _ => format!("{GREEN}{text}{RESET}"),
    }
}

fn status(value: f64, metric: &MetricDefinition) -> String {
    if !value.is_finite() {
        return "unknown".into();
    }
    match (metric.warn, metric.critical) {
        (_, Some(critical)) if value >= critical => format!("{BOLD_RED}critical{RESET}"),
        (Some(warn), _) if value >= warn => format!("{RED}warning{RESET}"),
        _ => format!("{GREEN}normal{RESET}"),
    }
}

fn threshold(value: Option<f64>) -> String {
    value
        .map(|v| format!("{v:.0}"))
        .unwrap_or_else(|| "-".into())
}

fn sparkline(values: &[f64], width: usize) -> String {
    let clean: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if clean.is_empty() {
        return String::new();
    }
    let compact = if clean.len() > width {
        let step = clean.len() as f64 / width as f64;
        (0..width)
            .map(|i| clean[(i as f64 * step) as usize])
            .collect()
    } else {
        clean
    };
    let min = compact.iter().copied().fold(f64::INFINITY, f64::min);
    let max = compact.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let chars = ['.', '_', '-', '~', '=', '*', '#'];
    if max <= min {
        return ".".repeat(compact.len());
    }
    compact
        .iter()
        .map(|v| {
            let idx = (((v - min) / (max - min)) * (chars.len() - 1) as f64).round() as usize;
            chars[idx.min(chars.len() - 1)]
        })
        .collect()
}
