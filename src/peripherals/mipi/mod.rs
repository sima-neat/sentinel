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
//!
//! The same topology read names the CSI-2 receiver the sensor's source pad
//! links to and the sensor's `/dev/v4l-subdevN`, whose active format and
//! pixel-rate and blanking controls give the sensor's frame-rate limit. Both
//! are optional: a graph or sub-device that does not provide them omits them.

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
    bounded_string, disappeared, errno_of, io_error, os_message, read_text_file, trim_c_space,
    vanished, CODE_DISCOVERY_FAILED, CODE_IO_OPEN,
};
use super::videodev2::{
    effective_capabilities, fourcc_string, Capability, EnumerationBudget, FmtDesc, FrmIvalEnum,
    FrmSizeEnum, MAX_ENUMERATION_ENTRIES, V4L2_BUF_TYPE_VIDEO_CAPTURE,
    V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, V4L2_CAP_VIDEO_CAPTURE_MPLANE, V4L2_FRMIVAL_TYPE_DISCRETE,
    V4L2_FRMSIZE_TYPE_DISCRETE,
};
use ioctl::*;

pub const PROVIDER_NAME: &str = "daemon.camera.mipi";

const SIMA_MEDIA_DRIVER: &str = "simaai-v4l2-vid";
const ISP_SYSFS_NAME: &str = "isp_v4l2-vid-cap-out";
const ISP_CARD_NAME: &str = "arm-isp-out";
/// The `"nominal"` rate of a size without discrete ISP frame intervals.
const NOMINAL_FRAMERATE: (u32, u32) = (30, 1);
/// The rates offered below a sensor's frame-rate limit (Insight's rule).
const STANDARD_FRAMERATES: [u32; 7] = [60, 30, 25, 20, 15, 10, 5];

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
            match probe_media_device(backend, &path) {
                Ok(found) => sensors.extend(found),
                // Unplugged mid-scan, though the failure did not say so (an
                // unregistered media device answers EIO): its node is gone.
                Err(_) if vanished(&path) => {}
                Err(error) => return Err(error),
            }
        }
        sensors.retain(|sensor| !vanished(&sensor.media_path));
        if sensors.is_empty() {
            return Ok(Vec::new());
        }
        let (isp, modes) = match probe_isp(&self.sys_root, &self.dev_root, backend) {
            Ok((paths, modes)) => (
                json!({"state": "available", "device_path": paths[0], "device_paths": paths}),
                modes,
            ),
            Err(reason) => (
                json!({"state": "unavailable", "reason": reason}),
                BTreeSet::new(),
            ),
        };
        // ISP enumeration can race with a media-device unplug. Drop stale
        // sensors before they participate in duplicate-name validation.
        sensors.retain(|sensor| !vanished(&sensor.media_path));
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
        let records = sensors.into_iter().filter_map(|sensor| {
            let timing = sensor
                .subdev
                .and_then(|(devnode, pad)| self.sensor_timing(devnode, pad));
            if vanished(&sensor.media_path) {
                return None;
            }
            let mut details = json!({
                "camera_name": sensor.name,
                "backend": "mipi",
                "connection": "mipi-csi2",
                "media_device": sensor.media_device,
                "availability": {"state": "unknown", "reason": AVAILABILITY_REASON},
                "modes": sensor_modes(&modes, timing.as_ref()),
                "isp": isp,
            });
            if let Some(receiver) = &sensor.csi_receiver {
                details["csi_receiver"] = json!(receiver);
            }
            if let Some(timing) = &timing {
                details["sensor_timing"] = json!(timing);
                details["max_fps"] = json!(timing.max_fps());
            }
            let model = sensor.name.split(' ').next().unwrap_or_default();
            if !model.is_empty() {
                details["model"] = json!(model);
            }
            if !sensor.bus_info.is_empty() {
                details["bus_info"] = json!(sensor.bus_info);
            }
            Some(Record {
                id: format!("camera:{}", sensor.name),
                kind: "camera".to_string(),
                provider: PROVIDER_NAME.to_string(),
                details,
            })
        });
        Ok(records.collect())
    }
}

