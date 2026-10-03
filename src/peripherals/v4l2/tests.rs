//! Tests for the V4L2 camera provider.
//!
//! Every fixture here is synthetic: sysfs trees are built in a temporary
//! directory and V4L2 nodes are served by `FakeBackend`, so no camera hardware
//! is needed. Two sysfs layouts are used:
//!
//! * `add_usb_node` builds a compact tree (absolute `device` symlink to the
//!   video node directory) for the identity and record cases.
//! * `RealLayout` reproduces the kernel's uvcvideo layout (relative class
//!   symlink, `device -> ../../../<interface>`) for the end-to-end scans.
//!
//! Identity, classification, decoding and record cases come first;
//! end-to-end class-coverage cases follow.

use super::ioctl::*;
use super::*;
use crate::peripherals::sysutil::testing::{write_file, TempDir};
use crate::peripherals::videodev2::testing::*;
use crate::peripherals::videodev2::*;

use std::collections::HashMap;
use std::os::unix::fs::symlink;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// A USB video node (046d:082d, interface `00`) under
/// `devices/pci0000:00/usb1/<port>`, with optional product, serial and index.
fn add_usb_node(
    sys: &Path,
    port: &str,
    interface_name: &str,
    video_name: &str,
    node_index: &str,
    product: &str,
    serial: &str,
) -> PathBuf {
    let device = sys.join("devices/pci0000:00/usb1").join(port);
    let interface = device.join(interface_name);
    let video = interface.join("video4linux").join(video_name);
    fs::create_dir_all(&video).unwrap();
    write_file(&device.join("idVendor"), "046d\n");
    write_file(&device.join("idProduct"), "082d\n");
    write_file(&interface.join("bInterfaceNumber"), "00\n");
    if !product.is_empty() {
        write_file(&device.join("product"), &format!("{product}\n"));
    }
    if !serial.is_empty() {
        write_file(&device.join("serial"), &format!("{serial}\n"));
    }
    if !node_index.is_empty() {
        write_file(&video.join("index"), &format!("{node_index}\n"));
    }
    let class_entry = sys.join("class/video4linux").join(video_name);
    fs::create_dir_all(&class_entry).unwrap();
    symlink(&video, class_entry.join("device")).unwrap();
    if !node_index.is_empty() {
        write_file(&class_entry.join("index"), &format!("{node_index}\n"));
    }
    class_entry
}

/// A USB device in the kernel's real sysfs shape.
struct RealLayout<'a> {
    sys: &'a Path,
    /// USB device directory relative to `sys`, e.g. `devices/platform/xhci/usb1/1-1`.
    usb_device: String,
}

impl<'a> RealLayout<'a> {
    fn usb(
        sys: &'a Path,
        usb_device: &str,
        vendor: &str,
        product_id: &str,
        product: Option<&str>,
        serial: Option<&str>,
    ) -> Self {
        let directory = sys.join(usb_device);
        write_file(&directory.join("idVendor"), &format!("{vendor}\n"));
        write_file(&directory.join("idProduct"), &format!("{product_id}\n"));
        if let Some(product) = product {
            write_file(&directory.join("product"), &format!("{product}\n"));
        }
        if let Some(serial) = serial {
            write_file(&directory.join("serial"), &format!("{serial}\n"));
        }
        Self {
            sys,
            usb_device: usb_device.to_string(),
        }
    }

    fn interface(&self, name: &str, number: &str) -> PathBuf {
        let directory = self.sys.join(&self.usb_device).join(name);
        write_file(&directory.join("bInterfaceNumber"), &format!("{number}\n"));
        directory
    }

    /// `<interface>/video4linux/<video>` plus the class symlink, exactly as
    /// uvcvideo registers a node.
    fn video(&self, interface: &str, video: &str, index: &str) {
        let relative = format!("{}/{interface}/video4linux/{video}", self.usb_device);
        let node = self.sys.join(&relative);
        fs::create_dir_all(&node).unwrap();
        write_file(&node.join("index"), &format!("{index}\n"));
        symlink(format!("../../../{interface}"), node.join("device")).unwrap();
        let class = self.sys.join("class/video4linux");
        fs::create_dir_all(&class).unwrap();
        symlink(format!("../../{relative}"), class.join(video)).unwrap();
    }
}

/// A USB camera (046d:082d, no product or serial strings) at
/// `devices/platform/xhci/usb1/<port>` with interface `00` and the given
/// `(video node, index)` pairs.
fn plain_usb_camera(sys: &Path, port: &str, videos: &[(&str, &str)]) {
    let usb = RealLayout::usb(
        sys,
        &format!("devices/platform/xhci/usb1/{port}"),
        "046d",
        "082d",
        None,
        None,
    );
    let interface = format!("{port}:1.0");
    usb.interface(&interface, "00");
    for (video, index) in videos {
        usb.video(&interface, video, index);
    }
}

/// A platform (non-USB) video node in the real sysfs shape.
fn add_platform_node(sys: &Path, parent: &str, video: &str) {
    let relative = format!("{parent}/video4linux/{video}");
    let node = sys.join(&relative);
    fs::create_dir_all(&node).unwrap();
    write_file(&node.join("index"), "0\n");
    symlink("../..", node.join("device")).unwrap();
    let class = sys.join("class/video4linux");
    fs::create_dir_all(&class).unwrap();
    symlink(format!("../../{relative}"), class.join(video)).unwrap();
}

// ---------------------------------------------------------------------------
// Fake ioctl backend
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct FakeBackend {
    nodes: HashMap<PathBuf, Result<FakeVideoNode, i32>>,
    opened: Arc<Mutex<Vec<PathBuf>>>,
}

impl FakeBackend {
    fn node(mut self, path: PathBuf, node: FakeVideoNode) -> Self {
        self.nodes.insert(path, Ok(node));
        self
    }

    fn open_error(mut self, path: PathBuf, errno: i32) -> Self {
        self.nodes.insert(path, Err(errno));
        self
    }
}

