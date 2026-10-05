//! Read-only discovery of USB/UVC cameras exposed through V4L2
//! (`camera.v4l2`).
//!
//! The scan walks `/sys/class/video4linux`, admits only nodes whose sysfs device has a
//! USB ancestor (platform and ISP nodes are skipped), opens each candidate
//! with `O_RDONLY | O_NONBLOCK | O_CLOEXEC`, and issues query ioctls only.
//! Metadata-only, output-only and memory-to-memory nodes are dropped after
//! `VIDIOC_QUERYCAP`. Camera ids are derived from the USB topology, interface
//! number and composite-node index, never from `/dev/videoN`.

#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::ops::ControlFlow::{self, Break, Continue};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::camera::{Availability, AvailabilityState, Camera, Fraction, Interval, Mode};
use super::camera::{RangeKind, SizeIntervals, SizeRange, Source, UsbCamera, UsbIdentity};
use super::sysutil::{
    bounded_string, disappeared, errno_of, io_error, read_text_file, trim_c_space, vanished,
    CODE_DISCOVERY_FAILED, CODE_IO_OPEN,
};
use super::videodev2::{
    effective_capabilities, enumerate, fourcc_string, open_read_only, Capability, EnumerationError,
    FmtDesc, FrmIvalEnum, FrmSizeEnum, VideoNode, MAX_DEVICE_ENUMERATIONS, MAX_ENUMERATION_ENTRIES,
    V4L2_BUF_TYPE_VIDEO_CAPTURE, V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, V4L2_CAP_VIDEO_CAPTURE,
    V4L2_CAP_VIDEO_CAPTURE_MPLANE, V4L2_CAP_VIDEO_M2M, V4L2_CAP_VIDEO_M2M_MPLANE,
    V4L2_FRMIVAL_TYPE_CONTINUOUS, V4L2_FRMIVAL_TYPE_DISCRETE, V4L2_FRMIVAL_TYPE_STEPWISE,
    V4L2_FRMSIZE_TYPE_CONTINUOUS, V4L2_FRMSIZE_TYPE_DISCRETE, V4L2_FRMSIZE_TYPE_STEPWISE,
};
use super::{Peripheral, Provider, ProviderError};

pub const PROVIDER_NAME: &str = "camera.v4l2";

const AVAILABILITY_REASON: &str = "V4L2 does not expose a reliable read-only ownership state; \
discovery does not acquire, configure, or stream from the camera.";

/// Opens a V4L2 node; tests substitute fake nodes.
type Opener = Box<dyn Fn(&Path) -> io::Result<Box<dyn VideoNode>> + Send>;

/// The `camera.v4l2` provider.
pub struct V4l2Provider {
    sys_root: PathBuf,
    dev_root: PathBuf,
    open: Opener,
}

impl V4l2Provider {
    /// Scan the live system (`/sys`, `/dev`).
    pub fn new() -> Self {
        Self::with_opener("/sys", "/dev", Box::new(open_system))
    }

    fn with_opener(
        sys_root: impl Into<PathBuf>,
        dev_root: impl Into<PathBuf>,
        open: Opener,
    ) -> Self {
        Self {
            sys_root: sys_root.into(),
            dev_root: dev_root.into(),
            open,
        }
    }
}

fn open_system(path: &Path) -> io::Result<Box<dyn VideoNode>> {
    Ok(Box::new(open_read_only(path)?))
}

impl Provider for V4l2Provider {
    fn name(&self) -> &'static str {
        PROVIDER_NAME
    }

    fn subsystems(&self) -> &'static [&'static str] {
        &["video4linux"]
    }

    fn discover(&mut self) -> Result<Vec<Peripheral>, ProviderError> {
        self.scan().map_err(|failure| failure.error)
    }
}

/// A failed scan. `errno` is the failed OS call's error (0 otherwise), so a
/// node that vanished mid-scan can be skipped instead.
struct Failure {
    errno: i32,
    error: ProviderError,
}

