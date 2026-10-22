//! Modalix board power telemetry from the kernel's PMBus drivers.
//!
//! Each rail is one `pout` channel of a PMBus hwmon device, found through the
//! regulator's I2C device in sysfs (`/sys/bus/i2c/devices/<bus>-<addr>`) and
//! the channel's label, never through an `hwmonN` index. The kernel driver
//! selects the PMBus page and reads the register under its own lock, so other
//! readers cannot interleave with Sentinel's reads.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::model::{MetricDefinition, PowerRailStatus, PowerStatus};

const I2C_DEVICES: &str = "/sys/bus/i2c/devices";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfile {
    ModalixSom,
    ModalixDvt,
}

impl PowerProfile {
    pub fn name(self) -> &'static str {
        match self {
            Self::ModalixSom => "modalix_som",
            Self::ModalixDvt => "modalix_dvt",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PowerRailConfig {
    pub key: &'static str,
    pub name: &'static str,
    /// I2C device name in sysfs, `<bus>-<address>`.
    pub device: &'static str,
    /// hwmon label of the rail's output power channel, `pout<page + 1>`.
    pub channel: &'static str,
}

#[derive(Debug, Clone)]
pub struct PowerConfig {
    pub profile: PowerProfile,
    pub sample_interval: Duration,
    pub rails: Vec<PowerRailConfig>,
}

impl PowerConfig {
    pub fn auto(sample_interval: Duration) -> Self {
        let profile = detect_power_profile();
        Self {
            profile,
            sample_interval: sample_interval.max(Duration::from_millis(1)),
            rails: rails_for_profile(profile),
        }
    }
}

#[derive(Debug, Clone)]
struct PowerRailReading {
    watts: Option<f64>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct PowerSnapshot {
    rails: Vec<PowerRailReading>,
}

impl PowerSnapshot {
    /// Total of every rail, or `None` when any rail is missing: a total over
    /// fewer rails would understate board power.
    fn total_watts(&self) -> Option<f64> {
        if self.rails.is_empty() {
            return None;
        }
        self.rails.iter().map(|rail| rail.watts).sum()
    }
}

pub trait PowerSource: Send + Sync {
    fn read_watts(&self, rail: &PowerRailConfig) -> Result<f64, String>;
}

/// Reads `power<N>_input` (microwatts) from the kernel hwmon device.
#[derive(Debug)]
struct HwmonPowerSource {
    i2c_devices: PathBuf,
    /// Resolved `power<N>_input` paths. An entry is dropped after a failed
    /// read so that a rebound driver's new `hwmonN` directory is found.
    inputs: Mutex<BTreeMap<(&'static str, &'static str), PathBuf>>,
}

impl HwmonPowerSource {
    fn new(i2c_devices: impl Into<PathBuf>) -> Self {
        Self {
            i2c_devices: i2c_devices.into(),
            inputs: Mutex::new(BTreeMap::new()),
        }
    }

    fn resolve(&self, rail: &PowerRailConfig) -> Result<PathBuf, String> {
        let device = self.i2c_devices.join(rail.device);
        if !device.exists() {
            return Err(format!(
                "no I2C device {} in {}: the device tree does not declare this power monitor",
                rail.device,
                self.i2c_devices.display()
            ));
        }
        let Some(hwmon) = hwmon_dir(&device) else {
            let chip = fs::read_to_string(device.join("name"))
                .map(|name| name.trim().to_string())
                .unwrap_or_default();
            return Err(if device.join("driver").exists() || chip.is_empty() {
                format!("{} has no hwmon device", rail.device)
            } else {
                format!(
                    "no kernel driver is bound to {} ({chip}); load it with `modprobe {chip}`",
                    rail.device
                )
            });
        };
        power_input(&hwmon, rail.channel).ok_or_else(|| {
            format!(
                "{} ({}) has no {} power channel",
                rail.device,
                hwmon.display(),
                rail.channel
            )
        })
    }
}

impl PowerSource for HwmonPowerSource {
    fn read_watts(&self, rail: &PowerRailConfig) -> Result<f64, String> {
        let mut inputs = self
            .inputs
            .lock()
            .map_err(|_| "power input cache lock poisoned".to_string())?;
        let key = (rail.device, rail.channel);
        let input = match inputs.get(&key) {
            Some(input) => input.clone(),
            None => {
                let input = self.resolve(rail)?;
                inputs.insert(key, input.clone());
                input
            }
        };
        let result = fs::read_to_string(&input)
            .map_err(|error| format!("read {}: {error}", input.display()))
            .and_then(|raw| {
                raw.trim()
                    .parse::<i64>()
                    .map(|microwatts| microwatts as f64 / 1_000_000.0)
                    .map_err(|error| format!("parse {}: {error}", input.display()))
            });
        if result.is_err() {
            inputs.remove(&key);
        }
        result
    }
}

fn hwmon_dir(device: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<_> = fs::read_dir(device.join("hwmon"))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("hwmon"))
        })
        .collect();
    dirs.sort();
    dirs.into_iter().next()
}