impl Backend for FakeBackend {
    fn open(&self, path: &Path) -> io::Result<Box<dyn VideoNode>> {
        self.opened.lock().unwrap().push(path.to_path_buf());
        match self.nodes.get(path) {
            Some(Ok(node)) => Ok(Box::new(node.clone())),
            Some(Err(errno)) => Err(io::Error::from_raw_os_error(*errno)),
            None => Err(io::Error::from_raw_os_error(libc::ENOENT)),
        }
    }
}

fn capability(card: &str, device_caps: u32) -> Capability {
    let mut value = Capability {
        capabilities: device_caps | V4L2_CAP_DEVICE_CAPS,
        device_caps,
        ..Capability::default()
    };
    value.card[..card.len()].copy_from_slice(card.as_bytes());
    value
}

/// `stepwise` member order: min_width, max_width, step_width, min_height,
/// max_height, step_height (as in `<linux/videodev2.h>`).
fn raw_size_range(kind: u32, stepwise: [u32; 6]) -> FrmSizeEnum {
    FrmSizeEnum {
        kind,
        data: stepwise,
        ..FrmSizeEnum::default()
    }
}

fn raw_interval_range(
    kind: u32,
    min: (u32, u32),
    max: (u32, u32),
    step: (u32, u32),
) -> FrmIvalEnum {
    FrmIvalEnum {
        kind,
        data: [min.0, min.1, max.0, max.1, step.0, step.1],
        ..FrmIvalEnum::default()
    }
}

/// A typical UVC webcam: MJPG 1920x1080 and 640x480, YUYV 640x480, listed in
/// non-canonical driver order.
fn uvc_camera(card: &str) -> FakeVideoNode {
    let mjpg = fourcc(b"MJPG");
    let yuyv = fourcc(b"YUYV");
    FakeVideoNode {
        capability: capability(card, V4L2_CAP_VIDEO_CAPTURE),
        ..FakeVideoNode::default()
    }
    .format(V4L2_BUF_TYPE_VIDEO_CAPTURE, yuyv)
    .format(V4L2_BUF_TYPE_VIDEO_CAPTURE, mjpg)
    .size(mjpg, raw_size_discrete(1920, 1080))
    .size(mjpg, raw_size_discrete(640, 480))
    .size(yuyv, raw_size_discrete(640, 480))
    .interval(mjpg, 1920, 1080, raw_interval_discrete(1, 15))
    .interval(mjpg, 1920, 1080, raw_interval_discrete(1, 30))
    .interval(mjpg, 640, 480, raw_interval_discrete(1, 30))
    .interval(yuyv, 640, 480, raw_interval_discrete(1, 30))
}

fn uvc_metadata_node() -> FakeVideoNode {
    FakeVideoNode {
        capability: capability("Fixture Camera", V4L2_CAP_META_CAPTURE),
        ..FakeVideoNode::default()
    }
}

fn provider(sys: &Path, dev: &Path, backend: &FakeBackend) -> V4l2Provider {
    V4l2Provider::with_backend(sys, dev, Box::new(backend.clone()))
}

fn discover(sys: &Path, dev: &Path, backend: &FakeBackend) -> Result<Vec<Record>, ProviderError> {
    provider(sys, dev, backend).discover()
}

// ---------------------------------------------------------------------------
// Sample data (`discrete_interval`, `sample_format`, `sample_device`)
// ---------------------------------------------------------------------------

fn discrete_interval(numerator: u32, denominator: u32) -> Interval {
    Interval {
        kind: RangeType::Discrete,
        value: fraction(numerator, denominator),
        ..Interval::default()
    }
}

fn sample_format() -> Format {
    let discrete = FrameSize {
        kind: RangeType::Discrete,
        width: 1920,
        height: 1080,
        interval_sets: vec![IntervalSet {
            width: 1920,
            height: 1080,
            intervals: vec![discrete_interval(1, 15), discrete_interval(1, 30)],
        }],
        ..FrameSize::default()
    };
    let stepwise = FrameSize {
        kind: RangeType::Stepwise,
        min_width: 320,
        min_height: 240,
        max_width: 1280,
        max_height: 720,
        step_width: 16,
        step_height: 8,
        interval_sets: vec![IntervalSet {
            width: 320,
            height: 240,
            intervals: vec![Interval {
                kind: RangeType::Stepwise,
                minimum: fraction(1, 60),
                maximum: fraction(1, 5),
                step: fraction(1, 60),
                ..Interval::default()
            }],
        }],
        ..FrameSize::default()
    };
    let mut continuous = stepwise.clone();
    continuous.kind = RangeType::Continuous;
    continuous.interval_sets[0].intervals[0].kind = RangeType::Continuous;
    Format {
        fourcc: "MJPG".to_string(),
        sizes: vec![discrete, stepwise, continuous],
    }
}

fn sample_device(topology: &str, device_path: &str) -> Device {
    Device {
        identity: UsbIdentity {
            topology: topology.to_string(),
            interface: "00".to_string(),
            node_index: "0".to_string(),
            vendor_id: "046d".to_string(),
            product_id: "082d".to_string(),
            serial: "fixture-serial".to_string(),
            model: "Fixture Camera".to_string(),
        },
        device_path: device_path.to_string(),
        formats: vec![sample_format()],
    }
}

fn probe_of(devices: Vec<Device>) -> Probe {
    Probe {
        devices,
        ..Probe::default()
    }
}

/// `catalog.json` `devices[1]` from the shared v1 fixture
/// (`core/tests/fixtures/peripherals/v1/catalog.json`), without `supported` and
/// `reason`: the support stage adds those after discovery.
const CANONICAL_V4L2_DEVICE: &str = r#"{
  "id": "camera:v4l2:295faa7ac0d61654",
  "type": "camera",
  "provider": "daemon.camera.v4l2",
  "camera": {
    "model": "HD Pro Webcam C920",
    "backend": "v4l2",
    "connection": "usb",
    "device_path": "/dev/video97",
    "availability": {
      "state": "unknown",
      "reason": "V4L2 does not expose a reliable read-only ownership state; discovery does not acquire, configure, or stream from the camera."
    },
    "identity": {
      "stable_key": "sysfs:devices/pci0000:00/usb1/1-2.3:interface=00:index=0",
      "topology": "devices/pci0000:00/usb1/1-2.3",
      "interface": "00",
      "node_index": "0",
      "vendor_id": "046d",
      "product_id": "082d"
    },
    "modes": [
      {
        "format": "MJPG",
        "width": 1920,
        "height": 1080,
        "framerate_num": 30,
        "framerate_den": 1,
        "frame_intervals": [
          {
            "width": 1920,
            "height": 1080,
            "intervals": [
              {"type": "discrete", "numerator": 1, "denominator": 30}
            ]
          }
        ]
      }
    ]
  }
}"#;

