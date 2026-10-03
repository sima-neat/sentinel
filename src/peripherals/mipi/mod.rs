//! Read-only discovery of SiMa MIPI CSI-2 cameras from the kernel's media
//! controller (`daemon.camera.mipi`).
//!
//! The scan mirrors how SiMa's libcamera `modalix` pipeline handler finds
//! cameras, without loading libcamera: every `/dev/mediaN` is opened with
//! `O_RDONLY | O_NONBLOCK | O_CLOEXEC`, `MEDIA_IOC_DEVICE_INFO` admits only
//! the `simaai-v4l2-vid` driver, and `MEDIA_IOC_G_TOPOLOGY` yields one camera
//! per `MEDIA_ENT_F_CAM_SENSOR` entity. The entity name (for example
//! `imx477 5-001a`) is the name `CameraInput` accepts, so it is the record id;
//! `/dev/mediaN` is routing metadata only.
//!
//! The ISP is not part of that media graph. Its output modes come from the
//! V4L2 node whose sysfs name is `isp_v4l2-vid-cap-out` and whose card is
//! `arm-isp-out`, queried with enumeration ioctls only; when several nodes
//! qualify, only the modes they all share are reported. An unreadable or
//! missing ISP degrades the records to `modes: []`; it never fails the scan.

mod ioctl;
#[cfg(test)]
mod tests;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::model::{Provider, ProviderError, Record};
use super::sysutil::{
    bounded_string, disappeared, errno_of, os_message, read_text_file, trim_c_space,
    CODE_DISCOVERY_FAILED, CODE_IO_OPEN, CODE_PERMISSION_DENIED,
};
use super::videodev2::{
    effective_capabilities, fourcc_string, Capability, EnumerationBudget, FmtDesc, FrmIvalEnum,
    FrmSizeEnum, VideoNode, MAX_ENUMERATION_ENTRIES, V4L2_BUF_TYPE_VIDEO_CAPTURE,
    V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, V4L2_CAP_VIDEO_CAPTURE_MPLANE, V4L2_FRMIVAL_TYPE_DISCRETE,
    V4L2_FRMSIZE_TYPE_DISCRETE,
};
use ioctl::{
    Backend, MediaDeviceInfo, MediaNode, MediaV2Entity, MediaV2Topology, SystemBackend,
    MEDIA_ENT_F_CAM_SENSOR,
};

pub const PROVIDER_NAME: &str = "daemon.camera.mipi";

/// `media_device_info.driver` of the SiMa capture media device.
const SIMA_MEDIA_DRIVER: &str = "simaai-v4l2-vid";
/// sysfs `name` and V4L2 card of the ISP output node.
const ISP_SYSFS_NAME: &str = "isp_v4l2-vid-cap-out";
const ISP_CARD_NAME: &str = "arm-isp-out";
/// The rate reported, with `framerate_source: "nominal"`, when the ISP
/// advertises no discrete frame intervals for a size.
const NOMINAL_FRAMERATE: (u32, u32) = (30, 1);

/// `MEDIA_IOC_G_TOPOLOGY` is read twice (count, then fill); a graph that keeps
/// changing between the two calls is retried this many times.
const MAX_TOPOLOGY_ATTEMPTS: u32 = 4;

const AVAILABILITY_REASON: &str = "The media controller does not expose a reliable read-only \
ownership state; discovery does not acquire, configure, or stream from the camera.";

/// The `daemon.camera.mipi` provider.
pub struct MipiProvider {
    sys_root: PathBuf,
    dev_root: PathBuf,
    subsystems: Vec<String>,
    backend: Box<dyn Backend>,
}

impl MipiProvider {
    /// Scan the live system (`/sys`, `/dev`).
    pub fn new() -> Self {
        Self::with_roots("/sys", "/dev")
    }

    /// Scan a sysfs tree and device directory rooted elsewhere.
    pub fn with_roots(sys_root: impl Into<PathBuf>, dev_root: impl Into<PathBuf>) -> Self {
        Self::with_backend(sys_root, dev_root, Box::new(SystemBackend))
    }

