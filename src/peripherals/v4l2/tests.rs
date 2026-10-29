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

/// A USB camera (1a2b:3c4d plus `attributes`) at `<sys>/<usb>` with interface
/// `<port>:1.0` (number `00`, bound to `uvcvideo`) and one video node per
/// `(name, index)`.
fn add_camera(sys: &Path, usb: &str, attributes: &[(&str, &str)], videos: &[(&str, &str)]) {
    let defaults = [("idVendor", "1a2b"), ("idProduct", "3c4d")];
    for (name, value) in defaults.iter().chain(attributes) {
        write_file(&sys.join(usb).join(name), &format!("{value}\n"));
    }
    let interface = format!("{usb}/{}:1.0", usb.rsplit('/').next().unwrap());
    write_file(&sys.join(&interface).join("bInterfaceNumber"), "00\n");
    bind(sys, &interface, "uvcvideo");
    for (video, index) in videos {
        add_video(sys, &interface, video, index);
    }
}

/// Binds the device at `<sys>/<device>` to `driver`, as the kernel links it.
fn bind(sys: &Path, device: &str, driver: &str) {
    let drivers = sys.join("bus/usb/drivers");
    fs::create_dir_all(drivers.join(driver)).unwrap();
    let link = sys.join(device).join("driver");
    let _ = fs::remove_file(&link);
    symlink(drivers.join(driver), link).unwrap();
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

/// A UVC webcam with a pinned id, identity, model and mode. Its metadata
/// node is excluded. Then with the details a fuller
/// system provides: manufacturer, speed and serial from the USB device,
/// `by_id_path` from the udev link that resolves to the node (none once the
/// node does not resolve), and the format description.
#[test]
fn usb_camera_carries_its_facts_plus_optional_details() {
    let root = TempDir::new();
    let (sys, dev) = (root.path().join("sys"), root.path().join("dev"));
    let usb = "devices/pci0000:00/usb1/1-2.3";
    let videos = [("video97", "0"), ("video98", "1")];
    add_camera(&sys, usb, &[("product", "UVC Webcam")], &videos);
    let mut camera = node("UVC Webcam", V4L2_CAP_VIDEO_CAPTURE);
    add_format(&mut camera, CAPTURE, b"MJPG", &[(1920, 1080, &[30])]);
    let catalog = |camera: &FakeVideoNode| {
        let nodes = vec![("video97", Ok(camera.clone())), ("video98", Ok(metadata()))];
        serde_json::to_value(scan(root.path(), nodes).unwrap()).unwrap()
    };
    let mut expected = json!([{
        "type": "camera", "id": "camera:v4l2:295faa7ac0d61654", "backend": "v4l2",
        "model": "UVC Webcam", "device_path": "/dev/video97",
        "availability": {"state": "unknown", "reason": "V4L2 does not expose a reliable \
            read-only ownership state; discovery does not acquire, configure, or stream \
            from the camera."},
        "identity": {
            "stable_key": "sysfs:devices/pci0000:00/usb1/1-2.3:interface=00:index=0",
            "topology": "devices/pci0000:00/usb1/1-2.3", "interface": "00",
            "node_index": "0", "vendor_id": "1a2b", "product_id": "3c4d",
            "driver": "uvcvideo"
        },
        "modes": [{
            "format": "MJPG", "width": 1920, "height": 1080,
            "frame_intervals": [{"width": 1920, "height": 1080, "intervals": [
                {"type": "discrete", "numerator": 1, "denominator": 30}]}]
        }]
    }]);
    assert_eq!(catalog(&camera), expected);

    let identity = [
        ("manufacturer", " Example Vendor \n"),
        ("speed", "480"),
        ("serial", "A1B2"),
    ];
    for (name, value) in identity {
        write_file(&sys.join(usb).join(name), value);
        expected[0]["identity"][name] = json!(value.trim());
    }
    // An ACPI controller (synthetic) names the camera to `UsbCameraInput`.
    write_file(
        &sys.join("devices/pci0000:00/firmware_node/path"),
        "\\_SB_.PCI0\n",
    );
    expected[0]["identity"]["camera_name"] = json!("\\_SB_.PCI0-2.3:1.0-1a2b:3c4d");
    write_file(&dev.join("video97"), "");
    let by_id = dev.join("v4l/by-id");
    fs::create_dir_all(&by_id).unwrap();
    let link = "usb-1a2b_UVC_Webcam_A1B2-video-index0";
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
    let expected = json!({"node_index": "0", "interface": "00", "vendor_id": "1a2b",
                          "product_id": "3c4d", "topology": USB, "stable_key": key,
                          "driver": "uvcvideo"});
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

/// `camera_name` is built as Neat Core's `UsbCameraInput` builds its IDs: the
/// firmware node of the USB device or its nearest ancestor with a firmware
/// node (a device-tree `of_node` before an ACPI `firmware_node/path`), the interface
/// without its bus number, and `vendor:product`. It is omitted when a part is
/// missing, and so is it for a node of another driver, whose driver is still
/// reported. The tree follows the topology of a UVC camera on a DevKit
/// (`docs/peripherals/catalog-example.json`); its firmware nodes are
/// synthetic, after Core's DevKit-shaped fixture.
#[test]
fn camera_name_names_the_controller_port_and_usb_ids() {
    let root = TempDir::new();
    let sys = root.path().join("sys");
    let host = "devices/platform/7ff0000000.pci";
    let xhci = format!("{host}/pci0003:00/0003:00:00.0/0003:01:00.0");
    let device = format!("{xhci}/usb1/1-3/1-3.1");
    add_camera(&sys, &device, &[("idProduct", "5e6f")], &[("video0", "0")]);
    let of_node = |device: &str, node: &str| {
        fs::create_dir_all(sys.join("firmware/devicetree").join(node)).unwrap();
        let link = sys.join(device).join("of_node");
        symlink(sys.join("firmware/devicetree").join(node), link).unwrap();
    };
    of_node(host, "base/pci@7000000000");
    let identity = || {
        let records = scan(root.path(), vec![("video0", Ok(uvc_camera("UVC Webcam")))]).unwrap();
        let identity = &usb(&records[0]).1.identity;
        (identity.driver.clone(), identity.camera_name.clone())
    };
    let uvc = |id: &str| (Some("uvcvideo".to_string()), Some(id.to_string()));
    assert_eq!(identity(), uvc("/base/pci@7000000000-3.1:1.0-1a2b:5e6f"));

    // The nearest firmware-described ancestor wins, its of_node first.
    write_file(&sys.join(&xhci).join("firmware_node/path"), "\\_SB_.XHC_\n");
    assert_eq!(identity(), uvc("\\_SB_.XHC_-3.1:1.0-1a2b:5e6f"));
    of_node(&xhci, "base/pci@7000000000/usb@0");
    assert_eq!(
        identity(),
        uvc("/base/pci@7000000000/usb@0-3.1:1.0-1a2b:5e6f")
    );

    for path in [xhci.clone(), host.to_string()] {
        let _ = fs::remove_file(sys.join(&path).join("of_node"));
        let _ = fs::remove_dir_all(sys.join(&path).join("firmware_node"));
    }
    assert_eq!(
        identity(),
        (Some("uvcvideo".into()), None),
        "no firmware node"
    );
    of_node(host, "base/pci@7000000000");
    fs::remove_file(sys.join(&device).join("idProduct")).unwrap();
    assert_eq!(identity(), (Some("uvcvideo".into()), None), "no idProduct");
    write_file(&sys.join(&device).join("idProduct"), "5e6f\n");
    write_file(&sys.join(&device).join("idVendor"), "\n");
    assert_eq!(
        identity(),
        (Some("uvcvideo".into()), None),
        "empty idVendor"
    );
    write_file(&sys.join(&device).join("idVendor"), "1a2b\n");

    // The walk starts at the USB device: an of_node on the interface is not
    // read, one on the device is.
    let interface = format!("{device}/1-3.1:1.0");
    of_node(&interface, "base/interface");
    let host_name = uvc("/base/pci@7000000000-3.1:1.0-1a2b:5e6f");
    assert_eq!(identity(), host_name);
    of_node(&device, "base/camera");
    assert_eq!(identity(), uvc("/base/camera-3.1:1.0-1a2b:5e6f"));
    fs::remove_file(sys.join(&device).join("of_node")).unwrap();
    // A node outside the device tree keeps its full path (`scan` shows paths
    // below the temporary root as on a board).
    let outside = root.path().join("elsewhere/usb@0");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, sys.join(&xhci).join("of_node")).unwrap();
    assert_eq!(identity(), uvc("/elsewhere/usb@0-3.1:1.0-1a2b:5e6f"));
    fs::remove_file(sys.join(&xhci).join("of_node")).unwrap();
    // A looping of_node names nothing, so the walk goes on to the parent's
    // ACPI path; an empty ACPI path names nothing either.
    let looping = sys.join(&device).join("of_node");
    symlink(&looping, &looping).unwrap();
    let hub = format!("{xhci}/usb1/1-3");
    write_file(&sys.join(&hub).join("firmware_node/path"), "\\_SB_.HS03\n");
    assert_eq!(identity(), uvc("\\_SB_.HS03-3.1:1.0-1a2b:5e6f"));
    write_file(&sys.join(&hub).join("firmware_node/path"), "\n");
    assert_eq!(identity(), host_name);
    fs::remove_file(&looping).unwrap();

    bind(&sys, &interface, "gspca_zc3xx");
    assert_eq!(identity(), (Some("gspca_zc3xx".into()), None));
    fs::remove_file(sys.join(&interface).join("driver")).unwrap();
    assert_eq!(identity(), (None, None));
}

/// Composite cameras, synthetic in uvcvideo's layout. A camera with two
/// streaming interfaces under one VideoControl interface has two capture
/// nodes there, video0 and video2 (each followed by its metadata node), and
/// both get that interface's `camera_name`. Neat opens video0 for a mode both
/// list, so video2's 640x480 MJPEG is unavailable and its other modes are
/// judged as usual. A device with two VideoControl interfaces (`:1.0` and
/// `:1.2`) names them apart, so the same mode is available on both.
#[test]
fn composite_cameras_are_judged_in_neats_node_order() {
    let root = TempDir::new();
    let sys = root.path().join("sys");
    let host = "devices/platform/xhci";
    fs::create_dir_all(sys.join("firmware/devicetree/base/usb@0")).unwrap();
    fs::create_dir_all(sys.join(host)).unwrap();
    symlink(
        sys.join("firmware/devicetree/base/usb@0"),
        sys.join(host).join("of_node"),
    )
    .unwrap();
    let mjpg = |sizes: &[Size]| {
        let mut camera = node("Composite", V4L2_CAP_VIDEO_CAPTURE);
        add_format(&mut camera, CAPTURE, b"MJPG", sizes);
        camera
    };
    let first = mjpg(&[(1920, 1080, &[30]), (640, 480, &[30, 15])]);
    let mut second = mjpg(&[(640, 480, &[30, 15]), (320, 240, &[30])]);
    add_format(&mut second, CAPTURE, b"YUYV", &[(640, 480, &[30])]);
    let judged = |nodes: Nodes| {
        let mut records = scan(root.path(), nodes).unwrap();
        crate::peripherals::support::judge(&mut records, None, &[]);
        let verdicts = |record: &Peripheral| {
            let (camera, source) = usb(record);
            let mode = |mode: &Mode| {
                let size = format!("{} {}x{}", mode.format, mode.width?, mode.height?);
                Some((size, mode.available, mode.reason.clone()))
            };
            let modes = camera.modes.iter().map(|m| mode(m).unwrap()).collect();
            let name = source.identity.camera_name.clone().unwrap();
            (source.device_path.clone(), name, modes)
        };
        Vec::from_iter(records.iter().map(verdicts))
    };
    let path = |video: &str| format!("/dev/{video}");
    let verdict = |size: &str, reason: Option<&str>| {
        (
            size.to_string(),
            Some(reason.is_none()),
            reason.map(str::to_string),
        )
    };
    type Verdicts = Vec<(String, String, Vec<(String, Option<bool>, Option<String>)>)>;

    let videos = [
        ("video0", "0"),
        ("video1", "1"),
        ("video2", "2"),
        ("video3", "3"),
    ];
    add_camera(&sys, USB, &[], &videos);
    let nodes = vec![
        ("video0", Ok(first.clone())),
        ("video1", Ok(metadata())),
        ("video2", Ok(second)),
        ("video3", Ok(metadata())),
    ];
    let name = "/base/usb@0-1:1.0-1a2b:3c4d";
    let elsewhere = "Neat's USB camera input opens another node of this camera for this mode.";
    let expected: Verdicts = vec![
        (
            path("video0"),
            name.into(),
            vec![
                verdict("MJPG 640x480", None),
                verdict("MJPG 1920x1080", None),
            ],
        ),
        (
            path("video2"),
            name.into(),
            vec![
                verdict("MJPG 320x240", None),
                verdict("MJPG 640x480", Some(elsewhere)),
                verdict(
                    "YUYV 640x480",
                    Some("Neat's USB camera input reads MJPEG only."),
                ),
            ],
        ),
    ];
    assert_eq!(judged(nodes), expected);

    fs::remove_dir_all(sys.join(USB)).unwrap();
    fs::remove_dir_all(sys.join("class")).unwrap();
    add_camera(&sys, USB, &[], &[("video0", "0")]);
    let control = format!("{USB}/1-1:1.2");
    write_file(&sys.join(&control).join("bInterfaceNumber"), "02\n");
    bind(&sys, &control, "uvcvideo");
    add_video(&sys, &control, "video2", "0");
    let nodes = vec![("video0", Ok(first.clone())), ("video2", Ok(first))];
    let available = vec![
        verdict("MJPG 640x480", None),
        verdict("MJPG 1920x1080", None),
    ];
    let expected: Verdicts = vec![
        (path("video0"), name.into(), available.clone()),
        (
            path("video2"),
            "/base/usb@0-1:1.2-1a2b:3c4d".into(),
            available,
        ),
    ];
    assert_eq!(judged(nodes), expected);
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
    let mut camera = uvc_camera("UVC Webcam");
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
    let usb_camera = || Ok(uvc_camera("UVC Webcam"));
    let (code, _) = fail(usb_camera());
    assert_eq!(code, "peripherals.discovery_failed", "no stable interface");
    write_file(&interface, "00\n");
    // `idVendor` alone marks the USB device; `idProduct` is optional.
    fs::remove_file(sys.join(USB).join("idProduct")).unwrap();
    let records = scan(root.path(), vec![("video0", usb_camera())]).unwrap();
    assert_eq!(usb(&records[0]).1.identity.product_id, None);
    // An attribute that exists but cannot be read fails the scan rather than
    // reading as absent.
    let serial = sys.join(USB).join("serial");
    let _ = fs::remove_file(&serial);
    fs::create_dir(&serial).unwrap();
    let (code, reason) = fail(usb_camera());
    assert_eq!(code, "io.open");
    assert!(
        reason.starts_with("failed to read V4L2 sysfs attribute"),
        "{reason}"
    );
    fs::remove_dir(&serial).unwrap();
    // So does a driver link that exists but cannot be read as a link.
    let driver = sys.join(USB).join("1-1:1.0/driver");
    fs::remove_file(&driver).unwrap();
    write_file(&driver, "");
    let (code, reason) = fail(usb_camera());
    assert_eq!(code, "io.open");
    let link = "failed to read V4L2 sysfs driver link";
    assert!(reason.starts_with(link), "{reason}");
    fs::remove_file(&driver).unwrap();

    // A class entry whose device link loops.
    let entry = sys.join("class/video4linux/video5");
    fs::create_dir_all(&entry).unwrap();
    symlink(entry.join("device"), entry.join("device")).unwrap();
    let (code, reason) = fail(usb_camera());
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
    let mut vanishing = uvc_camera("UVC Webcam");
    vanishing.fail = Some((VIDIOC_ENUM_FRAMESIZES, libc::ENXIO));
    let nodes = vec![
        ("video0", Err(libc::ENODEV)),
        ("video1", Ok(vanishing)),
        ("video3", Ok(uvc_camera("UVC Webcam"))),
    ];
    let records = scan(root.path(), nodes).unwrap();
    assert_eq!(device_paths(&records), ["/dev/video3"]);
}

/// A driver without `VIDIOC_ENUM_FRAMESIZES` or `VIDIOC_ENUM_FRAMEINTERVALS`
/// (an analog USB grabber, for example) answers `ENOTTY`. Its camera is listed
/// without modes instead of failing the scan, as `camera.mipi` does, and the
/// other cameras keep theirs.
#[test]
fn enumeration_ioctls_a_driver_lacks_give_no_modes() {
    for request in [VIDIOC_ENUM_FRAMESIZES, VIDIOC_ENUM_FRAMEINTERVALS] {
        let root = TempDir::new();
        let sys = root.path().join("sys");
        add_camera(&sys, USB, &[], &[("video0", "0")]);
        add_camera(
            &sys,
            "devices/platform/xhci/usb1/1-2",
            &[],
            &[("video2", "0")],
        );
        let mut grabber = uvc_camera("Grabber");
        grabber.fail = Some((request, libc::ENOTTY));
        let nodes = vec![
            ("video0", Ok(grabber)),
            ("video2", Ok(uvc_camera("UVC Webcam"))),
        ];
        let records = scan(root.path(), nodes).unwrap();
        let modes = |path: &str| {
            let device = records.iter().find(|d| usb(d).1.device_path == path);
            usb(device.unwrap()).0.modes.len()
        };
        assert_eq!(device_paths(&records).len(), 2);
        assert_eq!(modes("/dev/video0"), 0, "request {request:#x}");
        assert_eq!(modes("/dev/video2"), 3);
    }
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
    let camera = || Ok(uvc_camera("UVC Webcam"));
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
