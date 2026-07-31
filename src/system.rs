use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::model::{MetricDefinition, ProcessInfo};

const PROCESS_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

pub fn system_metric_definitions() -> Vec<MetricDefinition> {
    let mut out = vec![
        MetricDefinition::new(
            "cpu_usage_pct",
            "CPU usage",
            "CPU%",
            "CPU",
            "%",
            "Aggregate Linux CPU utilization.",
            Some(80.0),
            Some(95.0),
        ),
        MetricDefinition::new(
            "cpu_load_1",
            "CPU load 1m",
            "Load1",
            "CPU",
            "load",
            "Linux 1-minute load average.",
            None,
            None,
        ),
        MetricDefinition::new(
            "cpu_load_1_pct",
            "CPU load 1m percentage",
            "Load%",
            "CPU",
            "%",
            "Linux 1-minute load average normalized by logical CPU core count.",
            Some(80.0),
            Some(95.0),
        ),
        MetricDefinition::new(
            "linux_mem_used_pct",
            "Linux memory used",
            "Mem%",
            "Memory",
            "%",
            "Linux memory usage based on MemTotal and MemAvailable.",
            Some(80.0),
            Some(90.0),
        ),
        MetricDefinition::new(
            "linux_mem_used_mb",
            "Linux memory used MB",
            "MemMB",
            "Memory",
            "MB",
            "Linux memory in active use.",
            None,
            None,
        ),
        MetricDefinition::new(
            "mla_mem_allocated_mb",
            "MLA memory allocated",
            "MLAMB",
            "MLA",
            "MB",
            "MLA allocator total allocated size parsed from /dev/simaai-mem.",
            None,
            None,
        ),
        MetricDefinition::new(
            "ev74_cma_total_mb",
            "EV74 CMA reserved",
            "CMATotal",
            "EV74",
            "MB",
            "Boot-time contiguous memory reservation reported by CmaTotal in /proc/meminfo.",
            None,
            None,
        ),
        MetricDefinition::new(
            "ev74_cma_free_mb",
            "EV74 CMA free",
            "CMAFree",
            "EV74",
            "MB",
            "Currently free contiguous memory reported by CmaFree in /proc/meminfo.",
            None,
            None,
        ),
        MetricDefinition::new(
            "ev74_cma_used_mb",
            "EV74 CMA used",
            "CMAUsed",
            "EV74",
            "MB",
            "CmaTotal minus CmaFree; a proxy for EV74 memory consumption when EV74 owns the CMA pool.",
            None,
            None,
        ),
        MetricDefinition::new(
            "net_rx_mbps",
            "Network RX",
            "NetRX",
            "Network",
            "MB/s",
            "Aggregate non-loopback network receive rate.",
            None,
            None,
        ),
        MetricDefinition::new(
            "net_tx_mbps",
            "Network TX",
            "NetTX",
            "Network",
            "MB/s",
            "Aggregate non-loopback network transmit rate.",
            None,
            None,
        ),
    ];
    let core_count = read_cpu_core_times().len();
    for idx in 0..core_count {
        out.push(MetricDefinition::new(
            &format!("cpu_core_{idx}_usage_pct"),
            &format!("CPU core {idx} usage"),
            &format!("c{idx}"),
            "CPU",
            "%",
            &format!("Linux CPU core {idx} utilization."),
            Some(80.0),
            Some(95.0),
        ));
    }
    out.extend(disk_metric_definitions("emmc", "eMMC"));
    if Path::new("/media/nvme").is_mount() {
        out.extend(disk_metric_definitions("nvme", "NVMe"));
    }
    out
}

fn disk_metric_definitions(prefix: &str, label: &str) -> Vec<MetricDefinition> {
    vec![
        MetricDefinition::new(
            &format!("disk_{prefix}_used_pct"),
            &format!("{label} disk used"),
            &format!("{label}%"),
            "Disk",
            "%",
            &format!("{label} filesystem usage."),
            Some(80.0),
            Some(90.0),
        ),
        MetricDefinition::new(
            &format!("disk_{prefix}_used_mb"),
            &format!("{label} disk used MB"),
            &format!("{label}MB"),
            "Disk",
            "MB",
            &format!("{label} used space."),
            None,
            None,
        ),
        MetricDefinition::new(
            &format!("disk_{prefix}_read_mbps"),
            &format!("{label} read"),
            &format!("{label}R"),
            "DiskIO",
            "MB/s",
            &format!("{label} block-device read rate."),
            None,
            None,
        ),
        MetricDefinition::new(
            &format!("disk_{prefix}_write_mbps"),
            &format!("{label} write"),
            &format!("{label}W"),
            "DiskIO",
            "MB/s",
            &format!("{label} block-device write rate."),
            None,
            None,
        ),
    ]
}