/// The `power<N>_input` whose `power<N>_label` is `channel`.
fn power_input(hwmon: &Path, channel: &str) -> Option<PathBuf> {
    fs::read_dir(hwmon).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name().into_string().ok()?;
        let index = name.strip_prefix("power")?.strip_suffix("_label")?;
        let label = fs::read_to_string(entry.path()).ok()?;
        (label.trim() == channel).then(|| hwmon.join(format!("power{index}_input")))
    })
}

fn read_power_snapshot(config: &PowerConfig, source: &dyn PowerSource) -> PowerSnapshot {
    PowerSnapshot {
        rails: config
            .rails
            .iter()
            .map(|rail| match source.read_watts(rail) {
                Ok(watts) => PowerRailReading {
                    watts: Some(watts),
                    error: None,
                },
                Err(error) => PowerRailReading {
                    watts: None,
                    error: Some(error),
                },
            })
            .collect(),
    }
}

#[derive(Debug, Clone, Default)]
struct RailAggregate {
    current_watts: Option<f64>,
    samples: u64,
    errors: u64,
}

#[derive(Debug)]
pub struct PowerAccumulator {
    config: PowerConfig,
    started_at: Instant,
    current_watts: Option<f64>,
    total_watts_sum: f64,
    peak_watts: Option<f64>,
    valid_samples: u64,
    failed_samples: u64,
    last_sample_valid: bool,
    last_error: Option<String>,
    rails: Vec<RailAggregate>,
}

impl PowerAccumulator {
    pub fn new(config: PowerConfig) -> Self {
        let rails = vec![RailAggregate::default(); config.rails.len()];
        Self {
            config,
            started_at: Instant::now(),
            current_watts: None,
            total_watts_sum: 0.0,
            peak_watts: None,
            valid_samples: 0,
            failed_samples: 0,
            last_sample_valid: false,
            last_error: None,
            rails,
        }
    }

    fn record(&mut self, snapshot: PowerSnapshot) {
        let total = snapshot.total_watts();
        let mut errors: Vec<String> = Vec::new();
        for (aggregate, reading) in self.rails.iter_mut().zip(snapshot.rails) {
            // A rail that cannot be read has no current value; showing its
            // previous reading would present a stale number as live.
            aggregate.current_watts = reading.watts;
            match reading.watts {
                Some(_) => aggregate.samples += 1,
                None => {
                    aggregate.errors += 1;
                    if let Some(error) = reading.error {
                        if !errors.contains(&error) {
                            errors.push(error);
                        }
                    }
                }
            }
        }

        self.last_sample_valid = total.is_some();
        self.current_watts = total;
        if let Some(total) = total {
            self.total_watts_sum += total;
            self.valid_samples += 1;
            self.peak_watts = Some(self.peak_watts.map_or(total, |peak| peak.max(total)));
        } else {
            self.failed_samples += 1;
        }
        self.last_error = (!errors.is_empty()).then(|| errors.join("; "));
    }

    pub fn metric_values(&self) -> BTreeMap<String, f64> {
        let mut values = BTreeMap::new();
        if let Some(current) = self.current_watts {
            values.insert("power_current_watts".into(), current);
        }
        if self.valid_samples > 0 {
            values.insert(
                "power_average_watts".into(),
                self.total_watts_sum / self.valid_samples as f64,
            );
        }
        if let Some(peak) = self.peak_watts {
            values.insert("power_peak_watts".into(), peak);
        }
        for (config, aggregate) in self.config.rails.iter().zip(&self.rails) {
            if let Some(current) = aggregate.current_watts {
                values.insert(config.key.into(), current);
            }
        }
        values
    }

