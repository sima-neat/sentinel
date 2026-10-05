//! Tests for the V4L2 camera provider. Sysfs trees are synthetic, built in the
//! kernel's uvcvideo layout in a temporary directory; `/dev/videoN` nodes are
//! served by `FakeVideoNode`, so no camera hardware is needed.

use super::*;
use crate::peripherals::camera::{Camera, Source, UsbCamera};
use crate::peripherals::sysutil::os_message;
use crate::peripherals::sysutil::testing::{on_board, write_file, TempDir};
use crate::peripherals::videodev2::testing::*;
use crate::peripherals::videodev2::*;

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::os::unix::fs::symlink;
use std::sync::atomic::Ordering::SeqCst;

use serde_json::{json, Value};

const CAPTURE: u32 = V4L2_BUF_TYPE_VIDEO_CAPTURE;
const USB: &str = "devices/platform/xhci/usb1/1-1";

type Nodes<'a> = Vec<(&'a str, Result<FakeVideoNode, i32>)>;
/// `(width, height, frame period denominators)`.
type Size<'a> = (u32, u32, &'a [u32]);

/// A USB camera (046d:082d plus `attributes`) at `<sys>/<usb>` with interface
/// `<port>:1.0` (number `00`) and one video node per `(name, index)`.
fn add_camera(sys: &Path, usb: &str, attributes: &[(&str, &str)], videos: &[(&str, &str)]) {
    let defaults = [("idVendor", "046d"), ("idProduct", "082d")];
    for (name, value) in defaults.iter().chain(attributes) {
        write_file(&sys.join(usb).join(name), &format!("{value}\n"));
    }
    let interface = format!("{usb}/{}:1.0", usb.rsplit('/').next().unwrap());
    write_file(&sys.join(&interface).join("bInterfaceNumber"), "00\n");
    for (video, index) in videos {
        add_video(sys, &interface, video, index);
    }
}

/// `<parent>/video4linux/<video>` and its class symlink, as the kernel
/// registers a node.
fn add_video(sys: &Path, parent: &str, video: &str, index: &str) {
    let node = format!("{parent}/video4linux/{video}");
    write_file(&sys.join(&node).join("index"), &format!("{index}\n"));
    symlink("../..", sys.join(&node).join("device")).unwrap();
    let class = sys.join("class/video4linux");
    fs::create_dir_all(&class).unwrap();
    symlink(format!("../../{node}"), class.join(video)).unwrap();
}

/// A node whose valid `device_caps` are `capabilities`.
fn node(card: &str, capabilities: u32) -> FakeVideoNode {
    FakeVideoNode::new(card, capabilities | V4L2_CAP_DEVICE_CAPS, capabilities)
}

/// A uvcvideo metadata node: the device's aggregate capabilities include
/// video capture, the node's own `device_caps` only metadata capture.
fn metadata() -> FakeVideoNode {
    let caps = V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_META_CAPTURE | V4L2_CAP_DEVICE_CAPS;
    FakeVideoNode::new("UVC Camera", caps, V4L2_CAP_META_CAPTURE)
}

/// `code` at `sizes`, each at 1/denominator frame periods, in driver order.
fn add_format(node: &mut FakeVideoNode, buf: u32, code: &[u8; 4], sizes: &[Size]) {
    let format = fourcc(code);
    node.formats.push((buf, format, ""));
    for &(width, height, periods) in sizes {
        node.sizes.push((format, discrete_size(width, height)));
        for &denominator in periods {
            let interval = discrete_interval(1, denominator);
            node.intervals.push(((format, width, height), interval));
        }
    }
}

/// YUYV 640x480, then MJPG 1920x1080 and 640x480: non-canonical driver order.
fn uvc_camera(card: &str) -> FakeVideoNode {
    let mut camera = node(card, V4L2_CAP_VIDEO_CAPTURE);
    add_format(&mut camera, CAPTURE, b"YUYV", &[(640, 480, &[30])]);
    let mjpg: &[Size] = &[(1920, 1080, &[15, 30]), (640, 480, &[30])];
    add_format(&mut camera, CAPTURE, b"MJPG", mjpg);
    camera
}

