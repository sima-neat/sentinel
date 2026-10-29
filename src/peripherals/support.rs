//! Whether each camera mode is available on this board: the one judgement
//! Sentinel makes about devices, so every client shows the same answer. A mode
//! is available when the board's camera overlays support the camera's sensor,
//! the ISP outputs the exact size, and its format is the one Neat's
//! `CameraInput` outputs. A fixed ISP lists its output sizes. A run-time ISP
//! outputs a sensor mode at its own size when the mode is an `isp_input`;
//! libcamera narrows a larger size that no such mode covers, and may crop a
//! covered one, which query-only discovery cannot confirm, so a covered size
//! carries no verdict. A USB camera's mode is available when Neat's
//! `UsbCameraInput` opens the camera, a `uvcvideo` one with a `camera_name`,
//! at that mode on that node: the mode is MJPEG, which it captures at every
//! size and interval the camera lists, and no node of the camera it tries
//! first lists the mode's sizes and intervals. Every mode known not to work
//! carries the reason.

use super::board::Board;
use super::camera::{Camera, Fraction, Interval, MipiCamera, Mode, Source, UsbIdentity};
use super::v4l2::UVC_DRIVER;
use super::{CatalogError, Peripheral};

/// The default of `CameraInputOptions.format` in Neat Core
/// (`include/nodes/io/CameraInput.h`, unchanged since v0.4.0).
const CAMERAINPUT_FORMAT: &str = "NV12";
/// The format Neat Core's `UsbCameraInput` captures, and the driver of the
/// cameras it lists (`src/nodes/io/internal/UvcCamera.cpp`).
const USBCAMERAINPUT_FORMAT: &str = "MJPG";
const USB_FORMAT_REASON: &str = "Neat's USB camera input reads MJPEG only.";
const USB_DRIVER_REASON: &str = "Neat's USB camera input needs the uvcvideo driver.";
const USB_NAME_REASON: &str =
    "Neat's USB camera input cannot name this camera (no firmware node or USB ID).";
const USB_NODE_REASON: &str =
    "Neat's USB camera input opens another node of this camera for this mode.";
const NO_BOARD_REASON: &str = "This board's camera overlays could not be read.";
const NOT_ISP_SIZE_REASON: &str = "The ISP cannot output this size.";
const FORMAT_REASON: &str = "Neat's CameraInput outputs NV12 only.";
const ISP_INPUT_REASON: &str = "The ISP needs a 12-bit sensor mode at least this large whose \
width is a multiple of 64 (at most 4096x3072); the sensor has none, so libcamera would change \
the size.";

/// Sets a mode's `available` verdict when query-only discovery can decide it,
/// and clears any stale verdict when it cannot. `board` is the block read in
/// the same scan as `devices`.
pub fn judge(devices: &mut [Peripheral], board: Option<&Board>, errors: &[CatalogError]) {
    let field_complete = |field| {
        let provider = format!("board.{field}");
        !errors.iter().any(|error| error.provider == provider)
    };
    let configured_complete = field_complete("configured_cameras");
    let supported_complete = field_complete("supported_sensors");
    let earlier = earlier_mjpeg_listings(devices);
    for (device, earlier) in devices.iter_mut().zip(&earlier) {
        let Peripheral::Camera(camera) = device else {
            continue;
        };
        let mipi = match &camera.source {
            Source::V4l2(usb) => {
                for mode in &mut camera.modes {
                    set_mode_verdict(mode, usb_verdict(mode, &usb.identity, earlier));
                }
                continue;
            }
            Source::Mipi(mipi) => mipi,
        };
        let camera_verdict =
            camera_verdict(camera, mipi, board, configured_complete, supported_complete);
        let inputs: Vec<_> = camera
            .modes
            .iter()
            .filter(|mode| mode.isp_input == Some(true))
            .filter_map(|mode| mode.width.zip(mode.height))
            .collect();
        for mode in &mut camera.modes {
            match &camera_verdict {
                CameraVerdict::Unavailable(reason) => set_verdict(mode, Some(false), Some(reason)),
                CameraVerdict::Supported => set_mode_verdict(mode, mode_verdict(mode, &inputs)),
                CameraVerdict::Unknown => match mode_verdict(mode, &inputs) {
                    ModeVerdict::Unavailable(reason) => {
                        set_verdict(mode, Some(false), Some(reason))
                    }
                    ModeVerdict::Available | ModeVerdict::Unknown => set_verdict(mode, None, None),
                },
            }
        }
    }
}