/// [`io_error`] with its errno.
fn failed(action: &str, path: &Path, error: &io::Error, eperm: bool) -> Failure {
    let (errno, error) = (errno_of(error), io_error(action, path, error, eperm));
    Failure { errno, error }
}

pub(super) fn value_cmp(left: &Fraction, right: &Fraction) -> Ordering {
    let left_value = u64::from(left.numerator) * u64::from(right.denominator);
    left_value.cmp(&(u64::from(right.numerator) * u64::from(left.denominator)))
}

/// A discrete period or the minimum of a range.
pub(super) fn shortest(interval: &Interval) -> Fraction {
    match *interval {
        Interval::Discrete(period) => period,
        Interval::Stepwise { minimum, .. } | Interval::Continuous { minimum, .. } => minimum,
    }
}

/// Discrete before stepwise before continuous, then by the shortest period's
/// value, then by its terms.
pub(super) fn interval_order(left: &Interval, right: &Interval) -> Ordering {
    let key = |interval: &Interval| {
        let kind = match interval {
            Interval::Discrete(_) => 0,
            Interval::Stepwise { .. } => 1,
            Interval::Continuous { .. } => 2,
        };
        (kind, shortest(interval))
    };
    let ((left_kind, left), (right_kind, right)) = (key(left), key(right));
    left_kind
        .cmp(&right_kind)
        .then_with(|| value_cmp(&left, &right))
        .then_with(|| (left.numerator, left.denominator).cmp(&(right.numerator, right.denominator)))
}

/// By format, then discrete sizes before stepwise and continuous ranges, then
/// by dimensions.
fn mode_key(mode: &Mode) -> (&str, u8, [u32; 6]) {
    let Some(range) = mode.size_range else {
        let (width, height) = (
            mode.width.unwrap_or_default(),
            mode.height.unwrap_or_default(),
        );
        return (&mode.format, 0, [width, height, 0, 0, 0, 0]);
    };
    let kind = match range.kind {
        RangeKind::Stepwise => 1,
        RangeKind::Continuous => 2,
    };
    let SizeRange {
        min_width,
        min_height,
        max_width,
        max_height,
        step_width,
        step_height,
        ..
    } = range;
    let dims = [
        min_width,
        min_height,
        max_width,
        max_height,
        step_width,
        step_height,
    ];
    (&mode.format, kind, dims)
}

/// A frame size: its width and height, or the minimum of its `range`.
type Size = (u32, u32, Option<SizeRange>);

/// A stepwise or continuous range ends the list.
pub(super) fn decode_size(value: FrmSizeEnum) -> Option<ControlFlow<Size, Size>> {
    let kind = match value.kind {
        V4L2_FRMSIZE_TYPE_DISCRETE => {
            let [width, height, ..] = value.data;
            return (width != 0 && height != 0).then_some(Continue((width, height, None)));
        }
        V4L2_FRMSIZE_TYPE_STEPWISE => RangeKind::Stepwise,
        V4L2_FRMSIZE_TYPE_CONTINUOUS => RangeKind::Continuous,
        _ => return None,
    };
    let [min_width, max_width, step_width, min_height, max_height, step_height] = value.data;
    let range = SizeRange {
        kind,
        min_width,
        min_height,
        max_width,
        max_height,
        step_width,
        step_height,
    };
    let valid = min_width != 0 && min_height != 0 && max_width >= min_width;
    let valid = valid && max_height >= min_height && step_width != 0 && step_height != 0;
    valid.then_some(Break((min_width, min_height, Some(range))))
}