    fn with_backend(
        sys_root: impl Into<PathBuf>,
        dev_root: impl Into<PathBuf>,
        backend: Box<dyn Backend>,
    ) -> Self {
        Self {
            sys_root: sys_root.into(),
            dev_root: dev_root.into(),
            subsystems: vec!["media".to_string(), "video4linux".to_string()],
            backend,
        }
    }
}

impl Default for MipiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for MipiProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn subsystems(&self) -> &[String] {
        &self.subsystems
    }

    fn discover(&mut self) -> Result<Vec<Record>, ProviderError> {
        let sensors = probe_sensors(&self.dev_root, self.backend.as_ref())?;
        if sensors.is_empty() {
            return Ok(Vec::new());
        }
        let isp = probe_isp(&self.sys_root, &self.dev_root, self.backend.as_ref());
        build_records(&sensors, &isp)
    }
}

// ---------------------------------------------------------------------------
// Media controller: SiMa media devices and their sensor entities
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Sensor {
    name: String,
    media_device: String,
    bus_info: String,
}

/// A media-device failure, before it is mapped to a provider error.
#[derive(Debug)]
struct MediaError {
    errno: i32,
    error: ProviderError,
}

fn media_io_error(action: &str, path: &Path, error: &io::Error) -> MediaError {
    let errno = errno_of(error);
    let code = if errno == libc::EACCES || errno == libc::EPERM {
        CODE_PERMISSION_DENIED
    } else {
        CODE_IO_OPEN
    };
    MediaError {
        errno,
        error: ProviderError::new(
            code,
            format!("{action} {}: {}", path.display(), os_message(error)),
        ),
    }
}

fn malformed_media(what: &str, path: &Path) -> MediaError {
    MediaError {
        errno: libc::EPROTO,
        error: ProviderError::new(
            CODE_IO_OPEN,
            format!(
                "media device {} returned a malformed {what}",
                path.display()
            ),
        ),
    }
}

/// `/dev/mediaN` entries, sorted. A missing device directory means no media
/// devices.
fn media_device_paths(dev_root: &Path) -> Result<Vec<PathBuf>, ProviderError> {
    let read_failure = |error: &io::Error| {
        let code = if errno_of(error) == libc::EACCES {
            CODE_PERMISSION_DENIED
        } else {
            CODE_IO_OPEN
        };
        ProviderError::new(
            code,
            format!(
                "failed to read {}: {}",
                dev_root.display(),
                os_message(error)
            ),
        )
    };
    let iterator = match fs::read_dir(dev_root) {
        Ok(iterator) => iterator,
        Err(error) if errno_of(&error) == libc::ENOENT => return Ok(Vec::new()),
        Err(error) => return Err(read_failure(&error)),
    };
    let mut paths = Vec::new();
    for entry in iterator {
        let entry = entry.map_err(|error| read_failure(&error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_media = name
            .strip_prefix("media")
            .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()));
        if is_media {
            paths.push(dev_root.join(entry.file_name()));
        }
    }
    paths.sort();
    Ok(paths)
}

/// Read every entity with the count-then-fill `MEDIA_IOC_G_TOPOLOGY` protocol,
/// retrying when the graph changes between the two calls.
fn read_entities(node: &mut dyn MediaNode, path: &Path) -> Result<Vec<MediaV2Entity>, MediaError> {
    const ACTION: &str = "failed to read the media topology of";
    for _ in 0..MAX_TOPOLOGY_ATTEMPTS {
        let mut counted = MediaV2Topology::default();
        node.topology(&mut counted, &mut [])
            .map_err(|error| media_io_error(ACTION, path, &error))?;
        if counted.num_entities > MAX_ENUMERATION_ENTRIES {
            return Err(malformed_media("entity list", path));
        }
        let mut entities = vec![MediaV2Entity::default(); counted.num_entities as usize];
        let mut filled = MediaV2Topology::default();
        match node.topology(&mut filled, &mut entities) {
            Ok(()) => {}
            Err(error) if errno_of(&error) == libc::ENOSPC => continue,
            Err(error) => return Err(media_io_error(ACTION, path, &error)),
        }
        if filled.topology_version == counted.topology_version
            && filled.num_entities == counted.num_entities
        {
            return Ok(entities);
        }
    }
    Err(MediaError {
        errno: libc::EAGAIN,
        error: ProviderError::new(
            CODE_IO_OPEN,
            format!(
                "media device {} kept changing its topology during discovery",
                path.display()
            ),
        ),
    })
}