fn set_verdict(mode: &mut Mode, available: Option<bool>, reason: Option<&str>) {
    mode.available = available;
    mode.reason = reason.map(str::to_string);
}

fn set_mode_verdict(mode: &mut Mode, verdict: ModeVerdict) {
    match verdict {
        ModeVerdict::Available => set_verdict(mode, Some(true), None),
        ModeVerdict::Unavailable(reason) => set_verdict(mode, Some(false), Some(reason)),
        ModeVerdict::Unknown => set_verdict(mode, None, None),
    }
}

enum CameraVerdict {
    Supported,
    Unavailable(String),
    /// A board field required to prove absence was incomplete. Facts already
    /// present can still prove support; missing facts cannot prove rejection.
    Unknown,
}

/// Whether the board's overlays support the camera's sensor. The device tree
/// names a sensor by its `compatible` string, which the board block links to
/// the camera by id.
fn camera_verdict(
    camera: &Camera,
    mipi: &MipiCamera,
    board: Option<&Board>,
    configured_complete: bool,
    supported_complete: bool,
) -> CameraVerdict {
    let Some(board) = board else {
        return CameraVerdict::Unavailable(NO_BOARD_REASON.to_string());
    };
    let configured = board.configured_cameras.iter();
    let compatible = configured
        .filter(|entry| entry.camera_id.as_deref() == Some(camera.id.as_str()))
        .map(|entry| entry.compatible.as_str())
        .next();
    let sensors = &board.supported_sensors;
    if compatible.is_some_and(|compatible| sensors.iter().any(|s| s.compatible == compatible)) {
        return CameraVerdict::Supported;
    }
    match compatible {
        None if !configured_complete => return CameraVerdict::Unknown,
        Some(_) if !supported_complete => return CameraVerdict::Unknown,
        _ => {}
    }
    let name = match (compatible, camera.model.as_deref()) {
        (Some(compatible), _) => sensor_name(compatible),
        (None, Some(model)) if !model.is_empty() => model.to_uppercase(),
        (None, _) => mipi.camera_name.clone(),
    };
    CameraVerdict::Unavailable(format!("{name} is not in this board's camera overlays."))
}

enum ModeVerdict {
    Available,
    Unavailable(&'static str),
    /// The sensor lists this size, and a larger `isp_input` mode covers it,
    /// but libcamera may crop or adjust it. Sentinel does not configure the
    /// camera to find out. Also a sensor mode from a catalog without
    /// `isp_input`; a USB mode whose interface had no driver during the scan;
    /// and a USB mode only some of whose intervals another node of the camera
    /// lists, so that Neat opens this node at some rates and not at others.
    Unknown,
}

/// The verdict for one mode of a camera whose overlays support its sensor.
/// `inputs` are the sizes of the camera's `isp_input` modes.
fn mode_verdict(mode: &Mode, inputs: &[(u32, u32)]) -> ModeVerdict {
    if mode.isp_output != Some(true) && mode.sensor_mode != Some(true) {
        return ModeVerdict::Unavailable(NOT_ISP_SIZE_REASON);
    }
    if mode.format != CAMERAINPUT_FORMAT {
        return ModeVerdict::Unavailable(FORMAT_REASON);
    }
    if mode.isp_output == Some(true) || mode.isp_input == Some(true) {
        return ModeVerdict::Available;
    }
    let (Some(false), Some(width), Some(height)) = (mode.isp_input, mode.width, mode.height) else {
        return ModeVerdict::Unknown;
    };
    match inputs.iter().any(|&(w, h)| w >= width && h >= height) {
        true => ModeVerdict::Unknown,
        false => ModeVerdict::Unavailable(ISP_INPUT_REASON),
    }
}

/// A size and frame interval a mode lists.
type Listing = (u32, u32, Interval);

/// The sizes and intervals a mode lists.
fn listings(mode: &Mode) -> impl Iterator<Item = Listing> + '_ {
    let sizes = mode.frame_intervals.iter();
    sizes.flat_map(|size| size.intervals.iter().map(|&i| (size.width, size.height, i)))
}