/// `data` is the discrete fraction, or the `min`, `max` and `step` fractions
/// of a range, which ends the list.
pub(super) fn decode_interval(value: FrmIvalEnum) -> Option<ControlFlow<Interval, Interval>> {
    let fraction = |at: usize| {
        let (numerator, denominator) = (value.data[2 * at], value.data[2 * at + 1]);
        (numerator != 0 && denominator != 0).then_some(Fraction {
            numerator,
            denominator,
        })
    };
    if value.kind == V4L2_FRMIVAL_TYPE_DISCRETE {
        return Some(Continue(Interval::Discrete(fraction(0)?)));
    }
    let (minimum, maximum, step) = (fraction(0)?, fraction(1)?, fraction(2)?);
    let interval = match value.kind {
        V4L2_FRMIVAL_TYPE_STEPWISE => Interval::Stepwise {
            minimum,
            maximum,
            step,
        },
        V4L2_FRMIVAL_TYPE_CONTINUOUS => Interval::Continuous {
            minimum,
            maximum,
            step,
        },
        _ => return None,
    };
    value_cmp(&minimum, &maximum)
        .is_le()
        .then_some(Break(interval))
}

/// The enumeration ioctls of one open node; `budget` is the queries left for
/// the device.
struct Enumerator<'a> {
    node: Box<dyn VideoNode>,
    path: &'a Path,
    budget: u32,
}

impl Enumerator<'_> {
    fn list<T, D>(
        &mut self,
        list: &str,
        mut query: impl FnMut(&mut dyn VideoNode, u32) -> io::Result<T>,
        decode: impl FnMut(T) -> Option<ControlFlow<D, D>>,
    ) -> Result<Vec<D>, Failure> {
        let node = self.node.as_mut();
        let entries = enumerate(&mut self.budget, list, |index| query(node, index), decode);
        entries.map_err(|error| match error {
            EnumerationError::Malformed(what) => {
                let path = self.path.display();
                let reason = format!("V4L2 camera {path} returned a malformed {what}");
                Failure {
                    errno: 0,
                    error: ProviderError::new(CODE_IO_OPEN, reason),
                }
            }
            EnumerationError::Failed(error) => {
                let action = format!("failed to enumerate V4L2 {list}s for");
                failed(&action, self.path, &error, true)
            }
        })
    }

    /// The modes of every capture format, sorted. The single- and
    /// multi-planar listings of one format merge, keeping the first non-empty
    /// description.
    fn modes(&mut self, capabilities: u32) -> Result<Vec<Mode>, Failure> {
        let mut formats = Vec::new();
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
            let query = |node: &mut dyn VideoNode, index| {
                let mut value = FmtDesc {
                    index,
                    buf_type,
                    ..FmtDesc::default()
                };
                node.enum_format(&mut value).map(|()| value)
            };
            formats.extend(self.list("format", query, |value| Some(Continue(value)))?);
        }
        let mut descriptions = BTreeMap::new();
        for format in &formats {
            let description: &mut String = descriptions.entry(format.pixelformat).or_default();
            if description.is_empty() {
                *description = trim_c_space(&bounded_string(&format.description)).to_string();
            }
        }
        // Frame sizes and intervals are keyed by pixel format alone, so each
        // format is enumerated once however many lists it appears in.
        let mut modes = Vec::new();
        for (&pixel_format, description) in &descriptions {
            modes.extend(self.format_modes(pixel_format, description)?);
        }
        modes.sort_by(|left, right| mode_key(left).cmp(&mode_key(right)));
        modes.dedup_by(|left, right| mode_key(left) == mode_key(right));
        Ok(modes)
    }

    /// One mode per size, with every interval of each probed size; a size
    /// without intervals has no mode. A range is probed at its minimum and
    /// maximum.
    fn format_modes(&mut self, pixel_format: u32, description: &str) -> Result<Vec<Mode>, Failure> {
        let query = |node: &mut dyn VideoNode, index| {
            let mut value = FrmSizeEnum {
                index,
                pixel_format,
                ..FrmSizeEnum::default()
            };
            node.enum_frame_size(&mut value).map(|()| value)
        };
        let mut modes = Vec::new();
        for (width, height, size_range) in self.list("frame size", query, decode_size)? {
            let maximum = size_range.map(|range| (range.max_width, range.max_height));
            let probes = Some((width, height)).into_iter();
            let probes = probes.chain(maximum.filter(|&size| size != (width, height)));
            let mut frame_intervals = Vec::new();
            for (width, height) in probes {
                let query = |node: &mut dyn VideoNode, index| {
                    let mut value = FrmIvalEnum {
                        index,
                        pixel_format,
                        width,
                        height,
                        ..FrmIvalEnum::default()
                    };
                    node.enum_frame_interval(&mut value).map(|()| value)
                };
                let mut intervals = self.list("frame interval", query, decode_interval)?;
                intervals.sort_by(interval_order);
                intervals.dedup();
                frame_intervals.push(SizeIntervals {
                    width,
                    height,
                    intervals,
                });
            }
            if frame_intervals.iter().all(|size| size.intervals.is_empty()) {
                continue;
            }
            let discrete = size_range.is_none();
            modes.push(Mode {
                format: fourcc_string(pixel_format),
                format_description: (!description.is_empty()).then(|| description.to_string()),
                width: discrete.then_some(width),
                height: discrete.then_some(height),
                size_range,
                frame_intervals,
                isp_output: None,
            });
        }
        Ok(modes)
    }
}