pub struct SystemCollector {
    last_time: Option<Instant>,
    last_cpu: Option<(u64, u64)>,
    last_cpu_cores: Vec<(u64, u64)>,
    last_diskstats: BTreeMap<String, (u64, u64)>,
    last_netdev: BTreeMap<String, (u64, u64)>,
    last_process_time: Option<Instant>,
    last_proc_times: BTreeMap<u32, u64>,
    processes: Vec<ProcessInfo>,
    process_interval: Duration,
    clock_ticks: f64,
    page_size: f64,
}

impl SystemCollector {
    pub fn new() -> Self {
        Self {
            last_time: None,
            last_cpu: None,
            last_cpu_cores: Vec::new(),
            last_diskstats: BTreeMap::new(),
            last_netdev: BTreeMap::new(),
            last_process_time: None,
            last_proc_times: BTreeMap::new(),
            processes: Vec::new(),
            process_interval: PROCESS_SAMPLE_INTERVAL,
            clock_ticks: clock_ticks_per_second(),
            page_size: page_size_bytes(),
        }
    }

    pub fn sample(&mut self) -> BTreeMap<String, f64> {
        let now = Instant::now();
        let dt = self
            .last_time
            .map(|last| (now - last).as_secs_f64().max(0.001))
            .unwrap_or(1.0);
        self.last_time = Some(now);

        let mut values = BTreeMap::new();
        values.extend(self.sample_cpu());
        values.extend(sample_memory());
        values.insert("mla_mem_allocated_mb".into(), read_mla_memory_mb());
        values.extend(self.sample_disks(dt));
        values.extend(self.sample_network(dt));
        if self
            .last_process_time
            .is_none_or(|last| now.duration_since(last) >= self.process_interval)
        {
            self.processes = self.sample_processes(now);
        }
        values
    }

    pub fn processes(&self) -> Vec<ProcessInfo> {
        self.processes.clone()
    }

    fn sample_cpu(&mut self) -> BTreeMap<String, f64> {
        let mut values = BTreeMap::new();
        let load1 = read_loadavg().unwrap_or(f64::NAN);
        let core_count = read_cpu_core_times().len().max(1) as f64;
        values.insert("cpu_load_1".into(), load1);
        values.insert(
            "cpu_load_1_pct".into(),
            (load1 / core_count * 100.0).max(0.0),
        );
        let current = read_cpu_times();
        let mut usage = f64::NAN;
        if let (Some(prev), Some(cur)) = (self.last_cpu, current) {
            let total_delta = cur.0.saturating_sub(prev.0);
            let idle_delta = cur.1.saturating_sub(prev.1);
            if total_delta > 0 {
                usage = (1.0 - idle_delta as f64 / total_delta as f64) * 100.0;
            }
        }
        self.last_cpu = current;
        values.insert("cpu_usage_pct".into(), usage.clamp(0.0, 100.0));

        let current_cores = read_cpu_core_times();
        for (idx, cur) in current_cores.iter().copied().enumerate() {
            let mut core_usage = f64::NAN;
            if let Some(prev) = self.last_cpu_cores.get(idx).copied() {
                let total_delta = cur.0.saturating_sub(prev.0);
                let idle_delta = cur.1.saturating_sub(prev.1);
                if total_delta > 0 {
                    core_usage = (1.0 - idle_delta as f64 / total_delta as f64) * 100.0;
                }
            }
            values.insert(
                format!("cpu_core_{idx}_usage_pct"),
                core_usage.clamp(0.0, 100.0),
            );
        }
        self.last_cpu_cores = current_cores;
        values
    }