/// Scan `<root>/sys` with `<root>/dev/<name>` served by `nodes` (`Err` is the
/// errno `open` fails with; unknown names fail with `ENOENT`).
fn scan(root: &Path, nodes: Nodes) -> Result<Vec<Peripheral>, ProviderError> {
    let dev = root.join("dev");
    let mut provider = V4l2Provider::with_opener(root.join("sys"), &dev, opener(&dev, nodes));
    on_board(root, provider.discover())
}

/// A scanned camera and its USB source.
fn usb(device: &Peripheral) -> (&Camera, &UsbCamera) {
    let Peripheral::Camera(camera) = device else {
        panic!("not a camera: {device:?}");
    };
    let Source::V4l2(ref source) = camera.source else {
        panic!("not a USB camera: {device:?}");
    };
    (camera, source)
}

fn device_paths(devices: &[Peripheral]) -> Vec<&str> {
    Vec::from_iter(devices.iter().map(|d| usb(d).1.device_path.as_str()))
}

/// Serves `<dev>/<name>` from `nodes`.
fn opener(dev: &Path, nodes: Nodes) -> Opener {
    let nodes = nodes.into_iter().map(|(name, node)| (dev.join(name), node));
    let nodes: HashMap<_, _> = nodes.collect();
    Box::new(move |path: &Path| match nodes.get(path) {
        Some(Ok(node)) => Ok(Box::new(node.clone()) as Box<dyn VideoNode>),
        Some(Err(errno)) => Err(io::Error::from_raw_os_error(*errno)),
        None => Err(io::Error::from_raw_os_error(libc::ENOENT)),
    })
}

/// A temporary root with one camera at `USB` exposing `videos`.
fn one_camera(videos: &[(&str, &str)]) -> TempDir {
    let root = TempDir::new();
    add_camera(&root.path().join("sys"), USB, &[], videos);
    root
}

/// The C920 of Core's v1 catalog fixture (`devices[1]` of
/// `tests/fixtures/peripherals/v1/catalog.json`) with the same id, identity,
/// model and mode, in the flat schema: without the v1 envelope (`provider`,
/// nested `camera`), `connection`, or the support stage's `supported` and
/// `reason`. Its metadata node is excluded. Then with the details a fuller
/// system provides: manufacturer, speed and serial from the USB device,
/// `by_id_path` from the udev link that resolves to the node (none once the
/// node does not resolve), and the format description.
#[test]
fn usb_camera_carries_the_v1_fixture_facts_plus_optional_details() {
    let root = TempDir::new();
    let (sys, dev) = (root.path().join("sys"), root.path().join("dev"));
    let usb = "devices/pci0000:00/usb1/1-2.3";
    let videos = [("video97", "0"), ("video98", "1")];
    add_camera(&sys, usb, &[("product", "HD Pro Webcam C920")], &videos);
    let mut camera = node("HD Pro Webcam C920", V4L2_CAP_VIDEO_CAPTURE);
    add_format(&mut camera, CAPTURE, b"MJPG", &[(1920, 1080, &[30])]);
    let catalog = |camera: &FakeVideoNode| {
        let nodes = vec![("video97", Ok(camera.clone())), ("video98", Ok(metadata()))];
        serde_json::to_value(scan(root.path(), nodes).unwrap()).unwrap()
    };
    let mut expected = json!([{
        "type": "camera", "id": "camera:v4l2:295faa7ac0d61654", "backend": "v4l2",
        "model": "HD Pro Webcam C920", "device_path": "/dev/video97",
        "availability": {"state": "unknown", "reason": "V4L2 does not expose a reliable \
            read-only ownership state; discovery does not acquire, configure, or stream \
            from the camera."},
        "identity": {
            "stable_key": "sysfs:devices/pci0000:00/usb1/1-2.3:interface=00:index=0",
            "topology": "devices/pci0000:00/usb1/1-2.3", "interface": "00",
            "node_index": "0", "vendor_id": "046d", "product_id": "082d"
        },
        "modes": [{
            "format": "MJPG", "width": 1920, "height": 1080,
            "frame_intervals": [{"width": 1920, "height": 1080, "intervals": [
                {"type": "discrete", "numerator": 1, "denominator": 30}]}]
        }]
    }]);
    assert_eq!(catalog(&camera), expected);

    let identity = [
        ("manufacturer", " Logitech \n"),
        ("speed", "480"),
        ("serial", "A1B2"),
    ];
    for (name, value) in identity {
        write_file(&sys.join(usb).join(name), value);
        expected[0]["identity"][name] = json!(value.trim());
    }
    write_file(&dev.join("video97"), "");
    let by_id = dev.join("v4l/by-id");
    fs::create_dir_all(&by_id).unwrap();
    let link = "usb-046d_HD_Pro_Webcam_C920_A1B2-video-index0";
    symlink("../../video97", by_id.join(link)).unwrap();
    symlink("../../video9", by_id.join("usb-dangling-video-index0")).unwrap();
    camera.formats[0].2 = "Motion-JPEG";
    let details = &mut expected[0];
    details["modes"][0]["format_description"] = json!("Motion-JPEG");
    details["by_id_path"] = json!(format!("/dev/v4l/by-id/{link}"));
    assert_eq!(catalog(&camera), expected);

    fs::remove_file(dev.join("video97")).unwrap(); // The fake opener still serves it.
    let details = expected[0].as_object_mut().unwrap();
    details.remove("by_id_path");
    assert_eq!(catalog(&camera), expected);
    assert_eq!(V4l2Provider::new().name(), "camera.v4l2");
    assert_eq!(V4l2Provider::new().subsystems(), ["video4linux"]);
}