struct Sensor {
    name: String,
    /// Original path for filesystem checks; `media_device` is display-only.
    media_path: PathBuf,
    media_device: String,
    bus_info: String,
    /// The entity the sensor's source pad links to.
    csi_receiver: Option<String>,
    /// The sensor's sub-device interface `(major, minor)` and source pad index.
    subdev: Option<((u32, u32), u32)>,
}

/// The sensor's active source-pad format and the controls that bound its
/// frame rate.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct SensorTiming {
    pixel_rate: u64,
    hblank_min: u32,
    vblank_min: u32,
    width: u32,
    height: u32,
}

impl SensorTiming {
    /// Pixels per second over pixels per frame at minimum blanking, to two
    /// decimals.
    fn max_fps(&self) -> f64 {
        let line = f64::from(self.width) + f64::from(self.hblank_min);
        let frame = line * (f64::from(self.height) + f64::from(self.vblank_min));
        (self.pixel_rate as f64 / frame * 100.0).round() / 100.0
    }
}

/// The frame rates offered up to `max_fps`, fastest first: its nearest whole
/// rate (at least 1) and every standard rate below it.
fn sensor_rates(max_fps: f64) -> Vec<u32> {
    let limit = ((max_fps + 0.5) as u32).max(1);
    let standard = STANDARD_FRAMERATES
        .into_iter()
        .filter(|&rate| rate <= limit);
    let rates: BTreeSet<u32> = standard.chain([limit]).collect();
    rates.into_iter().rev().collect()
}

/// A sensor's modes: with a known frame-rate limit, each nominal-rate ISP
/// size is offered at `sensor_rates` instead; ISP-reported rates are kept.
fn sensor_modes(modes: &BTreeSet<IspMode>, timing: Option<&SensorTiming>) -> Vec<IspMode> {
    let Some(rates) = timing.map(|timing| sensor_rates(timing.max_fps())) else {
        return modes.iter().cloned().collect();
    };
    let mut expanded = Vec::new();
    for mode in modes {
        if mode.framerate_source != "nominal" {
            expanded.push(mode.clone());
            continue;
        }
        expanded.extend(rates.iter().map(|&rate| IspMode {
            framerate_num: rate,
            framerate_den: 1,
            framerate_source: "sensor_timing",
            ..mode.clone()
        }));
    }
    expanded
}

/// The data link from one of `entity`'s source pads: enabled links first,
/// then the lowest pad index. Returns the source pad and the sink pad's entity.
fn source_link(graph: &Graph, entity: u32) -> Option<(&MediaV2Pad, &MediaV2Entity)> {
    let pad = |id: u32| graph.pads.iter().find(|pad| pad.id == id);
    let links = graph.links.iter().filter(|link| {
        link.flags & MEDIA_LNK_FL_LINK_TYPE == MEDIA_LNK_FL_DATA_LINK
            && pad(link.source_id).is_some_and(|source| {
                source.entity_id == entity && source.flags & MEDIA_PAD_FL_SOURCE != 0
            })
    });
    let link = links.min_by_key(|link| {
        let enabled = link.flags & MEDIA_LNK_FL_ENABLED != 0;
        (!enabled, pad(link.source_id).map(|pad| pad.index))
    })?;
    let sink = pad(link.sink_id)?;
    let receiver = graph.entities.iter().find(|e| e.id == sink.entity_id)?;
    Some((pad(link.source_id)?, receiver))
}

/// The `(major, minor)` of `entity`'s V4L2 sub-device interface.
fn subdev_devnode(graph: &Graph, entity: u32) -> Option<(u32, u32)> {
    let links = graph.links.iter().filter(|link| {
        link.flags & MEDIA_LNK_FL_LINK_TYPE == MEDIA_LNK_FL_INTERFACE_LINK && link.sink_id == entity
    });
    links
        .filter_map(|link| graph.interfaces.iter().find(|i| i.id == link.source_id))
        .find(|interface| interface.intf_type == MEDIA_INTF_T_V4L_SUBDEV)
        .map(MediaV2Interface::devnode)
}