    pub fn status(&self) -> PowerStatus {
        PowerStatus {
            profile: self.config.profile.name().into(),
            sample_interval_ms: self.config.sample_interval.as_millis() as u64,
            duration_seconds: self.started_at.elapsed().as_secs_f64(),
            valid_samples: self.valid_samples,
            failed_samples: self.failed_samples,
            last_sample_valid: self.last_sample_valid,
            last_error: self.last_error.clone(),
            rails: self
                .config
                .rails
                .iter()
                .zip(&self.rails)
                .map(|(config, aggregate)| PowerRailStatus {
                    key: config.key.into(),
                    label: config.name.into(),
                    current_watts: aggregate.current_watts,
                    samples: aggregate.samples,
                    errors: aggregate.errors,
                })
                .collect(),
        }
    }
}

pub fn power_metric_definitions(config: &PowerConfig) -> Vec<MetricDefinition> {
    let mut metrics = vec![
        MetricDefinition::new(
            "power_current_watts",
            "Current board power",
            "Current",
            "Power",
            "W",
            "Latest total output power of the configured board rails, read from the kernel's \
             PMBus driver. Unavailable while any rail cannot be read.",
            None,
            None,
        ),
        MetricDefinition::new(
            "power_average_watts",
            "Average board power",
            "Average",
            "Power",
            "W",
            "Average board power over samples in which every rail was read, since the Sentinel \
             daemon started.",
            None,
            None,
        ),
        MetricDefinition::new(
            "power_peak_watts",
            "Peak board power",
            "Peak",
            "Power",
            "W",
            "Peak board power over samples in which every rail was read, since the Sentinel \
             daemon started.",
            None,
            None,
        ),
    ];
    metrics.extend(config.rails.iter().map(|rail| {
        MetricDefinition::new(
            rail.key,
            rail.name,
            rail.name,
            "PowerRail",
            "W",
            &format!(
                "Latest output power of this rail: the {} channel of PMBus device {}, read from \
                 the kernel's hwmon driver.",
                rail.channel, rail.device
            ),
            None,
            None,
        )
    }));
    metrics
}

pub fn spawn_power_collector(
    stopped: Arc<AtomicBool>,
    config: PowerConfig,
    accumulator: Arc<Mutex<PowerAccumulator>>,
) {
    let source: Arc<dyn PowerSource> = Arc::new(HwmonPowerSource::new(I2C_DEVICES));
    let _ = thread::Builder::new()
        .name("sentinel-power".into())
        .spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let sample_start = Instant::now();
                let snapshot = read_power_snapshot(&config, source.as_ref());
                if let Ok(mut state) = accumulator.lock() {
                    state.record(snapshot);
                }
                let elapsed = sample_start.elapsed();
                if elapsed < config.sample_interval {
                    sleep_interruptible(&stopped, config.sample_interval - elapsed);
                }
            }
        });
}

fn sleep_interruptible(stopped: &AtomicBool, duration: Duration) {
    if !stopped.load(Ordering::Relaxed) {
        // The power interval is 250 ms, so one sleep has the same sampling
        // cadence with at most 250 ms shutdown latency and no 10 ms polling.
        thread::sleep(duration);
    }
}

pub fn detect_power_profile() -> PowerProfile {
    let model = fs::read("/proc/device-tree/model")
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', ""))
        .unwrap_or_default();
    detect_power_profile_from_model(&model)
}

fn detect_power_profile_from_model(model: &str) -> PowerProfile {
    if model.to_ascii_lowercase().contains("modalix dvt") {
        PowerProfile::ModalixDvt
    } else {
        PowerProfile::ModalixSom
    }
}

