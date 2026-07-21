use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;

use crate::cache::write_cache;
use crate::model::{CachePayload, MetricDefinition, PowerStatus, ProcessInfo, Sample};
use crate::power::{
    power_metric_definitions, spawn_power_collector, PowerAccumulator, PowerConfig,
};
use crate::ring::Ring;
use crate::system::{system_metric_definitions, SystemCollector};
use crate::thermal::{thermal_metric_definitions, ThermalCollector};

pub fn run(cache_path: &str, interval: Duration, history: usize) -> Result<()> {
    let stopped = Arc::new(AtomicBool::new(false));
    install_signal_handlers(stopped.clone());

    let mut metrics = Vec::<MetricDefinition>::new();
    metrics.extend(thermal_metric_definitions());
    metrics.extend(system_metric_definitions());
    let power_config = PowerConfig::auto(Duration::from_millis(100));
    metrics.extend(power_metric_definitions(&power_config));

    let mut system = SystemCollector::new();
    let mut samples = Ring::new(history.max(1));
    let thermal_values = Arc::new(Mutex::new(BTreeMap::new()));
    spawn_thermal_collector(stopped.clone(), thermal_values.clone());
    let power = Arc::new(Mutex::new(PowerAccumulator::new(power_config.clone())));
    spawn_power_collector(stopped.clone(), power_config, power.clone());

    while !stopped.load(Ordering::Relaxed) {
        let loop_start = Instant::now();
        let mut values = BTreeMap::new();
        if let Ok(latest_thermal) = thermal_values.lock() {
            values.extend(latest_thermal.clone());
        }
        values.extend(system.sample());
        let power_status = power.lock().ok().map(|state| {
            values.extend(state.metric_values());
            state.status()
        });
        samples.push(Sample {
            timestamp: Utc::now(),
            values: sanitize_values(&values),
        });

        let payload = payload(
            &metrics,
            &samples,
            system.processes(),
            power_status,
            Vec::new(),
        );
        write_cache(cache_path, &payload)?;

        let elapsed = loop_start.elapsed();
        if elapsed < interval {
            thread::sleep(interval - elapsed);
        }
    }

    let power_status = power.lock().ok().map(|state| state.status());
    let payload = payload(
        &metrics,
        &samples,
        system.processes(),
        power_status,
        vec!["daemon stopped".into()],
    );
    write_cache(cache_path, &payload)?;
    Ok(())
}

fn spawn_thermal_collector(
    stopped: Arc<AtomicBool>,
    thermal_values: Arc<Mutex<BTreeMap<String, f64>>>,
) {
    thread::spawn(move || {
        let mut thermal = match ThermalCollector::new() {
            Ok(thermal) => thermal,
            Err(err) => {
                eprintln!("Sentinel thermal collector failed to start: {err}");
                return;
            }
        };
        while !stopped.load(Ordering::Relaxed) {
            let values = thermal.sample();
            if let Ok(mut latest) = thermal_values.lock() {
                *latest = values;
            }
            sleep_interruptible(&stopped, Duration::from_secs(2));
        }
    });
}

fn sleep_interruptible(stopped: &AtomicBool, duration: Duration) {
    let deadline = Instant::now() + duration;
    while !stopped.load(Ordering::Relaxed) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
}

fn sanitize_values(values: &BTreeMap<String, f64>) -> BTreeMap<String, Option<f64>> {
    values
        .iter()
        .map(|(key, value)| (key.clone(), value.is_finite().then_some(*value)))
        .collect()
}

fn payload(
    metrics: &[MetricDefinition],
    samples: &Ring<Sample>,
    processes: Vec<ProcessInfo>,
    power: Option<PowerStatus>,
    errors: Vec<String>,
) -> CachePayload {
    CachePayload {
        schema: 1,
        version: env!("CARGO_PKG_VERSION").into(),
        updated_at: Utc::now(),
        metrics: metrics.to_vec(),
        latest: samples.last().cloned(),
        samples: samples.to_vec(),
        processes,
        power,
        errors,
    }
}

fn install_signal_handlers(stopped: Arc<AtomicBool>) {
    let _ = ctrlc::set_handler(move || {
        stopped.store(true, Ordering::Relaxed);
    });
}