    fn sample_disks(&mut self, dt: f64) -> BTreeMap<String, f64> {
        let mut values = BTreeMap::new();
        let mounts = read_mounts();
        let diskstats = read_diskstats();
        for (prefix, mount_path) in [("emmc", "/"), ("nvme", "/media/nvme")] {
            let Some((used_pct, used_mb)) = disk_usage(mount_path) else {
                continue;
            };
            values.insert(format!("disk_{prefix}_used_pct"), used_pct);
            values.insert(format!("disk_{prefix}_used_mb"), used_mb);
            let mut device = mounts.get(mount_path).and_then(|d| block_parent(d));
            if device.as_ref().is_none_or(|d| !diskstats.contains_key(d)) {
                device = fallback_block_device(prefix, &diskstats);
            }
            let current = device.as_ref().and_then(|d| diskstats.get(d)).copied();
            let previous = device
                .as_ref()
                .and_then(|d| self.last_diskstats.get(d))
                .copied();
            if let (Some(cur), Some(prev)) = (current, previous) {
                values.insert(
                    format!("disk_{prefix}_read_mbps"),
                    cur.0.saturating_sub(prev.0) as f64 / dt / 1024.0 / 1024.0,
                );
                values.insert(
                    format!("disk_{prefix}_write_mbps"),
                    cur.1.saturating_sub(prev.1) as f64 / dt / 1024.0 / 1024.0,
                );
            } else {
                values.insert(format!("disk_{prefix}_read_mbps"), 0.0);
                values.insert(format!("disk_{prefix}_write_mbps"), 0.0);
            }
        }
        self.last_diskstats = diskstats;
        values
    }

    fn sample_network(&mut self, dt: f64) -> BTreeMap<String, f64> {
        let current = read_netdev();
        let mut rx = 0.0;
        let mut tx = 0.0;
        for (name, counters) in &current {
            if let Some(prev) = self.last_netdev.get(name) {
                rx += counters.0.saturating_sub(prev.0) as f64 / dt;
                tx += counters.1.saturating_sub(prev.1) as f64 / dt;
            }
        }
        self.last_netdev = current;
        BTreeMap::from([
            ("net_rx_mbps".into(), rx / 1024.0 / 1024.0),
            ("net_tx_mbps".into(), tx / 1024.0 / 1024.0),
        ])
    }

    fn sample_processes(&mut self, now: Instant) -> Vec<ProcessInfo> {
        let dt = self
            .last_process_time
            .map(|last| now.duration_since(last).as_secs_f64().max(0.001))
            .unwrap_or_else(|| self.process_interval.as_secs_f64().max(0.001));
        self.last_process_time = Some(now);
        let snapshots = read_process_stats();
        let mut next_times = BTreeMap::new();
        let mut processes = Vec::new();
        for proc_ in snapshots {
            next_times.insert(proc_.pid, proc_.cpu_ticks);
            let prev = self.last_proc_times.get(&proc_.pid).copied();
            let cpu_pct = prev
                .map(|prev| {
                    proc_.cpu_ticks.saturating_sub(prev) as f64 / self.clock_ticks / dt * 100.0
                })
                .unwrap_or(0.0)
                .clamp(0.0, 1000.0);
            processes.push(ProcessInfo {
                pid: proc_.pid,
                name: proc_.name,
                cpu_pct,
                rss_mb: proc_.rss_pages as f64 * self.page_size / 1024.0 / 1024.0,
                cpu_core: proc_.cpu_core,
            });
        }
        self.last_proc_times = next_times;
        processes.sort_by(|a, b| {
            b.cpu_pct
                .partial_cmp(&a.cpu_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.rss_mb
                        .partial_cmp(&a.rss_mb)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        processes.truncate(64);
        processes
    }
}

struct ProcStat {
    pid: u32,
    name: String,
    cpu_ticks: u64,
    rss_pages: i64,
    cpu_core: Option<usize>,
}

fn read_process_stats() -> Vec<ProcStat> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(pid_text) = file_name.to_str() else {
            continue;
        };
        let Ok(pid) = pid_text.parse::<u32>() else {
            continue;
        };
        let Ok(raw) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        if let Some(proc_) = parse_proc_stat(pid, &raw) {
            out.push(proc_);
        }
    }
    out
}

fn parse_proc_stat(pid: u32, raw: &str) -> Option<ProcStat> {
    let open = raw.find('(')?;
    let close = raw.rfind(')')?;
    let name = raw[open + 1..close].to_string();
    let fields: Vec<&str> = raw[close + 2..].split_whitespace().collect();
    let utime = fields.get(11)?.parse::<u64>().ok()?;
    let stime = fields.get(12)?.parse::<u64>().ok()?;
    let rss_pages = fields.get(21)?.parse::<i64>().ok()?;
    let cpu_core = fields.get(36).and_then(|field| field.parse::<usize>().ok());
    Some(ProcStat {
        pid,
        name,
        cpu_ticks: utime.saturating_add(stime),
        rss_pages,
        cpu_core,
    })
}

fn clock_ticks_per_second() -> f64 {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 {
        ticks as f64
    } else {
        100.0
    }
}

fn page_size_bytes() -> f64 {
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size > 0 {
        size as f64
    } else {
        4096.0
    }
}

fn read_loadavg() -> Option<f64> {
    fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn read_cpu_times() -> Option<(u64, u64)> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    let fields: Vec<u64> = stat
        .lines()
        .next()?
        .split_whitespace()
        .skip(1)
        .filter_map(|v| v.parse().ok())
        .collect();
    if fields.len() < 4 {
        return None;
    }
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    let total = fields.iter().sum();
    Some((total, idle))
}

fn read_cpu_core_times() -> Vec<(u64, u64)> {
    let Ok(stat) = fs::read_to_string("/proc/stat") else {
        return Vec::new();
    };
    stat.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?;
            if name == "cpu" || !name.starts_with("cpu") {
                return None;
            }
            if !name[3..].chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let fields: Vec<u64> = parts.filter_map(|v| v.parse().ok()).collect();
            if fields.len() < 4 {
                return None;
            }
            let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
            let total = fields.iter().sum();
            Some((total, idle))
        })
        .collect()
}

