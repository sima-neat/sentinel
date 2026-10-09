//! Read-only discovery of SiMa MIPI CSI-2 cameras (`camera.mipi`).
//!
//! Like SiMa's libcamera `modalix` pipeline handler, without loading
//! libcamera: each `/dev/mediaN` is opened read-only, only the `simaai-v4l2-vid`
//! driver is admitted, and each `MEDIA_ENT_F_CAM_SENSOR` entity is a camera
//! whose entity name (`imx477 5-001a`), as `CameraInput` accepts it, is the id.
//! The ISP is not in that graph: its modes come from the V4L2 nodes with sysfs
//! name `isp_v4l2-vid-cap-out` and card `arm-isp-out` (enumeration ioctls only;
//! several nodes report the modes they share). A missing or unreadable ISP
//! leaves the cameras with `modes: []`. An ISP whose driver sets its sizes at
//! run time (it lists 0x0 until it is configured, as on Platform 3.0) gives
//! only formats: each camera's modes are then those formats at the discrete
//! frame sizes of its sensor's sub-device.
//!
//! The same topology read names the CSI-2 receiver the sensor's source pad
//! links to and the sensor's `/dev/v4l-subdevN`, whose active format and
//! pixel-rate and blanking controls give the sensor's frame-rate limit. Both
//! are optional: a graph or sub-device that does not provide them omits them.

mod ioctl;
#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::ops::ControlFlow::{self, Continue};
use std::path::{Path, PathBuf};

use super::camera::{
    Camera, Fraction, Interval, Isp, IspSizing, MipiCamera, Mode, SensorTiming, SizeIntervals,
    Source,
};
use super::sysutil::{
    bounded_string, disappeared, errno_of, io_error, os_message, read_text_file, trim_c_space,
    vanished, CODE_DISCOVERY_FAILED,
};
use super::v4l2::{decode_interval, decode_size, interval_order, value_cmp};
use super::videodev2::{
    effective_capabilities, enumerate, fourcc_string, Capability, EnumerationError, FmtDesc,
    FrmIvalEnum, FrmSizeEnum, MAX_DEVICE_ENUMERATIONS, MAX_ENUMERATION_ENTRIES,
    V4L2_BUF_TYPE_VIDEO_CAPTURE, V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, V4L2_CAP_VIDEO_CAPTURE,
    V4L2_CAP_VIDEO_CAPTURE_MPLANE, V4L2_FRMSIZE_TYPE_DISCRETE,
};
use super::{Availability, AvailabilityState};
use super::{Peripheral, Provider, ProviderError};
use ioctl::*;

pub const PROVIDER_NAME: &str = "camera.mipi";

const SIMA_MEDIA_DRIVER: &str = "simaai-v4l2-vid";
const ISP_SYSFS_NAME: &str = "isp_v4l2-vid-cap-out";
const ISP_CARD_NAME: &str = "arm-isp-out";

const AVAILABILITY_REASON: &str = "The media controller does not expose a reliable read-only \
ownership state; discovery does not acquire, configure, or stream from the camera.";

/// The `camera.mipi` provider.
pub struct MipiProvider {
    sys_root: PathBuf,
    dev_root: PathBuf,
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
            backend: Box::new(SystemBackend),
        }
    }
}

