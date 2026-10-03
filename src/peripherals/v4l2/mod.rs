//! Read-only discovery of USB/UVC cameras exposed through V4L2
//! (`daemon.camera.v4l2`).
//!
//! The scan walks `/sys/class/video4linux`, admits only nodes whose sysfs device has a
//! USB ancestor (platform and ISP nodes are skipped), opens each candidate
//! with `O_RDONLY | O_NONBLOCK | O_CLOEXEC`, and issues query ioctls only.
//! Metadata-only, output-only and memory-to-memory nodes are dropped after
//! `VIDIOC_QUERYCAP`. Record ids are derived from the USB topology, interface
//! number and composite-node index, never from `/dev/videoN`.

mod ioctl;
#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::model::{Provider, ProviderError, Record};
use super::sysutil::{
    bounded_string, disappeared, errno_of, os_message, read_text_file, trim_c_space,
    CODE_DISCOVERY_FAILED, CODE_IO_OPEN, CODE_PERMISSION_DENIED,
};
use super::videodev2::{
    effective_capabilities, fourcc_string, Capability, FmtDesc, FrmIvalEnum, FrmSizeEnum,
    VideoNode, MAX_ENUMERATION_ENTRIES, V4L2_BUF_TYPE_VIDEO_CAPTURE,
    V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, V4L2_CAP_VIDEO_CAPTURE, V4L2_CAP_VIDEO_CAPTURE_MPLANE,
    V4L2_CAP_VIDEO_M2M, V4L2_CAP_VIDEO_M2M_MPLANE, V4L2_FRMIVAL_TYPE_CONTINUOUS,
    V4L2_FRMIVAL_TYPE_DISCRETE, V4L2_FRMIVAL_TYPE_STEPWISE, V4L2_FRMSIZE_TYPE_CONTINUOUS,
    V4L2_FRMSIZE_TYPE_DISCRETE, V4L2_FRMSIZE_TYPE_STEPWISE,
};
use ioctl::{Backend, SystemBackend};

pub const PROVIDER_NAME: &str = "daemon.camera.v4l2";

const AVAILABILITY_REASON: &str = "V4L2 does not expose a reliable read-only ownership state; \
discovery does not acquire, configure, or stream from the camera.";

/// The `daemon.camera.v4l2` provider.
pub struct V4l2Provider {
    sys_root: PathBuf,
    dev_root: PathBuf,
    subsystems: Vec<String>,
    backend: Box<dyn Backend>,
}