fn sample_memory() -> BTreeMap<String, f64> {
    let meminfo = read_meminfo();
    let total = *meminfo.get("MemTotal").unwrap_or(&0) as f64;
    let available = *meminfo.get("MemAvailable").unwrap_or(&0) as f64;
    let used = (total - available).max(0.0);
    let cma_total = meminfo.get("CmaTotal").copied().map(|value| value as f64);
    let cma_free = meminfo.get("CmaFree").copied().map(|value| value as f64);
    let cma_used = cma_total
        .zip(cma_free)
        .map(|(total, free)| (total - free).max(0.0));
    BTreeMap::from([
        (
            "linux_mem_used_pct".into(),
            if total > 0.0 {
                used / total * 100.0
            } else {
                f64::NAN
            },
        ),
        ("linux_mem_used_mb".into(), used / 1024.0 / 1024.0),
        (
            "ev74_cma_total_mb".into(),
            cma_total.unwrap_or(f64::NAN) / 1024.0 / 1024.0,
        ),
        (
            "ev74_cma_free_mb".into(),
            cma_free.unwrap_or(f64::NAN) / 1024.0 / 1024.0,
        ),
        (
            "ev74_cma_used_mb".into(),
            cma_used.unwrap_or(f64::NAN) / 1024.0 / 1024.0,
        ),
    ])
}

fn read_meminfo() -> BTreeMap<String, u64> {
    fs::read_to_string("/proc/meminfo")
        .map(|raw| parse_meminfo(&raw))
        .unwrap_or_default()
}

fn parse_meminfo(raw: &str) -> BTreeMap<String, u64> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let key = parts.next()?.trim_end_matches(':');
            let kib = parts.next()?.parse::<u64>().ok()?;
            Some((key.into(), kib * 1024))
        })
        .collect()
}

fn read_mla_memory_mb() -> f64 {
    let Ok(raw) = fs::read_to_string("/dev/simaai-mem") else {
        return f64::NAN;
    };
    let marker = "Total allocated size:";
    let Some(rest) = raw.split(marker).nth(1) else {
        return f64::NAN;
    };
    let token = rest.split_whitespace().next().unwrap_or("");
    let hex = token.trim_start_matches("0x");
    u64::from_str_radix(hex, 16)
        .map(|bytes| bytes as f64 / 1024.0 / 1024.0)
        .unwrap_or(f64::NAN)
}

fn read_mounts() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Ok(raw) = fs::read_to_string("/proc/mounts") {
        for line in raw.lines() {
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                out.insert(parts[1].to_string(), parts[0].to_string());
            }
        }
    }
    out
}