/// The `/dev/v4l/by-id` links (udev's persistent names) by the node they
/// resolve to; the first in name order wins. A missing directory or an
/// unresolvable link contributes nothing.
fn by_id_links(dev_root: &Path) -> BTreeMap<PathBuf, PathBuf> {
    let directory = dev_root.join("v4l/by-id");
    let mut names: Vec<_> = fs::read_dir(&directory)
        .into_iter()
        .flatten()
        .take(MAX_ENUMERATION_ENTRIES as usize)
        .filter_map(|entry| Some(entry.ok()?.file_name()))
        .collect();
    names.sort();
    let mut links = BTreeMap::new();
    for name in names {
        let link = directory.join(name);
        if let Ok(node) = fs::canonicalize(&link) {
            links.entry(node).or_insert(link);
        }
    }
    links
}

impl V4l2Provider {
    fn scan(&self) -> Result<Vec<Peripheral>, Failure> {
        let class = self.sys_root.join("class/video4linux");
        let read_failure = |error: io::Error| failed("failed to read", &class, &error, false);
        let mut names = Vec::new();
        match fs::read_dir(&class) {
            Err(error) if errno_of(&error) == libc::ENOENT => return Ok(Vec::new()),
            entries => {
                for entry in entries.map_err(read_failure)? {
                    let name = entry.map_err(read_failure)?.file_name();
                    if name.as_bytes().starts_with(b"video") {
                        names.push(name);
                    }
                }
            }
        }
        names.sort();
        let sys = fs::canonicalize(&self.sys_root).map_err(|error| {
            failed(
                "failed to resolve V4L2 sysfs root",
                &self.sys_root,
                &error,
                false,
            )
        })?;
        let by_id = by_id_links(&self.dev_root);
        let mut cameras = Vec::new();
        for name in names {
            let (entry, device_path) = (class.join(&name), self.dev_root.join(&name));
            match self.probe(&sys, &entry, &device_path, &by_id) {
                Ok(camera) => cameras.extend(camera),
                // A node unplugged mid-scan is skipped: its failure says so,
                // or its class entry, removed before its attributes, is gone.
                Err(failure) if disappeared(failure.errno) || vanished(&entry) => {}
                Err(failure) => return Err(failure),
            }
        }
        Ok(cameras)
    }