// ---------------------------------------------------------------------------
// Identity, classification, decoding and record cases
// ---------------------------------------------------------------------------

#[test]
fn canonical_record_matches_v1_catalog_fixture() {
    let size = FrameSize {
        kind: RangeType::Discrete,
        width: 1920,
        height: 1080,
        interval_sets: vec![IntervalSet {
            width: 1920,
            height: 1080,
            intervals: vec![discrete_interval(1, 30)],
        }],
        ..FrameSize::default()
    };
    let probe = probe_of(vec![Device {
        identity: UsbIdentity {
            topology: "devices/pci0000:00/usb1/1-2.3".into(),
            interface: "00".into(),
            node_index: "0".into(),
            vendor_id: "046d".into(),
            product_id: "082d".into(),
            serial: String::new(),
            model: "HD Pro Webcam C920".into(),
        },
        device_path: "/dev/video97".into(),
        formats: vec![Format {
            fourcc: "MJPG".into(),
            sizes: vec![size],
        }],
    }]);
    let records = build_records(&probe).unwrap();
    assert_eq!(records.len(), 1);
    let expected: Value = serde_json::from_str(CANONICAL_V4L2_DEVICE).unwrap();
    let actual = records[0].to_catalog_value();
    assert_eq!(actual, expected, "V4L2 output drifted from the v1 fixture");
    // Object keys serialize sorted, as nlohmann::json does, so the encoded
    // bytes match too.
    assert_eq!(
        serde_json::to_string(&actual).unwrap(),
        serde_json::to_string(&expected).unwrap()
    );
}

#[test]
fn usb_identity_uses_canonical_topology_and_optional_metadata() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let first = add_usb_node(
        &sys,
        "1-3.2",
        "1-3.2:1.0",
        "video97",
        "0",
        "HD Pro Webcam C920",
        "A123",
    );
    let identity = usb_identity(&first, &sys)
        .unwrap()
        .expect("USB/UVC node admitted");
    assert_eq!(identity.topology, "devices/pci0000:00/usb1/1-3.2");
    assert_eq!(identity.interface, "00");
    assert_eq!(identity.node_index, "0");
    assert_eq!(identity.serial, "A123");
    assert_eq!(identity.model, "HD Pro Webcam C920");
    assert_eq!(identity.vendor_id, "046d");
    assert_eq!(identity.product_id, "082d");

    let mut serial_missing = identity.clone();
    serial_missing.serial.clear();
    assert_eq!(
        stable_id(&serial_missing),
        stable_id(&identity),
        "optional serial presence must never rename a same-port camera"
    );
}

#[test]
fn composite_capture_nodes_have_distinct_ids() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let first = add_usb_node(&sys, "1-3.2", "1-3.2:1.0", "video97", "0", "C920", "A123");
    let sibling = add_usb_node(&sys, "1-3.2", "1-3.2:1.1", "video98", "1", "", "");
    let first = usb_identity(&first, &sys).unwrap().unwrap();
    let sibling = usb_identity(&sibling, &sys).unwrap().unwrap();
    assert_ne!(stable_id(&sibling), stable_id(&first));
}

#[test]
fn capture_node_classification_uses_device_caps() {
    let mut value = Capability {
        capabilities: V4L2_CAP_DEVICE_CAPS | V4L2_CAP_VIDEO_CAPTURE,
        device_caps: V4L2_CAP_VIDEO_CAPTURE,
        ..Capability::default()
    };
    assert!(is_capture_node(&value), "node-specific capture admitted");
    value.device_caps = V4L2_CAP_META_CAPTURE;
    assert!(!is_capture_node(&value), "metadata node rejected");
    value.device_caps = V4L2_CAP_VIDEO_M2M;
    assert!(!is_capture_node(&value), "memory-to-memory node rejected");
    // Output-only, m2m-mplane, mplane capture, and drivers that do not set
    // V4L2_CAP_DEVICE_CAPS.
    value.device_caps = V4L2_CAP_VIDEO_OUTPUT;
    assert!(!is_capture_node(&value), "output-only node rejected");
    value.device_caps = V4L2_CAP_VIDEO_CAPTURE_MPLANE | V4L2_CAP_VIDEO_M2M_MPLANE;
    assert!(!is_capture_node(&value), "mplane m2m node rejected");
    value.device_caps = V4L2_CAP_VIDEO_CAPTURE_MPLANE;
    assert!(is_capture_node(&value), "mplane capture admitted");
    let legacy = Capability {
        capabilities: V4L2_CAP_VIDEO_CAPTURE,
        device_caps: 0,
        ..Capability::default()
    };
    assert!(
        is_capture_node(&legacy),
        "aggregate caps used without DEVICE_CAPS"
    );
}

#[test]
fn decode_frame_sizes() {
    let decoded = decode_frame_size(&raw_size_discrete(1280, 720)).unwrap();
    assert_eq!(
        (decoded.kind, decoded.width, decoded.height),
        (RangeType::Discrete, 1280, 720)
    );

    let mut stepwise = raw_size_range(V4L2_FRMSIZE_TYPE_STEPWISE, [320, 1920, 16, 240, 1080, 8]);
    let decoded = decode_frame_size(&stepwise).unwrap();
    assert_eq!(decoded.kind, RangeType::Stepwise);
    assert_eq!(decoded.min_width, 320);
    assert_eq!(decoded.max_width, 1920);
    assert_eq!(decoded.step_height, 8);
    stepwise.data[2] = 0;
    assert_eq!(decode_frame_size(&stepwise), None, "zero step rejected");
    stepwise.data[2] = 16;

    let mut continuous = stepwise;
    continuous.kind = V4L2_FRMSIZE_TYPE_CONTINUOUS;
    assert_eq!(
        decode_frame_size(&continuous).unwrap().kind,
        RangeType::Continuous
    );
    // Additional: zero discrete size, reversed range, unknown type.
    assert_eq!(decode_frame_size(&raw_size_discrete(0, 720)), None);
    continuous.data[1] = 100;
    assert_eq!(decode_frame_size(&continuous), None, "max < min rejected");
    assert_eq!(decode_frame_size(&raw_size_range(9, [1; 6])), None);
}

