//! The `camera` device type: image sensors a Neat application can capture
//! from. Each backend adds a [`Source`] variant.

use serde::{Deserialize, Serialize};

/// One camera, serialized flat with a `backend` tag from [`Source`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub availability: Availability,
    pub modes: Vec<Mode>,
    #[serde(flatten)]
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum Source {
    /// A USB Video Class camera.
    V4l2(UsbCamera),
}

/// Whether the device is free. Discovery never opens a stream, so a camera's
/// state is `unknown` with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Availability {
    pub state: AvailabilityState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilityState {
    Unknown,
}

/// One output format at one size (or size range) and frame rate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mode {
    /// V4L2 FourCC, e.g. `NV12`, `MJPG`, `YUYV`.
    pub format: String,
    /// The driver's `VIDIOC_ENUM_FMT` description, when it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_description: Option<String>,
    /// Discrete sizes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Stepwise or continuous sizes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_range: Option<SizeRange>,
    /// Every interval the device advertises, per probed size.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frame_intervals: Vec<SizeIntervals>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SizeRange {
    #[serde(rename = "type")]
    pub kind: RangeKind,
    pub min_width: u32,
    pub min_height: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub step_width: u32,
    pub step_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RangeKind {
    Stepwise,
    Continuous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SizeIntervals {
    pub width: u32,
    pub height: u32,
    pub intervals: Vec<Interval>,
}

/// A frame interval (seconds per frame), as `VIDIOC_ENUM_FRAMEINTERVALS`
/// reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Interval {
    Discrete(Fraction),
    Stepwise {
        minimum: Fraction,
        maximum: Fraction,
        step: Fraction,
    },
    Continuous {
        minimum: Fraction,
        maximum: Fraction,
        step: Fraction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fraction {
    pub numerator: u32,
    pub denominator: u32,
}

/// Routing and identity of a USB camera. Paths are routing only; `id` is
/// derived from `identity`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbCamera {
    /// `/dev/videoN`.
    pub device_path: String,
    /// The udev `/dev/v4l/by-id/...` link to `device_path`, when udev made one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_id_path: Option<String>,
    pub identity: UsbIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbIdentity {
    pub stable_key: String,
    pub topology: String,
    pub interface: String,
    pub node_index: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    /// The USB device's sysfs speed in Mb/s as the kernel prints it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
}

impl Camera {
    /// One line for `simaai-sentinel peripherals`.
    pub fn describe(&self) -> String {
        let name = self.model.as_deref().unwrap_or("-");
        let backend = match self.source {
            Source::V4l2(_) => "v4l2",
        };
        format!("{name}  ({backend}, {} modes)", self.modes.len())
    }
}
