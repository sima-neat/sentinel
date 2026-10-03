//! Read-only discovery of SiMa MIPI CSI-2 cameras (`daemon.camera.mipi`).
//!
//! Like SiMa's libcamera `modalix` pipeline handler, without loading
//! libcamera: each `/dev/mediaN` is opened read-only, only the `simaai-v4l2-vid`
//! driver is admitted, and each `MEDIA_ENT_F_CAM_SENSOR` entity is a camera
//! whose entity name (`imx477 5-001a`), as `CameraInput` accepts it, is the id.
//! The ISP is not in that graph: its modes come from the V4L2 nodes with sysfs
//! name `isp_v4l2-vid-cap-out` and card `arm-isp-out` (enumeration ioctls only;
//! several nodes report the modes they share). A missing or unreadable ISP
//! leaves the cameras with `modes: []`.

mod ioctl;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;

use super::model::{Provider, ProviderError, Record};
use super::sysutil::{
    bounded_string, disappeared, errno_of, os_message, read_text_file, trim_c_space,
    CODE_DISCOVERY_FAILED, CODE_IO_OPEN, CODE_PERMISSION_DENIED,
};
use super::videodev2::{
    effective_capabilities, fourcc_string, Capability, EnumerationBudget, FmtDesc, FrmIvalEnum,
    FrmSizeEnum, MAX_ENUMERATION_ENTRIES, V4L2_BUF_TYPE_VIDEO_CAPTURE,
    V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, V4L2_CAP_VIDEO_CAPTURE_MPLANE, V4L2_FRMIVAL_TYPE_DISCRETE,
    V4L2_FRMSIZE_TYPE_DISCRETE,
};
use ioctl::{Backend, MediaDeviceInfo, MediaV2Entity, SystemBackend, MEDIA_ENT_F_CAM_SENSOR};

pub const PROVIDER_NAME: &str = "daemon.camera.mipi";

const SIMA_MEDIA_DRIVER: &str = "simaai-v4l2-vid";
const ISP_SYSFS_NAME: &str = "isp_v4l2-vid-cap-out";
const ISP_CARD_NAME: &str = "arm-isp-out";
/// The `"nominal"` rate of a size without discrete ISP frame intervals.
const NOMINAL_FRAMERATE: (u32, u32) = (30, 1);

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
        Self {
            sys_root: sys_root.into(),
            dev_root: dev_root.into(),
            subsystems: vec!["media".to_string(), "video4linux".to_string()],
            backend: Box::new(SystemBackend),
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
        let backend = self.backend.as_ref();
        let mut sensors = Vec::new();
        for path in media_device_paths(&self.dev_root)? {
            sensors.extend(probe_media_device(backend, &path)?);
        }
        if sensors.is_empty() {
            return Ok(Vec::new());
        }
        let (isp, modes) = match probe_isp(&self.sys_root, &self.dev_root, backend) {
            Ok((paths, modes)) => (
                json!({"state": "available", "device_path": paths[0], "device_paths": paths}),
                json!(modes),
            ),
            Err(reason) => (json!({"state": "unavailable", "reason": reason}), json!([])),
        };
        // Name order is id order; the stable sort keeps the earlier device first.
        sensors.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some([a, b]) = sensors.windows(2).find(|pair| pair[0].name == pair[1].name) {
            let reason = format!(
                "MIPI sensor name {:?} is reported by both {} and {}; \
                 the camera name would be ambiguous.",
                a.name, a.media_device, b.media_device
            );
            return Err(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
        }
        let records = sensors.into_iter().map(|sensor| {
            let mut details = json!({
                "camera_name": sensor.name,
                "backend": "mipi",
                "connection": "mipi-csi2",
                "media_device": sensor.media_device,
                "availability": {"state": "unknown", "reason": AVAILABILITY_REASON},
                "modes": modes,
                "isp": isp,
            });
            let model = sensor.name.split(' ').next().unwrap_or_default();
            if !model.is_empty() {
                details["model"] = json!(model);
            }
            if !sensor.bus_info.is_empty() {
                details["bus_info"] = json!(sensor.bus_info);
            }
            Record {
                id: format!("camera:{}", sensor.name),
                kind: "camera".to_string(),
                provider: PROVIDER_NAME.to_string(),
                details,
            }
        });
        Ok(records.collect())
    }
}

struct Sensor {
    name: String,
    media_device: String,
    bus_info: String,
}

fn describe(what: &str, path: &Path, error: &io::Error) -> String {
    format!("{what} {}: {}", path.display(), os_message(error))
}

/// `EACCES` and `EPERM` are permission failures; anything else is `io.open`.
fn io_failure(what: &str, path: &Path, error: &io::Error) -> ProviderError {
    let denied = matches!(errno_of(error), libc::EACCES | libc::EPERM);
    let code = if denied {
        CODE_PERMISSION_DENIED
    } else {
        CODE_IO_OPEN
    };
    ProviderError::new(code, describe(what, path, error))
}

