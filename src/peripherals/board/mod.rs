//! The catalog's `board` block: read-only facts about how the board is set up
//! for MIPI cameras. The model and the configured cameras come from the live
//! device tree the kernel booted with, the overlay list from the U-Boot
//! environment, and the sensors each shipped overlay can configure from the
//! overlay files themselves. Like the providers, Sentinel itself writes
//! nothing and encodes no policy; the only program it runs is the platform's
//! read tool, `fw_printenv`, which may take its own lock file. Resolutions
//! still come from the ISP, not from overlays.

mod dt;
#[cfg(test)]
pub(crate) mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use super::camera::Source;
use super::sysutil::{
    bounded_string, disappeared, errno_of, io_error, os_message, trim_c_space,
    CODE_DISCOVERY_FAILED, CODE_PERMISSION_DENIED,
};
use super::{CatalogError, Peripheral, ProviderError};

/// The block's errors name the field they leave incomplete:
/// `board.<field>`, e.g. `board.overlays`.
pub const PROVIDER_NAME: &str = "board";

/// `fw_printenv` gets this long before it is killed.
const FW_PRINTENV_TIMEOUT: Duration = Duration::from_secs(2);
const FW_PRINTENV_POLL: Duration = Duration::from_millis(10);
/// A killed `fw_printenv` that has not exited after this long (stuck in
/// uninterruptible I/O) is left to a reaper thread.
const FW_PRINTENV_KILL_GRACE: Duration = Duration::from_millis(500);
/// `fw_printenv` output beyond this is not read.
const MAX_COMMAND_OUTPUT: usize = 64 * 1024;
/// `fw_printenv` error output beyond this is not read.
const MAX_COMMAND_ERRORS: usize = 4 * 1024;
/// I2C devices examined for configured cameras.
const MAX_I2C_DEVICES: usize = 1024;
/// Entries listed in `/boot` and in each of its directories.
const MAX_BOOT_ENTRIES: usize = 4096;
/// Overlay files read; the rest are reported as skipped.
const MAX_OVERLAY_FILES: usize = 512;
/// Larger overlay files are skipped.
const MAX_OVERLAY_BYTES: u64 = 1 << 20;

/// How the board is set up for cameras. Each fact that cannot be read is left
/// out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    /// The device tree's `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The `.dtbo` entries of the U-Boot `dtbos` variable, in order: every
    /// overlay U-Boot applies, whatever it configures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlays: Option<Vec<String>>,
    /// I2C devices in the live device tree that are MIPI CSI-2 sources.
    pub configured_cameras: Vec<ConfiguredCamera>,
    /// The sensors the overlay files under `/boot` can configure.
    pub supported_sensors: Vec<SupportedSensor>,
}

/// A camera sensor the booted device tree describes on an I2C bus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfiguredCamera {
    /// The first `compatible` string, e.g. `sony,imx477`.
    pub compatible: String,
    /// The node's path from the device-tree root.
    pub dt_node: String,
    /// The I2C device, `<bus>-<address>`, e.g. `5-001a`.
    pub i2c_device: String,
    /// Cells in the sensor endpoint's `data-lanes`.
    pub data_lanes: u32,
    /// The catalog camera this sensor is; absent when the catalog has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_id: Option<String>,
}

/// A sensor and the overlay files that configure it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportedSensor {
    pub compatible: String,
    pub overlays: Vec<String>,
}

/// Reads the [`Board`] block. Owned by the peripherals worker, which reads it
/// in every scan after the providers, so `camera_id` matches the devices of
/// the same catalog.
pub struct BoardProbe {
    sys_root: PathBuf,
    boot_root: PathBuf,
    /// The program and arguments that print the overlay list.
    overlay_command: Vec<String>,
    timeout: Duration,
    /// The overlay list of the last run that succeeded, published while
    /// later runs fail, as a failed provider's devices are.
    last_overlay_list: Option<Vec<String>>,
    /// Parsed overlays by path, reused while their [`Stamp`] matches.
    overlays: HashMap<PathBuf, CachedOverlay>,
}

/// Size, mtime, ctime and inode: a rewrite that restores the size and mtime
/// still changes the ctime, and a replacement changes the inode.
type Stamp = (u64, Option<SystemTime>, (i64, i64), u64);