#[test]
fn decode_frame_intervals() {
    let decoded = decode_frame_interval(&raw_interval_discrete(1, 30)).unwrap();
    assert_eq!(decoded.kind, RangeType::Discrete);
    assert_eq!(decoded.value, fraction(1, 30));

    let mut stepwise = raw_interval_range(V4L2_FRMIVAL_TYPE_STEPWISE, (1, 60), (1, 5), (1, 60));
    let decoded = decode_frame_interval(&stepwise).unwrap();
    assert_eq!(decoded.kind, RangeType::Stepwise);
    assert_eq!(decoded.minimum, fraction(1, 60));
    assert_eq!(decoded.maximum, fraction(1, 5));
    stepwise.data[4] = 0;
    stepwise.data[5] = 1;
    assert_eq!(decode_frame_interval(&stepwise), None, "zero step rejected");
    stepwise.data[4] = 1;
    stepwise.data[5] = 60;
    stepwise.data[0] = 1;
    stepwise.data[1] = 4;
    assert_eq!(
        decode_frame_interval(&stepwise),
        None,
        "reversed range rejected"
    );
    stepwise.data[1] = 60;

    let mut continuous = stepwise;
    continuous.kind = V4L2_FRMIVAL_TYPE_CONTINUOUS;
    assert_eq!(
        decode_frame_interval(&continuous).unwrap().kind,
        RangeType::Continuous
    );
    // Additional: zero discrete fraction, unknown type.
    assert_eq!(decode_frame_interval(&raw_interval_discrete(1, 0)), None);
    assert_eq!(
        decode_frame_interval(&raw_interval_range(9, (1, 1), (1, 1), (1, 1))),
        None
    );
}

#[test]
fn two_physical_cameras_produce_two_canonical_records() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let first = add_usb_node(
        &sys,
        "1-3.2",
        "1-3.2:1.0",
        "video97",
        "0",
        "HD Pro Webcam C920",
        "A123",
    );
    let identical = add_usb_node(
        &sys,
        "1-4",
        "1-4:1.0",
        "video99",
        "0",
        "HD Pro Webcam C920",
        "",
    );
    let mut devices = vec![
        sample_device("devices/pci0000:00/usb1/1-3.2", "/dev/video97"),
        sample_device("devices/pci0000:00/usb1/1-4", "/dev/video99"),
    ];
    devices[0].identity = usb_identity(&first, &sys).unwrap().unwrap();
    devices[1].identity = usb_identity(&identical, &sys).unwrap().unwrap();
    let records = build_records(&probe_of(devices)).unwrap();
    assert_eq!(records.len(), 2);

    let camera = &records
        .iter()
        .find(|record| record.details["identity"]["serial"] == "A123")
        .expect("serial-bearing camera record")
        .details;
    assert_eq!(camera["backend"], "v4l2");
    assert_eq!(camera["connection"], "usb");
    let stable_key = camera["identity"]["stable_key"].as_str().unwrap();
    assert!(
        !stable_key.contains("/dev/video"),
        "no routing path in identity"
    );
    assert!(!stable_key.contains("A123"), "serial is metadata only");
    assert_eq!(camera["identity"]["serial"], "A123");
    assert_eq!(camera["identity"]["vendor_id"], "046d");
    assert_eq!(camera["identity"]["product_id"], "082d");
    assert!(
        camera.get("camera_name").is_none(),
        "no libcamera camera_name"
    );
    assert_eq!(camera["availability"]["state"], "unknown");
    assert!(camera["availability"].get("reason").is_some());

    let modes = &camera["modes"];
    assert!(
        modes[0].get("supported").is_none(),
        "the support stage, not the provider, decides"
    );
    assert_eq!(modes[0]["framerate_num"], 30, "fastest interval selected");
    assert_eq!(modes[0]["framerate_den"], 1);
    let intervals = &modes[0]["frame_intervals"][0]["intervals"];
    assert_eq!(intervals.as_array().unwrap().len(), 2, "all intervals kept");
    assert_eq!(intervals[0]["denominator"], 30, "intervals canonicalized");
    assert_eq!(modes[1]["size_range"]["type"], "stepwise");
    assert_eq!(
        modes[1]["frame_intervals"][0]["intervals"][0]["type"],
        "stepwise"
    );
    assert_eq!(modes[2]["size_range"]["type"], "continuous");
    assert_eq!(
        modes[2]["frame_intervals"][0]["intervals"][0]["type"],
        "continuous"
    );
}

#[test]
fn duplicate_aliases_produce_one_record_with_deterministic_path() {
    let records = build_records(&probe_of(vec![
        sample_device("devices/pci0000:00/usb1/1-5", "/dev/video7"),
        sample_device("devices/pci0000:00/usb1/1-5", "/dev/v4l/by-id/camera"),
    ]))
    .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].details["device_path"], "/dev/v4l/by-id/camera");
}

#[test]
fn optional_model_and_device_path_are_omitted_and_serial_keeps_id() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let missing = add_usb_node(&sys, "1-5", "1-5:1.0", "video100", "0", "", "");
    let identity = usb_identity(&missing, &sys).unwrap().unwrap();
    assert!(identity.serial.is_empty() && identity.model.is_empty());
    let mut probe = probe_of(vec![Device {
        identity,
        device_path: String::new(),
        formats: vec![sample_format()],
    }]);
    let without = build_records(&probe).unwrap();
    assert_eq!(without.len(), 1);
    assert!(without[0].details.get("model").is_none());
    assert!(without[0].details.get("device_path").is_none());

    probe.devices[0].identity.serial = "appeared-later".into();
    let with_serial = build_records(&probe).unwrap();
    assert_eq!(with_serial[0].id, without[0].id);
    assert_eq!(
        with_serial[0].details["identity"]["serial"],
        "appeared-later"
    );
}

