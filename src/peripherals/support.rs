//! Whether each camera mode is available on this board: the one judgement
//! Sentinel makes about devices, so every client shows the same answer. A mode
//! is available when the board's camera overlays support the camera's sensor,
//! the ISP outputs the mode's size, and its format is the one Neat's
//! `CameraInput` outputs. Every other mode carries the reason it is not.

use super::board::Board;
use super::camera::{Camera, Mode, Source};
use super::Peripheral;

/// The default of `CameraInputOptions.format` in Neat Core
/// (`include/nodes/io/CameraInput.h`, unchanged since v0.4.0).
const CAMERAINPUT_FORMAT: &str = "NV12";
const USB_REASON: &str = "USB cameras have no camera overlay on this board.";
const NO_BOARD_REASON: &str = "This board's camera overlays could not be read.";
const NOT_ISP_SIZE_REASON: &str = "The ISP cannot output this size.";
const FORMAT_REASON: &str = "Neat's CameraInput outputs NV12 only.";

/// Sets `available`, and `reason` when it is false, on every camera mode.
/// `board` is the block read in the same scan as `devices`.
pub fn judge(devices: &mut [Peripheral], board: Option<&Board>) {
    for device in devices {
        let Peripheral::Camera(camera) = device else {
            continue;
        };
        let camera_reason = camera_reason(camera, board);
        for mode in &mut camera.modes {
            let reason = camera_reason.clone().or_else(|| mode_reason(mode));
            mode.available = Some(reason.is_none());
            mode.reason = reason;
        }
    }
}

/// Why none of the camera's modes is available; `None` when the board's
/// overlays support its sensor. The device tree names a camera's sensor by
/// its `compatible` string, which the board block links to the camera by id.
fn camera_reason(camera: &Camera, board: Option<&Board>) -> Option<String> {
    let mipi = match camera.source {
        Source::V4l2(_) => return Some(USB_REASON.to_string()),
        Source::Mipi(ref mipi) => mipi,
    };
    let Some(board) = board else {
        return Some(NO_BOARD_REASON.to_string());
    };
    let configured = board.configured_cameras.iter();
    let compatible = configured
        .filter(|entry| entry.camera_id.as_deref() == Some(camera.id.as_str()))
        .map(|entry| entry.compatible.as_str())
        .next();
    let sensors = &board.supported_sensors;
    if compatible.is_some_and(|compatible| sensors.iter().any(|s| s.compatible == compatible)) {
        return None;
    }
    let name = match (compatible, camera.model.as_deref()) {
        (Some(compatible), _) => sensor_name(compatible),
        (None, Some(model)) if !model.is_empty() => model.to_uppercase(),
        (None, _) => mipi.camera_name.clone(),
    };
    Some(format!("{name} is not in this board's camera overlays."))
}

/// Why one mode of a camera the overlays support is not available.
fn mode_reason(mode: &Mode) -> Option<String> {
    let size = mode.isp_output == Some(true) || mode.sensor_mode == Some(true);
    let reason = match () {
        _ if !size => NOT_ISP_SIZE_REASON,
        _ if mode.format != CAMERAINPUT_FORMAT => FORMAT_REASON,
        _ => return None,
    };
    Some(reason.to_string())
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
    /// support; other formats say why not, and USB modes never are.
    /// Microphones are left alone.
    #[test]
    fn modes_of_a_supported_sensor_are_available_in_nv12() {
        let (mut devices, board) = example();
        judge(&mut devices, Some(&board));
        for (id, mode, available, reason) in verdicts(&devices) {
            let expected = match () {
                _ if id.starts_with("camera:v4l2:") => Some(USB_REASON),
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

    /// A sensor no overlay supports, or one the device tree does not link to
    /// the camera, is named in the reason; so is a missing board block.
    #[test]
    fn unsupported_or_unknown_sensors_say_why() {
        let (devices, board) = example();
        let mipi_reasons = |board: Option<&Board>| {
            let mut devices = devices.clone();
            judge(&mut devices, board);
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

    /// A mode that is neither an ISP output size nor a sensor size is not
    /// available; a sensor size (Platform 3.0) is.
    #[test]
    fn only_isp_or_sensor_sizes_are_available() {
        let mode = |value| serde_json::from_value::<Mode>(value).unwrap();
        let other = mode(json!({"format": "NV12", "width": 640, "height": 480}));
        assert_eq!(mode_reason(&other).as_deref(), Some(NOT_ISP_SIZE_REASON));
        let sensor = json!({"format": "NV12", "width": 2028, "height": 1520, "sensor_mode": true});
        assert_eq!(mode_reason(&mode(sensor)), None);
        assert_eq!(sensor_name("sony,imx477"), "IMX477");
        assert_eq!(sensor_name("imx477"), "IMX477");
        assert_eq!(sensor_name("sony,"), "SONY,");
    }
}