    /// The camera of one `/sys/class/video4linux` entry; `Ok(None)` when it is
    /// not a USB video capture node.
    fn probe(
        &self,
        sys: &Path,
        entry: &Path,
        device_path: &Path,
        by_id: &BTreeMap<PathBuf, PathBuf>,
    ) -> Result<Option<Peripheral>, Failure> {
        let link = entry.join("device");
        let device = fs::canonicalize(&link)
            .map_err(|error| failed("failed to resolve V4L2 sysfs device", &link, &error, false))?;
        let Ok(device) = device.strip_prefix(sys) else {
            return Ok(None);
        };
        // The USB device is the nearest ancestor with `idVendor`; the
        // interface number is on the USB interface between it and the node.
        let read = |path: &Path| {
            read_text_file(path)
                .map_err(|error| failed("failed to read V4L2 sysfs attribute", path, &error, false))
        };
        let mut interface = None;
        let mut usb = None;
        for topology in device
            .ancestors()
            .take_while(|path| !path.as_os_str().is_empty())
        {
            let path = sys.join(topology);
            if interface.is_none() {
                interface = read(&path.join("bInterfaceNumber"))?;
            }
            let vendor = path.join("idVendor");
            let inspect = "failed to inspect V4L2 sysfs attribute";
            if vendor
                .try_exists()
                .map_err(|error| failed(inspect, &vendor, &error, false))?
            {
                usb = Some((path, topology));
                break;
            }
        }
        let Some((usb, topology)) = usb else {
            return Ok(None);
        };

        let mut node = (self.open)(device_path)
            .map_err(|error| failed("failed to open V4L2 camera", device_path, &error, true))?;
        let mut capability = Capability::default();
        node.query_capability(&mut capability).map_err(|error| {
            failed(
                "failed to query V4L2 capabilities for",
                device_path,
                &error,
                true,
            )
        })?;
        let capabilities = effective_capabilities(&capability);
        if capabilities & (V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_VIDEO_CAPTURE_MPLANE) == 0
            || capabilities & (V4L2_CAP_VIDEO_M2M | V4L2_CAP_VIDEO_M2M_MPLANE) != 0
        {
            return Ok(None);
        }
        let interface = interface.unwrap_or_default();
        let index = read(&entry.join("index"))?.unwrap_or_default();
        if interface.is_empty() || index.is_empty() {
            return Err(Failure {
                errno: 0,
                error: ProviderError::new(
                    CODE_DISCOVERY_FAILED,
                    "V4L2 USB camera did not publish stable topology, interface, and \
                     composite-node index attributes.",
                ),
            });
        }

        let modes = Enumerator {
            node,
            path: device_path,
            budget: MAX_DEVICE_ENUMERATIONS,
        }
        .modes(capabilities)?;

        let topology = topology.to_string_lossy();
        let stable_key = format!("sysfs:{topology}:interface={interface}:index={index}");
        let fnv1a = stable_key
            .bytes()
            .fold(14_695_981_039_346_656_037_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211)
            });
        let attribute =
            |name: &str| read(&usb.join(name)).map(|text| text.filter(|text| !text.is_empty()));
        let identity = UsbIdentity {
            stable_key,
            topology: topology.into_owned(),
            interface,
            node_index: index,
            vendor_id: attribute("idVendor")?,
            product_id: attribute("idProduct")?,
            serial: attribute("serial")?,
            manufacturer: attribute("manufacturer")?,
            speed: attribute("speed")?,
        };
        let model = attribute("product")?
            .unwrap_or_else(|| trim_c_space(&bounded_string(&capability.card)).to_string());
        let node = fs::canonicalize(device_path).ok();
        let by_id_path = node.and_then(|node| by_id.get(&node));
        let camera = Camera {
            id: format!("camera:v4l2:{fnv1a:016x}"),
            model: (!model.is_empty()).then_some(model),
            availability: Availability {
                state: AvailabilityState::Unknown,
                reason: Some(AVAILABILITY_REASON.to_string()),
            },
            modes,
            source: Source::V4l2(UsbCamera {
                device_path: device_path.to_string_lossy().into_owned(),
                by_id_path: by_id_path.map(|link| link.to_string_lossy().into_owned()),
                identity,
            }),
        };
        Ok(Some(Peripheral::Camera(camera)))
    }
}
