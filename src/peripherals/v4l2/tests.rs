//! Tests for the V4L2 camera provider. Sysfs trees are synthetic, built in the
//! kernel's uvcvideo layout in a temporary directory; `/dev/videoN` nodes are
//! served by `FakeVideoNode`, so no camera hardware is needed.

use super::*;
use crate::peripherals::sysutil::testing::{write_file, TempDir};
use crate::peripherals::videodev2::testing::*;
use crate::peripherals::videodev2::*;

use std::collections::HashMap;
use std::os::unix::fs::symlink;
use std::sync::{Arc, Mutex};

type Nodes<'a> = Vec<(&'a str, Result<FakeVideoNode, i32>)>;

/// A USB camera (046d:082d plus `attributes`) at `<sys>/<usb>` with interface
/// `<port>:1.0` (number `00`) and one video node per `(name, index)`.
fn add_camera(sys: &Path, usb: &str, attributes: &[(&str, &str)], videos: &[(&str, &str)]) {
    let defaults = [("idVendor", "046d"), ("idProduct", "082d")];
    for (name, value) in defaults.iter().chain(attributes) {
        write_file(&sys.join(usb).join(name), &format!("{value}\n"));
    }
    let interface = format!("{}:1.0", usb.rsplit('/').next().unwrap());
    let number = sys.join(usb).join(&interface).join("bInterfaceNumber");
    write_file(&number, "00\n");
    for (video, index) in videos {
        add_video(sys, &format!("{usb}/{interface}"), video, index);
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

fn node(card: &str, capabilities: u32) -> FakeVideoNode {
    FakeVideoNode::new(card, capabilities | V4L2_CAP_DEVICE_CAPS, capabilities)
}

/// `data` in `<linux/videodev2.h>` member order: min, max and step width,
/// then min, max and step height.
fn size_range(kind: u32, data: [u32; 6]) -> FrmSizeEnum {
    FrmSizeEnum {
        kind,
        data,
        ..FrmSizeEnum::default()
    }
}

fn interval_range(kind: u32, min: (u32, u32), max: (u32, u32), step: (u32, u32)) -> FrmIvalEnum {
    FrmIvalEnum {
        kind,
        data: [min.0, min.1, max.0, max.1, step.0, step.1],
        ..FrmIvalEnum::default()
    }
}

/// MJPG 1920x1080 and 640x480 and YUYV 640x480, in non-canonical driver order.
fn uvc_camera(card: &str) -> FakeVideoNode {
    let (mjpg, yuyv) = (fourcc(b"MJPG"), fourcc(b"YUYV"));
    node(card, V4L2_CAP_VIDEO_CAPTURE)
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

/// Scan `sys` with `/dev/<name>` served by `nodes` (`Err` is the errno `open`
/// fails with; unknown names fail with `ENOENT`). Also returns the opened paths.
fn scan(sys: &Path, nodes: Nodes) -> (Result<Vec<Record>, ProviderError>, Vec<String>) {
    let nodes: HashMap<String, _> = (nodes.into_iter())
        .map(|(name, node)| (format!("/dev/{name}"), node))
        .collect();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let log = opened.clone();
    let open: Opener = Box::new(move |path: &Path| {
        let path = path.display().to_string();
        log.lock().unwrap().push(path.clone());
        match nodes.get(&path) {
            Some(Ok(node)) => Ok(Box::new(node.clone()) as Box<dyn VideoNode>),
            Some(Err(errno)) => Err(io::Error::from_raw_os_error(*errno)),
            None => Err(io::Error::from_raw_os_error(libc::ENOENT)),
        }
    });
    let result = V4l2Provider::with_opener(sys, "/dev", open).discover();
    let opened = opened.lock().unwrap().clone();
    (result, opened)
}

fn records(sys: &Path, nodes: Nodes) -> Vec<Record> {
    scan(sys, nodes).0.unwrap()
}

/// The failed scan's code and reason.
fn failure(sys: &Path, nodes: Nodes) -> (String, String) {
    let error = scan(sys, nodes).0.unwrap_err();
    (error.code, error.reason)
}

fn sysfs() -> (TempDir, PathBuf) {
    let fixture = TempDir::new();
    let sys = fixture.path().join("sys");
    (fixture, sys)
}

const USB: &str = "devices/platform/xhci/usb1/1-1";

/// A temporary sysfs with one camera at `USB` exposing `videos`.
fn one_camera(videos: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let (fixture, sys) = sysfs();
    add_camera(&sys, USB, &[], videos);
    (fixture, sys)
}

/// `catalog.json` `devices[1]` from the shared v1 fixture
/// (`core/tests/fixtures/peripherals/v1/catalog.json`), without the `supported`
/// and `reason` fields the support stage adds; the metadata node is excluded.
#[test]
fn record_matches_v1_catalog_fixture() {
    let (_fixture, sys) = sysfs();
    let card = "HD Pro Webcam C920";
    let videos = [("video97", "0"), ("video98", "1")];
    let usb = "devices/pci0000:00/usb1/1-2.3";
    add_camera(&sys, usb, &[("product", card)], &videos);
    let mjpg = fourcc(b"MJPG");
    let camera = node(card, V4L2_CAP_VIDEO_CAPTURE)
        .format(V4L2_BUF_TYPE_VIDEO_CAPTURE, mjpg)
        .size(mjpg, raw_size_discrete(1920, 1080))
        .interval(mjpg, 1920, 1080, raw_interval_discrete(1, 30));
    let metadata = node(card, V4L2_CAP_META_CAPTURE);
    let nodes = vec![("video97", Ok(camera)), ("video98", Ok(metadata))];
    let found = records(&sys, nodes);
    let catalog: Vec<Value> = found.iter().map(Record::to_catalog_value).collect();
    let expected = json!({
        "id": "camera:v4l2:295faa7ac0d61654", "type": "camera", "provider": "daemon.camera.v4l2",
        "camera": {
            "model": "HD Pro Webcam C920", "backend": "v4l2", "connection": "usb",
            "device_path": "/dev/video97",
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
                "framerate_num": 30, "framerate_den": 1,
                "frame_intervals": [{"width": 1920, "height": 1080, "intervals": [
                    {"type": "discrete", "numerator": 1, "denominator": 30}]}]
            }]
        }
    });
    assert_eq!(catalog, [expected]);
    let provider = V4l2Provider::new();
    assert_eq!(provider.name(), "daemon.camera.v4l2");
    assert_eq!(provider.subsystems(), ["video4linux".to_string()]);
}

#[test]
fn id_follows_usb_topology_not_dev_node_or_serial() {
    let scan_as = |video: &str, attributes: &[(&str, &str)]| {
        let (_fixture, sys) = sysfs();
        add_camera(&sys, USB, attributes, &[(video, "0")]);
        let camera = uvc_camera("  USB 2.0 Camera  ");
        records(&sys, vec![(video, Ok(camera))]).remove(0)
    };
    let first = scan_as("video0", &[]);
    let replugged = scan_as("video7", &[("serial", "A123")]);
    assert_eq!(first.id, replugged.id);
    assert_eq!(first.details["device_path"], "/dev/video0");
    assert_eq!(replugged.details["device_path"], "/dev/video7");
    assert_eq!(replugged.details["identity"]["serial"], "A123");
    assert!(first.details["identity"].get("serial").is_none());
    assert_eq!(first.details["model"], "USB 2.0 Camera", "trimmed card");
}

#[test]
fn identical_cameras_on_a_hub_get_distinct_ids() {
    let (_fixture, sys) = sysfs();
    let mut nodes = Vec::new();
    for (port, capture, metadata) in [
        ("1-1.1", "video0", "video1"),
        ("1-1.2", "video2", "video3"),
        ("1-1.3", "video4", "video5"),
    ] {
        let usb = format!("devices/platform/xhci/usb1/1-1/{port}");
        let videos = [(capture, "0"), (metadata, "1")];
        add_camera(&sys, &usb, &[("product", "Webcam C270")], &videos);
        nodes.push((capture, Ok(uvc_camera("UVC Camera"))));
        nodes.push((metadata, Ok(node("UVC Camera", V4L2_CAP_META_CAPTURE))));
    }
    let found = records(&sys, nodes);
    let topologies: Vec<_> = (found.iter())
        .map(|record| record.details["identity"]["topology"].as_str().unwrap())
        .collect();
    let mut ids: Vec<_> = found.iter().map(|record| &record.id).collect();
    ids.dedup();
    assert_eq!(ids.len(), 3, "records are sorted by id and distinct");
    for port in ["1-1.1", "1-1.2", "1-1.3"] {
        let topology = format!("devices/platform/xhci/usb1/1-1/{port}");
        assert!(topologies.contains(&topology.as_str()), "{topologies:?}");
    }
}

#[test]
fn only_usb_video_capture_nodes_are_enumerated() {
    let (_fixture, sys) = sysfs();
    add_video(&sys, "devices/platform/soc/1a000000.isp", "video0", "0");
    add_camera(&sys, USB, &[], &[("video1", "0"), ("video2", "1")]);
    add_camera(&sys, USB, &[], &[("video3", "2"), ("video4", "3")]);
    let capture = V4L2_CAP_VIDEO_CAPTURE;
    let excluded = Arc::new(Mutex::new(Vec::new()));
    let exclude = |capabilities| FakeVideoNode {
        calls: excluded.clone(),
        ..node("excluded", capabilities)
    };
    // Without V4L2_CAP_DEVICE_CAPS the aggregate capabilities apply.
    let legacy = FakeVideoNode {
        capability: FakeVideoNode::new("legacy", capture, 0).capability,
        ..uvc_camera("legacy")
    };
    let nodes = vec![
        ("video0", Ok(uvc_camera("isp"))),
        ("video1", Ok(exclude(V4L2_CAP_VIDEO_OUTPUT))),
        ("video2", Ok(exclude(V4L2_CAP_VIDEO_M2M | capture))),
        ("video3", Ok(exclude(V4L2_CAP_META_CAPTURE))),
        ("video4", Ok(legacy)),
    ];
    let (result, opened) = scan(&sys, nodes);
    assert_eq!(result.unwrap().len(), 1);
    let opened: Vec<_> = opened.iter().map(|path| &path[5..]).collect();
    assert_eq!(
        opened,
        ["video1", "video2", "video3", "video4"],
        "not video0"
    );
    assert_eq!(*excluded.lock().unwrap(), [VideoOp::QueryCap; 3]);
}

#[test]
fn discrete_modes_are_canonical_and_planar_listings_merge() {
    let (_fixture, sys) = one_camera(&[("video0", "0")]);
    let (mjpg, yuyv) = (fourcc(b"MJPG"), fourcc(b"YUYV"));
    let both = V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_VIDEO_CAPTURE_MPLANE;
    let camera = FakeVideoNode {
        capability: node("C920", both).capability,
        ..uvc_camera("C920")
    }
    .format(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, mjpg)
    .format(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, yuyv)
    .size(yuyv, raw_size_discrete(320, 240)); // No intervals: no mode.
    let found = records(&sys, vec![("video0", Ok(camera))]);
    let modes = found[0].details["modes"].as_array().unwrap();
    let field = |name| {
        modes
            .iter()
            .map(|mode| mode[name].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        field("format"),
        ["MJPG", "MJPG", "YUYV"],
        "formats by fourcc"
    );
    assert_eq!(field("width"), [640, 1920, 640], "sizes ascending");
    assert_eq!(field("framerate_num"), [30, 30, 30], "fastest rate");
    let intervals = &modes[1]["frame_intervals"][0]["intervals"];
    assert_eq!(
        [&intervals[0]["denominator"], &intervals[1]["denominator"]],
        [30, 15]
    );
}

#[test]
fn range_sizes_probe_intervals_at_min_and_max() {
    let (_fixture, sys) = one_camera(&[("video0", "0")]);
    let (grey, nv12) = (fourcc(b"GREY"), fourcc(b"NV12"));
    let stepwise_size = size_range(V4L2_FRMSIZE_TYPE_STEPWISE, [320, 1280, 16, 240, 720, 8]);
    let continuous_size = size_range(V4L2_FRMSIZE_TYPE_CONTINUOUS, [64, 64, 1, 64, 64, 1]);
    let (step, continuous) = (V4L2_FRMIVAL_TYPE_STEPWISE, V4L2_FRMIVAL_TYPE_CONTINUOUS);
    let range = |kind, min| interval_range(kind, min, (1, 5), (1, 60));
    let camera = node("Range Camera", V4L2_CAP_VIDEO_CAPTURE)
        .format(V4L2_BUF_TYPE_VIDEO_CAPTURE, grey)
        .format(V4L2_BUF_TYPE_VIDEO_CAPTURE, nv12)
        .size(grey, stepwise_size)
        .size(grey, raw_size_discrete(9999, 9999)) // Nothing after a range is read.
        .size(nv12, continuous_size)
        .interval(grey, 320, 240, range(step, (1, 60)))
        .interval(grey, 320, 240, raw_interval_discrete(1, 999))
        .interval(grey, 1280, 720, range(continuous, (1, 30)))
        .interval(nv12, 64, 64, raw_interval_discrete(1, 120))
        .interval(nv12, 64, 64, raw_interval_discrete(1, 60));
    let found = records(&sys, vec![("video0", Ok(camera))]);
    let modes = found[0].details["modes"].as_array().unwrap();
    assert_eq!(modes.len(), 2);
    let fraction =
        |numerator, denominator| json!({"numerator": numerator, "denominator": denominator});
    let range = |kind, min| {
        json!([{"type": kind, "minimum": fraction(1, min),
        "maximum": fraction(1, 5), "step": fraction(1, 60)}])
    };
    let expected = json!({
        "format": "GREY", "framerate_num": 60, "framerate_den": 1,
        "size_range": {"type": "stepwise", "min_width": 320, "min_height": 240,
            "max_width": 1280, "max_height": 720, "step_width": 16, "step_height": 8},
        "frame_intervals": [
            {"width": 320, "height": 240, "intervals": range("stepwise", 60)},
            {"width": 1280, "height": 720, "intervals": range("continuous", 30)}
        ]
    });
    assert_eq!(modes[0], expected);
    assert_eq!(modes[1]["size_range"]["type"], "continuous");
    assert_eq!(modes[1]["framerate_num"], 120);
    let probes = modes[1]["frame_intervals"].as_array().unwrap();
    assert_eq!(probes.len(), 1, "min == max is probed once");
}

#[test]
fn malformed_entries_are_rejected() {
    let size = |data| decode_size(&size_range(V4L2_FRMSIZE_TYPE_STEPWISE, data)).is_some();
    assert!(size([320, 1920, 16, 240, 1080, 8]));
    assert!(!size([320, 1920, 0, 240, 1080, 8]), "zero step");
    assert!(!size([320, 160, 16, 240, 1080, 8]), "max < min");
    assert!(decode_size(&raw_size_discrete(0, 720)).is_none());
    assert!(
        decode_size(&size_range(9, [1; 6])).is_none(),
        "unknown type"
    );
    let kind = V4L2_FRMIVAL_TYPE_STEPWISE;
    let interval = |min, step| decode_interval(&interval_range(kind, min, (1, 5), step)).is_some();
    assert!(interval((1, 60), (1, 60)));
    assert!(!interval((1, 60), (0, 60)), "zero step");
    assert!(!interval((1, 4), (1, 60)), "min > max");
    assert!(decode_interval(&raw_interval_discrete(1, 0)).is_none());
}

#[test]
fn driver_and_sysfs_errors_fail_the_scan() {
    let (_fixture, sys) = one_camera(&[("video0", "0")]);
    let fail = |camera| failure(&sys, vec![("video0", camera)]);
    let message = |errno| os_message(&io::Error::from_raw_os_error(errno));
    for errno in [libc::EACCES, libc::EPERM] {
        let reason = format!("failed to open V4L2 camera /dev/video0: {}", message(errno));
        assert_eq!(fail(Err(errno)), ("io.permission_denied".into(), reason));
    }
    let camera = uvc_camera("bad").failing(VideoOp::EnumFrameIntervals, libc::EIO);
    let action = "failed to enumerate V4L2 frame intervals for";
    let reason = format!("{action} /dev/video0: {}", message(libc::EIO));
    assert_eq!(fail(Ok(camera)), ("io.open".into(), reason));
    let camera = uvc_camera("bad").size(fourcc(b"MJPG"), raw_size_discrete(0, 0));
    let reason = "V4L2 camera /dev/video0 returned a malformed frame size";
    assert_eq!(fail(Ok(camera)), ("io.open".into(), reason.into()));

    let interface = sys.join(USB).join("1-1:1.0/bInterfaceNumber");
    fs::remove_file(&interface).unwrap();
    let (code, _) = fail(Ok(uvc_camera("C920")));
    assert_eq!(code, "peripherals.discovery_failed", "no stable interface");
    write_file(&interface, "00\n");

    let entry = sys.join("class/video4linux/video1");
    fs::create_dir_all(&entry).unwrap();
    symlink(entry.join("device"), entry.join("device")).unwrap();
    let (code, reason) = fail(Ok(uvc_camera("C920")));
    assert_eq!(code, "io.open");
    assert!(
        reason.starts_with("failed to resolve V4L2 sysfs device"),
        "{reason}"
    );
}

#[test]
fn vanished_devices_are_skipped() {
    let (fixture, sys) = one_camera(&[("video0", "0"), ("video1", "1")]);
    assert!(records(&fixture.path().join("no-sysfs"), Vec::new()).is_empty());
    fs::create_dir_all(sys.join("class/video4linux/video9")).unwrap(); // No `device` link.
    add_camera(
        &sys,
        "devices/platform/xhci/usb1/1-2",
        &[],
        &[("video2", "0")],
    );
    let vanishing = uvc_camera("C920").failing(VideoOp::EnumFrameSizes, libc::ENXIO);
    let nodes = vec![
        ("video0", Err(libc::ENODEV)),
        ("video1", Ok(vanishing)),
        ("video2", Ok(uvc_camera("C920"))),
    ];
    let found = records(&sys, nodes);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].details["device_path"], "/dev/video2");
}

