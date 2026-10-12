//! Whether each camera mode is available on this board: the one judgement
//! Sentinel makes about devices, so every client shows the same answer. A mode
//! is available when the board's camera overlays support the camera's sensor,
//! the ISP advertises the exact output size, and its format is the one Neat's
//! `CameraInput` outputs. A run-time ISP exposes sensor sizes, but query-only
//! discovery cannot prove that libcamera will preserve one exactly, so those
//! NV12 modes carry no verdict. Every mode known not to work carries the reason.

use super::board::Board;
use super::camera::{Camera, Mode, Source};
use super::{CatalogError, Peripheral};

/// The default of `CameraInputOptions.format` in Neat Core
/// (`include/nodes/io/CameraInput.h`, unchanged since v0.4.0).
const CAMERAINPUT_FORMAT: &str = "NV12";
const USB_REASON: &str = "USB cameras have no camera overlay on this board.";
const NO_BOARD_REASON: &str = "This board's camera overlays could not be read.";
const NOT_ISP_SIZE_REASON: &str = "The ISP cannot output this size.";
const FORMAT_REASON: &str = "Neat's CameraInput outputs NV12 only.";

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
    for device in devices {
        let Peripheral::Camera(camera) = device else {
            continue;
        };
        let camera_verdict = camera_verdict(camera, board, configured_complete, supported_complete);
        for mode in &mut camera.modes {
            match &camera_verdict {
                CameraVerdict::Unavailable(reason) => set_verdict(mode, Some(false), Some(reason)),
                CameraVerdict::Supported => match mode_verdict(mode) {
                    ModeVerdict::Available => set_verdict(mode, Some(true), None),
                    ModeVerdict::Unavailable(reason) => {
                        set_verdict(mode, Some(false), Some(reason))
                    }
                    ModeVerdict::Unknown => set_verdict(mode, None, None),
                },
                CameraVerdict::Unknown => match mode_verdict(mode) {
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
    board: Option<&Board>,
    configured_complete: bool,
    supported_complete: bool,
) -> CameraVerdict {
    let mipi = match camera.source {
        Source::V4l2(_) => return CameraVerdict::Unavailable(USB_REASON.to_string()),
        Source::Mipi(ref mipi) => mipi,
    };
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
    /// The sensor lists this size, but the run-time ISP and libcamera may
    /// adjust it. Sentinel does not configure the camera to find out.
    Unknown,
}

/// The verdict for one mode of a camera whose overlays support its sensor.
fn mode_verdict(mode: &Mode) -> ModeVerdict {
    if mode.isp_output != Some(true) && mode.sensor_mode != Some(true) {
        return ModeVerdict::Unavailable(NOT_ISP_SIZE_REASON);
    }
    if mode.format != CAMERAINPUT_FORMAT {
        return ModeVerdict::Unavailable(FORMAT_REASON);
    }
    match mode.isp_output {
        Some(true) => ModeVerdict::Available,
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
        judge(&mut devices, Some(&board), &[]);
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

    /// An ISP output size can be available. A run-time sensor size remains
    /// unjudged because libcamera may adjust it when the camera is configured.
    #[test]
    fn only_exact_isp_output_sizes_are_available() {
        let mode = |value| serde_json::from_value::<Mode>(value).unwrap();
        let other = mode(json!({"format": "NV12", "width": 640, "height": 480}));
        assert!(matches!(
            mode_verdict(&other),
            ModeVerdict::Unavailable(NOT_ISP_SIZE_REASON)
        ));
        let isp = json!({"format": "NV12", "width": 1920, "height": 1080, "isp_output": true});
        assert!(matches!(mode_verdict(&mode(isp)), ModeVerdict::Available));
        let sensor = json!({"format": "NV12", "width": 2028, "height": 1520, "sensor_mode": true});
        assert!(matches!(mode_verdict(&mode(sensor)), ModeVerdict::Unknown));
        let sensor_rgb =
            json!({"format": "RGB3", "width": 2028, "height": 1520, "sensor_mode": true});
        assert!(matches!(
            mode_verdict(&mode(sensor_rgb)),
            ModeVerdict::Unavailable(FORMAT_REASON)
        ));
        assert_eq!(sensor_name("sony,imx477"), "IMX477");
        assert_eq!(sensor_name("imx477"), "IMX477");
        assert_eq!(sensor_name("sony,"), "SONY,");
    }

    /// Reduced from the 3.0.0 B1859 IMX477 catalog captured on 2026-10-08.
    /// Strict libcamera capture preserved nine sizes but adjusted both
    /// 4056-wide requests to 4032. Query-only discovery must not claim any of
    /// the run-time sizes is exact, and in particular must clear the capture's
    /// former positive verdicts for the two disproven sizes.
    #[test]
    fn captured_runtime_sensor_sizes_do_not_get_positive_verdicts() {
        let text = include_str!("fixtures/imx477-platform-3.0-nv12.json");
        let mut catalog: super::super::Catalog = serde_json::from_str(text).unwrap();
        judge(
            &mut catalog.devices,
            catalog.board.as_ref(),
            &catalog.errors,
        );

        let Peripheral::Camera(camera) = &catalog.devices[0] else {
            panic!("capture's first device is its camera");
        };
        assert_eq!(camera.modes.len(), 11);
        assert!(camera
            .modes
            .iter()
            .all(|mode| mode.available.is_none() && mode.reason.is_none()));
        let disproven: Vec<_> = camera
            .modes
            .iter()
            .filter(|mode| mode.width == Some(4056))
            .map(|mode| (mode.width, mode.height, mode.available))
            .collect();
        assert_eq!(
            disproven,
            [
                (Some(4056), Some(2160), None),
                (Some(4056), Some(3040), None)
            ]
        );
    }
}