#[test]
fn empty_probe_and_missing_video4linux_are_successful_empty_results() {
    assert!(build_records(&Probe::default()).unwrap().is_empty());
    let fixture = TempDir::new();
    let result = V4l2Provider::with_roots(
        fixture.path().join("no-sysfs"),
        fixture.path().join("no-dev"),
    )
    .discover()
    .unwrap();
    assert!(result.is_empty());
}

#[test]
fn disappeared_sysfs_device_is_skipped() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    fs::create_dir_all(sys.join("class/video4linux/video0")).unwrap();
    let result = V4l2Provider::with_roots(&sys, fixture.path().join("dev"))
        .discover()
        .unwrap();
    assert!(result.is_empty(), "hot-unplug race is not a failure");
}

#[test]
fn malformed_sysfs_symlink_loop_fails_with_io_open() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let entry = sys.join("class/video4linux/video0");
    fs::create_dir_all(&entry).unwrap();
    symlink(entry.join("device"), entry.join("device")).unwrap();
    let error = V4l2Provider::with_roots(&sys, fixture.path().join("dev"))
        .discover()
        .unwrap_err();
    assert_eq!(error.code, "io.open");
    assert!(
        error.reason.contains("failed to resolve V4L2 sysfs device"),
        "{}",
        error.reason
    );
}

#[test]
fn probe_failures_map_to_provider_error_codes() {
    for (failure, code) in [
        (ProbeFailure::PermissionDenied, "io.permission_denied"),
        (ProbeFailure::Unreadable, "io.open"),
    ] {
        let probe = Probe {
            failure,
            failure_detail: "/dev/video7".into(),
            devices: Vec::new(),
        };
        let error = build_records(&probe).unwrap_err();
        assert_eq!(error.code, code);
        assert!(error.reason.contains("/dev/video7"));
    }
}

// ---------------------------------------------------------------------------
// Class coverage (synthetic sysfs + fake ioctl backend, end to end)
// ---------------------------------------------------------------------------

#[test]
fn provider_metadata() {
    let provider = V4l2Provider::with_roots("/nonexistent/sys", "/nonexistent/dev");
    assert_eq!(provider.name(), "daemon.camera.v4l2");
    assert_eq!(provider.subsystems(), ["video4linux".to_string()]);
}

#[test]
fn several_identical_cameras_at_once_get_distinct_topology_ids() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    let mut backend = FakeBackend::default();
    // Three identical serial-less webcams on a hub; each registers a capture
    // node and a metadata node (uvcvideo >= 4.16).
    for (port, capture, metadata) in [
        ("1-1.1", "video0", "video1"),
        ("1-1.2", "video2", "video3"),
        ("1-1.3", "video4", "video5"),
    ] {
        let usb = RealLayout::usb(
            &sys,
            &format!("devices/platform/xhci/usb1/1-1/{port}"),
            "046d",
            "0825",
            Some("Webcam C270"),
            None,
        );
        usb.interface(&format!("{port}:1.0"), "00");
        usb.video(&format!("{port}:1.0"), capture, "0");
        usb.video(&format!("{port}:1.0"), metadata, "1");
        backend = backend
            .node(dev.join(capture), uvc_camera("UVC Camera (046d:0825)"))
            .node(dev.join(metadata), uvc_metadata_node());
    }
    let records = discover(&sys, &dev, &backend).unwrap();
    assert_eq!(records.len(), 3);
    let mut topologies: Vec<_> = records
        .iter()
        .map(|record| {
            record.details["identity"]["topology"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    topologies.sort();
    assert_eq!(
        topologies,
        [
            "devices/platform/xhci/usb1/1-1/1-1.1",
            "devices/platform/xhci/usb1/1-1/1-1.2",
            "devices/platform/xhci/usb1/1-1/1-1.3",
        ]
    );
    let mut ids: Vec<_> = records.iter().map(|record| record.id.clone()).collect();
    ids.dedup();
    assert_eq!(ids.len(), 3, "records sorted by id and distinct");
    for record in &records {
        assert_eq!(record.kind, "camera");
        assert_eq!(record.provider, "daemon.camera.v4l2");
        assert!(record.id.starts_with("camera:v4l2:"));
        assert!(!record.id.contains("video"));
        assert_eq!(record.details["model"], "Webcam C270");
        assert!(record.details["identity"].get("serial").is_none());
    }
}

#[test]
fn ids_survive_renumbering_of_dev_video_nodes() {
    let build = |capture: &str| {
        let fixture = TempDir::new();
        let sys = fixture.path().join("sys");
        let dev = fixture.path().join("dev");
        let usb = RealLayout::usb(
            &sys,
            "devices/platform/xhci/usb1/1-1",
            "046d",
            "082d",
            None,
            None,
        );
        usb.interface("1-1:1.0", "00");
        usb.video("1-1:1.0", capture, "0");
        let backend = FakeBackend::default().node(dev.join(capture), uvc_camera("C920"));
        let records = discover(&sys, &dev, &backend).unwrap();
        (
            records[0].id.clone(),
            records[0].details["device_path"].clone(),
        )
    };
    let (first_id, first_path) = build("video0");
    let (second_id, second_path) = build("video7");
    assert_eq!(
        first_id, second_id,
        "replug to another /dev/videoN keeps id"
    );
    assert_ne!(first_path, second_path);
}

#[test]
fn missing_vendor_strings_fall_back_to_card_name() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    let usb = RealLayout::usb(
        &sys,
        "devices/platform/xhci/usb1/1-2",
        "1bcf",
        "2284",
        None,
        None,
    );
    usb.interface("1-2:1.0", "00");
    usb.video("1-2:1.0", "video0", "0");
    let backend = FakeBackend::default().node(dev.join("video0"), uvc_camera("  USB 2.0 Camera  "));
    let records = discover(&sys, &dev, &backend).unwrap();
    assert_eq!(records.len(), 1);
    let details = &records[0].details;
    assert_eq!(details["model"], "USB 2.0 Camera", "card name, trimmed");
    assert!(details["identity"].get("serial").is_none());
    assert_eq!(details["identity"]["vendor_id"], "1bcf");
    assert_eq!(details["device_path"], dev.join("video0").to_str().unwrap());
}

#[test]
fn metadata_node_beside_capture_node_is_excluded() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    let usb = RealLayout::usb(
        &sys,
        "devices/platform/xhci/usb1/1-1",
        "046d",
        "082d",
        Some("HD Pro Webcam C920"),
        Some("A123"),
    );
    usb.interface("1-1:1.0", "00");
    usb.video("1-1:1.0", "video0", "0");
    usb.video("1-1:1.0", "video1", "1");
    let backend = FakeBackend::default()
        .node(dev.join("video0"), uvc_camera("HD Pro Webcam C920"))
        .node(dev.join("video1"), uvc_metadata_node());
    let records = discover(&sys, &dev, &backend).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].details["identity"]["node_index"], "0");
    assert_eq!(records[0].details["identity"]["serial"], "A123");
    assert_eq!(
        records[0].details["identity"]["stable_key"],
        "sysfs:devices/platform/xhci/usb1/1-1:interface=00:index=0"
    );
    // Both nodes were opened (classification needs QUERYCAP) but nothing else.
    assert_eq!(backend.opened.lock().unwrap().len(), 2);
}