#[test]
fn enumeration_is_bounded_per_list_and_per_device() {
    let (_fixture, sys) = one_camera(&[("video0", "0")]);
    let mut endless = node("endless", V4L2_CAP_VIDEO_CAPTURE);
    for _ in 0..=MAX_ENUMERATION_ENTRIES {
        endless = endless.format(V4L2_BUF_TYPE_VIDEO_CAPTURE, fourcc(b"MJPG"));
    }
    let (_, reason) = failure(&sys, vec![("video0", Ok(endless))]);
    assert_eq!(
        reason,
        "V4L2 camera /dev/video0 returned a malformed format list"
    );

    // Every list stays under the per-list cap, but 5 x 900 sizes exceed the budget.
    let mut busy = node("busy", V4L2_CAP_VIDEO_CAPTURE);
    for code in [b"MJPG", b"YUYV", b"NV12", b"GREY", b"H264"] {
        busy = busy.format(V4L2_BUF_TYPE_VIDEO_CAPTURE, fourcc(code));
        for width in 1..=900 {
            busy = busy.size(fourcc(code), raw_size_discrete(width, 480));
        }
    }
    let calls = busy.calls.clone();
    let (code, reason) = failure(&sys, vec![("video0", Ok(busy))]);
    assert_eq!(code, "io.open");
    assert!(
        reason.ends_with("malformed enumeration (more than 4096 queries)"),
        "{reason}"
    );
    let calls = calls.lock().unwrap();
    let enumerations = calls.iter().filter(|op| **op != VideoOp::QueryCap).count();
    assert_eq!(enumerations, MAX_DEVICE_ENUMERATIONS as usize);
}

#[test]
fn system_opener_rejects_a_file_that_is_not_a_v4l2_node() {
    let (fixture, sys) = one_camera(&[("video0", "0"), ("video1", "1")]);
    let dev = fixture.path().join("dev");
    write_file(&dev.join("video1"), "not a device"); // video0 is absent: skipped.
    let mut provider = V4l2Provider::with_opener(&sys, &dev, Box::new(open_system));
    let error = provider.discover().unwrap_err();
    let enotty = os_message(&io::Error::from_raw_os_error(libc::ENOTTY));
    let action = "failed to query V4L2 capabilities for";
    assert_eq!(error.code, "io.open");
    assert_eq!(
        error.reason,
        format!("{action} {}: {enotty}", dev.join("video1").display())
    );
}