/// Per device, the MJPEG sizes and intervals of the nodes `UsbCameraInput`
/// tries before it when opening the same `camera_name`: Core lists a
/// camera's `uvcvideo` nodes by device path, shorter first (`/dev/video2`
/// before `/dev/video10`), and opens the first MJPEG node that lists the
/// requested size and rate (`list_nodes` and `resolve_mjpeg_camera` in
/// `src/nodes/io/internal/UvcCamera.cpp`). Several capture nodes on one UVC
/// interface, as a camera with several streaming interfaces has, share a
/// name.
fn earlier_mjpeg_listings(devices: &[Peripheral]) -> Vec<Vec<Listing>> {
    let earlier = |device: &Peripheral| {
        let Some((name, order)) = uvc_node(device) else {
            return Vec::new();
        };
        let before =
            |other: &&Peripheral| uvc_node(other).is_some_and(|(n, o)| n == name && o < order);
        let cameras = devices
            .iter()
            .filter(before)
            .filter_map(|other| match other {
                Peripheral::Camera(camera) => Some(camera),
                _ => None,
            });
        let modes = cameras.flat_map(|camera| &camera.modes);
        let modes = modes.filter(|mode| mode.format == USBCAMERAINPUT_FORMAT);
        modes.flat_map(listings).collect()
    };
    devices.iter().map(earlier).collect()
}

/// A named `uvcvideo` node's `camera_name` and its place in Core's order.
fn uvc_node(device: &Peripheral) -> Option<(&str, (usize, &str))> {
    let Peripheral::Camera(Camera {
        source: Source::V4l2(usb),
        ..
    }) = device
    else {
        return None;
    };
    let path = usb.device_path.as_str();
    match usb.identity.driver.as_deref() {
        Some(UVC_DRIVER) => Some((usb.identity.camera_name.as_deref()?, (path.len(), path))),
        _ => None,
    }
}

/// The verdict for one mode of a USB camera whose earlier nodes, in the
/// order `UsbCameraInput` tries them, list `earlier`.
fn usb_verdict(mode: &Mode, identity: &UsbIdentity, earlier: &[Listing]) -> ModeVerdict {
    let uvc = match identity.driver.as_deref() {
        Some(UVC_DRIVER) => true,
        Some(_) => return ModeVerdict::Unavailable(USB_DRIVER_REASON),
        // The interface is unbound only while its driver binds or unbinds
        // during the scan; Neat lists the node once `uvcvideo` is bound.
        None => false,
    };
    if mode.format != USBCAMERAINPUT_FORMAT {
        return ModeVerdict::Unavailable(USB_FORMAT_REASON);
    }
    if !uvc {
        return ModeVerdict::Unknown;
    }
    if identity.camera_name.is_none() {
        return ModeVerdict::Unavailable(USB_NAME_REASON);
    }
    // Core matches a requested rate against a discrete interval exactly and
    // against a range by its bounds, ignoring the step. `uvcvideo` lists
    // discrete sizes only, so sizes match exactly.
    let periods = |interval: Interval| match interval {
        Interval::Discrete(period) => (period, period),
        Interval::Stepwise {
            minimum, maximum, ..
        }
        | Interval::Continuous {
            minimum, maximum, ..
        } => (minimum, maximum),
    };
    let at_most = |a: Fraction, b: Fraction| {
        u64::from(a.numerator) * u64::from(b.denominator)
            <= u64::from(b.numerator) * u64::from(a.denominator)
    };
    let (mut opened_elsewhere, mut opened_here) = (true, true);
    for (width, height, interval) in listings(mode) {
        let (low, high) = periods(interval);
        let same_size = earlier
            .iter()
            .filter(|&&(w, h, _)| (w, h) == (width, height));
        let ranges = same_size.map(|&(.., earlier)| periods(earlier));
        let (mut covered, mut overlapped) = (false, false);
        for (min, max) in ranges {
            covered |= at_most(min, low) && at_most(high, max);
            overlapped |= at_most(min, high) && at_most(low, max);
        }
        opened_elsewhere &= covered;
        opened_here &= !overlapped;
    }
    match () {
        _ if opened_here => ModeVerdict::Available,
        _ if opened_elsewhere => ModeVerdict::Unavailable(USB_NODE_REASON),
        _ => ModeVerdict::Unknown,
    }
}