/// The sensors of one media device; empty for a non-SiMa device or a SiMa
/// device with no sensor bound.
fn probe_media_device(backend: &dyn Backend, path: &Path) -> Result<Vec<Sensor>, MediaError> {
    let mut node = backend
        .open_media(path)
        .map_err(|error| media_io_error("failed to open media device", path, &error))?;
    let node = node.as_mut();

    let mut info = MediaDeviceInfo::default();
    node.device_info(&mut info).map_err(|error| {
        media_io_error("failed to query media device information for", path, &error)
    })?;
    if bounded_string(&info.driver) != SIMA_MEDIA_DRIVER {
        return Ok(Vec::new());
    }
    let bus_info = trim_c_space(&bounded_string(&info.bus_info)).to_string();

    let mut sensors = Vec::new();
    for entity in read_entities(node, path)? {
        if entity.function != MEDIA_ENT_F_CAM_SENSOR {
            continue;
        }
        let name = bounded_string(&entity.name);
        if name.is_empty() {
            return Err(MediaError {
                errno: libc::EPROTO,
                error: ProviderError::new(
                    CODE_DISCOVERY_FAILED,
                    format!(
                        "media device {} has an unnamed sensor entity {}",
                        path.display(),
                        entity.id
                    ),
                ),
            });
        }
        sensors.push(Sensor {
            name,
            media_device: path.to_string_lossy().into_owned(),
            bus_info: bus_info.clone(),
        });
    }
    Ok(sensors)
}

fn probe_sensors(dev_root: &Path, backend: &dyn Backend) -> Result<Vec<Sensor>, ProviderError> {
    let mut sensors = Vec::new();
    for path in media_device_paths(dev_root)? {
        match probe_media_device(backend, &path) {
            Ok(found) => sensors.extend(found),
            // Hot-unplug race: the node vanished between listing and query.
            Err(failure) if disappeared(failure.errno) => {}
            Err(failure) => return Err(failure.error),
        }
    }
    Ok(sensors)
}

// ---------------------------------------------------------------------------
// ISP output modes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FramerateSource {
    Isp,
    Nominal,
}

impl FramerateSource {
    fn name(self) -> &'static str {
        match self {
            FramerateSource::Isp => "isp",
            FramerateSource::Nominal => "nominal",
        }
    }
}

/// One ISP output mode. Field order is the canonical sort order: format, then
/// width, then height.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct IspMode {
    format: String,
    width: u32,
    height: u32,
    framerate_num: u32,
    framerate_den: u32,
    source: FramerateSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Isp {
    Available {
        device_paths: Vec<String>,
        modes: BTreeSet<IspMode>,
    },
    Unavailable(String),
}

fn malformed_isp(what: &str, path: &Path) -> String {
    format!("ISP node {} returned a malformed {what}", path.display())
}

/// The discrete rates of one ISP size, as frames per second. Empty when the
/// node reports none (`EINVAL` at index 0, or `ENOTTY` when the driver does
/// not implement the ioctl) or reports only a stepwise/continuous range.
fn enumerate_rates(
    node: &mut dyn VideoNode,
    budget: &mut EnumerationBudget,
    path: &Path,
    pixel_format: u32,
    width: u32,
    height: u32,
) -> Result<Vec<(u32, u32)>, String> {
    let mut rates = Vec::new();
    let mut index: u32 = 0;
    loop {
        if index >= MAX_ENUMERATION_ENTRIES {
            return Err(malformed_isp("frame interval list", path));
        }
        let mut value = FrmIvalEnum {
            index,
            pixel_format,
            width,
            height,
            ..FrmIvalEnum::default()
        };
        budget.spend().map_err(|what| malformed_isp(&what, path))?;
        if let Err(error) = node.enum_frame_interval(&mut value) {
            let errno = errno_of(&error);
            if errno == libc::EINVAL || errno == libc::ENOTTY {
                break;
            }
            return Err(format!(
                "VIDIOC_ENUM_FRAMEINTERVALS failed for {}: {}",
                path.display(),
                os_message(&error)
            ));
        }
        if value.kind != V4L2_FRMIVAL_TYPE_DISCRETE {
            break;
        }
        let (numerator, denominator) = value.discrete();
        if numerator == 0 || denominator == 0 {
            return Err(malformed_isp("frame interval", path));
        }
        // An interval is a frame period; the rate is its reciprocal.
        rates.push((denominator, numerator));
        index = index.wrapping_add(1);
    }
    Ok(rates)
}

