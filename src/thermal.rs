use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::model::MetricDefinition;

const MODALIX_HWMON_NAME: &str = "simaai_modalix_thermal_sensor";

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

const HWMON: [(&str, &str, &str, &str, &str, &str, &str); 3] = [
    (
        "lm96163_temp1",
        "LM96163 temp1",
        "LM96-1",
        "Board",
        "lm96163",
        "temp1_input",
        "SOM top-side board temperature from the LM96063 hardware-monitoring IC's internal sensor.",
    ),
    (
        "lm96163_temp2",
        "LM96163 temp2",
        "LM96-2",
        "Board",
        "lm96163",
        "temp2_input",
        "SOM bottom-side board temperature from a diode on the bottom side of the SOM, read through the LM96063 IC.",
    ),
    (
        "eth_mdio_temp1",
        "ETH/MDIO temp1",
        "ETH-1",
        "Board",
        "a800000ethernetmdio000",
        "temp1_input",
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
    for (key, label, short, group, _chip, _input, description) in HWMON {
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
    hwmon_root: PathBuf,
}

impl ThermalCollector {
    pub fn new() -> Result<Self> {
        Ok(Self {
            hwmon_root: PathBuf::from("/sys/class/hwmon"),
        })
    }

    pub fn sample(&mut self) -> BTreeMap<String, f64> {
        // Rediscover devices so driver rebinding/late registration cannot leave
        // stale hwmonN paths pointing at a different chip.
        let mut values = BTreeMap::new();
        let modalix = find_hwmon_by_name(&self.hwmon_root, MODALIX_HWMON_NAME);
        for idx in 0..RTSN_GROUPS.len() {
            // Linux hwmon channels are one-based; RTSN identifiers are zero-based.
            let value = modalix
                .as_ref()
                .and_then(|dir| read_hwmon(&dir.join(format!("temp{}_input", idx + 1))).ok())
                .unwrap_or(f64::NAN);
            values.insert(format!("rtsn_{idx}"), value);
        }
        for (key, _label, _short, _group, chip, input, _description) in HWMON {
            let value = find_hwmon_by_name(&self.hwmon_root, chip)
                .and_then(|dir| read_hwmon(&dir.join(input)).ok())
                .unwrap_or(f64::NAN);
            values.insert(key.to_string(), value);
        }
        values
    }
}

fn find_hwmon_by_name(root: &Path, chip: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let Ok(name) = fs::read_to_string(entry.path().join("name")) else {
            continue;
        };
        if name.trim() == chip {
            return Some(entry.path());
        }
    }
    None
}

fn read_hwmon(path: &Path) -> io::Result<f64> {
    let raw = fs::read_to_string(path)?;
    let milli_c: i64 = raw
        .trim()
        .parse()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(milli_c as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

    struct Sysfs(PathBuf);

    impl Sysfs {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "sentinel-thermal-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn chip(&self, dir: &str, name: &str) -> PathBuf {
            let path = self.0.join(dir);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("name"), format!("{name}\n")).unwrap();
            path
        }

        fn collector(&self) -> ThermalCollector {
            ThermalCollector {
                hwmon_root: self.0.clone(),
            }
        }
    }

    impl Drop for Sysfs {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn maps_all_kernel_channels_and_converts_millidegrees() {
        let sysfs = Sysfs::new();
        let dir = sysfs.chip("hwmon19", MODALIX_HWMON_NAME);
        for idx in 0..14 {
            fs::write(
                dir.join(format!("temp{}_input", idx + 1)),
                (40000 + idx * 125).to_string(),
            )
            .unwrap();
        }
        let values = sysfs.collector().sample();
        for idx in 0..14 {
            assert_eq!(values[&format!("rtsn_{idx}")], 40.0 + idx as f64 * 0.125);
        }
    }

    #[test]
    fn identifies_board_sensors_after_hwmon_renumbering() {
        let sysfs = Sysfs::new();
        let soc = sysfs.chip("hwmon0", MODALIX_HWMON_NAME);
        fs::write(soc.join("temp1_input"), "99000").unwrap();
        let board = sysfs.chip("hwmon7", "lm96163");
        fs::write(board.join("temp1_input"), "31000").unwrap();
        fs::write(board.join("temp2_input"), "32500").unwrap();
        let eth = sysfs.chip("hwmon2", "a800000ethernetmdio000");
        fs::write(eth.join("temp1_input"), "45000").unwrap();
        let values = sysfs.collector().sample();
        assert_eq!(values["rtsn_0"], 99.0);
        assert_eq!(values["lm96163_temp1"], 31.0);
        assert_eq!(values["lm96163_temp2"], 32.5);
        assert_eq!(values["eth_mdio_temp1"], 45.0);
    }

    #[test]
    fn absent_driver_never_uses_an_unrelated_chip() {
        let sysfs = Sysfs::new();
        let other = sysfs.chip("hwmon2", "unrelated");
        fs::write(other.join("temp1_input"), "120000").unwrap();
        fs::create_dir(sysfs.0.join("hwmon0")).unwrap(); // missing name
        let values = sysfs.collector().sample();
        assert_eq!(values.len(), 17);
        assert!(values.values().all(|v| v.is_nan()));
    }

    #[test]
    fn read_failure_is_unavailable_and_recovers_on_next_sample() {
        let sysfs = Sysfs::new();
        let dir = sysfs.chip("hwmon5", MODALIX_HWMON_NAME);
        let input = dir.join("temp1_input");
        let mut collector = sysfs.collector();
        for invalid in ["", "timeout", "NaN", "inf", "40000.5"] {
            fs::write(&input, invalid).unwrap();
            assert!(collector.sample()["rtsn_0"].is_nan());
        }
        fs::remove_file(&input).unwrap();
        assert!(collector.sample()["rtsn_0"].is_nan());
        fs::write(&input, "-12500\n").unwrap();
        assert_eq!(collector.sample()["rtsn_0"], -12.5);
        fs::write(&input, "120000\n").unwrap();
        assert_eq!(collector.sample()["rtsn_0"], 120.0); // Do not suppress hot readings.
    }

    #[test]
    fn rediscovers_late_and_rebound_driver() {
        let sysfs = Sysfs::new();
        let mut collector = sysfs.collector();
        assert!(collector.sample()["rtsn_0"].is_nan());
        let old = sysfs.chip("hwmon1", MODALIX_HWMON_NAME);
        fs::write(old.join("temp1_input"), "41000").unwrap();
        assert_eq!(collector.sample()["rtsn_0"], 41.0);
        fs::remove_dir_all(&old).unwrap();
        let other = sysfs.chip("hwmon1", "unrelated");
        fs::write(other.join("temp1_input"), "120000").unwrap();
        let new = sysfs.chip("hwmon8", MODALIX_HWMON_NAME);
        fs::write(new.join("temp1_input"), "42000").unwrap();
        assert_eq!(collector.sample()["rtsn_0"], 42.0);
    }
}