/// The id follows the USB port, interface and node index: not the
/// `/dev/videoN` number or the serial, and identical cameras differ. A blank
/// attribute is omitted; the card is the model when there is no product.
#[test]
fn ids_follow_usb_topology() {
    let scan_as = |video: &str, attributes: &[(&str, &str)]| {
        let root = TempDir::new();
        add_camera(&root.path().join("sys"), USB, attributes, &[(video, "0")]);
        let camera = uvc_camera("  USB 2.0 Camera  ");
        let mut records = scan(root.path(), vec![(video, Ok(camera))]).unwrap();
        records.remove(0)
    };
    let first = scan_as("video0", &[("manufacturer", "   ")]);
    let replugged = scan_as("video7", &[("serial", "A123")]);
    let ((first_camera, first), (replugged_camera, replugged)) = (usb(&first), usb(&replugged));
    assert_eq!(first_camera.id, replugged_camera.id);
    let key = format!("sysfs:{USB}:interface=00:index=0");
    let expected = json!({"node_index": "0", "interface": "00", "vendor_id": "046d",
                          "product_id": "082d", "topology": USB, "stable_key": key});
    assert_eq!(serde_json::to_value(&first.identity).unwrap(), expected);
    let mut expected = expected;
    expected["serial"] = json!("A123");
    assert_eq!(serde_json::to_value(&replugged.identity).unwrap(), expected);
    let paths = (first.device_path.as_str(), replugged.device_path.as_str());
    assert_eq!(paths, ("/dev/video0", "/dev/video7"));
    assert_eq!(first_camera.model.as_deref(), Some("USB 2.0 Camera"));

    let root = TempDir::new();
    let (sys, mut nodes) = (root.path().join("sys"), Vec::new());
    let product = [("product", "Webcam C270")];
    for (port, capture, meta) in [("1-1.1", "video0", "video1"), ("1-1.2", "video2", "video3")] {
        let (usb, videos) = (format!("{USB}/{port}"), [(capture, "0"), (meta, "1")]);
        add_camera(&sys, &usb, &product, &videos);
        nodes.push((capture, Ok(uvc_camera("UVC Camera"))));
        nodes.push((meta, Ok(metadata())));
    }
    let records = scan(root.path(), nodes).unwrap();
    assert_ne!(records[0].id(), records[1].id());
    let topology = |r: &Peripheral| usb(r).1.identity.topology.clone();
    let mut topologies = Vec::from_iter(records.iter().map(topology));
    topologies.sort();
    assert_eq!(topologies, [format!("{USB}/1-1.1"), format!("{USB}/1-1.2")]);
}

