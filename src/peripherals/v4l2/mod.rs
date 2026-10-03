//! Read-only discovery of USB/UVC cameras exposed through V4L2
//! (`daemon.camera.v4l2`).
//!
//! The scan walks `/sys/class/video4linux`, admits only nodes whose sysfs device has a
//! USB ancestor (platform and ISP nodes are skipped), opens each candidate
//! with `O_RDONLY | O_NONBLOCK | O_CLOEXEC`, and issues query ioctls only.
//! Metadata-only, output-only and memory-to-memory nodes are dropped after
//! `VIDIOC_QUERYCAP`. Record ids are derived from the USB topology, interface
//! number and composite-node index, never from `/dev/videoN`.

#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::model::{Provider, ProviderError, Record};
use super::sysutil::{
    bounded_string, disappeared, errno_of, os_message, read_text_file, trim_c_space,
    CODE_DISCOVERY_FAILED, CODE_IO_OPEN, CODE_PERMISSION_DENIED,
};
use super::videodev2::{
    effective_capabilities, fourcc_string, open_read_only, Capability, EnumerationBudget, FmtDesc,
    FrmIvalEnum, FrmSizeEnum, VideoNode, MAX_ENUMERATION_ENTRIES, V4L2_BUF_TYPE_VIDEO_CAPTURE,
    V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, V4L2_CAP_VIDEO_CAPTURE, V4L2_CAP_VIDEO_CAPTURE_MPLANE,
    V4L2_CAP_VIDEO_M2M, V4L2_CAP_VIDEO_M2M_MPLANE, V4L2_FRMIVAL_TYPE_CONTINUOUS,
    V4L2_FRMIVAL_TYPE_DISCRETE, V4L2_FRMIVAL_TYPE_STEPWISE, V4L2_FRMSIZE_TYPE_CONTINUOUS,
    V4L2_FRMSIZE_TYPE_DISCRETE, V4L2_FRMSIZE_TYPE_STEPWISE,
};

pub const PROVIDER_NAME: &str = "daemon.camera.v4l2";

const AVAILABILITY_REASON: &str = "V4L2 does not expose a reliable read-only ownership state; \
discovery does not acquire, configure, or stream from the camera.";

/// Opens a V4L2 node; tests substitute fake nodes.
type Opener = Box<dyn Fn(&Path) -> io::Result<Box<dyn VideoNode>> + Send>;

/// The `daemon.camera.v4l2` provider.
pub struct V4l2Provider {
    sys_root: PathBuf,
    dev_root: PathBuf,
    subsystems: Vec<String>,
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
            subsystems: vec!["video4linux".to_string()],
            open,
        }
    }
}

fn open_system(path: &Path) -> io::Result<Box<dyn VideoNode>> {
    Ok(Box::new(open_read_only(path)?))
}

impl Default for V4l2Provider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for V4l2Provider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn subsystems(&self) -> &[String] {
        &self.subsystems
    }

    fn discover(&mut self) -> Result<Vec<Record>, ProviderError> {
        self.scan().map_err(|failure| failure.error)
    }
}

/// A failed scan. `errno` is the failed OS call's error (0 otherwise), so a
/// node that vanished mid-scan can be skipped instead.
struct Failure {
    errno: i32,
    error: ProviderError,
}

fn os_failure(action: &str, path: &Path, error: &io::Error) -> Failure {
    let errno = errno_of(error);
    let code = if errno == libc::EACCES || errno == libc::EPERM {
        CODE_PERMISSION_DENIED
    } else {
        CODE_IO_OPEN
    };
    let reason = format!("{action} {}: {}", path.display(), os_message(error));
    Failure {
        errno,
        error: ProviderError::new(code, reason),
    }
}

/// A sysfs read error: only `EACCES` is a permission failure there.
fn sysfs_failure(action: &str, path: &Path, error: &io::Error) -> Failure {
    let errno = errno_of(error);
    let code = if errno == libc::EACCES {
        CODE_PERMISSION_DENIED
    } else {
        CODE_IO_OPEN
    };
    let reason = format!("{action} {}: {}", path.display(), os_message(error));
    Failure {
        errno,
        error: ProviderError::new(code, reason),
    }
}