/// The modes of one candidate ISP node. An empty set means the node is not the
/// ISP output (wrong card) or advertises no discrete size. The node is opened
/// read-only and receives enumeration ioctls only.
fn enumerate_isp_node(backend: &dyn Backend, path: &Path) -> Result<BTreeSet<IspMode>, String> {
    let mut modes = BTreeSet::new();
    let mut node = backend
        .open_video(path)
        .map_err(|error| format!("could not open {}: {}", path.display(), os_message(&error)))?;
    let node = node.as_mut();

    let mut capability = Capability::default();
    node.query_capability(&mut capability).map_err(|error| {
        format!(
            "VIDIOC_QUERYCAP failed for {}: {}",
            path.display(),
            os_message(&error)
        )
    })?;
    if bounded_string(&capability.card) != ISP_CARD_NAME {
        return Ok(modes);
    }
    let budget = &mut EnumerationBudget::new();
    let buffer_type = if effective_capabilities(&capability) & V4L2_CAP_VIDEO_CAPTURE_MPLANE != 0 {
        V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE
    } else {
        V4L2_BUF_TYPE_VIDEO_CAPTURE
    };

    for format_index in 0.. {
        if format_index >= MAX_ENUMERATION_ENTRIES {
            return Err(malformed_isp("format list", path));
        }
        let mut format = FmtDesc {
            index: format_index,
            buf_type: buffer_type,
            ..FmtDesc::default()
        };
        budget.spend().map_err(|what| malformed_isp(&what, path))?;
        if let Err(error) = node.enum_format(&mut format) {
            if errno_of(&error) == libc::EINVAL {
                break;
            }
            return Err(format!(
                "VIDIOC_ENUM_FMT failed for {}: {}",
                path.display(),
                os_message(&error)
            ));
        }
        let mut sizes = BTreeSet::new();
        for size_index in 0.. {
            if size_index >= MAX_ENUMERATION_ENTRIES {
                return Err(malformed_isp("frame size list", path));
            }
            let mut size = FrmSizeEnum {
                index: size_index,
                pixel_format: format.pixelformat,
                ..FrmSizeEnum::default()
            };
            budget.spend().map_err(|what| malformed_isp(&what, path))?;
            if let Err(error) = node.enum_frame_size(&mut size) {
                if errno_of(&error) == libc::EINVAL {
                    break;
                }
                return Err(format!(
                    "VIDIOC_ENUM_FRAMESIZES failed for {}: {}",
                    path.display(),
                    os_message(&error)
                ));
            }
            if size.kind == V4L2_FRMSIZE_TYPE_DISCRETE {
                sizes.insert((size.discrete_width(), size.discrete_height()));
            }
        }
        let token = fourcc_string(format.pixelformat);
        for (width, height) in sizes {
            let rates = enumerate_rates(node, budget, path, format.pixelformat, width, height)?;
            let mode = |(framerate_num, framerate_den), source| IspMode {
                format: token.clone(),
                width,
                height,
                framerate_num,
                framerate_den,
                source,
            };
            if rates.is_empty() {
                modes.insert(mode(NOMINAL_FRAMERATE, FramerateSource::Nominal));
            }
            for rate in rates {
                modes.insert(mode(rate, FramerateSource::Isp));
            }
        }
    }
    Ok(modes)
}