fn block_parent(device_path: &str) -> Option<String> {
    let name = Path::new(device_path).file_name()?.to_str()?;
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        return Some(trim_partition_suffix(name));
    }
    Some(
        name.trim_end_matches(|c: char| c.is_ascii_digit())
            .to_string(),
    )
}

fn trim_partition_suffix(name: &str) -> String {
    if let Some((base, _)) = name.rsplit_once('p') {
        if name
            .rsplit_once('p')
            .unwrap()
            .1
            .chars()
            .all(|c| c.is_ascii_digit())
        {
            return base.to_string();
        }
    }
    name.to_string()
}

fn fallback_block_device(prefix: &str, diskstats: &BTreeMap<String, (u64, u64)>) -> Option<String> {
    diskstats.keys().find_map(|name| {
        let ok = match prefix {
            "emmc" => name.starts_with("mmcblk") && !name.contains('p'),
            "nvme" => name.starts_with("nvme") && !name.contains('p'),
            _ => false,
        };
        ok.then(|| name.clone())
    })
}

fn read_diskstats() -> BTreeMap<String, (u64, u64)> {
    let mut out = BTreeMap::new();
    if let Ok(raw) = fs::read_to_string("/proc/diskstats") {
        for line in raw.lines() {
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.len() < 10 {
                continue;
            }
            let name = parts[2];
            let sectors_read = parts[5].parse::<u64>().unwrap_or(0);
            let sectors_written = parts[9].parse::<u64>().unwrap_or(0);
            let sector_size = read_sector_size(name);
            out.insert(
                name.into(),
                (sectors_read * sector_size, sectors_written * sector_size),
            );
        }
    }
    out
}

fn read_sector_size(name: &str) -> u64 {
    fs::read_to_string(format!("/sys/block/{name}/queue/hw_sector_size"))
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(512)
}

fn disk_usage(path: &str) -> Option<(f64, f64)> {
    if !Path::new(path).is_mount() {
        return None;
    }
    let c_path = std::ffi::CString::new(path).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    let total = stat.f_blocks as f64 * stat.f_frsize as f64;
    let available = stat.f_bavail as f64 * stat.f_frsize as f64;
    let used = (total - available).max(0.0);
    let pct = if total > 0.0 {
        used / total * 100.0
    } else {
        f64::NAN
    };
    Some((pct, used / 1024.0 / 1024.0))
}

fn read_netdev() -> BTreeMap<String, (u64, u64)> {
    let mut out = BTreeMap::new();
    if let Ok(raw) = fs::read_to_string("/proc/net/dev") {
        for line in raw.lines().skip(2) {
            let Some((name, data)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim();
            if name == "lo" {
                continue;
            }
            let fields: Vec<_> = data.split_whitespace().collect();
            if fields.len() >= 16 {
                let rx = fields[0].parse().unwrap_or(0);
                let tx = fields[8].parse().unwrap_or(0);
                out.insert(name.into(), (rx, tx));
            }
        }
    }
    out
}

trait MountCheck {
    fn is_mount(&self) -> bool;
}

impl MountCheck for Path {
    fn is_mount(&self) -> bool {
        let Ok(meta) = fs::metadata(self) else {
            return false;
        };
        let Some(parent) = self.parent() else {
            return true;
        };
        let Ok(parent_meta) = fs::metadata(parent) else {
            return false;
        };
        use std::os::unix::fs::MetadataExt;
        meta.dev() != parent_meta.dev() || meta.ino() == parent_meta.ino()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_meminfo;

    #[test]
    fn parses_cma_meminfo_fields_as_bytes() {
        let values = parse_meminfo(
            "MemTotal:       8192000 kB\n\
             MemAvailable:   4096000 kB\n\
             CmaTotal:       1048576 kB\n\
             CmaFree:         786432 kB\n",
        );

        assert_eq!(values["CmaTotal"], 1_073_741_824);
        assert_eq!(values["CmaFree"], 805_306_368);
    }

    #[test]
    fn ignores_malformed_meminfo_fields() {
        let values = parse_meminfo("CmaTotal: unavailable kB\nMalformed\nCmaFree: 42 kB\n");

        assert!(!values.contains_key("CmaTotal"));
        assert_eq!(values["CmaFree"], 43_008);
    }
}
