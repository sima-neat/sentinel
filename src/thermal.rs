use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

use crate::model::MetricDefinition;

const PVT_BASE: libc::off_t = 0x0FF0E000;
const PRC_BASE: libc::off_t = 0x0FF00000;
const MAP_SIZE: usize = 4096;

const RTSN_GROUPS: [&str; 14] = [
    "MLA", "MLA", "MLA", "MLA", "APU", "CVU", "TOP", "MLA", "MLA", "MLA", "MLA", "APU", "CVU",
    "TOP",
];

const RTSN_SITES: [&str; 7] = [
    "MLA Q0",
    "MLA Q1",
    "MLA Q2",
    "MLA Q3",
    "APU",
    "CVU",
    "TOP (near PCIE/ETH area)",
];

const HWMON: [(&str, &str, &str, &str, Option<&str>, &str, &str, &str); 3] = [
    (
        "lm96163_temp1",
        "LM96163 temp1",
        "LM96-1",
        "Board",
        Some("lm96163"),
        "temp1_input",
        "/sys/class/hwmon/hwmon2/temp1_input",
        "SOM top-side board temperature from the LM96063 hardware-monitoring IC's internal sensor.",
    ),
    (
        "lm96163_temp2",
        "LM96163 temp2",
        "LM96-2",
        "Board",
        Some("lm96163"),
        "temp2_input",
        "/sys/class/hwmon/hwmon2/temp2_input",
        "SOM bottom-side board temperature from a diode on the bottom side of the SOM, read through the LM96063 IC.",
    ),
    (
        "eth_mdio_temp1",
        "ETH/MDIO temp1",
        "ETH-1",
        "Board",
        None,
        "temp1_input",
        "/sys/class/hwmon/hwmon0/temp1_input",
        "Ethernet/MDIO temperature reported through the ETH hwmon device.",
    ),
];

pub fn thermal_metric_definitions() -> Vec<MetricDefinition> {
    let mut out = Vec::new();
    for (idx, group) in RTSN_GROUPS.iter().enumerate() {
        let site = RTSN_SITES[idx % 7];
        let role = if idx < 7 { "Alert" } else { "Trip" };
        out.push(MetricDefinition::new(
            &format!("rtsn_{idx}"),
            &format!("{group} RTSN-{idx}"),
            &format!("{group}-{idx}"),
            group,
            "C",
            &format!("On-die RTSN at the {site} site; feeds DTS Hub {role} channel {idx}."),
            Some(70.0),
            Some(85.0),
        ));
    }
    for (key, label, short, group, _chip, _input, _fallback, description) in HWMON {
        out.push(MetricDefinition::new(
            key,
            label,
            short,
            group,
            "C",
            description,
            Some(70.0),
            Some(85.0),
        ));
    }
    out
}

pub struct ThermalCollector {
    sampler: Option<ModalixSampler>,
    hwmon_paths: BTreeMap<String, String>,
}

impl ThermalCollector {
    pub fn new() -> Result<Self> {
        Ok(Self {
            sampler: ModalixSampler::new().ok(),
            hwmon_paths: resolve_hwmon_paths(),
        })
    }

    pub fn sample(&mut self) -> BTreeMap<String, f64> {
        let mut values = BTreeMap::new();
        if let Some(sampler) = self.sampler.as_mut() {
            match sampler.sample_rtsn() {
                Ok(rtsn) => values.extend(rtsn),
                Err(err) => {
                    eprintln!("Sentinel thermal sample failed: {err}");
                    self.sampler = None;
                }
            }
        }
        for (key, path) in &self.hwmon_paths {
            values.insert(key.clone(), read_hwmon(path).unwrap_or(f64::NAN));
        }
        values
    }
}

fn resolve_hwmon_paths() -> BTreeMap<String, String> {
    let mut paths = BTreeMap::new();
    for (key, _label, _short, _group, chip, input, fallback, _description) in HWMON {
        let resolved = chip
            .and_then(find_hwmon_by_name)
            .map(|dir| format!("{dir}/{input}"))
            .filter(|path| Path::new(path).exists())
            .unwrap_or_else(|| fallback.to_string());
        paths.insert(key.to_string(), resolved);
    }
    paths
}

