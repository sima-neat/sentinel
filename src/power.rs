//! Standalone Modalix PMBus power telemetry.
//!
//! The register protocol and built-in rail profiles intentionally mirror
//! Neat Core's `PowerTelemetry` implementation, but Sentinel does not link to
//! or otherwise depend on Neat Core.

use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::model::{MetricDefinition, PowerRailStatus, PowerStatus};

const PAGE_REGISTER: u8 = 0x00;
const POUT_REGISTER: u8 = 0x96;
const I2C_SLAVE: libc::c_ulong = 0x0703;
const I2C_SMBUS: libc::c_ulong = 0x0720;
const I2C_SMBUS_WRITE: u8 = 0;
const I2C_SMBUS_READ: u8 = 1;
const I2C_SMBUS_BYTE_DATA: u32 = 2;
const I2C_SMBUS_BLOCK_MAX: usize = 32;

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
    pub i2c_bus: i32,
    pub i2c_addr: u8,
    pub page: u8,
    pub pout_exponent: i32,
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
    total_watts: f64,
    rails_with_power: usize,
}

pub trait PowerI2cBackend: Send + Sync {
    fn set_page(&self, bus: i32, addr: u8, page: u8) -> Result<(), String>;
    fn read_register(&self, bus: i32, addr: u8, register: u8) -> Result<u8, String>;

    fn read_power(&self, rail: &PowerRailConfig) -> Result<u8, String> {
        self.set_page(rail.i2c_bus, rail.i2c_addr, rail.page)?;
        self.read_register(rail.i2c_bus, rail.i2c_addr, POUT_REGISTER)
    }
}

#[derive(Debug, Default)]
struct NativePowerI2cBackend {
    devices: Mutex<BTreeMap<i32, fs::File>>,
}

impl PowerI2cBackend for NativePowerI2cBackend {
    fn set_page(&self, bus: i32, addr: u8, page: u8) -> Result<(), String> {
        let device = open_device(bus, addr)?;
        smbus_write_byte_data(device.as_raw_fd(), PAGE_REGISTER, page).map_err(|error| {
            format!("i2c write page 0x{page:x} failed on /dev/i2c-{bus} addr 0x{addr:x}: {error}")
        })
    }

    fn read_register(&self, bus: i32, addr: u8, register: u8) -> Result<u8, String> {
        let device = open_device(bus, addr)?;
        smbus_read_byte_data(device.as_raw_fd(), register).map_err(|error| {
            format!(
                "i2c read register 0x{register:x} failed on /dev/i2c-{bus} addr 0x{addr:x}: {error}"
            )
        })
    }

    fn read_power(&self, rail: &PowerRailConfig) -> Result<u8, String> {
        let mut devices = self
            .devices
            .lock()
            .map_err(|_| "I2C device cache lock poisoned".to_string())?;
        let result = (|| {
            let device = match devices.entry(rail.i2c_bus) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let path = format!("/dev/i2c-{}", rail.i2c_bus);
                    let device = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .custom_flags(libc::O_CLOEXEC)
                        .open(&path)
                        .map_err(|error| format!("open {path} failed: {error}"))?;
                    entry.insert(device)
                }
            };
            select_slave(device.as_raw_fd(), rail.i2c_bus, rail.i2c_addr)?;
            smbus_write_byte_data(device.as_raw_fd(), PAGE_REGISTER, rail.page).map_err(
                |error| {
                    format!(
                        "i2c write page 0x{:x} failed on /dev/i2c-{} addr 0x{:x}: {error}",
                        rail.page, rail.i2c_bus, rail.i2c_addr
                    )
                },
            )?;
            smbus_read_byte_data(device.as_raw_fd(), POUT_REGISTER).map_err(|error| {
                format!(
                    "i2c read register 0x{POUT_REGISTER:x} failed on /dev/i2c-{} addr 0x{:x}: {error}",
                    rail.i2c_bus, rail.i2c_addr
                )
            })
        })();
        if result.is_err() {
            // Reopen on the next sample after a driver reset or transient bus
            // failure rather than retaining a stale descriptor indefinitely.
            devices.remove(&rail.i2c_bus);
        }
        result
    }
}