fn sorted_names(directory: &Path) -> io::Result<Vec<OsString>> {
    let entries = fs::read_dir(directory)?.map(|entry| entry.map(|entry| entry.file_name()));
    let mut names = entries.collect::<io::Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

/// Only `EACCES` is a permission failure when listing a directory.
fn listing_failure(directory: &Path, error: &io::Error) -> ProviderError {
    let code = match errno_of(error) {
        libc::EACCES => CODE_PERMISSION_DENIED,
        _ => CODE_IO_OPEN,
    };
    ProviderError::new(code, describe("failed to read", directory, error))
}

/// `/dev/mediaN`, sorted; a missing device directory means none.
fn media_device_paths(dev_root: &Path) -> Result<Vec<PathBuf>, ProviderError> {
    let names = match sorted_names(dev_root) {
        Err(error) if errno_of(&error) == libc::ENOENT => return Ok(Vec::new()),
        names => names.map_err(|error| listing_failure(dev_root, &error))?,
    };
    let number = |name: &OsString| name.to_str()?.strip_prefix("media").map(str::to_owned);
    let digits = |n: String| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
    let names = names.into_iter().filter(|n| number(n).is_some_and(digits));
    Ok(names.map(|name| dev_root.join(name)).collect())
}

/// The sensors of one media device: none for a non-SiMa device, or for one
/// that vanished mid-scan (hot-unplug race).
fn probe_media_device(backend: &dyn Backend, path: &Path) -> Result<Vec<Sensor>, ProviderError> {
    let failed = |what: &str, error: io::Error| match disappeared(errno_of(&error)) {
        true => Ok(Vec::new()),
        false => Err(io_failure(what, path, &error)),
    };
    let mut node = match backend.open_media(path) {
        Ok(node) => node,
        Err(error) => return failed("failed to open media device", error),
    };
    let mut info = MediaDeviceInfo::default();
    if let Err(error) = node.device_info(&mut info) {
        return failed("failed to query media device information for", error);
    }
    if bounded_string(&info.driver) != SIMA_MEDIA_DRIVER {
        return Ok(Vec::new());
    }
    // One call reads the whole graph atomically, so it cannot change between
    // counting and filling; a graph larger than the cap is malformed.
    let mut entities = vec![MediaV2Entity::default(); MAX_ENUMERATION_ENTRIES as usize];
    match node.entities(&mut entities) {
        Ok(count) => entities.truncate(count),
        Err(error) if errno_of(&error) == libc::ENOSPC => {
            let shown = path.display();
            let reason = format!("media device {shown} returned a malformed entity list");
            return Err(ProviderError::new(CODE_IO_OPEN, reason));
        }
        Err(error) => return failed("failed to read the media topology of", error),
    }
    let bus_info = trim_c_space(&bounded_string(&info.bus_info)).to_string();
    let mut sensors = Vec::new();
    entities.retain(|entity| entity.function == MEDIA_ENT_F_CAM_SENSOR);
    for entity in entities {
        let name = bounded_string(&entity.name);
        if name.is_empty() {
            let (path, id) = (path.display(), entity.id);
            let reason = format!("media device {path} has an unnamed sensor entity {id}");
            return Err(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
        }
        let (media_device, bus_info) = (path.to_string_lossy().into_owned(), bus_info.clone());
        sensors.push(Sensor {
            name,
            media_device,
            bus_info,
        });
    }
    Ok(sensors)
}

/// One ISP output mode; field order is the sort order and the JSON order.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct IspMode {
    format: String,
    width: u32,
    height: u32,
    framerate_num: u32,
    framerate_den: u32,
    /// `"isp"` for a discrete ISP frame interval, `"nominal"` otherwise.
    framerate_source: &'static str,
    isp_output: bool,
}

type IspModes = (Vec<String>, BTreeSet<IspMode>);

/// Every `/sys/class/video4linux` entry named like the ISP output, in sorted
/// order. Nodes with another card or no discrete size are skipped, the first
/// failing node makes the ISP unavailable, and several nodes contribute only
/// the modes they all share.
fn probe_isp(sys_root: &Path, dev_root: &Path, backend: &dyn Backend) -> Result<IspModes, String> {
    let class = sys_root.join("class/video4linux");
    let names = sorted_names(&class).map_err(|error| describe("could not read", &class, &error))?;
    let mut common: Option<IspModes> = None;
    for name in names {
        if read_text_file(&class.join(&name).join("name")).as_deref() != Some(ISP_SYSFS_NAME) {
            continue;
        }
        let path = dev_root.join(&name);
        let modes = isp_modes(backend, &path)?;
        if modes.is_empty() {
            continue;
        }
        let (paths, shared) = common.get_or_insert_with(|| (Vec::new(), modes.clone()));
        paths.push(path.to_string_lossy().into_owned());
        shared.retain(|mode| modes.contains(mode));
    }
    match common {
        None => Err("no Modalix ISP output node was found".to_string()),
        Some((_, modes)) if modes.is_empty() => {
            Err("Modalix ISP output nodes reported no common discrete sizes".to_string())
        }
        Some(found) => Ok(found),
    }
}

fn malformed(what: &str, path: &Path) -> String {
    format!("ISP node {} returned a malformed {what}", path.display())
}

/// Entries 0, 1, … of one V4L2 enumeration, until the driver answers `EINVAL`
/// or `query` returns `None`. Every query spends the device budget, and a list
/// may not exceed `MAX_ENUMERATION_ENTRIES`.
fn enumerate<T>(
    budget: &mut EnumerationBudget,
    path: &Path,
    (ioctl, list): (&str, &str),
    mut query: impl FnMut(u32) -> io::Result<Option<T>>,
) -> Result<Vec<T>, String> {
    let mut entries = Vec::new();
    for index in 0..MAX_ENUMERATION_ENTRIES {
        budget.spend().map_err(|what| malformed(&what, path))?;
        match query(index) {
            Ok(Some(entry)) => entries.push(entry),
            Err(error) if errno_of(&error) != libc::EINVAL => {
                return Err(describe(&format!("{ioctl} failed for"), path, &error));
            }
            _ => return Ok(entries),
        }
    }
    Err(malformed(list, path))
}

/// The modes of one candidate ISP node, opened read-only; empty when its card
/// is not the ISP output's.
fn isp_modes(backend: &dyn Backend, path: &Path) -> Result<BTreeSet<IspMode>, String> {
    let node = backend.open_video(path);
    let mut node = node.map_err(|error| describe("could not open", path, &error))?;
    let mut capability = Capability::default();
    let queried = node.query_capability(&mut capability);
    queried.map_err(|error| describe("VIDIOC_QUERYCAP failed for", path, &error))?;
    if bounded_string(&capability.card) != ISP_CARD_NAME {
        return Ok(BTreeSet::new());
    }
    let buf_type = if effective_capabilities(&capability) & V4L2_CAP_VIDEO_CAPTURE_MPLANE != 0 {
        V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE
    } else {
        V4L2_BUF_TYPE_VIDEO_CAPTURE
    };
    let budget = &mut EnumerationBudget::new();
    let formats = enumerate(budget, path, ("VIDIOC_ENUM_FMT", "format list"), |index| {
        let mut format = FmtDesc::default();
        (format.index, format.buf_type) = (index, buf_type);
        node.enum_format(&mut format)
            .map(|()| Some(format.pixelformat))
    })?;
    let mut modes = BTreeSet::new();
    for pixel_format in formats {
        let ioctl = ("VIDIOC_ENUM_FRAMESIZES", "frame size list");
        let sizes = enumerate(budget, path, ioctl, |index| {
            let mut size = FrmSizeEnum::default();
            (size.index, size.pixel_format) = (index, pixel_format);
            node.enum_frame_size(&mut size).map(|()| {
                let discrete = size.kind == V4L2_FRMSIZE_TYPE_DISCRETE;
                Some(discrete.then(|| (size.discrete_width(), size.discrete_height())))
            })
        })?;
        for (width, height) in sizes.into_iter().flatten().collect::<BTreeSet<_>>() {
            // Discrete intervals only: the list ends at a stepwise or
            // continuous range, or at ENOTTY from a driver without the ioctl.
            let ioctl = ("VIDIOC_ENUM_FRAMEINTERVALS", "frame interval list");
            let intervals = enumerate(budget, path, ioctl, |index| {
                let mut value = FrmIvalEnum::default();
                (value.index, value.pixel_format) = (index, pixel_format);
                (value.width, value.height) = (width, height);
                match node.enum_frame_interval(&mut value) {
                    Err(error) if errno_of(&error) == libc::ENOTTY => Ok(None),
                    result => result.map(|()| {
                        (value.kind == V4L2_FRMIVAL_TYPE_DISCRETE).then(|| value.discrete())
                    }),
                }
            })?;
            if intervals.iter().any(|&(n, d)| n == 0 || d == 0) {
                return Err(malformed("frame interval", path));
            }
            // An interval is a frame period; the rate is its reciprocal.
            let mut rates: Vec<_> = intervals.iter().map(|&(n, d)| ((d, n), "isp")).collect();
            if rates.is_empty() {
                rates.push((NOMINAL_FRAMERATE, "nominal"));
            }
            for ((framerate_num, framerate_den), framerate_source) in rates {
                let format = fourcc_string(pixel_format);
                modes.insert(IspMode {
                    format,
                    width,
                    height,
                    framerate_num,
                    framerate_den,
                    framerate_source,
                    isp_output: true,
                });
            }
        }
    }
    Ok(modes)
}
