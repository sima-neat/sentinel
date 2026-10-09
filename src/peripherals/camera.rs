//! The `camera` device type: image sensors a Neat application can capture
//! from. Each backend adds a [`Source`] variant.

use serde::{Deserialize, Serialize};

use super::Availability;

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
    /// A MIPI CSI-2 sensor behind the Modalix ISP.
    Mipi(MipiCamera),
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
    /// MIPI: `true` for an ISP output size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isp_output: Option<bool>,
    /// MIPI: `true` for a sensor frame size, which the ISP outputs when it
    /// sets its sizes at run time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor_mode: Option<bool>,
    /// Whether the board is set up for this mode; see `support`. Set by the
    /// catalog, not the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    /// Why the mode is not available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

/// Routing and topology of a MIPI CSI-2 sensor. `id` is derived from
/// `camera_name`; paths are routing only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MipiCamera {
    /// The sensor's media-controller entity name, e.g. `imx477 5-001a`, as
    /// `CameraInput` accepts it.
    pub camera_name: String,
    /// `/dev/mediaN`.
    pub media_device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bus_info: Option<String>,
    pub isp: Isp,
    /// The CSI-2 receiver entity the sensor's source pad links to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csi_receiver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor_timing: Option<SensorTiming>,
    /// The sensor's frame-rate limit at its active format, to two decimals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fps: Option<f64>,
}

/// The Modalix ISP output nodes the camera's modes come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Isp {
    Available {
        device_path: String,
        device_paths: Vec<String>,
        /// Where the ISP's output sizes come from; absent from catalogs of
        /// older Sentinels, which only read fixed tables.
        #[serde(default)]
        sizing: IspSizing,
    },
    /// The camera then has no modes.
    Unavailable { reason: String },
}

/// How the ISP driver gets its output sizes, detected from the sizes its
/// output nodes list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IspSizing {
    /// A table built into the driver, listed by `VIDIOC_ENUM_FRAMESIZES`
    /// (Platform 2.1.x).
    #[default]
    Fixed,
    /// Set at run time from the sensor the ISP is configured for: a node
    /// lists 0x0 until then, and the listed sizes are not the camera's modes
    /// (Platform 3.0).
    Runtime,
}

/// The sensor's active source-pad format and the controls that bound its
/// frame rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensorTiming {
    /// Pixels per second.
    pub pixel_rate: u64,
    pub hblank_min: u32,
    pub vblank_min: u32,
    pub width: u32,
    pub height: u32,
}

impl Camera {
    /// One line for `simaai-sentinel peripherals`.
    pub fn describe(&self) -> String {
        let name = match self.source {
            Source::Mipi(ref mipi) => &mipi.camera_name,
            _ => self.model.as_deref().unwrap_or("-"),
        };
        let backend = match self.source {
            Source::V4l2(_) => "v4l2",
            Source::Mipi(_) => "mipi",
        };
        format!("{name}  ({backend}, {} modes)", self.modes.len())
    }
}