impl Provider for MipiProvider {
    fn name(&self) -> &'static str {
        PROVIDER_NAME
    }

    fn subsystems(&self) -> &'static [&'static str] {
        &["media", "video4linux"]
    }

    fn discover(&mut self) -> Result<Vec<Peripheral>, ProviderError> {
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
        if sensors.is_empty() {
            return Ok(Vec::new());
        }
        // Name order is id order.
        sensors.sort_by(|a, b| a.name.cmp(&b.name));
        // The id is the sensor name, which is also how CameraInput selects a
        // camera, so two sensors with one name could not be told apart.
        if let Some(pair) = sensors.windows(2).find(|pair| pair[0].name == pair[1].name) {
            let reason = format!(
                "sensor '{}' is reported by both {} and {}, so its camera id would not be unique",
                pair[0].name, pair[0].media_device, pair[1].media_device
            );
            return Err(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
        }
        let probe = probe_isp(&self.sys_root, &self.dev_root, backend);
        let cameras = sensors.into_iter().map(|sensor| {
            let timing = sensor
                .subdev
                .and_then(|(devnode, pad)| self.sensor_timing(devnode, pad));
            let (isp, modes) = match &probe {
                Ok(probe) if probe.sizing == IspSizing::Runtime => {
                    match self.sensor_sizes(sensor.subdev) {
                        Ok(sizes) => (probe.isp(), runtime_modes(&probe.formats, &sizes)),
                        Err(reason) => (Isp::Unavailable { reason }, Vec::new()),
                    }
                }
                Ok(probe) => (
                    probe.isp(),
                    probe.modes.iter().map(IspMode::to_mode).collect(),
                ),
                Err(reason) => (
                    Isp::Unavailable {
                        reason: reason.clone(),
                    },
                    Vec::new(),
                ),
            };
            let model = sensor.name.split(' ').next().unwrap_or_default();
            Peripheral::Camera(Camera {
                id: format!("camera:{}", sensor.name),
                model: (!model.is_empty()).then(|| model.to_string()),
                availability: Availability {
                    state: AvailabilityState::Unknown,
                    reason: Some(AVAILABILITY_REASON.to_string()),
                    subdevices: None,
                    subdevices_available: None,
                },
                modes,
                source: Source::Mipi(MipiCamera {
                    media_device: sensor.media_device,
                    bus_info: (!sensor.bus_info.is_empty()).then_some(sensor.bus_info),
                    isp,
                    csi_receiver: sensor.csi_receiver,
                    max_fps: timing.as_ref().map(max_fps),
                    sensor_timing: timing,
                    camera_name: sensor.name,
                }),
            })
        });
        Ok(cameras.collect())
    }
}

struct Sensor {
    name: String,
    media_device: String,
    bus_info: String,
    /// The entity the sensor's source pad links to.
    csi_receiver: Option<String>,
    /// The sensor's sub-device interface `(major, minor)` and source pad index.
    subdev: Option<((u32, u32), u32)>,
}