impl MipiProvider {
    /// The timing of the sensor whose sub-device is `devnode`, read from
    /// `/dev/v4l-subdevN` (named by `/sys/dev/char/<major>:<minor>`): the
    /// active format of `pad`, the current pixel rate, and the minimum
    /// horizontal and vertical blanking. `None` when any part is missing.
    fn sensor_timing(&self, (major, minor): (u32, u32), pad: u32) -> Option<SensorTiming> {
        let class = self.sys_root.join(format!("dev/char/{major}:{minor}"));
        let name = fs::read_link(class).ok()?.file_name()?.to_owned();
        if !name.to_string_lossy().starts_with("v4l-subdev") {
            return None;
        }
        let mut node = self.backend.open_subdev(&self.dev_root.join(name)).ok()?;
        let mut format = SubdevFormat {
            which: V4L2_SUBDEV_FORMAT_ACTIVE,
            pad,
            ..SubdevFormat::default()
        };
        node.format(&mut format).ok()?;
        let (width, height) = (format.format.width, format.format.height);
        let mut minimum = |id| {
            let mut control = QueryExtCtrl {
                id,
                ..QueryExtCtrl::default()
            };
            node.query_control(&mut control).ok()?;
            let enabled = control.flags & V4L2_CTRL_FLAG_DISABLED == 0;
            enabled.then(|| u32::try_from(control.minimum).ok())?
        };
        let (hblank_min, vblank_min) = (minimum(V4L2_CID_HBLANK)?, minimum(V4L2_CID_VBLANK)?);
        let pixel_rate = node.control_value(V4L2_CID_PIXEL_RATE).ok()?;
        let pixel_rate = u64::try_from(pixel_rate).ok().filter(|&rate| rate > 0)?;
        (width > 0 && height > 0).then_some(SensorTiming {
            pixel_rate,
            hblank_min,
            vblank_min,
            width,
            height,
        })
    }
}

fn describe(what: &str, path: &Path, error: &io::Error) -> String {
    format!("{what} {}: {}", path.display(), os_message(error))
}

/// `EACCES` and `EPERM` are permission failures; anything else is `io.open`.
fn io_failure(what: &str, path: &Path, error: &io::Error) -> ProviderError {
    io_error(what, path, error, true)
}