#[test]
fn output_only_and_m2m_nodes_are_excluded() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    let usb = RealLayout::usb(
        &sys,
        "devices/platform/xhci/usb1/1-3",
        "1d6b",
        "0104",
        None,
        None,
    );
    usb.interface("1-3:1.0", "00");
    usb.video("1-3:1.0", "video0", "0");
    usb.video("1-3:1.0", "video1", "1");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let output = FakeVideoNode {
        capability: capability("Gadget output", V4L2_CAP_VIDEO_OUTPUT),
        calls: calls.clone(),
        ..FakeVideoNode::default()
    };
    let m2m = FakeVideoNode {
        capability: capability("Encoder", V4L2_CAP_VIDEO_M2M | V4L2_CAP_VIDEO_CAPTURE),
        calls: calls.clone(),
        ..FakeVideoNode::default()
    };
    let backend = FakeBackend::default()
        .node(dev.join("video0"), output)
        .node(dev.join("video1"), m2m);
    assert!(discover(&sys, &dev, &backend).unwrap().is_empty());
    assert_eq!(
        *calls.lock().unwrap(),
        [VideoOp::QueryCap, VideoOp::QueryCap],
        "excluded nodes are not enumerated"
    );
}

#[test]
fn non_usb_platform_node_is_never_opened() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    add_platform_node(&sys, "devices/platform/soc/1a000000.isp", "video0");
    add_platform_node(&sys, "devices/platform/soc/1b000000.mipi", "video1");
    plain_usb_camera(&sys, "1-1", &[("video2", "0")]);
    let backend = FakeBackend::default()
        .node(dev.join("video0"), uvc_camera("isp"))
        .node(dev.join("video1"), uvc_camera("mipi"))
        .node(dev.join("video2"), uvc_camera("C920"));
    let records = discover(&sys, &dev, &backend).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(*backend.opened.lock().unwrap(), [dev.join("video2")]);
    let platform_class = sys.join("class/video4linux/video0");
    assert_eq!(usb_identity(&platform_class, &sys).unwrap(), None);
}

#[test]
fn composite_camera_with_audio_interface_reports_video_interface() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    let usb = RealLayout::usb(
        &sys,
        "devices/platform/xhci/usb1/1-4",
        "046d",
        "085e",
        Some("Logitech BRIO"),
        Some("B1"),
    );
    usb.interface("1-4:1.0", "00");
    usb.interface("1-4:1.1", "01");
    // Audio control/streaming interfaces with an ALSA card, no V4L2 node.
    let audio = usb.interface("1-4:1.2", "02");
    usb.interface("1-4:1.3", "03");
    fs::create_dir_all(audio.join("sound/card1")).unwrap();
    usb.video("1-4:1.0", "video0", "0");
    usb.video("1-4:1.0", "video1", "1");
    let backend = FakeBackend::default()
        .node(dev.join("video0"), uvc_camera("Logitech BRIO"))
        .node(dev.join("video1"), uvc_metadata_node());
    let records = discover(&sys, &dev, &backend).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].details["identity"]["interface"], "00");
    assert_eq!(records[0].details["model"], "Logitech BRIO");
}