/// Every `/sys/class/video4linux` entry whose
/// `name` is the ISP output name is probed in sorted order; nodes with the
/// wrong card or no discrete size are skipped; the first failing node makes
/// the ISP unavailable; several ISP nodes contribute only the modes they all
/// share.
fn probe_isp(sys_root: &Path, dev_root: &Path, backend: &dyn Backend) -> Isp {
    let class_directory = sys_root.join("class/video4linux");
    let read_failure = |error: &io::Error| {
        Isp::Unavailable(format!(
            "could not read {}: {}",
            class_directory.display(),
            os_message(error)
        ))
    };
    let iterator = match fs::read_dir(&class_directory) {
        Ok(iterator) => iterator,
        Err(error) => return read_failure(&error),
    };
    let mut entries = Vec::new();
    for entry in iterator {
        match entry {
            Ok(entry) => {
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    entries.push(entry.file_name());
                }
            }
            Err(error) => return read_failure(&error),
        }
    }
    entries.sort();

    let mut common: Option<BTreeSet<IspMode>> = None;
    let mut device_paths = Vec::new();
    for entry in entries {
        let name = read_text_file(&class_directory.join(&entry).join("name"));
        if name.as_deref() != Some(ISP_SYSFS_NAME) {
            continue;
        }
        let path = dev_root.join(&entry);
        let modes = match enumerate_isp_node(backend, &path) {
            Ok(modes) => modes,
            Err(reason) => return Isp::Unavailable(reason),
        };
        if modes.is_empty() {
            continue;
        }
        device_paths.push(path.to_string_lossy().into_owned());
        common = Some(match common {
            None => modes,
            Some(current) => current.intersection(&modes).cloned().collect(),
        });
    }

    match common {
        None => Isp::Unavailable("no Modalix ISP output node was found".to_string()),
        Some(modes) if modes.is_empty() => Isp::Unavailable(
            "Modalix ISP output nodes reported no common discrete sizes".to_string(),
        ),
        Some(modes) => Isp::Available {
            device_paths,
            modes,
        },
    }
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

fn model_of(name: &str) -> &str {
    name.split_once(' ').map_or(name, |(model, _)| model)
}

fn isp_json(isp: &Isp) -> (Value, Vec<Value>) {
    match isp {
        Isp::Unavailable(reason) => (
            json!({"state": "unavailable", "reason": reason}),
            Vec::new(),
        ),
        Isp::Available {
            device_paths,
            modes,
        } => {
            let modes = modes
                .iter()
                .map(|mode| {
                    json!({
                        "format": mode.format,
                        "width": mode.width,
                        "height": mode.height,
                        "framerate_num": mode.framerate_num,
                        "framerate_den": mode.framerate_den,
                        "framerate_source": mode.source.name(),
                        "isp_output": true,
                    })
                })
                .collect();
            (
                json!({
                    "state": "available",
                    "device_path": device_paths[0],
                    "device_paths": device_paths,
                }),
                modes,
            )
        }
    }
}

fn build_records(sensors: &[Sensor], isp: &Isp) -> Result<Vec<Record>, ProviderError> {
    let (isp_value, modes) = isp_json(isp);
    let mut records: BTreeMap<String, Record> = BTreeMap::new();
    for sensor in sensors {
        let id = format!("camera:{}", sensor.name);
        let mut details = json!({
            "camera_name": sensor.name,
            "backend": "mipi",
            "connection": "mipi-csi2",
            "media_device": sensor.media_device,
            "availability": {"state": "unknown", "reason": AVAILABILITY_REASON},
            "modes": modes,
            "isp": isp_value,
        });
        let model = model_of(&sensor.name);
        if !model.is_empty() {
            details["model"] = json!(model);
        }
        if !sensor.bus_info.is_empty() {
            details["bus_info"] = json!(sensor.bus_info);
        }
        let record = Record {
            id: id.clone(),
            kind: "camera".to_string(),
            provider: PROVIDER_NAME.to_string(),
            details,
        };
        match records.entry(id) {
            Entry::Vacant(slot) => {
                slot.insert(record);
            }
            Entry::Occupied(slot) => {
                return Err(ProviderError::new(
                    CODE_DISCOVERY_FAILED,
                    format!(
                        "MIPI sensor name {:?} is reported by both {} and {}; \
                         the camera name would be ambiguous.",
                        sensor.name,
                        slot.get().details["media_device"].as_str().unwrap_or(""),
                        sensor.media_device
                    ),
                ));
            }
        }
    }
    Ok(records.into_values().collect())
}