fn find_hwmon_by_name(chip: &str) -> Option<String> {
    let entries = fs::read_dir("/sys/class/hwmon").ok()?;
    for entry in entries.flatten() {
        let name = fs::read_to_string(entry.path().join("name")).ok()?;
        if name.trim() == chip {
            return Some(entry.path().display().to_string());
        }
    }
    None
}

fn read_hwmon(path: &str) -> io::Result<f64> {
    let raw = fs::read_to_string(path)?;
    let milli_c: f64 = raw.trim().parse().unwrap_or(f64::NAN);
    Ok(milli_c / 1000.0)
}

struct RegisterRegion {
    file: fs::File,
    ptr: *mut libc::c_void,
}

impl RegisterRegion {
    fn new(base: libc::off_t) -> Result<Self> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/mem")
            .context("open /dev/mem")?;
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                MAP_SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                base,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(anyhow!("mmap /dev/mem failed"));
        }
        Ok(Self { file, ptr })
    }

    fn read32(&self, offset: usize) -> u32 {
        unsafe { std::ptr::read_volatile((self.ptr as *const u8).add(offset) as *const u32) }
    }

    fn write32(&self, offset: usize, value: u32) {
        unsafe { std::ptr::write_volatile((self.ptr as *mut u8).add(offset) as *mut u32, value) }
    }
}

impl Drop for RegisterRegion {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr, MAP_SIZE);
        }
        let _ = self.file.sync_all();
    }
}

struct ModalixSampler {
    prc: RegisterRegion,
    pvt: RegisterRegion,
}

impl ModalixSampler {
    fn new() -> Result<Self> {
        let prc = RegisterRegion::new(PRC_BASE)?;
        let pvt = RegisterRegion::new(PVT_BASE)?;
        let sampler = Self { prc, pvt };
        sampler.initialize();
        Ok(sampler)
    }

    fn initialize(&self) {
        self.prc.write32(0x514, 0x3F);
        self.pvt.write32(0x800, 0x01000404);
        self.pvt.write32(0x80C, 0x88000001);
        thread::sleep(Duration::from_secs(1));
        let _ = self.pvt.read32(0x808);
    }

    fn sample_rtsn(&mut self) -> Result<BTreeMap<String, f64>> {
        let mut values = BTreeMap::new();
        for sensor_id in 0..14 {
            self.trigger_sensor(sensor_id)?;
            let data = self.pvt.read32(0xA40 + sensor_id * 4);
            values.insert(
                format!("rtsn_{sensor_id}"),
                data as f64 * 698.9 / 4096.0 - 283.0,
            );
        }
        Ok(values)
    }

    fn trigger_sensor(&self, sensor_id: usize) -> Result<()> {
        let sid = sensor_id as u32;
        let _ = self.pvt.read32(0x808);
        thread::sleep(Duration::from_millis(100));
        self.pvt.write32(0x80C, 0x89000000 | (sid << 8));
        let _ = self.pvt.read32(0x808);
        thread::sleep(Duration::from_millis(10));
        self.pvt.write32(0x80C, 0x8E0000A3);
        let _ = self.pvt.read32(0x808);
        thread::sleep(Duration::from_millis(10));
        self.pvt.write32(0x80C, 0x8D000200);
        let _ = self.pvt.read32(0x808);
        thread::sleep(Duration::from_millis(10));
        self.pvt
            .write32(0x80C, 0x8C200000 | (1 << sid) | (sid << 16));
        let _ = self.pvt.read32(0x808);
        thread::sleep(Duration::from_millis(10));
        let _ = self.pvt.read32(0x808);
        self.pvt.write32(0x80C, 0x88000504);
        let _ = self.pvt.read32(0x808);
        thread::sleep(Duration::from_millis(10));

        let deadline = Instant::now() + Duration::from_secs(2);
        while self.pvt.read32(0xA34) == 0 {
            if Instant::now() > deadline {
                return Err(anyhow!("timed out waiting for RTSN sensor {sensor_id}"));
            }
            thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
}