/// Whether a sysfs attribute exists; absence is ENOENT or ENOTDIR, and any
/// other error is reported.
fn attribute_exists(path: &Path) -> Result<bool, Failure> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if matches!(errno_of(&error), libc::ENOENT | libc::ENOTDIR) => Ok(false),
        Err(error) => Err(sysfs_failure(
            "failed to inspect V4L2 sysfs attribute",
            path,
            &error,
        )),
    }
}

/// The discrete, stepwise and continuous range types, in canonical order: a
/// type's index here names it in `KIND_NAMES`, and 0 is discrete.
const SIZE_KINDS: [u32; 3] = [
    V4L2_FRMSIZE_TYPE_DISCRETE,
    V4L2_FRMSIZE_TYPE_STEPWISE,
    V4L2_FRMSIZE_TYPE_CONTINUOUS,
];
const INTERVAL_KINDS: [u32; 3] = [
    V4L2_FRMIVAL_TYPE_DISCRETE,
    V4L2_FRMIVAL_TYPE_STEPWISE,
    V4L2_FRMIVAL_TYPE_CONTINUOUS,
];
const KIND_NAMES: [&str; 3] = ["discrete", "stepwise", "continuous"];

/// `numerator / denominator`, ordered by value, then by its terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fraction(u32, u32);

impl Fraction {
    fn value_cmp(&self, other: &Self) -> Ordering {
        (u64::from(self.0) * u64::from(other.1)).cmp(&(u64::from(other.0) * u64::from(self.1)))
    }
}

impl Ord for Fraction {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value_cmp(other)
            .then_with(|| (self.0, self.1).cmp(&(other.0, other.1)))
    }
}

impl PartialOrd for Fraction {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A frame interval: `[period, 0, 0]` when discrete, else `[min, max, step]`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Interval {
    kind: usize,
    fractions: [Fraction; 3],
}

/// A frame size: `[width, height, 0, 0, 0, 0]` when discrete, else
/// `[min_width, min_height, max_width, max_height, step_width, step_height]`,
/// with the intervals of each probed `(width, height)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Size {
    kind: usize,
    dims: [u32; 6],
    probes: Vec<(u32, u32, Vec<Interval>)>,
}

fn decode_size(value: &FrmSizeEnum) -> Option<(usize, [u32; 6])> {
    let kind = SIZE_KINDS.iter().position(|&kind| kind == value.kind)?;
    if kind == 0 {
        let (width, height) = (value.discrete_width(), value.discrete_height());
        return (width != 0 && height != 0).then_some((kind, [width, height, 0, 0, 0, 0]));
    }
    let dims = [
        value.min_width(),
        value.min_height(),
        value.max_width(),
        value.max_height(),
        value.step_width(),
        value.step_height(),
    ];
    let [min_w, min_h, max_w, max_h, step_w, step_h] = dims;
    let valid = min_w != 0 && min_h != 0 && max_w >= min_w && max_h >= min_h;
    (valid && step_w != 0 && step_h != 0).then_some((kind, dims))
}

fn decode_interval(value: &FrmIvalEnum) -> Option<Interval> {
    let kind = INTERVAL_KINDS.iter().position(|&kind| kind == value.kind)?;
    let nonzero = |(numerator, denominator): (u32, u32)| {
        (numerator != 0 && denominator != 0).then_some(Fraction(numerator, denominator))
    };
    let fractions = if kind == 0 {
        [nonzero(value.discrete())?, Fraction(0, 0), Fraction(0, 0)]
    } else {
        [
            nonzero(value.min())?,
            nonzero(value.max())?,
            nonzero(value.step())?,
        ]
    };
    (kind == 0 || fractions[0].value_cmp(&fractions[1]).is_le())
        .then_some(Interval { kind, fractions })
}