impl V4l2Provider {
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
            subsystems: vec!["video4linux".to_string()],
            backend,
        }
    }
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
        build_records(&probe_backend(
            &self.sys_root,
            &self.dev_root,
            self.backend.as_ref(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Probe model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
enum RangeType {
    #[default]
    Discrete,
    Stepwise,
    Continuous,
}

impl RangeType {
    fn name(self) -> &'static str {
        match self {
            RangeType::Discrete => "discrete",
            RangeType::Stepwise => "stepwise",
            RangeType::Continuous => "continuous",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Fraction {
    numerator: u32,
    denominator: u32,
}

const fn fraction(numerator: u32, denominator: u32) -> Fraction {
    Fraction {
        numerator,
        denominator,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Interval {
    kind: RangeType,
    value: Fraction,
    minimum: Fraction,
    maximum: Fraction,
    step: Fraction,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct IntervalSet {
    width: u32,
    height: u32,
    intervals: Vec<Interval>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FrameSize {
    kind: RangeType,
    width: u32,
    height: u32,
    min_width: u32,
    min_height: u32,
    max_width: u32,
    max_height: u32,
    step_width: u32,
    step_height: u32,
    interval_sets: Vec<IntervalSet>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Format {
    fourcc: String,
    sizes: Vec<FrameSize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsbIdentity {
    topology: String,
    interface: String,
    node_index: String,
    vendor_id: String,
    product_id: String,
    serial: String,
    model: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Device {
    identity: UsbIdentity,
    device_path: String,
    formats: Vec<Format>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ProbeFailure {
    #[default]
    None,
    PermissionDenied,
    Unreadable,
}

#[derive(Debug, Clone, Default)]
struct Probe {
    failure: ProbeFailure,
    failure_detail: String,
    devices: Vec<Device>,
}

#[derive(Debug)]
struct ProbeError {
    failure: ProbeFailure,
    errno: i32,
    message: String,
}

impl ProbeError {
    fn new(failure: ProbeFailure, errno: i32, message: String) -> Self {
        Self {
            failure,
            errno,
            message,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Failure class for a sysfs read error: only `EACCES` is a permission
/// failure.
fn filesystem_failure(errno: i32) -> ProbeFailure {
    if errno == libc::EACCES {
        ProbeFailure::PermissionDenied
    } else {
        ProbeFailure::Unreadable
    }
}

fn probe_io_error(action: &str, path: &Path, error: &io::Error) -> ProbeError {
    let errno = errno_of(error);
    let failure = if errno == libc::EACCES || errno == libc::EPERM {
        ProbeFailure::PermissionDenied
    } else {
        ProbeFailure::Unreadable
    };
    ProbeError::new(
        failure,
        errno,
        format!("{action} {}: {}", path.display(), os_message(error)),
    )
}

fn malformed_probe_result(capability: &str, path: &Path) -> ProbeError {
    ProbeError::new(
        ProbeFailure::Unreadable,
        libc::EPROTO,
        format!(
            "V4L2 camera {} returned a malformed {capability}",
            path.display()
        ),
    )
}

fn relative_sysfs_path(path: &Path, sys_root: &Path) -> String {
    match path.strip_prefix(sys_root) {
        Ok(relative) => relative.to_string_lossy().into_owned(),
        Err(_) => String::new(),
    }
}

fn stable_key(identity: &UsbIdentity) -> String {
    let mut key = format!("sysfs:{}", identity.topology);
    if !identity.interface.is_empty() {
        key.push_str(":interface=");
        key.push_str(&identity.interface);
    }
    if !identity.node_index.is_empty() {
        key.push_str(":index=");
        key.push_str(&identity.node_index);
    }
    key
}

/// FNV-1a 64 of the stable key, formatted as `camera:v4l2:<16 hex digits>`.
fn stable_id(identity: &UsbIdentity) -> String {
    let mut hash: u64 = 14_695_981_039_346_656_037;
    for byte in stable_key(identity).bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    format!("camera:v4l2:{hash:016x}")
}

fn is_capture_node(capability: &Capability) -> bool {
    let capabilities = effective_capabilities(capability);
    let captures = capabilities & (V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_VIDEO_CAPTURE_MPLANE) != 0;
    let memory_to_memory = capabilities & (V4L2_CAP_VIDEO_M2M | V4L2_CAP_VIDEO_M2M_MPLANE) != 0;
    captures && !memory_to_memory
}

fn decode_frame_size(value: &FrmSizeEnum) -> Option<FrameSize> {
    match value.kind {
        V4L2_FRMSIZE_TYPE_DISCRETE => {
            if value.discrete_width() == 0 || value.discrete_height() == 0 {
                return None;
            }
            Some(FrameSize {
                kind: RangeType::Discrete,
                width: value.discrete_width(),
                height: value.discrete_height(),
                ..FrameSize::default()
            })
        }
        V4L2_FRMSIZE_TYPE_STEPWISE | V4L2_FRMSIZE_TYPE_CONTINUOUS => {
            if value.min_width() == 0
                || value.min_height() == 0
                || value.max_width() < value.min_width()
                || value.max_height() < value.min_height()
                || value.step_width() == 0
                || value.step_height() == 0
            {
                return None;
            }
            Some(FrameSize {
                kind: if value.kind == V4L2_FRMSIZE_TYPE_STEPWISE {
                    RangeType::Stepwise
                } else {
                    RangeType::Continuous
                },
                min_width: value.min_width(),
                min_height: value.min_height(),
                max_width: value.max_width(),
                max_height: value.max_height(),
                step_width: value.step_width(),
                step_height: value.step_height(),
                ..FrameSize::default()
            })
        }
        _ => None,
    }
}

fn decode_frame_interval(value: &FrmIvalEnum) -> Option<Interval> {
    match value.kind {
        V4L2_FRMIVAL_TYPE_DISCRETE => {
            let (numerator, denominator) = value.discrete();
            if numerator == 0 || denominator == 0 {
                return None;
            }
            Some(Interval {
                kind: RangeType::Discrete,
                value: fraction(numerator, denominator),
                ..Interval::default()
            })
        }
        V4L2_FRMIVAL_TYPE_STEPWISE | V4L2_FRMIVAL_TYPE_CONTINUOUS => {
            let (min, max, step) = (value.min(), value.max(), value.step());
            if min.0 == 0 || min.1 == 0 || max.0 == 0 || max.1 == 0 {
                return None;
            }
            if step.0 == 0 || step.1 == 0 {
                return None;
            }
            if u64::from(min.0) * u64::from(max.1) > u64::from(max.0) * u64::from(min.1) {
                return None;
            }
            Some(Interval {
                kind: if value.kind == V4L2_FRMIVAL_TYPE_STEPWISE {
                    RangeType::Stepwise
                } else {
                    RangeType::Continuous
                },
                minimum: fraction(min.0, min.1),
                maximum: fraction(max.0, max.1),
                step: fraction(step.0, step.1),
                ..Interval::default()
            })
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Canonical JSON
// ---------------------------------------------------------------------------

fn fraction_json(value: &Fraction) -> Map<String, Value> {
    let mut result = Map::new();
    result.insert("numerator".into(), json!(value.numerator));
    result.insert("denominator".into(), json!(value.denominator));
    result
}

fn interval_json(interval: &Interval) -> Value {
    let mut result = Map::new();
    result.insert("type".into(), json!(interval.kind.name()));
    if interval.kind == RangeType::Discrete {
        result.extend(fraction_json(&interval.value));
    } else {
        result.insert(
            "minimum".into(),
            Value::Object(fraction_json(&interval.minimum)),
        );
        result.insert(
            "maximum".into(),
            Value::Object(fraction_json(&interval.maximum)),
        );
        result.insert("step".into(), Value::Object(fraction_json(&interval.step)));
    }
    Value::Object(result)
}

/// The fastest advertised rate: the shortest discrete period, or the minimum
/// period of a range.
fn representative_framerate(size: &FrameSize) -> Option<Fraction> {
    let mut fastest: Option<Fraction> = None;
    for set in &size.interval_sets {
        for interval in &set.intervals {
            let period = if interval.kind == RangeType::Discrete {
                interval.value
            } else {
                interval.minimum
            };
            if period.numerator == 0 || period.denominator == 0 {
                continue;
            }
            let candidate = fraction(period.denominator, period.numerator);
            let faster = match fastest {
                None => true,
                Some(current) => {
                    u64::from(candidate.numerator) * u64::from(current.denominator)
                        > u64::from(current.numerator) * u64::from(candidate.denominator)
                }
            };
            if faster {
                fastest = Some(candidate);
            }
        }
    }
    fastest
}

fn mode_json(format: &Format, size: &FrameSize) -> Option<Value> {
    let framerate = representative_framerate(size)?;
    let mut mode = json!({
        "format": format.fourcc,
        "framerate_num": framerate.numerator,
        "framerate_den": framerate.denominator,
    });
    if size.kind == RangeType::Discrete {
        mode["width"] = json!(size.width);
        mode["height"] = json!(size.height);
    } else {
        mode["size_range"] = json!({
            "type": size.kind.name(),
            "min_width": size.min_width,
            "min_height": size.min_height,
            "max_width": size.max_width,
            "max_height": size.max_height,
            "step_width": size.step_width,
            "step_height": size.step_height,
        });
    }
    let frame_intervals: Vec<Value> = size
        .interval_sets
        .iter()
        .map(|set| {
            json!({
                "width": set.width,
                "height": set.height,
                "intervals": set.intervals.iter().map(interval_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    mode["frame_intervals"] = Value::Array(frame_intervals);
    Some(mode)
}

// ---------------------------------------------------------------------------
// Canonical ordering
// ---------------------------------------------------------------------------

fn compare_fraction(left: &Fraction, right: &Fraction) -> Ordering {
    let left_scaled = u64::from(left.numerator) * u64::from(right.denominator);
    let right_scaled = u64::from(right.numerator) * u64::from(left.denominator);
    left_scaled
        .cmp(&right_scaled)
        .then_with(|| (left.numerator, left.denominator).cmp(&(right.numerator, right.denominator)))
}

fn compare_interval(left: &Interval, right: &Interval) -> Ordering {
    if left.kind != right.kind {
        return left.kind.cmp(&right.kind);
    }
    if left.kind == RangeType::Discrete {
        return compare_fraction(&left.value, &right.value);
    }
    compare_fraction(&left.minimum, &right.minimum)
        .then_with(|| compare_fraction(&left.maximum, &right.maximum))
        .then_with(|| compare_fraction(&left.step, &right.step))
}

fn canonicalize_intervals(intervals: &mut Vec<Interval>) {
    intervals.sort_by(compare_interval);
    intervals.dedup();
}

fn size_key(size: &FrameSize) -> (RangeType, u32, u32, u32, u32, u32, u32, u32, u32) {
    (
        size.kind,
        size.width,
        size.height,
        size.min_width,
        size.min_height,
        size.max_width,
        size.max_height,
        size.step_width,
        size.step_height,
    )
}

fn canonicalize_formats(formats: &mut [Format]) {
    for format in formats.iter_mut() {
        for size in &mut format.sizes {
            for set in &mut size.interval_sets {
                canonicalize_intervals(&mut set.intervals);
            }
            size.interval_sets
                .sort_by_key(|set| (set.width, set.height));
            let mut merged: Vec<IntervalSet> = Vec::new();
            for set in std::mem::take(&mut size.interval_sets) {
                match merged.last_mut() {
                    Some(last) if last.width == set.width && last.height == set.height => {
                        last.intervals.extend(set.intervals);
                        canonicalize_intervals(&mut last.intervals);
                    }
                    _ => merged.push(set),
                }
            }
            size.interval_sets = merged;
        }
        format.sizes.sort_by_key(size_key);
        format.sizes.dedup();
    }
    formats.sort_by(|left, right| left.fourcc.cmp(&right.fourcc));
}

fn merge_formats(destination: &mut Vec<Format>, source: Vec<Format>) {
    for format in source {
        match destination
            .iter_mut()
            .find(|current| current.fourcc == format.fourcc)
        {
            None => destination.push(format),
            Some(existing) => {
                for size in format.sizes {
                    if !existing.sizes.contains(&size) {
                        existing.sizes.push(size);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// sysfs identity
// ---------------------------------------------------------------------------

/// Whether `path` exists: ENOENT and ENOTDIR mean "absent", any other error is
/// reported.
fn path_exists(path: &Path) -> Result<bool, ProbeError> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) => {
            let errno = errno_of(&error);
            if errno == libc::ENOENT || errno == libc::ENOTDIR {
                return Ok(false);
            }
            Err(ProbeError::new(
                filesystem_failure(errno),
                errno,
                format!(
                    "failed to inspect V4L2 sysfs attribute {}: {}",
                    path.display(),
                    os_message(&error)
                ),
            ))
        }
    }
}

/// Resolve the USB identity of one `/sys/class/video4linux/<node>` entry.
/// `Ok(None)` means the node is not USB-backed or disappeared mid-scan.
fn usb_identity(class_entry: &Path, sys_root: &Path) -> Result<Option<UsbIdentity>, ProbeError> {
    let device_link = class_entry.join("device");
    let device = match fs::canonicalize(&device_link) {
        Ok(device) => device,
        Err(error) => {
            let errno = errno_of(&error);
            if disappeared(errno) {
                return Ok(None);
            }
            return Err(ProbeError::new(
                filesystem_failure(errno),
                errno,
                format!(
                    "failed to resolve V4L2 sysfs device {}: {}",
                    device_link.display(),
                    os_message(&error)
                ),
            ));
        }
    };

    let canonical_sys = fs::canonicalize(sys_root).map_err(|error| {
        let errno = errno_of(&error);
        ProbeError::new(
            filesystem_failure(errno),
            errno,
            format!(
                "failed to resolve V4L2 sysfs root {}: {}",
                sys_root.display(),
                os_message(&error)
            ),
        )
    })?;
    if !device.starts_with(&canonical_sys) {
        return Err(ProbeError::new(
            ProbeFailure::Unreadable,
            libc::EXDEV,
            format!(
                "V4L2 sysfs device escaped the configured sysfs root: {}",
                device.display()
            ),
        ));
    }

    let mut current = device.clone();
    let mut usb_device: Option<PathBuf> = None;
    let mut interface = String::new();
    while current.starts_with(&canonical_sys) && current != canonical_sys {
        if interface.is_empty() {
            if let Some(value) = read_text_file(&current.join("bInterfaceNumber")) {
                interface = value;
            }
        }
        let has_vendor = path_exists(&current.join("idVendor"))?;
        let has_product = path_exists(&current.join("idProduct"))?;
        if has_vendor != has_product {
            return Err(ProbeError::new(
                ProbeFailure::Unreadable,
                libc::EPROTO,
                format!(
                    "V4L2 USB ancestor has incomplete vendor/product attributes: {}",
                    current.display()
                ),
            ));
        }
        if has_vendor {
            usb_device = Some(current);
            break;
        }
        current = match current.parent() {
            Some(parent) => parent.to_path_buf(),
            None => break,
        };
    }
    let Some(usb_device) = usb_device else {
        return Ok(None);
    };

    let topology = relative_sysfs_path(&usb_device, &canonical_sys);
    if topology.is_empty() {
        return Ok(None);
    }
    let node_index = read_text_file(&class_entry.join("index"))
        .or_else(|| read_text_file(&device.join("index")))
        .unwrap_or_default();
    let attribute = |name: &str| read_text_file(&usb_device.join(name)).unwrap_or_default();
    Ok(Some(UsbIdentity {
        topology,
        interface,
        node_index,
        vendor_id: attribute("idVendor"),
        product_id: attribute("idProduct"),
        serial: attribute("serial"),
        model: attribute("product"),
    }))
}

// ---------------------------------------------------------------------------
// ioctl enumeration
// ---------------------------------------------------------------------------

fn enumerate_intervals(
    node: &mut dyn VideoNode,
    device_path: &Path,
    pixel_format: u32,
    width: u32,
    height: u32,
) -> Result<Vec<Interval>, ProbeError> {
    let mut intervals = Vec::new();
    let mut index: u32 = 0;
    loop {
        if index >= MAX_ENUMERATION_ENTRIES {
            return Err(malformed_probe_result("frame interval list", device_path));
        }
        let mut value = FrmIvalEnum {
            index,
            pixel_format,
            width,
            height,
            ..FrmIvalEnum::default()
        };
        if let Err(error) = node.enum_frame_interval(&mut value) {
            if errno_of(&error) == libc::EINVAL {
                break;
            }
            return Err(probe_io_error(
                "failed to enumerate V4L2 frame intervals for",
                device_path,
                &error,
            ));
        }
        let decoded = decode_frame_interval(&value)
            .ok_or_else(|| malformed_probe_result("frame interval", device_path))?;
        intervals.push(decoded);
        if value.kind != V4L2_FRMIVAL_TYPE_DISCRETE {
            break;
        }
        index = index.wrapping_add(1);
    }
    Ok(intervals)
}

fn enumerate_sizes(
    node: &mut dyn VideoNode,
    device_path: &Path,
    pixel_format: u32,
) -> Result<Vec<FrameSize>, ProbeError> {
    let mut sizes = Vec::new();
    let mut index: u32 = 0;
    loop {
        if index >= MAX_ENUMERATION_ENTRIES {
            return Err(malformed_probe_result("frame size list", device_path));
        }
        let mut value = FrmSizeEnum {
            index,
            pixel_format,
            ..FrmSizeEnum::default()
        };
        if let Err(error) = node.enum_frame_size(&mut value) {
            if errno_of(&error) == libc::EINVAL {
                break;
            }
            return Err(probe_io_error(
                "failed to enumerate V4L2 frame sizes for",
                device_path,
                &error,
            ));
        }
        let mut decoded = decode_frame_size(&value)
            .ok_or_else(|| malformed_probe_result("frame size", device_path))?;

        let mut probes = Vec::new();
        if decoded.kind == RangeType::Discrete {
            probes.push((decoded.width, decoded.height));
        } else {
            probes.push((decoded.min_width, decoded.min_height));
            if decoded.max_width != decoded.min_width || decoded.max_height != decoded.min_height {
                probes.push((decoded.max_width, decoded.max_height));
            }
        }
        for (width, height) in probes {
            let intervals = enumerate_intervals(node, device_path, pixel_format, width, height)?;
            decoded.interval_sets.push(IntervalSet {
                width,
                height,
                intervals,
            });
        }
        sizes.push(decoded);
        if value.kind != V4L2_FRMSIZE_TYPE_DISCRETE {
            break;
        }
        index = index.wrapping_add(1);
    }
    Ok(sizes)
}

fn enumerate_formats(
    node: &mut dyn VideoNode,
    device_path: &Path,
    buffer_type: u32,
) -> Result<Vec<Format>, ProbeError> {
    let mut formats: Vec<Format> = Vec::new();
    let mut index: u32 = 0;
    loop {
        if index >= MAX_ENUMERATION_ENTRIES {
            return Err(malformed_probe_result("format list", device_path));
        }
        let mut value = FmtDesc {
            index,
            buf_type: buffer_type,
            ..FmtDesc::default()
        };
        if let Err(error) = node.enum_format(&mut value) {
            if errno_of(&error) == libc::EINVAL {
                break;
            }
            return Err(probe_io_error(
                "failed to enumerate V4L2 formats for",
                device_path,
                &error,
            ));
        }
        let format = Format {
            fourcc: fourcc_string(value.pixelformat),
            sizes: enumerate_sizes(node, device_path, value.pixelformat)?,
        };
        match formats
            .iter_mut()
            .find(|current| current.fourcc == format.fourcc)
        {
            None => formats.push(format),
            Some(existing) => existing.sizes.extend(format.sizes),
        }
        index = index.wrapping_add(1);
    }
    Ok(formats)
}

/// Open and query one node. `Ok(None)` means it is not a video capture node
/// (metadata, output, or memory-to-memory).
fn probe_device(
    backend: &dyn Backend,
    device_path: &Path,
    mut identity: UsbIdentity,
) -> Result<Option<Device>, ProbeError> {
    let mut node = backend
        .open(device_path)
        .map_err(|error| probe_io_error("failed to open V4L2 camera", device_path, &error))?;
    let node = node.as_mut();

    let mut capability = Capability::default();
    node.query_capability(&mut capability).map_err(|error| {
        probe_io_error("failed to query V4L2 capabilities for", device_path, &error)
    })?;
    let capabilities = effective_capabilities(&capability);
    let captures = capabilities & V4L2_CAP_VIDEO_CAPTURE != 0;
    let captures_multiplanar = capabilities & V4L2_CAP_VIDEO_CAPTURE_MPLANE != 0;
    if !is_capture_node(&capability) {
        return Ok(None);
    }

    if identity.model.is_empty() {
        identity.model = trim_c_space(&bounded_string(&capability.card)).to_string();
    }
    let mut device = Device {
        identity,
        device_path: device_path.to_string_lossy().into_owned(),
        formats: Vec::new(),
    };
    if captures {
        let formats = enumerate_formats(node, device_path, V4L2_BUF_TYPE_VIDEO_CAPTURE)?;
        merge_formats(&mut device.formats, formats);
    }
    if captures_multiplanar {
        let formats = enumerate_formats(node, device_path, V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE)?;
        merge_formats(&mut device.formats, formats);
    }
    canonicalize_formats(&mut device.formats);
    Ok(Some(device))
}

fn probe_backend(sys_root: &Path, dev_root: &Path, backend: &dyn Backend) -> Probe {
    let mut probe = Probe::default();
    let class_directory = sys_root.join("class/video4linux");
    let read_failure = |error: &io::Error| -> (ProbeFailure, String) {
        (
            filesystem_failure(errno_of(error)),
            format!(
                "failed to read {}: {}",
                class_directory.display(),
                os_message(error)
            ),
        )
    };
    let iterator = match fs::read_dir(&class_directory) {
        Ok(iterator) => iterator,
        Err(error) => {
            if errno_of(&error) == libc::ENOENT {
                return probe;
            }
            (probe.failure, probe.failure_detail) = read_failure(&error);
            return probe;
        }
    };

    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in iterator {
        match entry {
            Ok(entry) => {
                if entry.file_name().as_bytes().starts_with(b"video") {
                    entries.push(class_directory.join(entry.file_name()));
                }
            }
            Err(error) => {
                (probe.failure, probe.failure_detail) = read_failure(&error);
                return probe;
            }
        }
    }
    entries.sort();

    for class_entry in &entries {
        let result = usb_identity(class_entry, sys_root).and_then(|identity| {
            let Some(identity) = identity else {
                return Ok(());
            };
            let Some(name) = class_entry.file_name() else {
                return Ok(());
            };
            let device_path = dev_root.join(name);
            match probe_device(backend, &device_path, identity) {
                Ok(Some(device)) => {
                    if !device.identity.topology.is_empty() {
                        probe.devices.push(device);
                    }
                    Ok(())
                }
                Ok(None) => Ok(()),
                Err(failure) if disappeared(failure.errno) => Ok(()),
                Err(failure) => Err(failure),
            }
        });
        if let Err(failure) = result {
            probe.failure = failure.failure;
            probe.failure_detail = failure.message;
            probe.devices.clear();
            break;
        }
    }
    probe
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

fn build_records(probe: &Probe) -> Result<Vec<Record>, ProviderError> {
    match probe.failure {
        ProbeFailure::PermissionDenied => {
            return Err(ProviderError::new(
                CODE_PERMISSION_DENIED,
                probe.failure_detail.clone(),
            ))
        }
        ProbeFailure::Unreadable => {
            return Err(ProviderError::new(
                CODE_IO_OPEN,
                probe.failure_detail.clone(),
            ))
        }
        ProbeFailure::None => {}
    }

    let mut records: BTreeMap<String, Record> = BTreeMap::new();
    for discovered in &probe.devices {
        let mut device = discovered.clone();
        canonicalize_formats(&mut device.formats);
        let identity = &device.identity;
        if identity.topology.is_empty()
            || identity.interface.is_empty()
            || identity.node_index.is_empty()
        {
            return Err(ProviderError::new(
                CODE_DISCOVERY_FAILED,
                "V4L2 USB camera did not publish stable topology, interface, and \
                 composite-node index attributes.",
            ));
        }

        let id = stable_id(identity);
        let mut identity_json = Map::new();
        identity_json.insert("stable_key".into(), json!(stable_key(identity)));
        identity_json.insert("topology".into(), json!(identity.topology));
        for (key, value) in [
            ("interface", &identity.interface),
            ("node_index", &identity.node_index),
            ("vendor_id", &identity.vendor_id),
            ("product_id", &identity.product_id),
            ("serial", &identity.serial),
        ] {
            if !value.is_empty() {
                identity_json.insert(key.into(), json!(value));
            }
        }

        let modes: Vec<Value> = device
            .formats
            .iter()
            .flat_map(|format| {
                format
                    .sizes
                    .iter()
                    .filter_map(move |size| mode_json(format, size))
            })
            .collect();
        let mut details = json!({
            "backend": "v4l2",
            "connection": "usb",
            "identity": Value::Object(identity_json),
            "modes": modes,
            "availability": {"state": "unknown", "reason": AVAILABILITY_REASON},
        });
        if !identity.model.is_empty() {
            details["model"] = json!(identity.model);
        }
        if !device.device_path.is_empty() {
            details["device_path"] = json!(device.device_path);
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
            Entry::Occupied(mut slot) => {
                let without_path = |details: &Value| {
                    let mut details = details.clone();
                    if let Some(object) = details.as_object_mut() {
                        object.remove("device_path");
                    }
                    details
                };
                if without_path(&slot.get().details) != without_path(&record.details) {
                    return Err(ProviderError::new(
                        CODE_DISCOVERY_FAILED,
                        format!(
                            "V4L2 aliases for {} reported conflicting capabilities.",
                            record.id
                        ),
                    ));
                }
                let path_of = |details: &Value| {
                    details
                        .get("device_path")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                };
                let existing_path = path_of(&slot.get().details);
                let candidate_path = path_of(&record.details);
                if existing_path.is_empty()
                    || (!candidate_path.is_empty() && candidate_path < existing_path)
                {
                    slot.get_mut().details["device_path"] = json!(candidate_path);
                }
            }
        }
    }
    Ok(records.into_values().collect())
}
