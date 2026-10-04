//! `/dev` and `/sys` are temporary directories and the nodes are fakes.
//! **Transcribed** answers come from a Modalix DevKit (kernel 6.18.3,
//! `media-ctl -p` and `v4l2-ctl --list-formats-ext`, kept in Insight's
//! `tests/fixtures/peripherals/`; re-checked live on 2026-10-03). Everything
//! else is synthetic, including which `/dev/videoN` the ISP is, graph object
//! ids, and the sensor sub-device's device number.

use super::ioctl::*;
use super::*;
use crate::peripherals::support::{core_rules, SupportStage};
use crate::peripherals::sysutil::testing::{on_board, write_file, TempDir};
use crate::peripherals::sysutil::CODE_PERMISSION_DENIED;
use crate::peripherals::videodev2::testing::*;
use crate::peripherals::videodev2::*;

use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::Ordering::SeqCst;

const SENSOR: u32 = MEDIA_ENT_F_CAM_SENSOR;
const SUBDEV: u32 = 0x0002_0000; // MEDIA_ENT_F_V4L2_SUBDEV_UNKNOWN
const IMX477: (u32, &str, u32) = (33, "imx477 5-001a", SENSOR);
const SIZES: [(u32, u32); 3] = [(1920, 1080), (2048, 1080), (2432, 2048)];
const CAPTURE: u32 = V4L2_BUF_TYPE_VIDEO_CAPTURE;
/// The device number of the IMX477 sub-device (synthetic; V4L2's major).
const IMX477_DEVNODE: (u32, u32) = (81, 2);
/// The rates the IMX477's 66.18 fps limit offers.
const IMX477_RATES: [u32; 8] = [66, 60, 30, 25, 20, 15, 10, 5];