fn interval_json(interval: &Interval) -> Value {
    let fraction = |f: Fraction| json!({"numerator": f.0, "denominator": f.1});
    let [first, maximum, step] = interval.fractions;
    let mut value = if interval.kind == 0 {
        fraction(first)
    } else {
        json!({"minimum": fraction(first), "maximum": fraction(maximum), "step": fraction(step)})
    };
    value["type"] = json!(KIND_NAMES[interval.kind]);
    value
}

/// One mode per size, at its fastest advertised rate (the shortest discrete
/// period or range minimum). A size without intervals has no mode.
/// `description` is the driver's `VIDIOC_ENUM_FMT` text, omitted when empty.
fn mode_json(fourcc: &str, description: &str, size: &Size) -> Option<Value> {
    let Fraction(numerator, denominator) = size
        .probes
        .iter()
        .flat_map(|(_, _, intervals)| intervals)
        .map(|interval| interval.fractions[0])
        .min_by(Fraction::value_cmp)?;
    let frame_intervals: Vec<Value> = size
        .probes
        .iter()
        .map(|(width, height, intervals)| {
            let intervals: Vec<Value> = intervals.iter().map(interval_json).collect();
            json!({"width": width, "height": height, "intervals": intervals})
        })
        .collect();
    let mut mode = json!({
        "format": fourcc,
        "framerate_num": denominator,
        "framerate_den": numerator,
        "frame_intervals": frame_intervals,
    });
    if !description.is_empty() {
        mode["format_description"] = json!(description);
    }
    let [width, height, max_width, max_height, step_width, step_height] = size.dims;
    if size.kind == 0 {
        mode["width"] = json!(width);
        mode["height"] = json!(height);
    } else {
        mode["size_range"] = json!({
            "type": KIND_NAMES[size.kind], "min_width": width, "min_height": height,
            "max_width": max_width, "max_height": max_height,
            "step_width": step_width, "step_height": step_height,
        });
    }
    Some(mode)
}

/// The enumeration ioctls of one open node, capped per list and charged to the
/// device's budget.
struct Enumerator<'a> {
    node: Box<dyn VideoNode>,
    path: &'a Path,
    budget: EnumerationBudget,
}