/// Pixels per second over pixels per frame at minimum blanking, to two
/// decimals.
fn max_fps(timing: &SensorTiming) -> f64 {
    let line = f64::from(timing.width) + f64::from(timing.hblank_min);
    let frame = line * (f64::from(timing.height) + f64::from(timing.vblank_min));
    (timing.pixel_rate as f64 / frame * 100.0).round() / 100.0
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

/// Each discrete sensor frame size, with the frame intervals the sensor
/// reports for it.
type SensorSizes = BTreeMap<(u32, u32), Vec<Interval>>;

impl MipiProvider {
    /// The `/dev/v4l-subdevN` of the sub-device `devnode`, as
    /// `/sys/dev/char/<major>:<minor>` names it.
    fn subdev_path(&self, (major, minor): (u32, u32)) -> Option<PathBuf> {
        let class = self.sys_root.join(format!("dev/char/{major}:{minor}"));
        let name = fs::read_link(class).ok()?.file_name()?.to_owned();
        let subdev = name.to_string_lossy().starts_with("v4l-subdev");
        subdev.then(|| self.dev_root.join(name))
    }

    /// The discrete frame sizes of the sensor's source pad over all its
    /// media-bus codes, each with the frame intervals the sensor reports for
    /// it (none when its driver lists none). Stepwise and continuous sizes are
    /// skipped. Fails with the reason when the sizes cannot be listed.
    fn sensor_sizes(&self, subdev: Option<((u32, u32), u32)>) -> Result<SensorSizes, String> {
        let found = subdev.and_then(|(devnode, pad)| Some((self.subdev_path(devnode)?, pad)));
        let (path, pad) = found.ok_or("the sensor has no sub-device to list its frame sizes")?;
        let node = self.backend.open_subdev(&path);
        let mut node = node.map_err(|error| describe("could not open", &path, &error))?;
        let (mut budget, device) = (MAX_DEVICE_ENUMERATIONS, "sensor sub-device");
        let which = V4L2_SUBDEV_FORMAT_ACTIVE;
        let ioctl = ("VIDIOC_SUBDEV_ENUM_MBUS_CODE", "media bus code");
        let query = |index| {
            let mut value = SubdevMbusCodeEnum {
                pad,
                index,
                which,
                ..SubdevMbusCodeEnum::default()
            };
            node.enum_mbus_code(&mut value).map(|()| value.code)
        };
        let codes = node_list(device, &mut budget, &path, ioctl, query, |code| {
            Some(Continue(code))
        })?;
        let mut sizes = SensorSizes::new();
        for code in codes {
            let ioctl = ("VIDIOC_SUBDEV_ENUM_FRAME_SIZE", "frame size");
            let query = |index| {
                let mut value = SubdevFrameSizeEnum {
                    index,
                    pad,
                    code,
                    which,
                    ..SubdevFrameSizeEnum::default()
                };
                node.enum_mbus_size(&mut value).map(|()| value)
            };
            let listed = node_list(device, &mut budget, &path, ioctl, query, decode_mbus_size)?;
            for (width, height) in listed.into_iter().flatten() {
                let ioctl = ("VIDIOC_SUBDEV_ENUM_FRAME_INTERVAL", "frame interval");
                let query = |index| {
                    let mut value = SubdevFrameIntervalEnum {
                        index,
                        pad,
                        code,
                        width,
                        height,
                        which,
                        ..SubdevFrameIntervalEnum::default()
                    };
                    match node.enum_mbus_interval(&mut value) {
                        // A driver without the ioctl reports no intervals.
                        Err(error) if errno_of(&error) == libc::ENOTTY => {
                            Err(io::Error::from_raw_os_error(libc::EINVAL))
                        }
                        result => result.map(|()| value.interval),
                    }
                };
                let decode = decode_mbus_interval;
                let intervals = node_list(device, &mut budget, &path, ioctl, query, decode)?;
                sizes.entry((width, height)).or_default().extend(intervals);
            }
        }
        for intervals in sizes.values_mut() {
            intervals.sort_by(interval_order);
            intervals.dedup_by(|left, right| same_interval(left, right));
        }
        if sizes.is_empty() {
            let path = path.display();
            return Err(format!(
                "sensor sub-device {path} reported no discrete frame sizes"
            ));
        }
        Ok(sizes)
    }

    /// The timing of the sensor whose sub-device is `devnode`, read from
    /// `/dev/v4l-subdevN` (named by `/sys/dev/char/<major>:<minor>`): the
    /// active format of `pad`, the current pixel rate, and the minimum
    /// horizontal and vertical blanking. `None` when any part is missing.
    fn sensor_timing(&self, devnode: (u32, u32), pad: u32) -> Option<SensorTiming> {
        let mut node = self.backend.open_subdev(&self.subdev_path(devnode)?).ok()?;
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

fn sorted_names(directory: &Path) -> io::Result<Vec<OsString>> {
    let entries = fs::read_dir(directory)?.map(|entry| entry.map(|entry| entry.file_name()));
    let mut names = entries.collect::<io::Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

/// `/dev/mediaN`, sorted; a missing device directory means none. Only `EACCES`
/// is a permission failure when listing a directory.
fn media_device_paths(dev_root: &Path) -> Result<Vec<PathBuf>, ProviderError> {
    let names = match sorted_names(dev_root) {
        Err(error) if errno_of(&error) == libc::ENOENT => return Ok(Vec::new()),
        names => names.map_err(|error| io_error("failed to read", dev_root, &error, false))?,
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
        false => Err(io_error(what, path, &error, true)),
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
    // One call reads the whole graph atomically; a graph with more objects of
    // one kind than the room fails with ENOSPC.
    let room = MAX_ENUMERATION_ENTRIES as usize;
    let mut graph = Graph {
        entities: vec![MediaV2Entity::default(); room],
        interfaces: vec![MediaV2Interface::default(); room],
        pads: vec![MediaV2Pad::default(); room],
        links: vec![MediaV2Link::default(); room],
    };
    if let Err(error) = node.topology(&mut graph) {
        return failed("failed to read the media topology of", error);
    }
    let bus_info = trim_c_space(&bounded_string(&info.bus_info)).to_string();
    let mut sensors = Vec::new();
    let entities = graph.entities.iter();
    for entity in entities.filter(|entity| entity.function == MEDIA_ENT_F_CAM_SENSOR) {
        let name = bounded_string(&entity.name);
        if name.is_empty() {
            let (path, id) = (path.display(), entity.id);
            let reason = format!("media device {path} has an unnamed sensor entity {id}");
            return Err(ProviderError::new(CODE_DISCOVERY_FAILED, reason));
        }
        let link = source_link(&graph, entity.id);
        let csi_receiver = link
            .map(|(_, receiver)| bounded_string(&receiver.name))
            .filter(|name| !name.is_empty());
        let pad = link.map(|(pad, _)| pad.index);
        sensors.push(Sensor {
            name,
            media_device: path.to_string_lossy().into_owned(),
            bus_info: bus_info.clone(),
            csi_receiver,
            subdev: subdev_devnode(&graph, entity.id).zip(pad),
        });
    }
    Ok(sensors)
}

/// One ISP output mode; field order is the sort order. `intervals` are the
/// frame intervals the ISP reports for the size, if any.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct IspMode {
    format: String,
    width: u32,
    height: u32,
    intervals: Vec<Interval>,
}

impl IspMode {
    /// The catalog mode: an ISP output size.
    fn to_mode(&self) -> Mode {
        Mode {
            format: self.format.clone(),
            format_description: None,
            width: Some(self.width),
            height: Some(self.height),
            size_range: None,
            frame_intervals: (!self.intervals.is_empty())
                .then(|| SizeIntervals {
                    width: self.width,
                    height: self.height,
                    intervals: self.intervals.clone(),
                })
                .into_iter()
                .collect(),
            isp_output: Some(true),
            sensor_mode: None,
            available: None,
            reason: None,
        }
    }
}

/// A camera's modes when the ISP sets its sizes at run time: each ISP format
/// at each of the sensor's sizes, with the sensor's frame intervals.
fn runtime_modes(formats: &BTreeSet<String>, sizes: &SensorSizes) -> Vec<Mode> {
    let mode =
        |format: &String, (&(width, height), intervals): (&(u32, u32), &Vec<Interval>)| Mode {
            format: format.clone(),
            format_description: None,
            width: Some(width),
            height: Some(height),
            size_range: None,
            frame_intervals: (!intervals.is_empty())
                .then(|| SizeIntervals {
                    width,
                    height,
                    intervals: intervals.clone(),
                })
                .into_iter()
                .collect(),
            isp_output: None,
            sensor_mode: Some(true),
            available: None,
            reason: None,
        };
    let modes = formats
        .iter()
        .flat_map(|format| sizes.iter().map(move |size| mode(format, size)));
    modes.collect()
}

/// A discrete sensor frame size, or `None` for a range.
type SensorSize = Option<(u32, u32)>;

/// A sensor frame size: `Some` when discrete, `None` for a range, which is
/// skipped; malformed when zero or inverted.
fn decode_mbus_size(value: SubdevFrameSizeEnum) -> Option<ControlFlow<SensorSize, SensorSize>> {
    let (width, height) = (value.min_width, value.min_height);
    let valid = width != 0 && height != 0;
    let valid = valid && value.max_width >= width && value.max_height >= height;
    let discrete = value.max_width == width && value.max_height == height;
    valid.then_some(Continue(discrete.then_some((width, height))))
}

/// A sensor frame interval, seconds per frame; malformed when zero.
fn decode_mbus_interval(
    [numerator, denominator]: [u32; 2],
) -> Option<ControlFlow<Interval, Interval>> {
    let valid = numerator != 0 && denominator != 0;
    valid.then_some(Continue(Interval::Discrete(Fraction {
        numerator,
        denominator,
    })))
}

/// What the ISP output nodes report: their paths and sizing, and the modes
/// they share (fixed sizing) or the formats they share (run-time sizing).
struct IspProbe {
    paths: Vec<String>,
    sizing: IspSizing,
    modes: BTreeSet<IspMode>,
    formats: BTreeSet<String>,
}

impl IspProbe {
    fn isp(&self) -> Isp {
        Isp::Available {
            device_path: self.paths[0].clone(),
            device_paths: self.paths.clone(),
            sizing: self.sizing,
        }
    }
}

/// One ISP output node: its sizing, formats and, with a fixed table, modes.
struct IspNode {
    sizing: IspSizing,
    formats: BTreeSet<String>,
    modes: BTreeSet<IspMode>,
}

/// Every `/sys/class/video4linux` entry named like the ISP output, in sorted
/// order. Nodes with another card, or with a fixed table and no discrete size,
/// are skipped, and the first failing node makes the ISP unavailable. One node
/// with run-time sizing makes the ISP's sizing run-time: a node the ISP is
/// configured for lists a real size, the others 0x0. With fixed tables,
/// several nodes contribute only the formats and sizes they all share, with
/// the frame intervals they all report; with run-time sizing, the ISP gives
/// the formats every node lists.
fn probe_isp(sys_root: &Path, dev_root: &Path, backend: &dyn Backend) -> Result<IspProbe, String> {
    let class = sys_root.join("class/video4linux");
    let names = sorted_names(&class).map_err(|error| describe("could not read", &class, &error))?;
    let (mut paths, mut common) = (Vec::new(), None::<BTreeSet<IspMode>>);
    let (mut sizing, mut formats) = (None, None::<BTreeSet<String>>);
    for name in names {
        let entry = class.join(&name);
        let sysfs_name = entry.join("name");
        let card = read_text_file(&sysfs_name)
            .map_err(|error| describe("could not read", &sysfs_name, &error))?;
        if card.as_deref() != Some(ISP_SYSFS_NAME) {
            continue;
        }
        let path = dev_root.join(&name);
        let node = match isp_node(backend, &path) {
            Ok(Some(node)) => node,
            Ok(None) => continue,
            Err(_) if vanished(&entry) => continue,
            Err(error) => return Err(error),
        };
        if node.sizing == IspSizing::Fixed && node.modes.is_empty() {
            continue;
        }
        if sizing != Some(IspSizing::Runtime) {
            sizing = Some(node.sizing);
        }
        paths.push(path.to_string_lossy().into_owned());
        formats = Some(match formats {
            Some(shared) => &shared & &node.formats,
            None => node.formats,
        });
        if node.sizing == IspSizing::Runtime {
            continue;
        }
        let modes = node.modes;
        common = Some(match common {
            Some(shared) => shared
                .into_iter()
                .filter_map(|mut mode| {
                    let other = modes.iter().find(|other| {
                        (&other.format, other.width, other.height)
                            == (&mode.format, mode.width, mode.height)
                    })?;
                    let shared = |interval: &Interval| {
                        other
                            .intervals
                            .iter()
                            .any(|other| same_interval(interval, other))
                    };
                    mode.intervals.retain(shared);
                    Some(mode)
                })
                .collect(),
            None => modes,
        });
    }
    let (modes, formats) = (common.unwrap_or_default(), formats.unwrap_or_default());
    let reason = match sizing {
        None => "no Modalix ISP output node was found",
        Some(IspSizing::Runtime) if formats.is_empty() => {
            "Modalix ISP output nodes reported no common formats"
        }
        Some(IspSizing::Fixed) if modes.is_empty() => {
            "Modalix ISP output nodes reported no common discrete sizes"
        }
        Some(sizing) => {
            return Ok(IspProbe {
                paths,
                sizing,
                modes,
                formats,
            })
        }
    };
    Err(reason.to_string())
}

/// One ISP frame size: `Some` when discrete, `None` for a range. A discrete
/// 0x0 is what an ISP that sets its sizes at run time lists until it is
/// configured; any other zero side is malformed, as [`decode_size`] reads it.
fn decode_isp_size(value: FrmSizeEnum) -> Option<ControlFlow<IspSize, IspSize>> {
    if value.kind == V4L2_FRMSIZE_TYPE_DISCRETE && value.data[..2] == [0, 0] {
        return Some(Continue(Some((0, 0))));
    }
    let discrete =
        |(width, height, range): (u32, u32, Option<_>)| range.is_none().then_some((width, height));
    Some(match decode_size(value)? {
        Continue(size) => Continue(discrete(size)),
        ControlFlow::Break(size) => ControlFlow::Break(discrete(size)),
    })
}

/// A discrete ISP frame size, or `None` for a range.
type IspSize = Option<(u32, u32)>;

/// Whether two intervals are the same periods, whatever their fractions'
/// terms (`1/30` and `2/60` are one interval).
fn same_interval(left: &Interval, right: &Interval) -> bool {
    let same = |left: &Fraction, right: &Fraction| value_cmp(left, right) == Ordering::Equal;
    match (left, right) {
        (Interval::Discrete(l), Interval::Discrete(r)) => same(l, r),
        (
            Interval::Stepwise {
                minimum,
                maximum,
                step,
            },
            Interval::Stepwise {
                minimum: m,
                maximum: x,
                step: s,
            },
        )
        | (
            Interval::Continuous {
                minimum,
                maximum,
                step,
            },
            Interval::Continuous {
                minimum: m,
                maximum: x,
                step: s,
            },
        ) => same(minimum, m) && same(maximum, x) && same(step, s),
        _ => false,
    }
}

/// [`enumerate`] one list of the `device` node at `path`, with its failure as
/// the reason the camera's modes are unavailable.
fn node_list<T, D>(
    device: &str,
    budget: &mut u32,
    path: &Path,
    (ioctl, list): (&str, &str),
    query: impl FnMut(u32) -> io::Result<T>,
    decode: impl FnMut(T) -> Option<ControlFlow<D, D>>,
) -> Result<Vec<D>, String> {
    enumerate(budget, list, query, decode).map_err(|error| match error {
        EnumerationError::Malformed(what) => {
            format!("{device} {} returned a malformed {what}", path.display())
        }
        EnumerationError::Failed(error) => describe(&format!("{ioctl} failed for"), path, &error),
    })
}

/// One candidate ISP node, opened read-only; `None` when its card is not the
/// ISP output's. A node that lists a 0x0 size sets its sizes at run time and
/// gives only its formats: the sizes it reports are only those it is
/// configured for. Each discrete size of a fixed table carries the frame
/// intervals the ISP reports for it, if any.
fn isp_node(backend: &dyn Backend, path: &Path) -> Result<Option<IspNode>, String> {
    let node = backend.open_video(path);
    let mut node = node.map_err(|error| describe("could not open", path, &error))?;
    let mut capability = Capability::default();
    let queried = node.query_capability(&mut capability);
    queried.map_err(|error| describe("VIDIOC_QUERYCAP failed for", path, &error))?;
    if bounded_string(&capability.card) != ISP_CARD_NAME {
        return Ok(None);
    }
    // The formats of every advertised capture API, each enumerated once:
    // sizes and intervals are keyed by pixel format alone.
    let capabilities = effective_capabilities(&capability);
    let mut budget = MAX_DEVICE_ENUMERATIONS;
    let mut formats = BTreeSet::new();
    for (capture, buf_type) in [
        (V4L2_CAP_VIDEO_CAPTURE, V4L2_BUF_TYPE_VIDEO_CAPTURE),
        (
            V4L2_CAP_VIDEO_CAPTURE_MPLANE,
            V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
        ),
    ] {
        if capabilities & capture == 0 {
            continue;
        }
        let ioctl = ("VIDIOC_ENUM_FMT", "format");
        let query = |index| {
            let mut value = FmtDesc {
                index,
                buf_type,
                ..FmtDesc::default()
            };
            node.enum_format(&mut value).map(|()| value.pixelformat)
        };
        let listed = node_list("ISP node", &mut budget, path, ioctl, query, |format| {
            Some(Continue(format))
        })?;
        formats.extend(listed);
    }
    let names = formats
        .iter()
        .map(|&format| fourcc_string(format))
        .collect();
    let (mut modes, mut unset) = (BTreeSet::new(), false);
    for pixel_format in formats {
        let ioctl = ("VIDIOC_ENUM_FRAMESIZES", "frame size");
        let query = |index| {
            let mut value = FrmSizeEnum {
                index,
                pixel_format,
                ..FrmSizeEnum::default()
            };
            node.enum_frame_size(&mut value).map(|()| value)
        };
        let sizes = node_list("ISP node", &mut budget, path, ioctl, query, decode_isp_size)?;
        let mut sizes: BTreeSet<_> = sizes.into_iter().flatten().collect();
        unset |= sizes.remove(&(0, 0));
        for (width, height) in sizes {
            let ioctl = ("VIDIOC_ENUM_FRAMEINTERVALS", "frame interval");
            let query = |index| {
                let mut value = FrmIvalEnum {
                    index,
                    pixel_format,
                    width,
                    height,
                    ..FrmIvalEnum::default()
                };
                match node.enum_frame_interval(&mut value) {
                    // A driver without the ioctl reports no intervals.
                    Err(error) if errno_of(&error) == libc::ENOTTY => {
                        Err(io::Error::from_raw_os_error(libc::EINVAL))
                    }
                    result => result.map(|()| value),
                }
            };
            let mut intervals =
                node_list("ISP node", &mut budget, path, ioctl, query, decode_interval)?;
            intervals.sort_by(interval_order);
            intervals.dedup_by(|left, right| same_interval(left, right));
            modes.insert(IspMode {
                format: fourcc_string(pixel_format),
                width,
                height,
                intervals,
            });
        }
    }
    let (sizing, modes) = match unset {
        true => (IspSizing::Runtime, BTreeSet::new()),
        false => (IspSizing::Fixed, modes),
    };
    Ok(Some(IspNode {
        sizing,
        formats: names,
        modes,
    }))
}