/// Only USB nodes are opened, and only video capture nodes are enumerated:
/// output, memory-to-memory and metadata nodes stop after `VIDIOC_QUERYCAP`.
/// With `V4L2_CAP_DEVICE_CAPS` the node's `device_caps` apply even when they
/// are zero; without it, the aggregate capabilities.
#[test]
fn only_usb_video_capture_nodes_are_enumerated() {
    let root = TempDir::new();
    let sys = root.path().join("sys");
    add_video(&sys, "devices/platform/soc/1a000000.isp", "video0", "0");
    add_camera(&sys, USB, &[], &[("video1", "0"), ("video2", "1")]);
    add_camera(
        &sys,
        USB,
        &[],
        &[("video3", "2"), ("video4", "3"), ("video5", "4")],
    );
    let output = node("output", V4L2_CAP_VIDEO_OUTPUT);
    let m2m = node("m2m", V4L2_CAP_VIDEO_M2M | V4L2_CAP_VIDEO_CAPTURE);
    let mut zero = uvc_camera("zero");
    zero.capability.device_caps = 0;
    let excluded = [uvc_camera("isp"), output, m2m, metadata(), zero];
    let mut legacy = uvc_camera("legacy");
    legacy.capability.capabilities = V4L2_CAP_VIDEO_CAPTURE;
    legacy.capability.device_caps = 0;
    let names = ["video0", "video1", "video2", "video3", "video5"];
    let mut nodes: Nodes = names.into_iter().zip(excluded.clone().map(Ok)).collect();
    nodes.push(("video4", Ok(legacy)));
    assert_eq!(scan(root.path(), nodes).unwrap().len(), 1);
    let calls = excluded.map(|node| node.calls.load(SeqCst));
    assert_eq!(
        calls,
        [0, 1, 1, 1, 1],
        "the platform node is not even opened"
    );
}

/// Modes are listed by fourcc, then size, with the intervals of every probed
/// size; a size without intervals has no mode.
/// The single- and multi-planar listings of a format merge, keeping the first
/// non-empty description, trimmed. A stepwise or continuous size is the last
/// entry read, probed at its minimum and maximum (once when they are equal).
#[test]
fn modes_are_canonical_and_ranges_are_probed_at_both_ends() {
    let root = one_camera(&[("video0", "0")]);
    let mplane = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    let mut camera = uvc_camera("C920");
    camera.capability.device_caps |= V4L2_CAP_VIDEO_CAPTURE_MPLANE;
    camera.formats[0].2 = "YUYV 4:2:2";
    add_format(&mut camera, mplane, b"MJPG", &[]);
    add_format(&mut camera, mplane, b"YUYV", &[(320, 240, &[])]);
    add_format(&mut camera, mplane, b"GREY", &[]);
    add_format(&mut camera, mplane, b"NV12", &[]);
    camera.formats[2].2 = " Motion-JPEG ";
    camera.formats[3].2 = "ignored";
    let [grey, nv12] = [b"GREY", b"NV12"].map(fourcc);
    let stepwise = raw_size(V4L2_FRMSIZE_TYPE_STEPWISE, [320, 1280, 16, 240, 720, 8]);
    let continuous = raw_size(V4L2_FRMSIZE_TYPE_CONTINUOUS, [64, 64, 1, 64, 64, 1]);
    let after_range = discrete_size(9999, 9999);
    let ranges = [(grey, stepwise), (grey, after_range), (nv12, continuous)];
    camera.sizes.extend(ranges);
    let range = |kind, min| raw_interval(kind, [1, min, 1, 5, 1, 60]);
    camera.intervals.extend([
        ((grey, 320, 240), range(V4L2_FRMIVAL_TYPE_STEPWISE, 60)),
        ((grey, 320, 240), discrete_interval(1, 999)),
        ((grey, 1280, 720), range(V4L2_FRMIVAL_TYPE_CONTINUOUS, 30)),
        ((nv12, 64, 64), discrete_interval(1, 120)),
        ((nv12, 64, 64), discrete_interval(1, 60)),
    ]);
    let records = scan(root.path(), vec![("video0", Ok(camera))]).unwrap();
    let modes = serde_json::to_value(&usb(&records[0]).0.modes).unwrap();
    let modes = modes.as_array().unwrap();
    let summary = |mode: &Value| {
        json!({"format": mode["format"], "width": mode["width"],
               "description": mode["format_description"]})
    };
    let expected = json!([
        {"format": "GREY", "width": null, "description": null},
        {"format": "MJPG", "width": 640, "description": "Motion-JPEG"},
        {"format": "MJPG", "width": 1920, "description": "Motion-JPEG"},
        {"format": "NV12", "width": null, "description": null},
        {"format": "YUYV", "width": 640, "description": "YUYV 4:2:2"}
    ]);
    assert_eq!(Value::from_iter(modes.iter().map(summary)), expected);
    let intervals = json!([{"type": "discrete", "numerator": 1, "denominator": 30},
                           {"type": "discrete", "numerator": 1, "denominator": 15}]);
    assert_eq!(modes[2]["frame_intervals"][0]["intervals"], intervals);
    let range = |kind, min| {
        json!([{"type": kind, "minimum": {"numerator": 1, "denominator": min},
                "maximum": {"numerator": 1, "denominator": 5},
                "step": {"numerator": 1, "denominator": 60}}])
    };
    let grey_mode = json!({
        "format": "GREY",
        "size_range": {"type": "stepwise", "min_width": 320, "min_height": 240,
            "max_width": 1280, "max_height": 720, "step_width": 16, "step_height": 8},
        "frame_intervals": [
            {"width": 320, "height": 240, "intervals": range("stepwise", 60)},
            {"width": 1280, "height": 720, "intervals": range("continuous", 30)}
        ]
    });
    assert_eq!(modes[0], grey_mode);
    let probes = json!([{"width": 64, "height": 64, "intervals": [
        {"type": "discrete", "numerator": 1, "denominator": 120},
        {"type": "discrete", "numerator": 1, "denominator": 60}]}]);
    assert_eq!(modes[3]["frame_intervals"], probes);
    assert_eq!(modes[3]["size_range"]["type"], "continuous");
}