impl Enumerator<'_> {
    fn malformed(&self, what: &str) -> Failure {
        let reason = format!(
            "V4L2 camera {} returned a malformed {what}",
            self.path.display()
        );
        Failure {
            errno: 0,
            error: ProviderError::new(CODE_IO_OPEN, reason),
        }
    }

    /// Issue query `index` of a `list`; `Ok(false)` once the driver ends the
    /// list with `EINVAL`.
    fn query(
        &mut self,
        list: &str,
        index: u32,
        ioctl: impl FnOnce(&mut dyn VideoNode) -> io::Result<()>,
    ) -> Result<bool, Failure> {
        if index >= MAX_ENUMERATION_ENTRIES {
            return Err(self.malformed(&format!("{list} list")));
        }
        self.budget.spend().map_err(|what| self.malformed(&what))?;
        match ioctl(self.node.as_mut()) {
            Ok(()) => Ok(true),
            Err(error) if errno_of(&error) == libc::EINVAL => Ok(false),
            Err(error) => {
                let action = format!("failed to enumerate V4L2 {list}s for");
                Err(os_failure(&action, self.path, &error))
            }
        }
    }

    /// The modes of every capture format, by fourcc; the single- and
    /// multi-planar listings of one format merge, keeping the first non-empty
    /// description.
    fn modes(&mut self, capabilities: u32) -> Result<Vec<Value>, Failure> {
        let mut formats: BTreeMap<String, (String, Vec<Size>)> = BTreeMap::new();
        for (capture, buf_type) in [
            (V4L2_CAP_VIDEO_CAPTURE, V4L2_BUF_TYPE_VIDEO_CAPTURE),
            (
                V4L2_CAP_VIDEO_CAPTURE_MPLANE,
                V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
            ),
        ] {
            for index in 0.. {
                let mut value = FmtDesc {
                    index,
                    buf_type,
                    ..FmtDesc::default()
                };
                if capabilities & capture == 0
                    || !self.query("format", index, |node| node.enum_format(&mut value))?
                {
                    break;
                }
                let sizes = self.sizes(value.pixelformat)?;
                let (description, known) =
                    formats.entry(fourcc_string(value.pixelformat)).or_default();
                if description.is_empty() {
                    *description = trim_c_space(&bounded_string(&value.description)).to_string();
                }
                known.extend(sizes);
            }
        }
        let mut modes = Vec::new();
        for (fourcc, (description, sizes)) in &mut formats {
            sizes.sort();
            sizes.dedup();
            let mode = |size| mode_json(fourcc, description, size);
            modes.extend(sizes.iter().filter_map(mode));
        }
        Ok(modes)
    }

    /// Discrete sizes are listed one per index; a stepwise or continuous range
    /// is the only entry, probed for intervals at its minimum and maximum.
    fn sizes(&mut self, pixel_format: u32) -> Result<Vec<Size>, Failure> {
        let mut sizes = Vec::new();
        for index in 0.. {
            let mut value = FrmSizeEnum {
                index,
                pixel_format,
                ..FrmSizeEnum::default()
            };
            if !self.query("frame size", index, |node| node.enum_frame_size(&mut value))? {
                break;
            }
            let (kind, dims) = decode_size(&value).ok_or_else(|| self.malformed("frame size"))?;
            let mut probes = vec![(dims[0], dims[1])];
            if kind != 0 && (dims[2], dims[3]) != (dims[0], dims[1]) {
                probes.push((dims[2], dims[3]));
            }
            let probes = probes
                .into_iter()
                .map(|(width, height)| {
                    Ok((width, height, self.intervals(pixel_format, width, height)?))
                })
                .collect::<Result<_, Failure>>()?;
            sizes.push(Size { kind, dims, probes });
            if kind != 0 {
                break;
            }
        }
        Ok(sizes)
    }

    fn intervals(
        &mut self,
        pixel_format: u32,
        width: u32,
        height: u32,
    ) -> Result<Vec<Interval>, Failure> {
        let mut intervals = Vec::new();
        for index in 0.. {
            let mut value = FrmIvalEnum {
                index,
                pixel_format,
                width,
                height,
                ..FrmIvalEnum::default()
            };
            if !self.query("frame interval", index, |node| {
                node.enum_frame_interval(&mut value)
            })? {
                break;
            }
            let interval =
                decode_interval(&value).ok_or_else(|| self.malformed("frame interval"))?;
            let range = interval.kind != 0;
            intervals.push(interval);
            if range {
                break;
            }
        }
        intervals.sort();
        intervals.dedup();
        Ok(intervals)
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
    fn scan(&self) -> Result<Vec<Record>, Failure> {
        let class = self.sys_root.join("class/video4linux");
        let read_failure = |error: io::Error| sysfs_failure("failed to read", &class, &error);
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
            sysfs_failure("failed to resolve V4L2 sysfs root", &self.sys_root, &error)
        })?;
        let by_id = by_id_links(&self.dev_root);
        let mut records = Vec::new();
        for name in names {
            let device_path = self.dev_root.join(&name);
            match self.probe(&sys, &class.join(&name), &device_path, &by_id) {
                Ok(record) => records.extend(record),
                Err(failure) if disappeared(failure.errno) => {}
                Err(failure) => return Err(failure),
            }
        }
        records.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(records)
    }

    /// The record of one `/sys/class/video4linux` entry; `Ok(None)` when it is
    /// not a USB video capture node.
    fn probe(
        &self,
        sys: &Path,
        entry: &Path,
        device_path: &Path,
        by_id: &BTreeMap<PathBuf, PathBuf>,
    ) -> Result<Option<Record>, Failure> {
        let link = entry.join("device");
        let device = fs::canonicalize(&link)
            .map_err(|error| sysfs_failure("failed to resolve V4L2 sysfs device", &link, &error))?;
        if !device.starts_with(sys) {
            let reason = format!(
                "V4L2 sysfs device escaped the configured sysfs root: {}",
                device.display()
            );
            return Err(Failure {
                errno: libc::EXDEV,
                error: ProviderError::new(CODE_IO_OPEN, reason),
            });
        }
        // The nearest ancestor with `idVendor` and `idProduct` is the USB
        // device; the interface number is on the USB interface between it and
        // the node.
        let mut interface = None;
        let mut usb = None;
        for path in device.ancestors().take_while(|path| *path != sys) {
            if interface.is_none() {
                interface = read_text_file(&path.join("bInterfaceNumber"));
            }
            let vendor = attribute_exists(&path.join("idVendor"))?;
            if vendor != attribute_exists(&path.join("idProduct"))? {
                let reason = format!(
                    "V4L2 USB ancestor has incomplete vendor/product attributes: {}",
                    path.display()
                );
                return Err(Failure {
                    errno: libc::EPROTO,
                    error: ProviderError::new(CODE_IO_OPEN, reason),
                });
            }
            if vendor {
                usb = Some(path);
                break;
            }
        }
        let Some(usb) = usb else {
            return Ok(None);
        };

        let mut node = (self.open)(device_path)
            .map_err(|error| os_failure("failed to open V4L2 camera", device_path, &error))?;
        let mut capability = Capability::default();
        node.query_capability(&mut capability).map_err(|error| {
            os_failure("failed to query V4L2 capabilities for", device_path, &error)
        })?;
        let capabilities = effective_capabilities(&capability);
        if capabilities & (V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_VIDEO_CAPTURE_MPLANE) == 0
            || capabilities & (V4L2_CAP_VIDEO_M2M | V4L2_CAP_VIDEO_M2M_MPLANE) != 0
        {
            return Ok(None);
        }
        let interface = interface.unwrap_or_default();
        let index = read_text_file(&entry.join("index")).unwrap_or_default();
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

        let budget = EnumerationBudget::new();
        let modes = Enumerator {
            node,
            path: device_path,
            budget,
        }
        .modes(capabilities)?;

        let topology = usb.strip_prefix(sys).unwrap_or(usb).to_string_lossy();
        let stable_key = format!("sysfs:{topology}:interface={interface}:index={index}");
        let fnv1a = stable_key
            .bytes()
            .fold(14_695_981_039_346_656_037_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211)
            });
        let attribute =
            |name: &str| read_text_file(&usb.join(name)).filter(|text| !text.is_empty());
        let mut identity = json!({
            "stable_key": stable_key,
            "topology": topology,
            "interface": interface,
            "node_index": index,
        });
        for (key, name) in [
            ("vendor_id", "idVendor"),
            ("product_id", "idProduct"),
            ("serial", "serial"),
            ("manufacturer", "manufacturer"),
            ("speed", "speed"),
        ] {
            if let Some(value) = attribute(name) {
                identity[key] = json!(value);
            }
        }
        let mut details = json!({
            "backend": "v4l2",
            "connection": "usb",
            "identity": identity,
            "modes": modes,
            "availability": {"state": "unknown", "reason": AVAILABILITY_REASON},
            "device_path": device_path.to_string_lossy(),
        });
        let model = attribute("product")
            .unwrap_or_else(|| trim_c_space(&bounded_string(&capability.card)).to_string());
        if !model.is_empty() {
            details["model"] = json!(model);
        }
        let node = fs::canonicalize(device_path).ok();
        if let Some(link) = node.and_then(|node| by_id.get(&node)) {
            details["by_id_path"] = json!(link.to_string_lossy());
        }
        Ok(Some(Record {
            id: format!("camera:v4l2:{fnv1a:016x}"),
            kind: "camera".to_string(),
            provider: PROVIDER_NAME.to_string(),
            details,
        }))
    }
}