#[test]
fn discrete_sizes_and_intervals_are_canonicalized() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    plain_usb_camera(&sys, "1-1", &[("video0", "0")]);
    let backend = FakeBackend::default().node(dev.join("video0"), uvc_camera("C920"));
    let records = discover(&sys, &dev, &backend).unwrap();
    let modes = records[0].details["modes"].as_array().unwrap();
    let summary: Vec<_> = modes
        .iter()
        .map(|mode| {
            (
                mode["format"].as_str().unwrap().to_string(),
                mode["width"].as_u64().unwrap(),
                mode["height"].as_u64().unwrap(),
                mode["framerate_num"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("MJPG".to_string(), 640, 480, 30),
            ("MJPG".to_string(), 1920, 1080, 30),
            ("YUYV".to_string(), 640, 480, 30),
        ],
        "formats by fourcc, sizes ascending"
    );
    let intervals = &modes[1]["frame_intervals"][0]["intervals"];
    assert_eq!(intervals[0]["denominator"], 30);
    assert_eq!(intervals[1]["denominator"], 15);
}

#[test]
fn stepwise_and_continuous_sizes_probe_min_and_max_intervals() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    let usb = RealLayout::usb(
        &sys,
        "devices/platform/xhci/usb1/1-1",
        "0c45",
        "6366",
        None,
        None,
    );
    usb.interface("1-1:1.0", "00");
    usb.video("1-1:1.0", "video0", "0");
    let grey = fourcc(b"GREY");
    let nv12 = fourcc(b"NV12");
    let node = FakeVideoNode {
        capability: capability("Range Camera", V4L2_CAP_VIDEO_CAPTURE),
        ..FakeVideoNode::default()
    }
    .format(V4L2_BUF_TYPE_VIDEO_CAPTURE, grey)
    .format(V4L2_BUF_TYPE_VIDEO_CAPTURE, nv12)
    .size(
        grey,
        raw_size_range(V4L2_FRMSIZE_TYPE_STEPWISE, [320, 1280, 16, 240, 720, 8]),
    )
    // Anything after a non-discrete entry must not be enumerated.
    .size(grey, raw_size_discrete(9999, 9999))
    .size(
        nv12,
        raw_size_range(V4L2_FRMSIZE_TYPE_CONTINUOUS, [64, 64, 1, 64, 64, 1]),
    )
    .interval(
        grey,
        320,
        240,
        raw_interval_range(V4L2_FRMIVAL_TYPE_STEPWISE, (1, 60), (1, 5), (1, 60)),
    )
    .interval(grey, 320, 240, raw_interval_discrete(1, 999))
    .interval(
        grey,
        1280,
        720,
        raw_interval_range(V4L2_FRMIVAL_TYPE_CONTINUOUS, (1, 30), (1, 1), (1, 1)),
    )
    .interval(nv12, 64, 64, raw_interval_discrete(1, 120))
    .interval(nv12, 64, 64, raw_interval_discrete(1, 60));
    let backend = FakeBackend::default().node(dev.join("video0"), node);
    let records = discover(&sys, &dev, &backend).unwrap();
    let modes = records[0].details["modes"].as_array().unwrap();
    assert_eq!(modes.len(), 2, "only the first non-discrete size is read");

    let grey_mode = &modes[0];
    assert_eq!(grey_mode["format"], "GREY");
    assert!(grey_mode.get("width").is_none());
    assert_eq!(
        grey_mode["size_range"],
        json!({"type": "stepwise", "min_width": 320, "min_height": 240,
               "max_width": 1280, "max_height": 720, "step_width": 16, "step_height": 8})
    );
    assert_eq!(
        (
            grey_mode["framerate_num"].as_u64(),
            grey_mode["framerate_den"].as_u64()
        ),
        (Some(60), Some(1))
    );
    assert_eq!(
        grey_mode["frame_intervals"],
        json!([
            {"width": 320, "height": 240, "intervals": [
                {"type": "stepwise", "minimum": {"numerator": 1, "denominator": 60},
                 "maximum": {"numerator": 1, "denominator": 5},
                 "step": {"numerator": 1, "denominator": 60}}]},
            {"width": 1280, "height": 720, "intervals": [
                {"type": "continuous", "minimum": {"numerator": 1, "denominator": 30},
                 "maximum": {"numerator": 1, "denominator": 1},
                 "step": {"numerator": 1, "denominator": 1}}]}
        ]),
        "range interval enumeration stops after the first entry"
    );

    let nv12_mode = &modes[1];
    assert_eq!(nv12_mode["size_range"]["type"], "continuous");
    assert_eq!(
        nv12_mode["frame_intervals"].as_array().unwrap().len(),
        1,
        "min == max probes once"
    );
    assert_eq!(nv12_mode["framerate_num"], 120);
}

#[test]
fn size_without_intervals_is_omitted_and_unprintable_fourcc_is_hex() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    plain_usb_camera(&sys, "1-1", &[("video0", "0")]);
    let odd = 0x0000_0001;
    let node = FakeVideoNode {
        capability: capability("Odd", V4L2_CAP_VIDEO_CAPTURE),
        ..FakeVideoNode::default()
    }
    .format(V4L2_BUF_TYPE_VIDEO_CAPTURE, odd)
    .size(odd, raw_size_discrete(640, 480))
    .size(odd, raw_size_discrete(320, 240))
    .interval(odd, 320, 240, raw_interval_discrete(1, 10));
    let backend = FakeBackend::default().node(dev.join("video0"), node);
    let records = discover(&sys, &dev, &backend).unwrap();
    let modes = records[0].details["modes"].as_array().unwrap();
    assert_eq!(modes.len(), 1);
    assert_eq!(modes[0]["format"], "0x00000001");
    assert_eq!(modes[0]["width"], 320);
}

#[test]
fn multiplanar_formats_merge_with_single_planar() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    plain_usb_camera(&sys, "1-1", &[("video0", "0")]);
    let nv12 = fourcc(b"NV12");
    let node = FakeVideoNode {
        capability: capability(
            "Both",
            V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_VIDEO_CAPTURE_MPLANE,
        ),
        ..FakeVideoNode::default()
    }
    .format(V4L2_BUF_TYPE_VIDEO_CAPTURE, nv12)
    .format(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, nv12)
    .size(nv12, raw_size_discrete(1280, 720))
    .interval(nv12, 1280, 720, raw_interval_discrete(1, 30));
    let backend = FakeBackend::default().node(dev.join("video0"), node);
    let records = discover(&sys, &dev, &backend).unwrap();
    assert_eq!(records[0].details["modes"].as_array().unwrap().len(), 1);
}

#[test]
fn permission_denied_on_open_fails_with_permission_code() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    plain_usb_camera(&sys, "1-1", &[("video0", "0")]);
    for errno in [libc::EACCES, libc::EPERM] {
        let backend = FakeBackend::default().open_error(dev.join("video0"), errno);
        let error = discover(&sys, &dev, &backend).unwrap_err();
        assert_eq!(error.code, "io.permission_denied");
        assert!(error.reason.starts_with(&format!(
            "failed to open V4L2 camera {}: ",
            dev.join("video0").display()
        )));
        assert!(!error.reason.contains("os error"), "{}", error.reason);
    }
    let backend = FakeBackend::default().open_error(dev.join("video0"), libc::EACCES);
    let error = discover(&sys, &dev, &backend).unwrap_err();
    assert_eq!(
        error.reason,
        format!(
            "failed to open V4L2 camera {}: {}",
            dev.join("video0").display(),
            os_message(&io::Error::from_raw_os_error(libc::EACCES))
        )
    );
}