/// What the bytes of one overlay file gave: its sensors, or why it is
/// malformed. I/O errors are not cached.
struct CachedOverlay {
    stamp: Stamp,
    sensors: Result<Vec<String>, ProviderError>,
}

impl BoardProbe {
    /// Read the live system (`/sys`, `/boot`, `fw_printenv`).
    pub fn new() -> Self {
        Self::with_roots("/sys", "/boot")
    }

    /// Read a sysfs tree and a boot directory rooted elsewhere.
    pub fn with_roots(sys_root: impl Into<PathBuf>, boot_root: impl Into<PathBuf>) -> Self {
        Self {
            sys_root: sys_root.into(),
            boot_root: boot_root.into(),
            overlay_command: ["fw_printenv", "-n", "dtbos"].map(String::from).to_vec(),
            timeout: FW_PRINTENV_TIMEOUT,
            last_overlay_list: None,
            overlays: HashMap::new(),
        }
    }

    /// The block for one scan, with an error for each part that could not
    /// be read completely. `devices` are the scan's devices.
    pub fn scan(&mut self, devices: &[Peripheral]) -> (Board, Vec<CatalogError>) {
        let mut errors = Vec::new();
        let mut report = |field: &str, error: ProviderError| {
            errors.push(CatalogError {
                provider: format!("{PROVIDER_NAME}.{field}"),
                code: error.code,
                reason: error.reason,
            })
        };
        let base = self.sys_root.join("firmware/devicetree/base");
        let model = read_model(&base).unwrap_or_else(|error| {
            report("model", error);
            None
        });
        // A failed run (a timeout, e.g. while fw_setenv holds the lock)
        // keeps the last list, so the field does not flicker.
        let overlays = match overlay_list(&self.overlay_command, self.timeout) {
            Ok(list) => {
                self.last_overlay_list.clone_from(&list);
                list
            }
            Err(error) => {
                report("overlays", error);
                self.last_overlay_list.clone()
            }
        };
        let (configured_cameras, configured_error) =
            configured_cameras(&self.sys_root, &base, devices);
        if let Some(error) = configured_error {
            report("configured_cameras", error);
        }
        let (supported_sensors, overlay_error) = self.supported_sensors();
        if let Some(error) = overlay_error {
            report("supported_sensors", error);
        }
        let board = Board {
            model,
            overlays,
            configured_cameras,
            supported_sensors,
        };
        (board, errors)
    }

    /// Sensors by `compatible`, from every `*.dtbo` one level under the boot
    /// directory (`/boot/boot-0/`, `/boot/boot-1/`). A file name in several
    /// directories (A/B slots) is listed once.
    fn supported_sensors(&mut self) -> (Vec<SupportedSensor>, Option<ProviderError>) {
        let mut skipped = Skipped::default();
        let files = overlay_files(&self.boot_root, &mut skipped);
        let mut cache = HashMap::new();
        let mut sensors: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (path, file_name) in files {
            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if disappeared(errno_of(&error)) => continue,
                Err(error) => {
                    skipped.add(io_error("failed to read", &path, &error, false));
                    continue;
                }
            };
            let stamp = (
                metadata.len(),
                metadata.modified().ok(),
                (metadata.ctime(), metadata.ctime_nsec()),
                metadata.ino(),
            );
            let overlay = match self.overlays.remove(&path) {
                Some(cached) if cached.stamp == stamp => cached,
                _ => match parse_overlay(&path, metadata.len()) {
                    Ok(sensors) => CachedOverlay { stamp, sensors },
                    Err(error) => {
                        skipped.add(error);
                        continue;
                    }
                },
            };
            match overlay.sensors {
                Ok(ref found) => {
                    for compatible in found {
                        let overlays = sensors.entry(compatible.clone()).or_default();
                        overlays.insert(file_name.clone());
                    }
                }
                Err(ref error) => skipped.add(error.clone()),
            }
            cache.insert(path, overlay);
        }
        self.overlays = cache;
        let sensors = sensors
            .into_iter()
            .map(|(compatible, overlays)| SupportedSensor {
                compatible,
                overlays: overlays.into_iter().collect(),
            });
        (sensors.collect(), skipped.into_error("overlay files"))
    }
}