#[test]
fn malformed_sizes_and_intervals_are_rejected() {
    let size = |kind, data| decode_size(raw_size(kind, data)).is_some();
    let stepwise = V4L2_FRMSIZE_TYPE_STEPWISE;
    assert!(size(stepwise, [320, 1920, 16, 240, 1080, 8]));
    assert!(!size(stepwise, [320, 1920, 0, 240, 1080, 8]), "zero step");
    assert!(!size(stepwise, [320, 160, 16, 240, 1080, 8]), "max < min");
    assert!(!size(V4L2_FRMSIZE_TYPE_DISCRETE, [0, 720, 0, 0, 0, 0]));
    assert!(!size(9, [1; 6]), "unknown type");
    let interval = |kind, data| decode_interval(raw_interval(kind, data)).is_some();
    let stepwise = V4L2_FRMIVAL_TYPE_STEPWISE;
    assert!(interval(stepwise, [1, 60, 1, 5, 1, 60]));
    assert!(!interval(stepwise, [1, 60, 1, 5, 0, 60]), "zero step");
    assert!(!interval(stepwise, [1, 4, 1, 5, 1, 60]), "min > max");
    assert!(!interval(V4L2_FRMIVAL_TYPE_DISCRETE, [1, 0, 0, 0, 0, 0]));
}