#[test]
fn node_that_vanishes_during_probe_is_skipped() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    plain_usb_camera(&sys, "1-1", &[("video0", "0"), ("video1", "1")]);
    plain_usb_camera(&sys, "1-2", &[("video2", "0")]);
    let backend = FakeBackend::default()
        .open_error(dev.join("video0"), libc::ENODEV)
        .node(
            dev.join("video1"),
            uvc_camera("C920").failing(VideoOp::EnumFrameSizes, libc::ENXIO),
        )
        .node(dev.join("video2"), uvc_camera("C920"));
    let records = discover(&sys, &dev, &backend).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].details["device_path"],
        dev.join("video2").to_str().unwrap()
    );
}

#[test]
fn ioctl_error_fails_the_whole_scan_with_io_open() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    plain_usb_camera(&sys, "1-1", &[("video0", "0")]);
    plain_usb_camera(&sys, "1-2", &[("video1", "0")]);
    for (op, action) in [
        (VideoOp::QueryCap, "failed to query V4L2 capabilities for"),
        (VideoOp::EnumFmt, "failed to enumerate V4L2 formats for"),
        (
            VideoOp::EnumFrameSizes,
            "failed to enumerate V4L2 frame sizes for",
        ),
        (
            VideoOp::EnumFrameIntervals,
            "failed to enumerate V4L2 frame intervals for",
        ),
    ] {
        let backend = FakeBackend::default()
            .node(dev.join("video0"), uvc_camera("ok"))
            .node(dev.join("video1"), uvc_camera("bad").failing(op, libc::EIO));
        let error = discover(&sys, &dev, &backend).unwrap_err();
        assert_eq!(error.code, "io.open", "{op:?}");
        assert_eq!(
            error.reason,
            format!(
                "{action} {}: {}",
                dev.join("video1").display(),
                os_message(&io::Error::from_raw_os_error(libc::EIO))
            )
        );
    }
}

#[test]
fn malformed_driver_answers_fail_with_io_open() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    plain_usb_camera(&sys, "1-1", &[("video0", "0")]);
    let mjpg = fourcc(b"MJPG");
    let base = || {
        FakeVideoNode {
            capability: capability("bad", V4L2_CAP_VIDEO_CAPTURE),
            ..FakeVideoNode::default()
        }
        .format(V4L2_BUF_TYPE_VIDEO_CAPTURE, mjpg)
    };
    let bad_size = base().size(mjpg, raw_size_discrete(0, 0));
    let bad_interval = base().size(mjpg, raw_size_discrete(640, 480)).interval(
        mjpg,
        640,
        480,
        raw_interval_discrete(0, 30),
    );
    for (node, what) in [(bad_size, "frame size"), (bad_interval, "frame interval")] {
        let backend = FakeBackend::default().node(dev.join("video0"), node);
        let error = discover(&sys, &dev, &backend).unwrap_err();
        assert_eq!(error.code, "io.open");
        assert_eq!(
            error.reason,
            format!(
                "V4L2 camera {} returned a malformed {what}",
                dev.join("video0").display()
            )
        );
    }
}

#[test]
fn endless_enumeration_is_capped() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    plain_usb_camera(&sys, "1-1", &[("video0", "0")]);
    let mjpg = fourcc(b"MJPG");
    let mut node = FakeVideoNode {
        capability: capability("endless", V4L2_CAP_VIDEO_CAPTURE),
        ..FakeVideoNode::default()
    };
    for _ in 0..=MAX_ENUMERATION_ENTRIES {
        node = node.format(V4L2_BUF_TYPE_VIDEO_CAPTURE, mjpg);
    }
    let backend = FakeBackend::default().node(dev.join("video0"), node);
    let error = discover(&sys, &dev, &backend).unwrap_err();
    assert_eq!(error.code, "io.open");
    assert!(
        error.reason.ends_with("returned a malformed format list"),
        "{}",
        error.reason
    );
}

#[test]
fn usb_interface_without_interface_number_fails_discovery() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    plain_usb_camera(&sys, "1-1", &[("video0", "0")]);
    fs::remove_file(sys.join("devices/platform/xhci/usb1/1-1/1-1:1.0/bInterfaceNumber")).unwrap();
    let backend = FakeBackend::default().node(dev.join("video0"), uvc_camera("C920"));
    let error = discover(&sys, &dev, &backend).unwrap_err();
    assert_eq!(error.code, "peripherals.discovery_failed");
}

#[test]
fn incomplete_vendor_product_attributes_fail_with_io_open() {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    plain_usb_camera(&sys, "1-1", &[("video0", "0")]);
    fs::remove_file(sys.join("devices/platform/xhci/usb1/1-1/idProduct")).unwrap();
    let backend = FakeBackend::default().node(dev.join("video0"), uvc_camera("C920"));
    let error = discover(&sys, &dev, &backend).unwrap_err();
    assert_eq!(error.code, "io.open");
    assert!(
        error.reason.contains("incomplete vendor/product"),
        "{}",
        error.reason
    );
}

#[test]
fn conflicting_aliases_fail_discovery() {
    let mut other = sample_device("devices/pci0000:00/usb1/1-5", "/dev/video8");
    other.formats[0].fourcc = "YUYV".into();
    let error = build_records(&probe_of(vec![
        sample_device("devices/pci0000:00/usb1/1-5", "/dev/video7"),
        other,
    ]))
    .unwrap_err();
    assert_eq!(error.code, "peripherals.discovery_failed");
    assert!(error.reason.contains("conflicting capabilities"));
}

#[test]
fn system_backend_reports_non_v4l2_file_as_unreadable() {
    // Exercises the real open/ioctl path: a regular file opens read-only but
    // rejects VIDIOC_QUERYCAP with ENOTTY.
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    let dev = fixture.path().join("dev");
    plain_usb_camera(&sys, "1-1", &[("video0", "0"), ("video1", "1")]);
    // video0 is absent from /dev (a hot-unplug race) and is skipped.
    write_file(&dev.join("video1"), "not a device");
    let error = V4l2Provider::with_roots(&sys, &dev).discover().unwrap_err();
    assert_eq!(error.code, "io.open");
    assert_eq!(
        error.reason,
        format!(
            "failed to query V4L2 capabilities for {}: {}",
            dev.join("video1").display(),
            os_message(&io::Error::from_raw_os_error(libc::ENOTTY))
        )
    );
}