fn sorted_names(directory: &Path) -> io::Result<Vec<OsString>> {
    let entries = fs::read_dir(directory)?.map(|entry| entry.map(|entry| entry.file_name()));
    let mut names = entries.collect::<io::Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

/// Only `EACCES` is a permission failure when listing a directory.
fn listing_failure(directory: &Path, error: &io::Error) -> ProviderError {
    io_error("failed to read", directory, error, false)
}

/// `/dev/mediaN`, sorted; a missing device directory means none.
fn media_device_paths(dev_root: &Path) -> Result<Vec<PathBuf>, ProviderError> {
    let names = match sorted_names(dev_root) {
        Err(error) if errno_of(&error) == libc::ENOENT => return Ok(Vec::new()),
        names => names.map_err(|error| listing_failure(dev_root, &error))?,
    };
    let media = |name: &OsString| {
        let number = name.to_str().and_then(|name| name.strip_prefix("media"));
        number.is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    let names = names.into_iter().filter(media);
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
    // counting and filling. Interfaces, pads or links beyond the cap only
    // drop what they describe; more entities than the cap are malformed.
    let room = MAX_ENUMERATION_ENTRIES as usize;
    let mut graph = Graph {
        entities: vec![MediaV2Entity::default(); room],
        interfaces: vec![MediaV2Interface::default(); room],
        pads: vec![MediaV2Pad::default(); room],
        links: vec![MediaV2Link::default(); room],
    };
    let mut result = node.topology(&mut graph);
    if result
        .as_ref()
        .is_err_and(|error| errno_of(error) == libc::ENOSPC)
    {
        graph = Graph {
            entities: vec![MediaV2Entity::default(); room],
            ..Graph::default()
        };
        result = node.topology(&mut graph);
    }
    match result {
        Ok(()) => {}
        Err(error) if errno_of(&error) == libc::ENOSPC => {
            let shown = path.display();
            let reason = format!("media device {shown} returned a malformed entity list");
            return Err(ProviderError::new(CODE_IO_OPEN, reason));
        }
        Err(error) => return failed("failed to read the media topology of", error),
    }
    let bus_info = trim_c_space(&bounded_string(&info.bus_info)).to_string();
    let pad_index_known = info.versions[0] >= MEDIA_V2_PAD_INDEX_VERSION;
    let mut sensors = Vec::new();
    let entities = graph.entities.iter();
    for entity in entities.filter(|entity| entity.function == MEDIA_ENT_F_CAM_SENSOR) {
        let name = bounded_string(&entity.name);
        if name.is_empty() {
            let (path, id) = (path.display(), entity.id);
            let reason = format!("media device {path} has an unnamed sensor entity {id}");
            return Err(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
        }
        let media_path = path.to_path_buf();
        let (media_device, bus_info) = (path.to_string_lossy().into_owned(), bus_info.clone());
        let link = source_link(&graph, entity.id);
        let csi_receiver = link
            .map(|(_, receiver)| bounded_string(&receiver.name))
            .filter(|name| !name.is_empty());
        let pad = link.map(|(pad, _)| pad.index).filter(|_| pad_index_known);
        let subdev = subdev_devnode(&graph, entity.id).zip(pad);
        sensors.push(Sensor {
            name,
            media_path,
            media_device,
            bus_info,
            csi_receiver,
            subdev,
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

impl IspMode {
    /// The output a client can request. Provenance explains the evidence for
    /// the rate, but is not part of whether two ISP nodes share that output.
    fn same_output(&self, other: &Self) -> bool {
        self.format == other.format
            && self.width == other.width
            && self.height == other.height
            && self.framerate_num == other.framerate_num
            && self.framerate_den == other.framerate_den
            && self.isp_output == other.isp_output
    }
}

type IspModes = (Vec<String>, BTreeSet<IspMode>);

/// Keep the outputs both sets can produce. An explicit interval from any node
/// constrains a shared rate, so sensor timing must not expand that rate later.
fn common_modes(left: &BTreeSet<IspMode>, right: &BTreeSet<IspMode>) -> BTreeSet<IspMode> {
    left.iter()
        .filter_map(|mode| {
            let other = right.iter().find(|candidate| mode.same_output(candidate))?;
            let mut shared = mode.clone();
            if other.framerate_source == "isp" {
                shared.framerate_source = "isp";
            }
            Some(shared)
        })
        .collect()
}

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
        *shared = common_modes(shared, &modes);
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

fn normalized_rate(mut numerator: u32, mut denominator: u32) -> (u32, u32) {
    let (original_numerator, original_denominator) = (numerator, denominator);
    while denominator != 0 {
        (numerator, denominator) = (denominator, numerator % denominator);
    }
    (
        original_numerator / numerator,
        original_denominator / numerator,
    )
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
        let sizes: BTreeSet<_> = sizes.into_iter().flatten().collect();
        // As in the V4L2 provider, a zero dimension is a malformed size, not
        // a mode a client could open.
        if sizes
            .iter()
            .any(|&(width, height)| width == 0 || height == 0)
        {
            return Err(malformed("frame size", path));
        }
        for (width, height) in sizes {
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
            for (rate, framerate_source) in rates {
                let (framerate_num, framerate_den) = normalized_rate(rate.0, rate.1);
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