/// Driver and sysfs errors fail the scan. From `open`, EACCES and EPERM are
/// permission failures; on sysfs only EACCES is.
#[test]
fn driver_and_sysfs_errors_fail_the_scan() {
    let root = one_camera(&[("video0", "0")]);
    let (sys, dev) = (root.path().join("sys"), root.path().join("dev"));
    let fail = |camera| {
        let error = scan(root.path(), vec![("video0", camera)]).unwrap_err();
        (error.code, error.reason)
    };
    let failed = |code: &str, what: &str, errno| {
        let message = os_message(&io::Error::from_raw_os_error(errno));
        (code.to_string(), format!("{what} /dev/video0: {message}"))
    };
    let denied = "io.permission_denied";
    for errno in [libc::EACCES, libc::EPERM] {
        let expected = failed(denied, "failed to open V4L2 camera", errno);
        assert_eq!(fail(Err(errno)), expected);
    }
    let mut camera = uvc_camera("bad");
    camera.fail = Some((VIDIOC_ENUM_FRAMEINTERVALS, libc::EIO));
    let action = "failed to enumerate V4L2 frame intervals for";
    assert_eq!(fail(Ok(camera)), failed("io.open", action, libc::EIO));
    let mut camera = uvc_camera("bad");
    camera.sizes.push((fourcc(b"MJPG"), discrete_size(0, 0)));
    let malformed = "V4L2 camera /dev/video0 returned a malformed frame size";
    assert_eq!(fail(Ok(camera)), ("io.open".into(), malformed.into()));

    let interface = sys.join(USB).join("1-1:1.0/bInterfaceNumber");
    fs::remove_file(&interface).unwrap();
    let c920 = || Ok(uvc_camera("C920"));
    let (code, _) = fail(c920());
    assert_eq!(code, "peripherals.discovery_failed", "no stable interface");
    write_file(&interface, "00\n");
    // `idVendor` alone marks the USB device; `idProduct` is optional.
    fs::remove_file(sys.join(USB).join("idProduct")).unwrap();
    let records = scan(root.path(), vec![("video0", c920())]).unwrap();
    assert_eq!(usb(&records[0]).1.identity.product_id, None);
    // An attribute that exists but cannot be read fails the scan rather than
    // reading as absent.
    let serial = sys.join(USB).join("serial");
    let _ = fs::remove_file(&serial);
    fs::create_dir(&serial).unwrap();
    let (code, reason) = fail(c920());
    assert_eq!(code, "io.open");
    assert!(
        reason.starts_with("failed to read V4L2 sysfs attribute"),
        "{reason}"
    );
    fs::remove_dir(&serial).unwrap();

    // A class entry whose device link loops.
    let entry = sys.join("class/video4linux/video5");
    fs::create_dir_all(&entry).unwrap();
    symlink(entry.join("device"), entry.join("device")).unwrap();
    let (code, reason) = fail(c920());
    assert_eq!(code, "io.open");
    let resolve = "failed to resolve V4L2 sysfs device";
    assert!(reason.starts_with(resolve), "{reason}");
    fs::remove_dir_all(&entry).unwrap();

    for (errno, code) in [(libc::EACCES, denied), (libc::EPERM, "io.open")] {
        let error = io::Error::from_raw_os_error(errno);
        let failure = super::failed("failed to read", &sys, &error, false);
        assert_eq!(failure.error.code, code);
    }

    // The real opener on a regular file: `VIDIOC_QUERYCAP` fails with ENOTTY.
    write_file(&dev.join("video0"), "not a device");
    let mut provider = V4l2Provider::with_opener(&sys, &dev, Box::new(open_system));
    let error = on_board(root.path(), provider.discover()).unwrap_err();
    let action = "failed to query V4L2 capabilities for";
    let expected = failed("io.open", action, libc::ENOTTY);
    assert_eq!((error.code, error.reason), expected);
}

/// A node that vanishes mid-scan (ENODEV, ENXIO, ENOENT, or a class entry
/// without its device link) is skipped; without sysfs there is no camera.
#[test]
fn vanished_devices_are_skipped() {
    let root = one_camera(&[("video0", "0"), ("video1", "1")]);
    let without_sysfs = scan(&root.path().join("none"), vec![]);
    assert!(without_sysfs.unwrap().is_empty());
    let sys = root.path().join("sys");
    fs::create_dir_all(sys.join("class/video4linux/video9")).unwrap();
    let videos = [("video2", "0"), ("video3", "1")];
    add_camera(&sys, "devices/platform/xhci/usb1/1-2", &[], &videos);
    let mut vanishing = uvc_camera("C920");
    vanishing.fail = Some((VIDIOC_ENUM_FRAMESIZES, libc::ENXIO));
    let nodes = vec![
        ("video0", Err(libc::ENODEV)),
        ("video1", Ok(vanishing)),
        ("video3", Ok(uvc_camera("C920"))),
    ];
    let records = scan(root.path(), nodes).unwrap();
    assert_eq!(device_paths(&records), ["/dev/video3"]);
}