type Node<T> = Result<T, i32>;
type Video = (&'static str, Node<FakeVideoNode>);
/// A sensor sub-device: its `/dev` name, device number and node.
type Subdev = (&'static str, (u32, u32), Node<FakeSubdev>);

fn copy_into(destination: &mut [u8], value: &str) {
    destination[..value.len()].copy_from_slice(value.as_bytes());
}

fn einval() -> io::Error {
    io::Error::from_raw_os_error(libc::EINVAL)
}

#[derive(Clone, Default)]
struct FakeMedia(MediaDeviceInfo, Graph);

/// Fills `room` with `items` (left alone when not requested); `ENOSPC` when
/// they do not fit.
fn fill<T: Copy>(room: &mut Vec<T>, items: &[T]) -> io::Result<()> {
    if !room.is_empty() {
        let enospc = io::Error::from_raw_os_error(libc::ENOSPC);
        let slots = room.get_mut(..items.len()).ok_or(enospc)?;
        slots.copy_from_slice(items);
        room.truncate(items.len());
    }
    Ok(())
}

impl MediaNode for FakeMedia {
    fn device_info(&mut self, value: &mut MediaDeviceInfo) -> io::Result<()> {
        *value = self.0;
        Ok(())
    }

    fn topology(&mut self, graph: &mut Graph) -> io::Result<()> {
        let mut read = graph.clone();
        fill(&mut read.entities, &self.1.entities)?;
        fill(&mut read.interfaces, &self.1.interfaces)?;
        fill(&mut read.pads, &self.1.pads)?;
        fill(&mut read.links, &self.1.links)?;
        *graph = read; // As the kernel: nothing is copied back on failure.
        Ok(())
    }
}

/// A media device of media API 6.18.3, as the DevKit reports it, with
/// `(id, name, function)` entities.
fn media(driver: &str, bus_info: &str, entities: &[(u32, &str, u32)]) -> Node<FakeMedia> {
    let mut fake = FakeMedia::default();
    copy_into(&mut fake.0.driver, driver);
    copy_into(&mut fake.0.bus_info, bus_info);
    fake.0.versions[0] = 0x0006_1203;
    for &(id, name, function) in entities {
        let mut entity = MediaV2Entity::default();
        (entity.id, entity.function) = (id, function);
        copy_into(&mut entity.name, name);
        fake.1.entities.push(entity);
    }
    Ok(fake)
}

/// The pad id of `(entity, index)` (synthetic numbering).
fn pad_id((entity, index): (u32, u32)) -> u32 {
    1000 + entity * 16 + index
}

impl FakeMedia {
    fn pad(mut self, entity: u32, index: u32, source: bool) -> Self {
        let mut pad = MediaV2Pad::default();
        let flags = if source { MEDIA_PAD_FL_SOURCE } else { 1 };
        let id = pad_id((entity, index));
        (pad.id, pad.entity_id, pad.flags, pad.index) = (id, entity, flags, index);
        self.1.pads.push(pad);
        self
    }

    fn link(mut self, source: u32, sink: u32, flags: u32) -> Self {
        let mut link = MediaV2Link::default();
        let id = 5000 + self.1.links.len() as u32;
        (link.id, link.source_id, link.sink_id, link.flags) = (id, source, sink, flags);
        self.1.links.push(link);
        self
    }

    /// A data link between `(entity, pad index)` pairs.
    fn data(self, source: (u32, u32), sink: (u32, u32), enabled: bool) -> Self {
        let flags = if enabled { MEDIA_LNK_FL_ENABLED } else { 0 };
        self.link(pad_id(source), pad_id(sink), flags)
    }

    /// A V4L2 sub-device interface for `entity`, with device number `devnode`.
    fn subdev(mut self, entity: u32, (major, minor): (u32, u32)) -> Self {
        let mut interface = MediaV2Interface::default();
        let id = 9000 + self.1.interfaces.len() as u32;
        (interface.id, interface.intf_type) = (id, MEDIA_INTF_T_V4L_SUBDEV);
        interface.data[..2].copy_from_slice(&[major, minor]);
        self.1.interfaces.push(interface);
        let flags = MEDIA_LNK_FL_INTERFACE_LINK | MEDIA_LNK_FL_ENABLED;
        self.link(id, entity, flags)
    }
}

/// Transcribed (`media_ctl_imx477_real.txt`): the SiMa media device and its
/// capture path (entities 1, 5 and 16) with their data links, with `sensors`
/// bound. The IMX477 (entity 33) has image pad 0 linked to the CSI-2
/// receiver's pad 0, an unlinked pad 1, and sub-device `/dev/v4l-subdev2`.
fn real_media(sensors: &[(u32, &str, u32)]) -> Node<FakeMedia> {
    let path = [(1, "raw-capture.1.0", 0x0001_0001), (5, "vdma.1", SUBDEV)];
    let path = [&path[..], &[(16, "csidev-40c3000.csi", SUBDEV)], sensors];
    let mut fake = media(SIMA_MEDIA_DRIVER, "platform:csi2video@1", &path.concat())?;
    fake = fake.pad(1, 0, false).pad(5, 0, false).pad(5, 4, true);
    fake = fake.pad(16, 0, false).pad(16, 1, true);
    fake = fake.data((16, 1), (5, 0), true).data((5, 4), (1, 0), true);
    fake = fake.subdev(16, (81, 1));
    if sensors.contains(&IMX477) {
        fake = fake.pad(33, 0, true).pad(33, 1, true);
        fake = fake.data((33, 0), (16, 0), true);
        fake = fake.subdev(33, IMX477_DEVNODE);
    }
    Ok(fake)
}

/// A sensor sub-device: the active format of one pad, and controls as
/// `(id, minimum or current value, flags)`.
#[derive(Clone, Default)]
struct FakeSubdev {
    format: Option<(u32, u32, u32)>,
    controls: Vec<(u32, i64, u32)>,
}

impl FakeSubdev {
    fn control(&self, id: u32) -> io::Result<(u32, i64, u32)> {
        let found = self.controls.iter().find(|control| control.0 == id);
        found.copied().ok_or_else(einval)
    }
}

impl SubdevNode for FakeSubdev {
    fn format(&mut self, value: &mut SubdevFormat) -> io::Result<()> {
        let active = value.which == V4L2_SUBDEV_FORMAT_ACTIVE;
        let found = self.format.filter(|&(pad, ..)| active && pad == value.pad);
        let (_, width, height) = found.ok_or_else(einval)?;
        (value.format.width, value.format.height) = (width, height);
        Ok(())
    }

    fn query_control(&mut self, value: &mut QueryExtCtrl) -> io::Result<()> {
        (_, value.minimum, value.flags) = self.control(value.id)?;
        Ok(())
    }

    fn control_value(&mut self, id: u32) -> io::Result<i64> {
        Ok(self.control(id)?.1)
    }
}

/// A sensor with the given pixel rate, minimum blanking and pad 0 format.
fn subdev(pixel_rate: i64, blanking: (i64, i64), size: (u32, u32)) -> FakeSubdev {
    let mut controls = vec![(V4L2_CID_HBLANK, blanking.0, 0)];
    controls.push((V4L2_CID_VBLANK, blanking.1, 0));
    controls.push((V4L2_CID_PIXEL_RATE, pixel_rate, 0));
    let format = Some((0, size.0, size.1));
    FakeSubdev { format, controls }
}

/// The IMX477 at 1920x1080 as the DevKit reports it: pixel rate 840 MHz,
/// minimum HBLANK 9332 and VBLANK 48, measured at 65-66 fps.
fn imx477_subdev() -> FakeSubdev {
    subdev(840_000_000, (9332, 48), (1920, 1080))
}

/// The IMX477 sub-device, changed by `change`.
fn imx477_timed(change: impl FnOnce(&mut FakeSubdev)) -> Vec<Subdev> {
    let mut sensor = imx477_subdev();
    change(&mut sensor);
    vec![("v4l-subdev2", IMX477_DEVNODE, Ok(sensor))]
}

/// Transcribed (`v4l2_isp_out_real.txt`): multiplanar capture, formats AR24,
/// RGB3 and NV12, each listing every size eight times, and no intervals.
fn real_isp() -> FakeVideoNode {
    let mut node = FakeVideoNode::new(ISP_CARD_NAME, 0x8520_1000, 0x0520_1000);
    let mplane = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    for format in [b"AR24", b"RGB3", b"NV12"].map(fourcc) {
        node.formats.push((mplane, format, ""));
        let sizes = [SIZES; 8].concat().into_iter();
        let sizes = sizes.map(|(width, height)| (format, discrete_size(width, height)));
        node.sizes.extend(sizes);
    }
    node
}

/// Synthetic: a single-planar ISP with NV12 at `sizes`.
fn nv12_isp(sizes: &[(u32, u32)]) -> FakeVideoNode {
    let mut node = FakeVideoNode::new(ISP_CARD_NAME, V4L2_CAP_VIDEO_CAPTURE, 0);
    let nv12 = fourcc(b"NV12");
    node.formats.push((CAPTURE, nv12, ""));
    for &(width, height) in sizes {
        node.sizes.push((nv12, discrete_size(width, height)));
    }
    node
}

fn isp(node: FakeVideoNode) -> Video {
    (ISP_SYSFS_NAME, Ok(node))
}

type Nodes<T> = HashMap<PathBuf, Node<T>>;

#[derive(Clone, Default)]
struct Fakes(Nodes<FakeMedia>, Nodes<FakeVideoNode>, Nodes<FakeSubdev>);

/// Opening a node the test did not declare is a test failure.
fn open<T: Clone>(nodes: &Nodes<T>, path: &Path) -> io::Result<T> {
    let node = nodes.get(path).unwrap_or_else(|| panic!("opened {path:?}"));
    node.clone().map_err(io::Error::from_raw_os_error)
}

impl Backend for Fakes {
    fn open_media(&self, path: &Path) -> io::Result<Box<dyn MediaNode>> {
        Ok(Box::new(open(&self.0, path)?))
    }

    fn open_video(&self, path: &Path) -> io::Result<Box<dyn VideoNode>> {
        Ok(Box::new(open(&self.1, path)?))
    }

    fn open_subdev(&self, path: &Path) -> io::Result<Box<dyn SubdevNode>> {
        Ok(Box::new(open(&self.2, path)?))
    }
}

/// Scan a board with `/dev/media<index>` for each of `media` (beside the
/// decoys `/dev/media` and `/dev/media0x`), `/dev/video<index>` with a sysfs
/// `name` for each of `video` (no video4linux class when empty), and sensor
/// sub-devices that `/sys/dev/char` links name as the kernel does.
fn scan(
    media: impl IntoIterator<Item = Node<FakeMedia>>,
    video: impl IntoIterator<Item = Video>,
    subdevs: impl IntoIterator<Item = Subdev>,
) -> Result<Vec<Record>, ProviderError> {
    let directory = TempDir::new();
    let (root, mut fakes) = (directory.path(), Fakes::default());
    for name in ["dev/media", "dev/media0x"] {
        write_file(&root.join(name), "");
    }
    for (index, node) in media.into_iter().enumerate() {
        write_file(&root.join(format!("dev/media{index}")), "");
        fakes.0.insert(root.join(format!("dev/media{index}")), node);
    }
    for (index, (name, node)) in video.into_iter().enumerate() {
        let sysfs = root.join(format!("sys/class/video4linux/video{index}/name"));
        write_file(&sysfs, &format!("{name}\n"));
        fakes.1.insert(root.join(format!("dev/video{index}")), node);
    }
    for (name, (major, minor), node) in subdevs {
        let link = root.join(format!("sys/dev/char/{major}:{minor}"));
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        let target = format!("../../devices/platform/csi/video4linux/{name}");
        std::os::unix::fs::symlink(target, link).unwrap();
        fakes.2.insert(root.join("dev").join(name), node);
    }
    let mut provider = MipiProvider::with_roots(root.join("sys"), root.join("dev"));
    provider.backend = Box::new(fakes);
    on_board(root, provider.discover())
}

/// The details of the one camera on the IMX477 board with `video` nodes
/// and `subdevs`.
fn imx477(video: impl IntoIterator<Item = Video>, subdevs: Vec<Subdev>) -> Value {
    let records = scan([real_media(&[IMX477])], video, subdevs).unwrap();
    assert_eq!(records.len(), 1);
    records[0].details.clone()
}

/// The DevKit board with `subdevs`. Opening its raw capture node would fail
/// the ISP probe.
fn real_board(subdevs: Vec<Subdev>) -> Vec<Record> {
    let raw = ("raw-capture.1.0", Err(libc::EIO));
    scan([real_media(&[IMX477])], [raw, isp(real_isp())], subdevs).unwrap()
}

fn mode(format: &str, size: (u32, u32), rate: (u32, u32), source: &str) -> Value {
    json!({"format": format, "width": size.0, "height": size.1, "framerate_num": rate.0,
           "framerate_den": rate.1, "framerate_source": source, "isp_output": true})
}

fn nominal(size: (u32, u32)) -> Value {
    mode("NV12", size, (30, 1), "nominal")
}

/// The real ISP's modes, each size at `rates` from `source`.
fn real_isp_modes(rates: &[u32], source: &str) -> Value {
    let mut modes = Vec::new();
    for format in ["AR24", "NV12", "RGB3"] {
        for size in SIZES {
            for &rate in rates {
                modes.push(mode(format, size, (rate, 1), source));
            }
        }
    }
    Value::from(modes)
}

/// `format@rate` of each mode Neat Core's rules support.
fn supported(mut records: Vec<Record>) -> String {
    let directory = TempDir::new();
    let rules = directory.path().join("neat-core.json");
    fs::write(&rules, core_rules().to_string()).unwrap();
    SupportStage::new(&rules).apply(&mut records, &mut Vec::new());
    let modes = records[0].details["modes"].as_array().unwrap().iter();
    let supported = modes.filter(|mode| mode["supported"] == true);
    let label = |mode: &Value| format!("{}@{}", mode["format"], mode["framerate_num"]);
    Vec::from_iter(supported.map(label)).join(" ")
}

/// Transcribed, then classified with Neat Core's rules: NV12 is supported at
/// every ISP size.
#[test]
fn real_imx477_devkit_board_produces_one_camera_with_isp_modes() {
    let records = real_board(vec![]);
    let expected = json!({
        "id": "camera:imx477 5-001a", "type": "camera", "provider": "daemon.camera.mipi",
        "camera": {
            "camera_name": "imx477 5-001a",
            "model": "imx477",
            "backend": "mipi",
            "connection": "mipi-csi2",
            "media_device": "/dev/media0",
            "bus_info": "platform:csi2video@1",
            "csi_receiver": "csidev-40c3000.csi",
            "availability": {"state": "unknown", "reason": AVAILABILITY_REASON},
            "modes": real_isp_modes(&[30], "nominal"),
            "isp": {"state": "available", "device_path": "/dev/video1",
                    "device_paths": ["/dev/video1"]},
        }
    });
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].to_catalog_value(), expected);
    assert_eq!(MipiProvider::new().subsystems(), ["media", "video4linux"]);
    assert_eq!(supported(records), r#""NV12"@30 "NV12"@30 "NV12"@30"#);
}

/// Only `/dev/media<N>` devices of the SiMa driver are read (uvcvideo also
/// registers a MEDIA_ENT_F_CAM_SENSOR entity), past devices that vanished
/// mid-scan; each sensor is a camera (`econ-imx568-fpga 5-0042` is from
/// `cam_info_imx568.txt`). Transcribed (`media_ctl_no_sensor.txt`): without a
/// sensor there is no camera and no ISP query; likewise without `/dev`.
#[test]
fn sensors_of_sima_media_devices_are_cameras() {
    let uvc = media("uvcvideo", "usb-xhci-hcd.0-1", &[(3, "Camera 1", SENSOR)]);
    let first = real_media(&[IMX477, (40, "econ-imx568-fpga 5-0042", SENSOR)]);
    let ov5647 = [(9, "ov5647", SENSOR)];
    let second = media(SIMA_MEDIA_DRIVER, "platform:csi2video@2", &ov5647);
    let media = [uvc, first, Err(libc::ENODEV), Err(libc::ENOENT), second];
    let records = scan(media, [isp(real_isp())], []).unwrap();
    let modes = real_isp_modes(&[30], "nominal");
    let fields = |record: &Record| {
        let d = &record.details;
        assert_eq!(d["modes"], modes);
        json!({"name": d["camera_name"], "model": d["model"], "media": d["media_device"],
               "bus": d["bus_info"]})
    };
    let expected = json!([
        {"name": "econ-imx568-fpga 5-0042", "model": "econ-imx568-fpga", "media": "/dev/media1",
         "bus": "platform:csi2video@1"},
        {"name": "imx477 5-001a", "model": "imx477", "media": "/dev/media1",
         "bus": "platform:csi2video@1"},
        {"name": "ov5647", "model": "ov5647", "media": "/dev/media4", "bus": "platform:csi2video@2"}
    ]);
    assert_eq!(Value::from_iter(records.iter().map(fields)), expected);

    let node = real_isp();
    let calls = node.calls.clone();
    assert!(scan([real_media(&[])], [isp(node)], []).unwrap().is_empty());
    assert_eq!(calls.load(SeqCst), 0);
    let mut no_dev = MipiProvider::with_roots("/nonexistent", "/nonexistent");
    assert!(no_dev.discover().unwrap().is_empty());
}

/// A media device that cannot be read, or whose sensors cannot be addressed
/// by name, fails the scan. Listing `/dev`, only EACCES is a permission
/// failure; opening a device, EPERM is too.
#[test]
fn media_device_failures_fail_the_scan() {
    let failure = |media: Vec<Node<FakeMedia>>| {
        let error = scan(media, [], []).unwrap_err();
        (error.code, error.reason)
    };
    let message = |errno| os_message(&io::Error::from_raw_os_error(errno));
    for errno in [libc::EACCES, libc::EPERM] {
        let reason = format!(
            "failed to open media device /dev/media0: {}",
            message(errno)
        );
        let expected = (CODE_PERMISSION_DENIED.into(), reason);
        assert_eq!(failure(vec![Err(errno)]), expected);
        let error = listing_failure(Path::new("/dev"), &io::Error::from_raw_os_error(errno));
        let denied = errno == libc::EACCES;
        assert_eq!(error.code == CODE_PERMISSION_DENIED, denied);
    }
    let (code, reason) = failure(vec![real_media(&[IMX477]); 2]);
    let both = "\"imx477 5-001a\" is reported by both /dev/media0 and /dev/media1";
    assert_eq!(code, CODE_DISCOVERY_FAILED);
    assert!(reason.contains(both), "{reason}");
    let unnamed = "media device /dev/media0 has an unnamed sensor entity 4";
    let expected = (CODE_DISCOVERY_FAILED.into(), unnamed.into());
    assert_eq!(failure(vec![real_media(&[(4, "", SENSOR)])]), expected);
    let entities = vec![(0, "vdma", SUBDEV); MAX_ENUMERATION_ENTRIES as usize + 1];
    let malformed = "media device /dev/media0 returned a malformed entity list";
    let expected = (CODE_IO_OPEN.into(), malformed.into());
    assert_eq!(failure(vec![real_media(&entities)]), expected);

    // The real open and ioctl path on a regular file (ENOTTY).
    let directory = TempDir::new();
    write_file(&directory.path().join("dev/media0"), "not a device");
    let mut provider = MipiProvider::with_roots("/nonexistent", directory.path().join("dev"));
    let error = provider.discover().unwrap_err();
    let reason = "failed to query media device information for";
    assert!(error.code == CODE_IO_OPEN && error.reason.starts_with(reason));
}

/// Several ISP nodes report the modes they share; a node with another card
/// or no discrete size is skipped. Discrete ISP intervals replace the nominal
/// 30/1; a size without them, a range-only size, and a driver without the
/// ioctl keep it.
#[test]
fn isp_nodes_provide_the_modes() {
    let mut card = real_isp();
    card.capability.card = [0; 32];
    let second = nv12_isp(&[(3840, 2160), (2048, 1080), (1920, 1080)]);
    let isps = [real_isp(), second, card, nv12_isp(&[])].map(isp);
    let details = imx477(isps, vec![]);
    let modes = json!([nominal((1920, 1080)), nominal((2048, 1080))]);
    assert_eq!(details["modes"], modes);
    let paths = ["/dev/video0", "/dev/video1"];
    let available = json!({"state": "available", "device_path": paths[0], "device_paths": paths});
    assert_eq!(details["isp"], available);

    let nv12 = fourcc(b"NV12");
    let mut continuous = discrete_interval(1, 60);
    continuous.kind = V4L2_FRMIVAL_TYPE_CONTINUOUS;
    let mut node = nv12_isp(&SIZES);
    node.intervals = vec![
        ((nv12, 1920, 1080), discrete_interval(1, 60)),
        ((nv12, 1920, 1080), discrete_interval(1001, 30000)),
        ((nv12, 2432, 2048), continuous),
    ];
    let rate = |rate| mode("NV12", (1920, 1080), rate, "isp");
    let mut expected = vec![rate((60, 1)), rate((30000, 1001))];
    expected.extend(SIZES[1..].iter().map(|&size| nominal(size)));
    let details = imx477([isp(node.clone())], vec![]);
    assert_eq!(details["modes"], json!(expected));
    node.fail = Some((VIDIOC_ENUM_FRAMEINTERVALS, libc::ENOTTY));
    let nominal_modes = json!(SIZES.map(nominal));
    assert_eq!(imx477([isp(node)], vec![])["modes"], nominal_modes);
}

/// A missing, unreadable or malformed ISP keeps the camera, without modes.
/// No ISP list may exceed `MAX_ENUMERATION_ENTRIES`, and one ISP node gets
/// `MAX_DEVICE_ENUMERATIONS` queries in all.
#[test]
fn isp_failures_leave_the_camera_without_modes() {
    let mut eio = real_isp();
    eio.fail = Some((VIDIOC_ENUM_FMT, libc::EIO));
    let mut zero = nv12_isp(&SIZES);
    let key = (fourcc(b"NV12"), 1920, 1080);
    zero.intervals.push((key, discrete_interval(0, 30)));
    let mut formats = nv12_isp(&[]);
    formats.formats = vec![(CAPTURE, 1, ""); MAX_ENUMERATION_ENTRIES as usize + 1];
    let mut queries = nv12_isp(&[]);
    for format in [b"AR24", b"RGB3", b"GREY", b"YUYV"].map(fourcc) {
        queries.formats.push((CAPTURE, format, ""));
        let sizes = (1..=900).map(|width| (format, discrete_size(width, 480)));
        queries.sizes.extend(sizes);
    }
    let calls = queries.calls.clone();
    let videos: [Vec<Video>; 8] = [
        vec![],
        vec![("raw-capture.1.0", Ok(real_isp()))],
        vec![(ISP_SYSFS_NAME, Err(libc::EACCES))],
        vec![isp(eio)],
        vec![isp(real_isp()), isp(nv12_isp(&[(640, 480)]))],
        vec![isp(zero)],
        vec![isp(formats)],
        vec![isp(queries)],
    ];
    let failed = |what: &str, errno| {
        let message = os_message(&io::Error::from_raw_os_error(errno));
        format!("{what}: {message}")
    };
    let malformed = "ISP node /dev/video0 returned a malformed";
    let reasons = [
        failed("could not read /sys/class/video4linux", libc::ENOENT),
        "no Modalix ISP output node was found".into(),
        failed("could not open /dev/video0", libc::EACCES),
        failed("VIDIOC_ENUM_FMT failed for /dev/video0", libc::EIO),
        "Modalix ISP output nodes reported no common discrete sizes".into(),
        format!("{malformed} frame interval"),
        format!("{malformed} format list"),
        format!("{malformed} enumeration (more than 4096 queries)"),
    ];
    for (video, reason) in videos.into_iter().zip(reasons) {
        let details = imx477(video, vec![]);
        let isp = json!({"state": "unavailable", "reason": reason});
        assert_eq!((&details["modes"], &details["isp"]), (&json!([]), &isp));
    }
    let enumerations = calls.load(SeqCst) - 1; // Not VIDIOC_QUERYCAP.
    assert_eq!(enumerations, MAX_DEVICE_ENUMERATIONS as usize);
}

/// Transcribed graph and ISP with the sensor values the DevKit reports: the
/// receiver, the timing, a 66.18 fps limit, and every ISP size offered at the
/// sensor's rates instead of the nominal 30/1; rates the ISP reports are
/// kept. With Neat Core's rules only NV12 at 30/1 is supported.
#[test]
fn imx477_sensor_timing_sets_max_fps_and_rates() {
    let records = real_board(imx477_timed(|_| ()));
    let details = &records[0].details;
    let timing = json!({"pixel_rate": 840_000_000_u64, "hblank_min": 9332,
                        "vblank_min": 48, "width": 1920, "height": 1080});
    assert_eq!(details["sensor_timing"], timing);
    assert_eq!(details["max_fps"], json!(66.18));
    assert_eq!(details["csi_receiver"], "csidev-40c3000.csi");
    let modes = real_isp_modes(&IMX477_RATES, "sensor_timing");
    assert_eq!(details["modes"], modes);
    assert_eq!(supported(records), r#""NV12"@30 "NV12"@30 "NV12"@30"#);

    let mut node = nv12_isp(&SIZES[..2]);
    let key = (fourcc(b"NV12"), 1920, 1080);
    node.intervals.push((key, discrete_interval(1, 60)));
    let details = imx477([isp(node)], imx477_timed(|_| ()));
    let mut expected = vec![mode("NV12", SIZES[0], (60, 1), "isp")];
    let timed = |rate| mode("NV12", SIZES[1], (rate, 1), "sensor_timing");
    expected.extend(IMX477_RATES.map(timed));
    assert_eq!(details["modes"], json!(expected));
}

/// Synthetic: a sensor without a readable sub-device, format or control, or
/// a graph without the links, interfaces or pad index that lead to them,
/// keeps the nominal modes without timing, and the scan succeeds. Without
/// links there is no receiver either.
#[test]
fn sensor_without_timing_keeps_the_nominal_modes() {
    let without = |id| imx477_timed(|s| s.controls.retain(|control| control.0 != id));
    let sensors = [
        vec![("v4l-subdev2", IMX477_DEVNODE, Err(libc::EACCES))],
        without(V4L2_CID_HBLANK),
        without(V4L2_CID_VBLANK),
        without(V4L2_CID_PIXEL_RATE),
        imx477_timed(|s| s.controls[1].2 = V4L2_CTRL_FLAG_DISABLED),
        imx477_timed(|s| s.controls[0].1 = -1),
        imx477_timed(|s| s.controls[2].1 = 0),
        imx477_timed(|s| s.format = Some((1, 1920, 1080))),
        imx477_timed(|s| s.format = Some((0, 0, 0))),
        // No `/sys/dev/char` entry, or one naming a video node: not opened.
        vec![],
        vec![("video5", IMX477_DEVNODE, Ok(imx477_subdev()))],
    ];
    let real = || real_media(&[IMX477]);
    let mut cases = Vec::from_iter(sensors.into_iter().map(|s| (real(), s, true)));
    // No links or interfaces; links that do not fit; no pad index before
    // media API 4.19.
    let bare = media(SIMA_MEDIA_DRIVER, "platform:csi2video@1", &[IMX477]);
    let mut crowded = real().unwrap();
    let filler = vec![MediaV2Link::default(); MAX_ENUMERATION_ENTRIES as usize];
    crowded.1.links.extend(filler);
    let mut old = real().unwrap();
    old.0.versions[0] = (4 << 16) | (18 << 8);
    for (media, receiver) in [(bare, false), (Ok(crowded), false), (Ok(old), true)] {
        cases.push((media, imx477_timed(|_| ()), receiver));
    }
    let nominal_modes = real_isp_modes(&[30], "nominal");
    for (case, (media, subdevs, receiver)) in cases.into_iter().enumerate() {
        let records = scan([media], [isp(real_isp())], subdevs).unwrap();
        let details = &records[0].details;
        let expected = receiver.then(|| json!("csidev-40c3000.csi"));
        assert_eq!(details.get("csi_receiver"), expected.as_ref(), "{case}");
        let timing = (details.get("sensor_timing"), details.get("max_fps"));
        assert_eq!(timing, (None, None), "{case}");
        assert_eq!(details["modes"], nominal_modes, "{case}");
    }
}

/// Synthetic: each sensor gets its own receiver, timing and rates; an
/// enabled link wins over a disabled one from a lower pad.
#[test]
fn several_sensors_report_their_own_timing() {
    let ov = (40, "ov5647 4-0036", SENSOR);
    let receivers = [(17, "csidev-40d0000.csi", SUBDEV), (18, "unused", SUBDEV)];
    let mut graph = real_media(&[&[IMX477, ov][..], &receivers].concat()).unwrap();
    graph = graph.pad(17, 0, false).pad(18, 0, false);
    graph = graph.pad(40, 0, true).pad(40, 1, true);
    graph = graph.data((40, 0), (18, 0), false);
    graph = graph.data((40, 1), (17, 0), true);
    graph = graph.subdev(40, (81, 7));
    let mut ov_sensor = subdev(58_333_000, (1896, 4), (640, 480));
    ov_sensor.format = Some((1, 640, 480)); // The linked pad.
    let mut subdevs = imx477_timed(|_| ());
    subdevs.push(("v4l-subdev7", (81, 7), Ok(ov_sensor)));
    let records = scan([Ok(graph)], [isp(real_isp())], subdevs).unwrap();
    let fields = |record: &Record| {
        let d = &record.details;
        json!({"name": d["camera_name"], "receiver": d["csi_receiver"], "max_fps": d["max_fps"],
               "rate": d["modes"][0]["framerate_num"]})
    };
    // 58333000 / ((640 + 1896) * (480 + 4)) = 47.52
    let expected = json!([
        {"name": "imx477 5-001a", "receiver": "csidev-40c3000.csi", "max_fps": 66.18, "rate": 66},
        {"name": "ov5647 4-0036", "receiver": "csidev-40d0000.csi", "max_fps": 47.52, "rate": 48}
    ]);
    assert_eq!(Value::from_iter(records.iter().map(fields)), expected);
}

#[test]
fn sensor_rates_follow_insight_rule() {
    assert_eq!(sensor_rates(66.18), IMX477_RATES);
    assert_eq!(sensor_rates(30.0), [30, 25, 20, 15, 10, 5]);
    assert_eq!(sensor_rates(120.0), [120, 60, 30, 25, 20, 15, 10, 5]);
    assert_eq!(sensor_rates(29.6), [30, 25, 20, 15, 10, 5], "snaps up");
    assert_eq!(sensor_rates(29.4), [29, 25, 20, 15, 10, 5]);
    assert_eq!(sensor_rates(7.0), [7, 5]);
    assert_eq!(sensor_rates(0.2), [1], "at least 1");
    let mut timing = SensorTiming {
        pixel_rate: 840_000_000,
        hblank_min: 9332,
        vblank_min: 48,
        width: 1920,
        height: 1080,
    };
    assert_eq!(timing.max_fps(), 66.18);
    (timing.width, timing.height) = (4056, 3040);
    assert_eq!(timing.max_fps(), 20.32);
}