/// A sensor's name from its device-tree `compatible` string: `sony,imx477` is
/// `IMX477`.
fn sensor_name(compatible: &str) -> String {
    let model = compatible
        .split_once(',')
        .map_or(compatible, |(_, model)| model);
    match model.is_empty() {
        true => compatible.to_uppercase(),
        false => model.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peripherals::board::{ConfiguredCamera, SupportedSensor};
    use crate::peripherals::camera::{RangeKind, SizeRange, UsbIdentity};
    use serde_json::json;

    /// The IMX477 camera of the published example, with its ISP modes.
    fn example() -> (Vec<Peripheral>, Board) {
        let text = include_str!("../../docs/peripherals/catalog-example.json");
        let catalog: super::super::Catalog = serde_json::from_str(text).unwrap();
        (catalog.devices, catalog.board.unwrap())
    }

    fn verdicts(devices: &[Peripheral]) -> Vec<(String, String, Option<bool>, Option<String>)> {
        let cameras = devices.iter().filter_map(|device| match device {
            Peripheral::Camera(camera) => Some(camera),
            _ => None,
        });
        let modes = cameras.flat_map(|camera| camera.modes.iter().map(move |mode| (camera, mode)));
        let verdict = |(camera, mode): (&Camera, &Mode)| {
            let size = format!("{} {:?}x{:?}", mode.format, mode.width, mode.height);
            (camera.id.clone(), size, mode.available, mode.reason.clone())
        };
        modes.map(verdict).collect()
    }

    /// NV12 at the ISP's sizes is available for a sensor the overlays
    /// support, and MJPEG for a USB camera `UsbCameraInput` lists; other
    /// formats say why not. Microphones are left alone.
    #[test]
    fn modes_of_a_supported_sensor_are_available_in_nv12() {
        let (mut devices, board) = example();
        judge(&mut devices, Some(&board), &[]);
        for (id, mode, available, reason) in verdicts(&devices) {
            let expected = match () {
                _ if id.starts_with("camera:v4l2:") && mode.starts_with("MJPG ") => None,
                _ if id.starts_with("camera:v4l2:") => Some(USB_FORMAT_REASON),
                _ if mode.starts_with("NV12 ") => None,
                _ => Some(FORMAT_REASON),
            };
            assert_eq!(
                (available, reason.as_deref()),
                (Some(expected.is_none()), expected),
                "{id} {mode}"
            );
        }
        let Peripheral::Microphone(ref microphone) = devices[2] else {
            panic!("the example's third device is its microphone");
        };
        let mode = serde_json::to_value(&microphone.modes[0]).unwrap();
        assert!(mode.get("available").is_none());
    }

    /// A USB camera of another driver is unavailable, and so are the MJPEG
    /// modes, size ranges included, of a `uvcvideo` camera without a
    /// `camera_name`, which Neat cannot open. Without a driver (an interface
    /// binding or unbinding during the scan) MJPEG modes lose a stale
    /// verdict. Other formats still say why not, and USB verdicts ignore the
    /// board.
    #[test]
    fn usb_modes_follow_the_driver_and_the_format() {
        let (devices, _) = example();
        let usb_verdicts = |edit: &dyn Fn(&mut UsbIdentity)| {
            let mut devices = devices.clone();
            let Peripheral::Camera(ref mut camera) = devices[1] else {
                panic!("the example's second device is its USB camera");
            };
            let Source::V4l2(ref mut usb) = camera.source else {
                panic!("the example's second device is its USB camera");
            };
            edit(&mut usb.identity);
            let mut range = camera.modes[0].clone();
            (range.width, range.height) = (None, None);
            range.size_range = Some(SizeRange {
                kind: RangeKind::Continuous,
                min_width: 160,
                min_height: 120,
                max_width: 1920,
                max_height: 1080,
                step_width: 1,
                step_height: 1,
            });
            camera.modes.push(range);
            judge(&mut devices, None, &[]);
            let verdicts = verdicts(&devices).into_iter();
            let verdicts = verdicts.filter(|(id, ..)| id.starts_with("camera:v4l2:"));
            let verdicts = verdicts.map(|(_, _, available, reason)| (available, reason));
            verdicts.collect::<Vec<_>>()
        };
        let reason = |reason: &str| (Some(false), Some(reason.to_string()));
        let mjpeg = reason(USB_FORMAT_REASON);
        let available = (Some(true), None);
        assert_eq!(
            usb_verdicts(&|_| {}),
            [available.clone(), mjpeg.clone(), available]
        );
        let gspca = |identity: &mut UsbIdentity| {
            identity.driver = Some("gspca_zc3xx".into());
            identity.camera_name = None;
        };
        assert_eq!(usb_verdicts(&gspca), vec![reason(USB_DRIVER_REASON); 3]);
        let unnamed = reason(USB_NAME_REASON);
        assert_eq!(
            usb_verdicts(&|identity| identity.camera_name = None),
            [unnamed.clone(), mjpeg.clone(), unnamed]
        );
        let unknown = [(None, None), mjpeg, (None, None)];
        assert_eq!(usb_verdicts(&|identity| identity.driver = None), unknown);
    }

    /// Nodes of one USB camera share its `camera_name`, and Neat opens the
    /// first node by device path (`/dev/video2` before `/dev/video10`) that
    /// lists the requested size and rate. A later node's MJPEG mode is
    /// unavailable when earlier nodes list its size at each of its
    /// intervals, a discrete one exactly or within a range, and has no
    /// verdict when they list only some. Another camera is not affected.
    #[test]
    fn later_nodes_of_a_usb_camera_lose_the_modes_earlier_nodes_list() {
        let (devices, _) = example();
        let Peripheral::Camera(ref usb_camera) = devices[1] else {
            panic!("the example's second device is its USB camera");
        };
        let mjpg = |width: u32, height: u32, intervals: serde_json::Value| -> Mode {
            let size = json!({"width": width, "height": height, "intervals": intervals});
            let mode = json!({"format": "MJPG", "width": width, "height": height,
                              "frame_intervals": [size]});
            serde_json::from_value(mode).unwrap()
        };
        let fraction = |denominator: u32| json!({"numerator": 1, "denominator": denominator});
        let discrete = |denominators: &[u32]| {
            let interval = |&d: &u32| json!({"type": "discrete", "numerator": 1, "denominator": d});
            json!(denominators.iter().map(interval).collect::<Vec<_>>())
        };
        let range = |fastest, slowest| {
            json!([{"type": "continuous", "minimum": fraction(fastest),
                    "maximum": fraction(slowest), "step": fraction(1)}])
        };
        let node = |path: &str, name: &str, modes: Vec<Mode>| {
            let mut camera = usb_camera.clone();
            camera.modes = modes;
            let Source::V4l2(ref mut usb) = camera.source else {
                panic!("the example's second device is its USB camera");
            };
            usb.device_path = path.into();
            usb.identity.camera_name = Some(name.into());
            Peripheral::Camera(camera)
        };
        let first = || {
            vec![
                mjpg(1920, 1080, discrete(&[30, 15])),
                mjpg(1280, 720, range(30, 5)),
            ]
        };
        // `/dev/video10` comes first in the catalog but second in Core's order.
        let judged = |name: &str, modes: Vec<Mode>| {
            let mut devices = vec![
                node("/dev/video10", name, modes),
                node("/dev/video2", "camera", first()),
            ];
            judge(&mut devices, None, &[]);
            let verdicts = verdicts(&devices).into_iter();
            let verdicts = verdicts.map(|(_, _, available, reason)| (available, reason));
            verdicts.collect::<Vec<_>>()
        };
        let available = (Some(true), None);
        let elsewhere = (Some(false), Some(USB_NODE_REASON.to_string()));
        let yuyv = usb_camera.modes[1].clone();
        let later = || {
            vec![
                mjpg(1920, 1080, discrete(&[30, 15])),
                mjpg(1280, 720, discrete(&[15])),
                mjpg(640, 480, discrete(&[30])),
                yuyv.clone(),
            ]
        };
        let format = (Some(false), Some(USB_FORMAT_REASON.to_string()));
        let firsts = [available.clone(), available.clone()];
        let expected = [
            elsewhere.clone(),
            elsewhere,
            available.clone(),
            format.clone(),
        ];
        assert_eq!(judged("camera", later()), [&expected[..], &firsts].concat());
        let some_rates = vec![
            mjpg(1920, 1080, discrete(&[30, 60])),
            mjpg(1280, 720, range(60, 5)),
        ];
        let unknown = [(None, None), (None, None)];
        assert_eq!(
            judged("camera", some_rates),
            [&unknown[..], &firsts].concat()
        );
        let other = [available.clone(), available.clone(), available, format];
        assert_eq!(judged("other", later()), [&other[..], &firsts].concat());
    }

    /// The verdicts stored in the published example, for its USB and MIPI
    /// cameras alike, are the ones `judge` gives, so they cannot drift.
    #[test]
    fn the_published_example_carries_the_verdicts_sentinel_gives() {
        let text = include_str!("../../docs/peripherals/catalog-example.json");
        let catalog: super::super::Catalog = serde_json::from_str(text).unwrap();
        let mut judged = catalog.clone();
        let cameras = judged.devices.iter_mut().filter_map(|device| match device {
            Peripheral::Camera(camera) => Some(camera),
            _ => None,
        });
        for mode in cameras.flat_map(|camera| &mut camera.modes) {
            set_verdict(mode, Some(false), Some("stale"));
        }
        judge(&mut judged.devices, judged.board.as_ref(), &judged.errors);
        assert_eq!(judged, catalog);
    }

    /// A sensor no overlay supports, or one the device tree does not link to
    /// the camera, is named in the reason; so is a missing board block.
    #[test]
    fn unsupported_or_unknown_sensors_say_why() {
        let (devices, board) = example();
        let mipi_reasons = |board: Option<&Board>| {
            let mut devices = devices.clone();
            judge(&mut devices, board, &[]);
            let reasons = verdicts(&devices).into_iter();
            let reasons = reasons.filter(|(id, ..)| !id.starts_with("camera:v4l2:"));
            reasons
                .map(|(_, _, available, reason)| (available, reason))
                .collect::<Vec<_>>()
        };
        let all = |reason: &str| vec![(Some(false), Some(reason.to_string())); 9];

        let mut imx568 = board.clone();
        imx568.supported_sensors = vec![SupportedSensor {
            compatible: "sony,imx568".into(),
            overlays: vec!["modalix-som-imx568.dtbo".into()],
        }];
        let not_in = "IMX477 is not in this board's camera overlays.";
        assert_eq!(mipi_reasons(Some(&imx568)), all(not_in));

        let mut unlinked = board.clone();
        unlinked.configured_cameras = vec![ConfiguredCamera {
            camera_id: None,
            ..board.configured_cameras[0].clone()
        }];
        assert_eq!(mipi_reasons(Some(&unlinked)), all(not_in));
        assert_eq!(mipi_reasons(None), all(NO_BOARD_REASON));
    }

    /// A partial board list can prove a match from facts it did read, but an
    /// absent match cannot prove rejection. Mode-local format failures remain
    /// decisive while otherwise usable modes have stale verdicts cleared.
    #[test]
    fn incomplete_board_fields_do_not_prove_a_camera_is_unsupported() {
        let (devices, board) = example();
        let error = |field: &str| CatalogError {
            provider: format!("board.{field}"),
            code: "io.discovery_failed".into(),
            reason: "synthetic partial board scan".into(),
        };
        let check_unknown = |board: &Board, error: CatalogError| {
            let mut devices = devices.clone();
            judge(&mut devices, Some(board), &[error]);
            for (id, mode, available, reason) in verdicts(&devices) {
                if id.starts_with("camera:v4l2:") {
                    continue;
                }
                let expected = match mode.starts_with("NV12 ") {
                    true => (None, None),
                    false => (Some(false), Some(FORMAT_REASON)),
                };
                assert_eq!((available, reason.as_deref()), expected, "{id} {mode}");
            }
        };

        let mut no_configured_camera = board.clone();
        no_configured_camera.configured_cameras.clear();
        check_unknown(&no_configured_camera, error("configured_cameras"));

        let mut no_supported_sensor = board.clone();
        no_supported_sensor.supported_sensors.clear();
        check_unknown(&no_supported_sensor, error("supported_sensors"));

        // A matching pair is positive evidence even if another entry was
        // skipped from either list.
        let mut matched = devices.clone();
        let errors = [error("configured_cameras"), error("supported_sensors")];
        judge(&mut matched, Some(&board), &errors);
        let mipi = verdicts(&matched)
            .into_iter()
            .filter(|(id, ..)| !id.starts_with("camera:v4l2:"));
        assert!(mipi
            .filter(|(_, mode, ..)| mode.starts_with("NV12 "))
            .all(|(_, _, available, reason)| available == Some(true) && reason.is_none()));
    }

    /// An ISP output size or ISP input sensor size can be available. Another
    /// sensor size is unknown when an ISP input covers it, which libcamera may
    /// crop, and unavailable when none does, which libcamera narrows.
    #[test]
    fn exact_isp_sizes_are_available() {
        let mode = |value| serde_json::from_value::<Mode>(value).unwrap();
        let verdict = |value, inputs: &[(u32, u32)]| mode_verdict(&mode(value), inputs);
        let other = json!({"format": "NV12", "width": 640, "height": 480});
        assert!(matches!(
            verdict(other, &[]),
            ModeVerdict::Unavailable(NOT_ISP_SIZE_REASON)
        ));
        let isp = json!({"format": "NV12", "width": 1920, "height": 1080, "isp_output": true});
        assert!(matches!(verdict(isp, &[]), ModeVerdict::Available));
        let sensor = |width, height, input: Option<bool>| {
            json!({"format": "NV12", "width": width, "height": height,
                   "sensor_mode": true, "isp_input": input})
        };
        let inputs = [(4032, 3040), (1920, 1080)];
        let cases = [
            (sensor(1920, 1080, Some(true)), "available"),
            (sensor(2028, 1520, Some(false)), "unknown"),
            (sensor(4056, 3040, Some(false)), "unavailable"),
            (sensor(4032, 3072, Some(false)), "unavailable"),
            // A catalog from a Sentinel without `isp_input`.
            (sensor(4056, 3040, None), "unknown"),
        ];
        for (value, expected) in cases {
            let size = (value["width"].clone(), value["height"].clone());
            let actual = match verdict(value, &inputs) {
                ModeVerdict::Available => "available",
                ModeVerdict::Unknown => "unknown",
                ModeVerdict::Unavailable(ISP_INPUT_REASON) => "unavailable",
                ModeVerdict::Unavailable(reason) => reason,
            };
            assert_eq!(actual, expected, "{size:?}");
        }
        let sensor_rgb = json!({"format": "RGB3", "width": 4032, "height": 3040, "sensor_mode": true,
                   "isp_input": true});
        assert!(matches!(
            verdict(sensor_rgb, &inputs),
            ModeVerdict::Unavailable(FORMAT_REASON)
        ));
        assert_eq!(sensor_name("sony,imx477"), "IMX477");
        assert_eq!(sensor_name("imx477"), "IMX477");
        assert_eq!(sensor_name("sony,"), "SONY,");
    }

    /// Reduced from the 3.0.0 B1859 IMX477 catalog captured on 2026-10-08,
    /// whose Sentinel did not report `isp_input`. Strict libcamera capture
    /// preserved nine sizes and narrowed both 4056-wide requests to 4032.
    /// Without `isp_input` no run-time size gets a verdict, which clears the
    /// capture's former positive verdicts. With the `isp_input` the same build
    /// gives (every size is listed as SRGGB12 on 2026-10-13), the 64-aligned
    /// sizes are available, the 4056-wide ones are not, and the rest, which a
    /// 4032x3040 input covers, stay unjudged.
    #[test]
    fn captured_runtime_sensor_sizes_follow_the_isp_input_rule() {
        let text = include_str!("fixtures/imx477-platform-3.0-nv12.json");
        let mut catalog: super::super::Catalog = serde_json::from_str(text).unwrap();
        let judged = |catalog: &mut super::super::Catalog| {
            judge(
                &mut catalog.devices,
                catalog.board.as_ref(),
                &catalog.errors,
            );
            let Peripheral::Camera(camera) = &catalog.devices[0] else {
                panic!("capture's first device is its camera");
            };
            let verdict = |mode: &Mode| (mode.width.unwrap(), mode.height.unwrap(), mode.available);
            camera.modes.iter().map(verdict).collect::<Vec<_>>()
        };
        let mut old = catalog.clone();
        assert!(judged(&mut old)
            .iter()
            .all(|&(.., available)| available.is_none()));

        let aligned = [
            (1280, 990),
            (1920, 1080),
            (2048, 1080),
            (3840, 2160),
            (3968, 3040),
            (4032, 3040),
        ];
        let Peripheral::Camera(camera) = &mut catalog.devices[0] else {
            panic!("capture's first device is its camera");
        };
        for mode in &mut camera.modes {
            let size = (mode.width.unwrap(), mode.height.unwrap());
            mode.isp_input = Some(aligned.contains(&size));
        }
        assert_eq!(
            judged(&mut catalog),
            [
                (1280, 990, Some(true)),
                (1332, 990, None),
                (1920, 1080, Some(true)),
                (2028, 1080, None),
                (2028, 1520, None),
                (2048, 1080, Some(true)),
                (3840, 2160, Some(true)),
                (3968, 3040, Some(true)),
                (4032, 3040, Some(true)),
                (4056, 2160, Some(false)),
                (4056, 3040, Some(false)),
            ]
        );
    }
}