fn select_slave(fd: i32, bus: i32, addr: u8) -> Result<(), String> {
    let result = unsafe { libc::ioctl(fd, I2C_SLAVE as _, i32::from(addr)) };
    if result < 0 {
        Err(format!(
            "I2C_SLAVE 0x{addr:x} on /dev/i2c-{bus} failed: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[repr(C)]
union SmbusData {
    byte: u8,
    word: u16,
    block: [u8; I2C_SMBUS_BLOCK_MAX + 2],
}

#[repr(C)]
struct SmbusIoctlData {
    read_write: u8,
    command: u8,
    size: u32,
    data: *mut SmbusData,
}

fn open_device(bus: i32, addr: u8) -> Result<fs::File, String> {
    let path = format!("/dev/i2c-{bus}");
    let device = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(&path)
        .map_err(|error| format!("open {path} failed: {error}"))?;
    select_slave(device.as_raw_fd(), bus, addr)?;
    Ok(device)
}

fn smbus_access(
    fd: i32,
    read_write: u8,
    command: u8,
    size: u32,
    data: &mut SmbusData,
) -> Result<(), std::io::Error> {
    let mut args = SmbusIoctlData {
        read_write,
        command,
        size,
        data,
    };
    let result = unsafe { libc::ioctl(fd, I2C_SMBUS as _, &mut args) };
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn smbus_write_byte_data(fd: i32, register: u8, value: u8) -> Result<(), std::io::Error> {
    let mut data = SmbusData { byte: value };
    smbus_access(
        fd,
        I2C_SMBUS_WRITE,
        register,
        I2C_SMBUS_BYTE_DATA,
        &mut data,
    )
}

fn smbus_read_byte_data(fd: i32, register: u8) -> Result<u8, std::io::Error> {
    let mut data = SmbusData { byte: 0 };
    smbus_access(fd, I2C_SMBUS_READ, register, I2C_SMBUS_BYTE_DATA, &mut data)?;
    Ok(unsafe { data.byte })
}

fn read_power_snapshot(config: &PowerConfig, backend: &dyn PowerI2cBackend) -> PowerSnapshot {
    let mut readings = Vec::with_capacity(config.rails.len());
    let mut total_watts = 0.0;
    let mut rails_with_power = 0;

    for rail in &config.rails {
        match backend.read_power(rail) {
            Ok(raw) => {
                let watts = f64::from(raw) * 2f64.powi(rail.pout_exponent);
                total_watts += watts;
                rails_with_power += 1;
                readings.push(PowerRailReading {
                    watts: Some(watts),
                    error: None,
                });
            }
            Err(error) => readings.push(PowerRailReading {
                watts: None,
                error: Some(error),
            }),
        }
    }

    PowerSnapshot {
        rails: readings,
        total_watts,
        rails_with_power,
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
        let mut errors = Vec::new();
        for (aggregate, reading) in self.rails.iter_mut().zip(snapshot.rails) {
            match reading.watts {
                Some(watts) => {
                    aggregate.current_watts = Some(watts);
                    aggregate.samples += 1;
                }
                None => {
                    aggregate.errors += 1;
                    if let Some(error) = reading.error {
                        errors.push(error);
                    }
                }
            }
        }

        self.last_sample_valid = snapshot.rails_with_power > 0;
        if self.last_sample_valid {
            self.current_watts = Some(snapshot.total_watts);
            self.total_watts_sum += snapshot.total_watts;
            self.valid_samples += 1;
            self.peak_watts = Some(
                self.peak_watts
                    .map_or(snapshot.total_watts, |peak| peak.max(snapshot.total_watts)),
            );
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
            "Latest valid total PMBus POUT reading across configured board rails.",
            None,
            None,
        ),
        MetricDefinition::new(
            "power_average_watts",
            "Average board power",
            "Average",
            "Power",
            "W",
            "Average valid total board power since the Sentinel daemon started.",
            None,
            None,
        ),
        MetricDefinition::new(
            "power_peak_watts",
            "Peak board power",
            "Peak",
            "Power",
            "W",
            "Peak valid total board power since the Sentinel daemon started.",
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
            "Latest valid POUT reading for this PMBus rail.",
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
    let backend: Arc<dyn PowerI2cBackend> = Arc::new(NativePowerI2cBackend::default());
    let _ = thread::Builder::new()
        .name("sentinel-power".into())
        .spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let sample_start = Instant::now();
                let snapshot = read_power_snapshot(&config, backend.as_ref());
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
        // The power interval is 100 ms, so one sleep has the same sampling
        // cadence with at most 100 ms shutdown latency and no 10 ms polling.
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
    match profile {
        PowerProfile::ModalixSom => vec![
            PowerRailConfig {
                key: "power_rail_soc_ddr_vdd_watts",
                name: "SoC and DDR VDD",
                i2c_bus: 3,
                i2c_addr: 0x4f,
                page: 0x00,
                pout_exponent: -2,
            },
            PowerRailConfig {
                key: "power_rail_hdmi_watts",
                name: "HDMI 1.2V",
                i2c_bus: 3,
                i2c_addr: 0x4f,
                page: 0x01,
                pout_exponent: -5,
            },
            PowerRailConfig {
                key: "power_rail_soc_ddr_vddq_watts",
                name: "SoC and DDR VDDQ",
                i2c_bus: 3,
                i2c_addr: 0x4f,
                page: 0x02,
                pout_exponent: -5,
            },
            PowerRailConfig {
                key: "power_rail_soc_ddr_vdd2h_watts",
                name: "SoC and DDR VDD2H",
                i2c_bus: 3,
                i2c_addr: 0x4f,
                page: 0x03,
                pout_exponent: -5,
            },
            PowerRailConfig {
                key: "power_rail_mla_watts",
                name: "MLA 0.68V",
                i2c_bus: 3,
                i2c_addr: 0x4d,
                page: 0x00,
                pout_exponent: -2,
            },
            PowerRailConfig {
                key: "power_rail_pcie_eth_watts",
                name: "PCIe and ETH VP",
                i2c_bus: 3,
                i2c_addr: 0x4d,
                page: 0x01,
                pout_exponent: -5,
            },
            PowerRailConfig {
                key: "power_rail_platform_1v8_watts",
                name: "SOM Platform 1.8V",
                i2c_bus: 3,
                i2c_addr: 0x4d,
                page: 0x02,
                pout_exponent: -5,
            },
            PowerRailConfig {
                key: "power_rail_platform_3v3_watts",
                name: "SOM Platform 3.3V",
                i2c_bus: 3,
                i2c_addr: 0x4d,
                page: 0x03,
                pout_exponent: -5,
            },
        ],
        PowerProfile::ModalixDvt => vec![PowerRailConfig {
            key: "power_rail_dvt_pmic_4d_page0_watts",
            name: "DVT PMIC 0x4d page0",
            i2c_bus: 4,
            i2c_addr: 0x4d,
            page: 0x00,
            pout_exponent: -2,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeBackend {
        current_pages: Mutex<HashMap<(i32, u8), u8>>,
        registers: Mutex<HashMap<(i32, u8, u8, u8), u8>>,
        failures: Mutex<HashMap<(i32, u8, u8), String>>,
        calls: Mutex<Vec<String>>,
    }

    impl PowerI2cBackend for FakeBackend {
        fn set_page(&self, bus: i32, addr: u8, page: u8) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("page:{bus}:{addr:x}:{page:x}"));
            if let Some(error) = self.failures.lock().unwrap().get(&(bus, addr, page)) {
                return Err(error.clone());
            }
            self.current_pages.lock().unwrap().insert((bus, addr), page);
            Ok(())
        }

        fn read_register(&self, bus: i32, addr: u8, register: u8) -> Result<u8, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("read:{bus}:{addr:x}:{register:x}"));
            if let Some(error) = self.failures.lock().unwrap().get(&(bus, addr, register)) {
                return Err(error.clone());
            }
            let page = self.current_pages.lock().unwrap()[&(bus, addr)];
            self.registers
                .lock()
                .unwrap()
                .get(&(bus, addr, page, register))
                .copied()
                .ok_or_else(|| "missing fake register".into())
        }
    }

    fn som_config() -> PowerConfig {
        PowerConfig {
            profile: PowerProfile::ModalixSom,
            sample_interval: Duration::from_millis(100),
            rails: rails_for_profile(PowerProfile::ModalixSom),
        }
    }

    #[test]
    fn profiles_match_core_power_telemetry() {
        let som = rails_for_profile(PowerProfile::ModalixSom);
        assert_eq!(som.len(), 8);
        assert_eq!((som[0].i2c_bus, som[0].i2c_addr, som[0].page), (3, 0x4f, 0));
        assert_eq!(som[0].pout_exponent, -2);
        assert_eq!((som[4].i2c_addr, som[4].page), (0x4d, 0));
        assert_eq!(som[4].pout_exponent, -2);

        let dvt = rails_for_profile(PowerProfile::ModalixDvt);
        assert_eq!(dvt.len(), 1);
        assert_eq!((dvt[0].i2c_bus, dvt[0].i2c_addr, dvt[0].page), (4, 0x4d, 0));
        assert_eq!(dvt[0].pout_exponent, -2);
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
    fn reads_pout_with_scaling_and_partial_failure() {
        let mut config = som_config();
        config.rails.truncate(2);
        let backend = FakeBackend::default();
        backend
            .registers
            .lock()
            .unwrap()
            .insert((3, 0x4f, 0, POUT_REGISTER), 8);

        let snapshot = read_power_snapshot(&config, &backend);
        assert_eq!(snapshot.rails_with_power, 1);
        assert_eq!(snapshot.total_watts, 2.0);
        assert_eq!(snapshot.rails[0].watts, Some(2.0));
        assert!(snapshot.rails[1].error.is_some());
        assert_eq!(
            backend.calls.lock().unwrap().as_slice(),
            ["page:3:4f:0", "read:3:4f:96", "page:3:4f:1", "read:3:4f:96"]
        );
    }

    #[test]
    fn aggregates_only_valid_totals_and_tracks_errors() {
        let mut config = som_config();
        config.rails.truncate(2);
        let mut aggregate = PowerAccumulator::new(config.clone());
        aggregate.record(PowerSnapshot {
            rails: vec![
                PowerRailReading {
                    watts: Some(2.0),
                    error: None,
                },
                PowerRailReading {
                    watts: Some(1.0),
                    error: None,
                },
            ],
            total_watts: 3.0,
            rails_with_power: 2,
        });
        aggregate.record(PowerSnapshot {
            rails: vec![
                PowerRailReading {
                    watts: Some(5.0),
                    error: None,
                },
                PowerRailReading {
                    watts: None,
                    error: Some("read failed".into()),
                },
            ],
            total_watts: 5.0,
            rails_with_power: 1,
        });
        aggregate.record(PowerSnapshot {
            rails: vec![
                PowerRailReading {
                    watts: None,
                    error: Some("read failed".into()),
                },
                PowerRailReading {
                    watts: None,
                    error: Some("read failed".into()),
                },
            ],
            total_watts: 0.0,
            rails_with_power: 0,
        });

        let values = aggregate.metric_values();
        assert_eq!(values["power_current_watts"], 5.0);
        assert_eq!(values["power_average_watts"], 4.0);
        assert_eq!(values["power_peak_watts"], 5.0);
        let status = aggregate.status();
        assert_eq!(status.valid_samples, 2);
        assert_eq!(status.failed_samples, 1);
        assert!(!status.last_sample_valid);
        assert_eq!(status.rails[0].samples, 2);
        assert_eq!(status.rails[0].errors, 1);
        assert_eq!(status.rails[1].errors, 2);
        assert!(status.last_error.unwrap().contains("read failed"));
    }
}