/// The first problem of a kind and how many there were, reported as one
/// error.
#[derive(Default)]
struct Skipped {
    first: Option<ProviderError>,
    count: usize,
}

impl Skipped {
    fn add(&mut self, error: ProviderError) {
        self.count += 1;
        self.first.get_or_insert(error);
    }

    fn into_error(self, what: &str) -> Option<ProviderError> {
        let mut error = self.first?;
        if self.count > 1 {
            let more = self.count - 1;
            error.reason = format!("{} (and {more} more {what} skipped)", error.reason);
        }
        Some(error)
    }
}

/// The device tree's `model`, NUL-terminated; `None` without a device tree.
fn read_model(base: &Path) -> Result<Option<String>, ProviderError> {
    let path = base.join("model");
    match dt::read_bounded(&path, dt::MAX_PROPERTY_BYTES) {
        Ok(bytes) => {
            let model = trim_c_space(&bounded_string(&bytes)).to_string();
            Ok((!model.is_empty()).then_some(model))
        }
        Err(error) if disappeared(errno_of(&error)) || errno_of(&error) == libc::ENOTDIR => {
            Ok(None)
        }
        Err(error) => Err(io_error("failed to read", &path, &error, false)),
    }
}

/// The `.dtbo` entries that `command` (`fw_printenv -n dtbos`) prints, split
/// on whitespace. `None` when the program is missing, or exits unsuccessfully
/// saying the variable is `not defined`, as u-boot-tools does when it is not
/// set. An error, and `None`, when it exits unsuccessfully for another reason,
/// cannot be started, or does not finish within `timeout`; it is then killed.
fn overlay_list(
    command: &[String],
    timeout: Duration,
) -> Result<Option<Vec<String>>, ProviderError> {
    let Some((program, arguments)) = command.split_first() else {
        return Ok(None);
    };
    let spawned = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            let code = match errno_of(&error) {
                libc::EACCES | libc::EPERM => CODE_PERMISSION_DENIED,
                _ => CODE_DISCOVERY_FAILED,
            };
            let reason = format!("failed to run {program}: {}", os_message(&error));
            return Err(ProviderError::new(code, reason));
        }
    };
    // Read both pipes while waiting: a child that fills a pipe would block
    // until the timeout otherwise. Output past each limit is read and dropped.
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    // A pipe left blocking would hold `drain` past the timeout.
    let nonblocking = set_nonblocking(stdout.as_ref()).and(set_nonblocking(stderr.as_ref()));
    if let Err(error) = nonblocking {
        stop(child, FW_PRINTENV_KILL_GRACE);
        let reason = format!("failed to read from {program}: {}", os_message(&error));
        return Err(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
    }
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let deadline = Instant::now() + timeout;
    let status = loop {
        drain(&mut stdout, &mut output, MAX_COMMAND_OUTPUT);
        drain(&mut stderr, &mut errors, MAX_COMMAND_ERRORS);
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(FW_PRINTENV_POLL),
            outcome => {
                stop(child, FW_PRINTENV_KILL_GRACE);
                let reason = match outcome {
                    Err(error) => format!("failed to wait for {program}: {}", os_message(&error)),
                    _ => format!("{program} did not finish within {timeout:?} and was killed"),
                };
                return Err(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
            }
        }
    };
    drain(&mut stdout, &mut output, MAX_COMMAND_OUTPUT);
    drain(&mut stderr, &mut errors, MAX_COMMAND_ERRORS);
    if !status.success() {
        let errors = String::from_utf8_lossy(&errors);
        if errors.contains("not defined") {
            return Ok(None);
        }
        let mut reason = format!("{program} failed ({status})");
        if let Some(line) = errors.lines().map(str::trim).find(|line| !line.is_empty()) {
            reason = format!("{reason}: {line}");
        }
        return Err(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
    }
    let entries = String::from_utf8_lossy(&output)
        .split_ascii_whitespace()
        .filter(|entry| entry.ends_with(".dtbo"))
        .map(str::to_owned)
        .collect();
    Ok(Some(entries))
}

/// Kills `child` and waits at most `grace` for it to exit. A child that is
/// still there (SIGKILL waits for uninterruptible I/O to return) is handed to
/// a thread that reaps it whenever it exits, so the caller never blocks on it.
fn stop(mut child: Child, grace: Duration) {
    let _ = child.kill();
    reap_within(child, grace);
}

/// Waits at most `grace` for `child` to exit, then leaves it to a reaper
/// thread. Returns whether it was reaped here.
fn reap_within(mut child: Child, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => thread::sleep(FW_PRINTENV_POLL),
            Ok(None) => break,
            // Already reaped, or not ours to wait for.
            Err(_) => return true,
        }
    }
    let reaper = thread::Builder::new().name("fw_printenv-reaper".into());
    // Without a thread the child stays a zombie; nothing else can be done.
    let _ = reaper.spawn(move || child.wait());
    false
}