/// A camera unplugged once its node is open is skipped, though its `index`
/// then reads as nothing rather than failing with a disappearance errno;
/// other cameras are kept. A present node without `index` still fails the
/// scan as before.
#[test]
fn camera_unplugged_after_open_is_skipped() {
    let root = one_camera(&[("video0", "0")]);
    let (sys, dev) = (root.path().join("sys"), root.path().join("dev"));
    let second = "devices/platform/xhci/usb1/1-2";
    add_camera(&sys, second, &[], &[("video1", "0")]);
    let camera = || Ok(uvc_camera("C920"));
    let nodes = vec![("video0", camera()), ("video1", camera())];
    let (usb, served) = (sys.join(USB), opener(&dev, nodes));
    let unplug: Opener = Box::new(move |path: &Path| {
        let node = served(path)?;
        if path.ends_with("video0") {
            fs::remove_dir_all(&usb).unwrap();
        }
        Ok(node)
    });
    let mut provider = V4l2Provider::with_opener(&sys, &dev, unplug);
    let records = on_board(root.path(), provider.discover()).unwrap();
    assert_eq!(device_paths(&records), ["/dev/video1"]);
    assert!(!sys.join(USB).exists(), "the race was not exercised");

    let root = one_camera(&[("video0", "0")]);
    let node = format!("{USB}/1-1:1.0/video4linux/video0/index");
    fs::remove_file(root.path().join("sys").join(node)).unwrap();
    let error = scan(root.path(), vec![("video0", camera())]).unwrap_err();
    let reason = "V4L2 USB camera did not publish stable topology, interface, and \
                  composite-node index attributes.";
    let expected = (CODE_DISCOVERY_FAILED.into(), reason.into());
    assert_eq!((error.code, error.reason), expected);
}

/// No list may exceed `MAX_ENUMERATION_ENTRIES`, and one device gets
/// `MAX_DEVICE_ENUMERATIONS` enumeration queries in all.
#[test]
fn enumeration_is_bounded_per_list_and_per_device() {
    let root = one_camera(&[("video0", "0")]);
    let reason = |camera| {
        let nodes = vec![("video0", Ok(camera))];
        scan(root.path(), nodes).unwrap_err().reason
    };
    let mut endless = node("endless", V4L2_CAP_VIDEO_CAPTURE);
    let entries = MAX_ENUMERATION_ENTRIES as usize + 1;
    endless.formats = vec![(CAPTURE, fourcc(b"MJPG"), ""); entries];
    let malformed = "V4L2 camera /dev/video0 returned a malformed";
    assert_eq!(reason(endless), format!("{malformed} format list"));
    // A list may hold exactly MAX_ENUMERATION_ENTRIES entries.
    let list = |length: u32| {
        let mut budget = MAX_DEVICE_ENUMERATIONS;
        let query = |index| match index < length {
            true => Ok(index),
            false => Err(io::Error::from_raw_os_error(libc::EINVAL)),
        };
        enumerate(&mut budget, "test", query, |entry| {
            Some(ControlFlow::<u32, u32>::Continue(entry))
        })
    };
    assert_eq!(
        list(MAX_ENUMERATION_ENTRIES).ok().map(|e| e.len()),
        Some(1024)
    );
    let overflow = list(MAX_ENUMERATION_ENTRIES + 1);
    assert!(matches!(overflow, Err(EnumerationError::Malformed(_))));

    // Every list stays under the per-list cap, but 5 x 900 sizes exceed the budget.
    let mut busy = node("busy", V4L2_CAP_VIDEO_CAPTURE);
    let sizes = Vec::from_iter((1..=900).map(|width| (width, 480, &[][..])));
    for format in [b"MJPG", b"YUYV", b"NV12", b"GREY", b"H264"] {
        add_format(&mut busy, CAPTURE, format, &sizes);
    }
    let calls = busy.calls.clone();
    let budget = "enumeration (more than 4096 queries)";
    assert_eq!(reason(busy), format!("{malformed} {budget}"));
    let enumerations = calls.load(SeqCst) - 1; // Not VIDIOC_QUERYCAP.
    assert_eq!(enumerations, MAX_DEVICE_ENUMERATIONS as usize);

    // A format in both the single- and multi-planar lists is enumerated once:
    // 1000 sizes with one rate cost about 3000 queries, twice that would not fit.
    let capture = V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_VIDEO_CAPTURE_MPLANE;
    let mut both = node("both", capture);
    let sizes = Vec::from_iter((1..=1000).map(|width| (width, 480, &[30][..])));
    add_format(&mut both, CAPTURE, b"MJPG", &sizes);
    let mplane = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    both.formats.push((mplane, fourcc(b"MJPG"), ""));
    let records = scan(root.path(), vec![("video0", Ok(both))]).unwrap();
    let records = serde_json::to_value(&records).unwrap();
    assert_eq!(records[0]["modes"].as_array().unwrap().len(), 1000);
}