pub fn rails_for_profile(profile: PowerProfile) -> Vec<PowerRailConfig> {
    let rail = |key, name, device, channel| PowerRailConfig {
        key,
        name,
        device,
        channel,
    };
    match profile {
        PowerProfile::ModalixSom => vec![
            rail(
                "power_rail_soc_ddr_vdd_watts",
                "SoC and DDR VDD",
                "3-004f",
                "pout1",
            ),
            rail("power_rail_hdmi_watts", "HDMI 1.2V", "3-004f", "pout2"),
            rail(
                "power_rail_soc_ddr_vddq_watts",
                "SoC and DDR VDDQ",
                "3-004f",
                "pout3",
            ),
            rail(
                "power_rail_soc_ddr_vdd2h_watts",
                "SoC and DDR VDD2H",
                "3-004f",
                "pout4",
            ),
            rail("power_rail_mla_watts", "MLA 0.68V", "3-004d", "pout1"),
            rail(
                "power_rail_pcie_eth_watts",
                "PCIe and ETH VP",
                "3-004d",
                "pout2",
            ),
            rail(
                "power_rail_platform_1v8_watts",
                "SOM Platform 1.8V",
                "3-004d",
                "pout3",
            ),
            rail(
                "power_rail_platform_3v3_watts",
                "SOM Platform 3.3V",
                "3-004d",
                "pout4",
            ),
        ],
        PowerProfile::ModalixDvt => vec![rail(
            "power_rail_dvt_pmic_4d_page0_watts",
            "DVT PMIC 0x4d page0",
            "4-004d",
            "pout1",
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

    /// A `/sys/bus/i2c/devices` tree laid out as the kernel's PMBus core
    /// publishes it.
    struct I2cDevices(PathBuf);

    impl I2cDevices {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "sentinel-power-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        /// An I2C device without a bound driver.
        fn device(&self, device: &str, chip: &str) -> PathBuf {
            let path = self.0.join(device);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("name"), format!("{chip}\n")).unwrap();
            path
        }

        /// A bound PMBus device; `channels` are `(index, label, microwatts)`.
        fn pmbus(&self, device: &str, hwmon: &str, channels: &[(u32, &str, i64)]) -> PathBuf {
            let dir = self.device(device, "irps5401");
            fs::create_dir_all(dir.join("driver")).unwrap();
            let hwmon = dir.join("hwmon").join(hwmon);
            fs::create_dir_all(&hwmon).unwrap();
            for (index, label, microwatts) in channels {
                fs::write(
                    hwmon.join(format!("power{index}_label")),
                    format!("{label}\n"),
                )
                .unwrap();
                fs::write(
                    hwmon.join(format!("power{index}_input")),
                    format!("{microwatts}\n"),
                )
                .unwrap();
            }
            hwmon
        }

        fn source(&self) -> HwmonPowerSource {
            HwmonPowerSource::new(&self.0)
        }
    }

    impl Drop for I2cDevices {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    // The irps5401 driver numbers pin1..5 before pout1..5, so pout1 is power6.
    const IRPS5401_4F: [(u32, &str, i64); 6] = [
        (1, "pin1", 9_000_000),
        (6, "pout1", 3_250_000),
        (7, "pout2", 562_500),
        (8, "pout3", 125_000),
        (9, "pout4", 656_250),
        (10, "pout5", 0),
    ];

    fn som_config() -> PowerConfig {
        PowerConfig {
            profile: PowerProfile::ModalixSom,
            sample_interval: Duration::from_millis(250),
            rails: rails_for_profile(PowerProfile::ModalixSom),
        }
    }

    fn reading(watts: Option<f64>) -> PowerRailReading {
        PowerRailReading {
            watts,
            error: watts.is_none().then(|| "read failed".to_string()),
        }
    }

    #[test]
    fn profiles_name_each_rail_by_device_and_pout_channel() {
        let som = rails_for_profile(PowerProfile::ModalixSom);
        let channels: Vec<_> = som.iter().map(|r| (r.device, r.channel)).collect();
        assert_eq!(
            channels,
            [
                ("3-004f", "pout1"),
                ("3-004f", "pout2"),
                ("3-004f", "pout3"),
                ("3-004f", "pout4"),
                ("3-004d", "pout1"),
                ("3-004d", "pout2"),
                ("3-004d", "pout3"),
                ("3-004d", "pout4"),
            ]
        );
        assert_eq!(som[4].key, "power_rail_mla_watts");

        let dvt = rails_for_profile(PowerProfile::ModalixDvt);
        assert_eq!(dvt.len(), 1);
        assert_eq!((dvt[0].device, dvt[0].channel), ("4-004d", "pout1"));
    }

    #[test]
    fn detects_dvt_and_defaults_to_som() {
        assert_eq!(
            detect_power_profile_from_model("SiMa.ai Modalix DVT"),
            PowerProfile::ModalixDvt
        );
        assert_eq!(
            detect_power_profile_from_model("SiMa.ai Modalix SOM"),
            PowerProfile::ModalixSom
        );
        assert_eq!(
            detect_power_profile_from_model("unknown"),
            PowerProfile::ModalixSom
        );
    }

    #[test]
    fn reads_rails_by_channel_label_in_watts() {
        let sysfs = I2cDevices::new();
        sysfs.pmbus("3-004f", "hwmon4", &IRPS5401_4F);
        let mut config = som_config();
        config.rails.truncate(4);

        let snapshot = read_power_snapshot(&config, &sysfs.source());
        let watts: Vec<_> = snapshot.rails.iter().map(|r| r.watts).collect();
        assert_eq!(
            watts,
            [Some(3.25), Some(0.5625), Some(0.125), Some(0.65625)]
        );
        assert_eq!(snapshot.total_watts(), Some(4.59375));
    }

    #[test]
    fn names_the_driver_to_load_when_none_is_bound() {
        let sysfs = I2cDevices::new();
        sysfs.device("3-004f", "irps5401");
        let rail = &som_config().rails[0];

        let error = sysfs.source().read_watts(rail).unwrap_err();
        assert!(error.contains("modprobe irps5401"), "{error}");
    }

    #[test]
    fn reports_a_missing_device_or_channel() {
        let sysfs = I2cDevices::new();
        let rails = som_config().rails;
        let error = sysfs.source().read_watts(&rails[0]).unwrap_err();
        assert!(error.contains("no I2C device 3-004f"), "{error}");

        sysfs.pmbus("3-004f", "hwmon4", &IRPS5401_4F[..2]);
        let error = sysfs.source().read_watts(&rails[1]).unwrap_err();
        assert!(error.contains("no pout2 power channel"), "{error}");
    }

    #[test]
    fn finds_the_new_hwmon_directory_after_the_driver_rebinds() {
        let sysfs = I2cDevices::new();
        let old = sysfs.pmbus("3-004f", "hwmon4", &IRPS5401_4F);
        let source = sysfs.source();
        let rail = &som_config().rails[0];
        assert_eq!(source.read_watts(rail), Ok(3.25));

        fs::remove_dir_all(&old).unwrap();
        assert!(source.read_watts(rail).is_err());
        let new = sysfs.0.join("3-004f/hwmon/hwmon9");
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("power6_label"), "pout1\n").unwrap();
        fs::write(new.join("power6_input"), "4500000\n").unwrap();
        assert_eq!(source.read_watts(rail), Ok(4.5));
    }

    #[test]
    fn totals_only_complete_samples_and_never_keeps_stale_values() {
        let mut config = som_config();
        config.rails.truncate(2);
        let mut aggregate = PowerAccumulator::new(config);
        aggregate.record(PowerSnapshot {
            rails: vec![reading(Some(2.0)), reading(Some(1.0))],
        });
        aggregate.record(PowerSnapshot {
            rails: vec![reading(Some(5.0)), reading(None)],
        });

        // A sample missing a rail has no total and leaves average and peak alone.
        let values = aggregate.metric_values();
        assert!(!values.contains_key("power_current_watts"));
        assert_eq!(values["power_average_watts"], 3.0);
        assert_eq!(values["power_peak_watts"], 3.0);
        assert_eq!(values["power_rail_soc_ddr_vdd_watts"], 5.0);
        assert!(!values.contains_key("power_rail_hdmi_watts"));

        aggregate.record(PowerSnapshot {
            rails: vec![reading(None), reading(None)],
        });
        let values = aggregate.metric_values();
        assert!(!values.contains_key("power_rail_soc_ddr_vdd_watts"));
        let status = aggregate.status();
        assert_eq!(status.valid_samples, 1);
        assert_eq!(status.failed_samples, 2);
        assert!(!status.last_sample_valid);
        assert_eq!(status.rails[0].current_watts, None);
        assert_eq!(status.rails[0].samples, 2);
        assert_eq!(status.rails[0].errors, 1);
        assert_eq!(status.rails[1].errors, 2);
        // The same reason from every rail is reported once.
        assert_eq!(status.last_error.as_deref(), Some("read failed"));

        aggregate.record(PowerSnapshot {
            rails: vec![reading(Some(4.0)), reading(Some(1.5))],
        });
        assert_eq!(aggregate.metric_values()["power_current_watts"], 5.5);
        assert!(aggregate.status().last_sample_valid);
    }
}