/// Makes `pipe` non-blocking, so `drain` returns when it is empty.
fn set_nonblocking(pipe: Option<&impl AsRawFd>) -> io::Result<()> {
    let Some(pipe) = pipe else {
        return Ok(());
    };
    let fd = pipe.as_raw_fd();
    // SAFETY: fcntl on a descriptor owned by `pipe`, which outlives the calls.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Reads what `pipe` holds now into `output`, keeping at most `limit` bytes
/// and dropping the rest; at end of file or on an error the pipe is closed.
fn drain(pipe: &mut Option<impl Read>, output: &mut Vec<u8>, limit: usize) {
    let Some(reader) = pipe else {
        return;
    };
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break *pipe = None,
            Ok(read) => {
                let keep = read.min(limit.saturating_sub(output.len()));
                output.extend_from_slice(&chunk[..keep]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => break *pipe = None,
        }
    }
}

/// `<bus>-<4 hex digits>`, the kernel's name for an I2C client; adapters are
/// `i2c-<bus>`.
fn is_i2c_client(name: &str) -> bool {
    name.split_once('-').is_some_and(|(bus, address)| {
        !bus.is_empty()
            && bus.bytes().all(|byte| byte.is_ascii_digit())
            && address.len() == 4
            && address.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

/// Every I2C client whose `of_node` is a MIPI CSI-2 source in the live device
/// tree under `base`, sorted by `dt_node`. `camera_id` names the MIPI camera
/// whose sensor entity name has `<i2c_device>` as a space-separated word, as
/// V4L2 names an I2C sub-device `<driver> <bus>-<address>`, sometimes with a
/// suffix (`ccs 5-0010 pixel_array`).
fn configured_cameras(
    sys_root: &Path,
    base: &Path,
    devices: &[Peripheral],
) -> (Vec<ConfiguredCamera>, Option<ProviderError>) {
    let directory = sys_root.join("bus/i2c/devices");
    let (names, more) = match dt::listed(&directory, MAX_I2C_DEVICES) {
        Ok(listing) => listing,
        Err(error) if disappeared(errno_of(&error)) => return (Vec::new(), None),
        Err(error) => {
            let error = io_error("failed to read", &directory, &error, false);
            return (Vec::new(), Some(error));
        }
    };
    // Without a live device tree no device has a node in it.
    let Ok(base) = fs::canonicalize(base) else {
        return (Vec::new(), None);
    };
    let mut skipped = Skipped::default();
    if more {
        let reason = dt::too_many(MAX_I2C_DEVICES, "I2C devices", &directory);
        skipped.add(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
    }
    let mut cameras = Vec::new();
    for name in names.into_iter().filter(|name| is_i2c_client(name)) {
        let device = directory.join(&name);
        // Devices not described by the device tree have no of_node, and a
        // device that went away mid-scan has none either; any other failure
        // is reported, so a partial list is not mistaken for a complete one.
        let link = device.join("of_node");
        let node = match fs::canonicalize(&link) {
            Ok(node) => node,
            Err(error) if disappeared(errno_of(&error)) => continue,
            Err(error) => {
                skipped.add(io_error("failed to resolve", &link, &error, false));
                continue;
            }
        };
        let Ok(relative) = node.strip_prefix(&base) else {
            continue;
        };
        // A subtree cut at a bound is still used, and reported.
        let tree = match dt::read_live(&node) {
            Ok((tree, truncated)) => {
                if let Some(reason) = truncated {
                    skipped.add(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
                }
                tree
            }
            Err(error) if disappeared(errno_of(&error)) => continue,
            Err(error) => {
                skipped.add(io_error("failed to read", &node, &error, false));
                continue;
            }
        };
        let sources = dt::csi2_sources(&tree);
        let Some(&(_, data_lanes)) = sources.iter().find(|&&(index, _)| index == 0) else {
            continue;
        };
        let Some(compatible) = tree.compatible(0) else {
            continue;
        };
        let camera_id = devices.iter().find_map(|device| match *device {
            Peripheral::Camera(ref camera) => match camera.source {
                Source::Mipi(ref mipi) if mipi.camera_name.split(' ').any(|word| word == name) => {
                    Some(camera.id.clone())
                }
                _ => None,
            },
            _ => None,
        });
        cameras.push(ConfiguredCamera {
            compatible,
            dt_node: format!("/{}", relative.to_string_lossy()),
            i2c_device: name,
            data_lanes,
            camera_id,
        });
    }
    cameras.sort_by(|left, right| left.dt_node.cmp(&right.dt_node));
    (cameras, skipped.into_error("I2C devices"))
}

/// `(path, file name)` of every `*.dtbo` file in the directories directly
/// under `boot_root`, sorted, at most [`MAX_OVERLAY_FILES`].
fn overlay_files(boot_root: &Path, skipped: &mut Skipped) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    let Some(slots) = boot_listing(boot_root, skipped) else {
        return files;
    };
    for slot in slots {
        let directory = boot_root.join(slot);
        if !fs::metadata(&directory).is_ok_and(|metadata| metadata.is_dir()) {
            continue;
        }
        let Some(names) = boot_listing(&directory, skipped) else {
            continue;
        };
        for name in names.into_iter().filter(|name| name.ends_with(".dtbo")) {
            let path = directory.join(&name);
            if !fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
                continue;
            }
            if files.len() == MAX_OVERLAY_FILES {
                let reason = format!(
                    "more than {MAX_OVERLAY_FILES} overlay files under {}; the rest were not read",
                    boot_root.display()
                );
                skipped.add(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
                return files;
            }
            files.push((path, name));
        }
    }
    files
}

/// At most [`MAX_BOOT_ENTRIES`] names in `directory`; `None` when it is gone
/// or unreadable. Unreadable and truncated listings are added to `skipped`.
fn boot_listing(directory: &Path, skipped: &mut Skipped) -> Option<Vec<String>> {
    match dt::listed(directory, MAX_BOOT_ENTRIES) {
        Ok((names, more)) => {
            if more {
                let reason = dt::too_many(MAX_BOOT_ENTRIES, "entries", directory);
                skipped.add(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
            }
            Some(names)
        }
        Err(error) if disappeared(errno_of(&error)) => None,
        Err(error) => {
            skipped.add(io_error("failed to read", directory, &error, false));
            None
        }
    }
}

/// The sensors an overlay file's bytes configure, or why they are not an
/// overlay (too large or malformed).
type ParsedOverlay = Result<Vec<String>, ProviderError>;

/// What one overlay file's bytes give; an error, which is not cached, when the
/// file was not read: it is unreadable, or too large by its size.
fn parse_overlay(path: &Path, length: u64) -> Result<ParsedOverlay, ProviderError> {
    let too_large = || {
        let reason = format!(
            "overlay {} is larger than {MAX_OVERLAY_BYTES} bytes",
            path.display()
        );
        ProviderError::new(CODE_DISCOVERY_FAILED, reason)
    };
    if length > MAX_OVERLAY_BYTES {
        return Err(too_large());
    }
    let bytes = dt::read_bounded(path, MAX_OVERLAY_BYTES + 1)
        .map_err(|error| io_error("failed to read", path, &error, false))?;
    if bytes.len() as u64 > MAX_OVERLAY_BYTES {
        return Ok(Err(too_large()));
    }
    let parsed = dt::parse_fdt(&bytes)
        .map(|tree| dt::overlay_sensors(&tree))
        .map_err(|reason| {
            let reason = format!("overlay {} is malformed: {reason}", path.display());
            ProviderError::new(CODE_DISCOVERY_FAILED, reason)
        });
    Ok(parsed)
}
