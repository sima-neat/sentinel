//! `/dev` and `/sys` are temporary directories and the nodes are fakes.
//! **Transcribed** answers come from a Modalix DevKit (kernel 6.18.3,
//! `media-ctl -p` and `v4l2-ctl --list-formats-ext`, kept in Insight's
//! `tests/fixtures/peripherals/`; re-checked live on 2026-10-03). Everything
//! else is synthetic, including which `/dev/videoN` the ISP is, graph object
//! ids, and the sensor sub-device's device number.

use super::ioctl::*;
use super::*;
use crate::peripherals::support::{core_rules, SupportStage};
use crate::peripherals::sysutil::testing::{write_file, TempDir};
use crate::peripherals::videodev2::testing::*;
use crate::peripherals::videodev2::*;

use serde_json::Value;
use std::collections::HashMap;

const SENSOR: u32 = MEDIA_ENT_F_CAM_SENSOR;
const SUBDEV: u32 = 0x0002_0000; // MEDIA_ENT_F_V4L2_SUBDEV_UNKNOWN
const IMX477: (u32, &str, u32) = (33, "imx477 5-001a", SENSOR);
const SIZES: [(u32, u32); 3] = [(1920, 1080), (2048, 1080), (2432, 2048)];
const CAPTURE: u32 = V4L2_BUF_TYPE_VIDEO_CAPTURE;

type Node<T> = Result<T, i32>;
type Video<'a> = (&'a str, Node<FakeVideoNode>);

fn copy_into(destination: &mut [u8], value: &str) {
    destination[..value.len()].copy_from_slice(value.as_bytes());
}

#[derive(Clone, Default)]
struct FakeMedia(MediaDeviceInfo, Graph);

/// Fills `room` with `items` (left alone when not requested); `ENOSPC` when
/// they do not fit.
fn fill<T: Copy>(room: &mut Vec<T>, items: &[T]) -> io::Result<()> {
    if room.is_empty() {
        return Ok(());
    }
    let enospc = io::Error::from_raw_os_error(libc::ENOSPC);
    room.get_mut(..items.len())
        .ok_or(enospc)?
        .copy_from_slice(items);
    room.truncate(items.len());
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

/// Media API 6.18.3, as the DevKit reports it.
const MEDIA_VERSION: u32 = 0x0006_1203;

/// A media device with `(id, name, function)` entities.
fn media(driver: &str, bus_info: &str, entities: &[(u32, &str, u32)]) -> Node<FakeMedia> {
    let mut fake = FakeMedia::default();
    copy_into(&mut fake.0.driver, driver);
    copy_into(&mut fake.0.bus_info, bus_info);
    fake.0.versions[0] = MEDIA_VERSION;
    for &(id, name, function) in entities {
        let mut entity = MediaV2Entity::default();
        (entity.id, entity.function) = (id, function);
        copy_into(&mut entity.name, name);
        fake.1.entities.push(entity);
    }
    Ok(fake)
}

/// The pad id of `(entity, index)` (synthetic numbering).
fn pad_id(entity: u32, index: u32) -> u32 {
    1000 + entity * 16 + index
}

impl FakeMedia {
    fn pad(mut self, entity: u32, index: u32, source: bool) -> Self {
        let flags = if source { MEDIA_PAD_FL_SOURCE } else { 1 };
        let (id, entity_id) = (pad_id(entity, index), entity);
        let pad = MediaV2Pad {
            id,
            entity_id,
            flags,
            index,
            ..MediaV2Pad::default()
        };
        self.1.pads.push(pad);
        self
    }

    /// A data link from `(entity, pad index)` to `(entity, pad index)`.
    fn link(mut self, source: (u32, u32), sink: (u32, u32), enabled: bool) -> Self {
        let link = MediaV2Link {
            id: 5000 + self.1.links.len() as u32,
            source_id: pad_id(source.0, source.1),
            sink_id: pad_id(sink.0, sink.1),
            flags: if enabled { MEDIA_LNK_FL_ENABLED } else { 0 },
            ..MediaV2Link::default()
        };
        self.1.links.push(link);
        self
    }

    /// An interface of `kind` for `entity`, with device number `devnode`.
    fn interface(mut self, entity: u32, kind: u32, (major, minor): (u32, u32)) -> Self {
        let id = 9000 + self.1.interfaces.len() as u32;
        let mut interface = MediaV2Interface {
            id,
            intf_type: kind,
            ..MediaV2Interface::default()
        };
        interface.data[..2].copy_from_slice(&[major, minor]);
        self.1.interfaces.push(interface);
        self.1.links.push(MediaV2Link {
            id: 5000 + self.1.links.len() as u32,
            source_id: id,
            sink_id: entity,
            flags: MEDIA_LNK_FL_INTERFACE_LINK | MEDIA_LNK_FL_ENABLED,
            ..MediaV2Link::default()
        });
        self
    }
}

/// The device number of the IMX477 sub-device (synthetic; V4L2's major).
const IMX477_DEVNODE: (u32, u32) = (81, 2);

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
    fake = fake.link((16, 1), (5, 0), true).link((5, 4), (1, 0), true);
    fake = fake.interface(16, MEDIA_INTF_T_V4L_SUBDEV, (81, 1));
    if sensors.iter().any(|&(id, ..)| id == IMX477.0) {
        fake = fake.pad(33, 0, true).pad(33, 1, true);
        fake = fake.link((33, 0), (16, 0), true);
        fake = fake.interface(33, MEDIA_INTF_T_V4L_SUBDEV, IMX477_DEVNODE);
    }
    Ok(fake)
}

/// A sensor sub-device: the active format of one pad, control minimums with
/// their flags, and current control values.
#[derive(Clone, Default)]
struct FakeSubdev {
    format: Option<(u32, u32, u32)>,
    controls: Vec<(u32, i64, u32)>,
    values: Vec<(u32, i64)>,
}

impl SubdevNode for FakeSubdev {
    fn format(&mut self, value: &mut SubdevFormat) -> io::Result<()> {
        let active = value.which == V4L2_SUBDEV_FORMAT_ACTIVE;
        let found = self.format.filter(|&(pad, ..)| active && pad == value.pad);
        let (_, width, height) = found.ok_or(io::Error::from_raw_os_error(libc::EINVAL))?;
        (value.format.width, value.format.height) = (width, height);
        Ok(())
    }

    fn query_control(&mut self, value: &mut QueryExtCtrl) -> io::Result<()> {
        let found = self.controls.iter().find(|(id, ..)| *id == value.id);
        let &(_, minimum, flags) = found.ok_or(io::Error::from_raw_os_error(libc::EINVAL))?;
        (value.minimum, value.flags) = (minimum, flags);
        Ok(())
    }

    fn control_value(&mut self, id: u32) -> io::Result<i64> {
        let found = self.values.iter().find(|(control, _)| *control == id);
        found
            .map(|&(_, value)| value)
            .ok_or(io::Error::from_raw_os_error(libc::EINVAL))
    }
}

/// A sensor with the given pixel rate, minimum blanking and pad 0 format.
fn subdev(
    pixel_rate: i64,
    (hblank, vblank): (i64, i64),
    (width, height): (u32, u32),
) -> FakeSubdev {
    FakeSubdev {
        format: Some((0, width, height)),
        controls: vec![
            (V4L2_CID_HBLANK, hblank, 0),
            (V4L2_CID_VBLANK, vblank, 0),
            (V4L2_CID_PIXEL_RATE, pixel_rate, V4L2_CTRL_FLAG_READ_ONLY),
        ],
        values: vec![(V4L2_CID_PIXEL_RATE, pixel_rate)],
    }
}

const V4L2_CTRL_FLAG_READ_ONLY: u32 = 0x0004;

/// The IMX477 at 1920x1080 as the DevKit reports it: pixel rate 840 MHz,
/// minimum HBLANK 9332 and VBLANK 48, measured at 65-66 fps.
fn imx477_subdev() -> FakeSubdev {
    subdev(840_000_000, (9332, 48), (1920, 1080))
}

/// Transcribed (`v4l2_isp_out_real.txt`): multiplanar capture, formats AR24,
/// RGB3 and NV12, each listing every size eight times, and no intervals.
fn real_isp() -> FakeVideoNode {
    let mut node = FakeVideoNode::new(ISP_CARD_NAME, 0x8520_1000, 0x0520_1000);
    for format in [b"AR24", b"RGB3", b"NV12"].map(fourcc) {
        node = node.format(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE, format);
        for (width, height) in [SIZES; 8].concat() {
            node = node.size(format, raw_size_discrete(width, height));
        }
    }
    node
}

/// Synthetic: a single-planar ISP with NV12 at `sizes`.
fn nv12_isp(sizes: &[(u32, u32)]) -> FakeVideoNode {
    let mut node = FakeVideoNode::new(ISP_CARD_NAME, V4L2_CAP_VIDEO_CAPTURE, 0);
    node = node.format(CAPTURE, fourcc(b"NV12"));
    for &(width, height) in sizes {
        node = node.size(fourcc(b"NV12"), raw_size_discrete(width, height));
    }
    node
}

fn isp(node: FakeVideoNode) -> Video<'static> {
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

/// A sensor sub-device: its `/dev` name, device number and node. The
/// `/sys/dev/char` link names it as the kernel does.
type Subdev<'a> = (&'a str, (u32, u32), Node<FakeSubdev>);

/// Scan a board with `/dev/media<index>` for each of `media` (beside the
/// decoys `/dev/media` and `/dev/media0x`), and `/dev/video<index>` with a
/// sysfs `name` for each of `video` (no video4linux class when empty). Paths
/// are reported as on a board: `/dev/media0`, not `/tmp/…/dev/media0`.
fn scan<'a>(
    media: impl IntoIterator<Item = Node<FakeMedia>>,
    video: impl IntoIterator<Item = Video<'a>>,
) -> Result<Vec<Record>, ProviderError> {
    scan_with(media, video, [])
}

/// `scan` with sensor sub-devices.
fn scan_with<'a>(
    media: impl IntoIterator<Item = Node<FakeMedia>>,
    video: impl IntoIterator<Item = Video<'a>>,
    subdevs: impl IntoIterator<Item = Subdev<'a>>,
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
    let strip = |text: String| text.replace(root.to_str().unwrap(), "");
    let result = provider.discover();
    let mut records = result.map_err(|e| ProviderError::new(e.code, strip(e.reason)))?;
    for record in &mut records {
        record.details = serde_json::from_str(&strip(record.details.to_string())).unwrap();
    }
    Ok(records)
}

fn real_board() -> Vec<Record> {
    // Opening the raw capture node would fail the ISP probe.
    let raw = ("raw-capture.1.0", Err(libc::EIO));
    scan(vec![real_media(&[IMX477])], vec![raw, isp(real_isp())]).unwrap()
}

/// The IMX477 camera's details with `video` as the video nodes.
fn imx477<'a>(video: impl IntoIterator<Item = Video<'a>>) -> Value {
    let records = scan([real_media(&[IMX477])], video).unwrap();
    records[0].details.clone()
}

/// The reason of an unavailable ISP, which leaves the camera without modes.
fn isp_failure<'a>(video: impl IntoIterator<Item = Video<'a>>) -> String {
    let details = imx477(video);
    assert_eq!(details["modes"], json!([]));
    assert_eq!(details["isp"]["state"], "unavailable");
    details["isp"]["reason"].as_str().unwrap().to_string()
}

fn ids(records: &[Record]) -> Vec<&str> {
    records.iter().map(|record| record.id.as_str()).collect()
}

fn mode(format: &str, (width, height): (u32, u32), (num, den): (u32, u32), source: &str) -> Value {
    json!({"format": format, "width": width, "height": height, "framerate_num": num,
           "framerate_den": den, "framerate_source": source, "isp_output": true})
}

fn nominal(size: (u32, u32)) -> Value {
    mode("NV12", size, (30, 1), "nominal")
}

/// Transcribed.
#[test]
fn real_imx477_devkit_board_produces_one_camera_with_isp_modes() {
    let records = real_board();
    assert_eq!(ids(&records), ["camera:imx477 5-001a"]);
    assert_eq!(records[0].kind, "camera");
    assert_eq!(records[0].provider, "daemon.camera.mipi");
    let formats = ["AR24", "NV12", "RGB3"];
    let modes = formats.map(|format| SIZES.map(|size| mode(format, size, (30, 1), "nominal")));
    assert_eq!(
        records[0].details,
        json!({
            "camera_name": "imx477 5-001a",
            "model": "imx477",
            "backend": "mipi",
            "connection": "mipi-csi2",
            "media_device": "/dev/media0",
            "bus_info": "platform:csi2video@1",
            "csi_receiver": "csidev-40c3000.csi",
            "availability": {"state": "unknown", "reason": AVAILABILITY_REASON},
            "modes": modes.concat(),
            "isp": {"state": "available", "device_path": "/dev/video1",
                    "device_paths": ["/dev/video1"]},
        })
    );
    assert_eq!(MipiProvider::new().subsystems(), ["media", "video4linux"]);
}

/// Transcribed, then classified with the rules Neat Core installs.
#[test]
fn real_imx477_modes_classified_by_core_rules() {
    let mut records = real_board();
    let directory = TempDir::new();
    let rules = directory.path().join("neat-core.json");
    fs::write(&rules, core_rules().to_string()).unwrap();
    let mut issues = Vec::new();
    let status = SupportStage::new(&rules).apply(&mut records, &mut issues);
    assert_eq!((&*status.state, issues.len()), ("applied", 0));
    let modes = records[0].details["modes"].as_array().unwrap();
    let label = |mode: &Value| format!("{} {}", mode["format"], mode["width"]);
    let supported = modes.iter().filter(|mode| mode["supported"] == true);
    let expected = SIZES.map(|(width, _)| format!("\"NV12\" {width}"));
    assert_eq!(supported.map(label).collect::<Vec<_>>(), expected);
    let rgb = modes.iter().find(|mode| mode["format"] == "RGB3").unwrap();
    assert!(rgb["reason"].as_str().unwrap().contains("NV12 output only"));
}

/// Transcribed (`media_ctl_no_sensor.txt`): no sensor, no camera, no ISP query;
/// likewise without `/dev`.
#[test]
fn media_device_without_sensor_yields_no_camera() {
    let node = real_isp();
    let calls = node.calls.clone();
    let records = scan([real_media(&[])], [isp(node)]).unwrap();
    assert!(records.is_empty() && calls.lock().unwrap().is_empty());
    let mut no_dev = MipiProvider::with_roots("/nonexistent", "/nonexistent");
    assert!(no_dev.discover().unwrap().is_empty());
}

/// Synthetic: uvcvideo also registers a MEDIA_ENT_F_CAM_SENSOR entity.
#[test]
fn non_sima_media_device_is_ignored() {
    let uvc = media("uvcvideo", "usb-xhci-hcd.0-1", &[(3, "Camera 1", SENSOR)]);
    let records = scan([uvc, real_media(&[IMX477])], [isp(real_isp())]);
    assert_eq!(ids(&records.unwrap()), ["camera:imx477 5-001a"]);
}

/// Synthetic: two sensors on one device (`econ-imx568-fpga 5-0042` is from
/// `cam_info_imx568.txt`) and one on another, past two devices that vanished
/// mid-scan.
#[test]
fn several_sensors_and_media_devices() {
    let first = real_media(&[IMX477, (40, "econ-imx568-fpga 5-0042", SENSOR)]);
    let second = [(9, "ov5647", SENSOR)];
    let second = media(SIMA_MEDIA_DRIVER, "platform:csi2video@2", &second);
    let media = [first, Err(libc::ENODEV), Err(libc::ENOENT), second];
    let records = scan(media, [isp(real_isp())]).unwrap();
    let field = |key| Vec::from_iter(records.iter().map(|r| r.details[key].clone()));
    let names = ["econ-imx568-fpga 5-0042", "imx477 5-001a", "ov5647"];
    assert_eq!(field("camera_name"), names);
    assert_eq!(field("model"), ["econ-imx568-fpga", "imx477", "ov5647"]);
    let media = ["/dev/media0", "/dev/media0", "/dev/media3"];
    assert_eq!(field("media_device"), media);
    let bus = |number| format!("platform:csi2video@{number}");
    assert_eq!(field("bus_info"), [bus(1), bus(1), bus(2)]);
    let modes = imx477([isp(real_isp())])["modes"].clone();
    assert_eq!(field("modes"), vec![modes; 3]);
}

/// Synthetic: a duplicated or empty sensor name cannot address one camera.
#[test]
fn unaddressable_sensor_names_fail_discovery() {
    let error = scan(vec![real_media(&[IMX477]); 2], []).unwrap_err();
    assert_eq!(error.code, CODE_DISCOVERY_FAILED);
    let both = "\"imx477 5-001a\" is reported by both /dev/media0 and /dev/media1";
    assert!(error.reason.contains(both), "{}", error.reason);
    let error = scan([real_media(&[(4, "", SENSOR)])], []).unwrap_err();
    assert_eq!(error.code, CODE_DISCOVERY_FAILED);
    assert!(error
        .reason
        .ends_with("/dev/media0 has an unnamed sensor entity 4"));
}

/// Synthetic: a missing or unreadable ISP keeps the camera, with no modes.
#[test]
fn missing_or_unreadable_isp_degrades_to_empty_modes() {
    let reason = isp_failure([]);
    assert!(reason.starts_with("could not read /sys/class/video4linux: "));
    let reason = isp_failure([("raw-capture.1.0", Ok(real_isp()))]);
    assert_eq!(reason, "no Modalix ISP output node was found");
    let reason = isp_failure([(ISP_SYSFS_NAME, Err(libc::EACCES))]);
    let denied = os_message(&io::Error::from_raw_os_error(libc::EACCES));
    assert_eq!(reason, format!("could not open /dev/video0: {denied}"));
    let reason = isp_failure([isp(real_isp().failing(VideoOp::EnumFmt, libc::EIO))]);
    assert!(reason.starts_with("VIDIOC_ENUM_FMT failed for /dev/video0: "));
}

/// Synthetic: several ISP nodes report the modes they share; a node with
/// another card or no discrete size is skipped.
#[test]
fn multiple_isp_nodes_report_the_modes_they_share() {
    let mut card = real_isp();
    card.capability.card = [0; 32];
    let second = nv12_isp(&[(3840, 2160), (2048, 1080), (1920, 1080)]);
    let details = imx477([real_isp(), second, card, nv12_isp(&[])].map(isp));
    let modes = json!([nominal((1920, 1080)), nominal((2048, 1080))]);
    assert_eq!(details["modes"], modes);
    let paths = ["/dev/video0", "/dev/video1"];
    let available = json!({"state": "available", "device_path": paths[0], "device_paths": paths});
    assert_eq!(details["isp"], available);
    let reason = isp_failure([real_isp(), nv12_isp(&[(640, 480)])].map(isp));
    assert_eq!(
        reason,
        "Modalix ISP output nodes reported no common discrete sizes"
    );
}

/// Synthetic: discrete intervals replace the nominal 30/1; a size without
/// them, a range-only size and a driver without the ioctl keep it; a zero
/// interval is malformed.
#[test]
fn isp_frame_intervals_replace_the_nominal_rate() {
    let nv12 = fourcc(b"NV12");
    let mut range = raw_interval_discrete(1, 60);
    range.kind = V4L2_FRMIVAL_TYPE_CONTINUOUS;
    let node = nv12_isp(&SIZES)
        .interval(nv12, 1920, 1080, raw_interval_discrete(1, 60))
        .interval(nv12, 1920, 1080, raw_interval_discrete(1001, 30000))
        .interval(nv12, 2432, 2048, range);
    let modes = |node| imx477([isp(node)])["modes"].clone();
    let rate = |rate| mode("NV12", (1920, 1080), rate, "isp");
    let mut expected = vec![rate((60, 1)), rate((30000, 1001))];
    expected.extend(SIZES[1..].iter().map(|&size| nominal(size)));
    assert_eq!(modes(node.clone()), json!(expected));
    let no_ioctl = node.failing(VideoOp::EnumFrameIntervals, libc::ENOTTY);
    assert_eq!(modes(no_ioctl), json!(SIZES.map(nominal)));
    let zero = nv12_isp(&SIZES).interval(nv12, 1920, 1080, raw_interval_discrete(0, 30));
    let reason = isp_failure([isp(zero)]);
    assert_eq!(
        reason,
        "ISP node /dev/video0 returned a malformed frame interval"
    );
}

/// Synthetic, plus the real open and ioctl path on a regular file (ENOTTY).
#[test]
fn media_device_failures_fail_the_scan() {
    for errno in [libc::EACCES, libc::EPERM] {
        let error = scan([Err(errno)], []).unwrap_err();
        let message = os_message(&io::Error::from_raw_os_error(errno));
        assert_eq!(error.code, CODE_PERMISSION_DENIED);
        assert_eq!(
            error.reason,
            format!("failed to open media device /dev/media0: {message}")
        );
    }
    let directory = TempDir::new();
    write_file(&directory.path().join("dev/media0"), "not a device");
    let mut provider = MipiProvider::with_roots("/nonexistent", directory.path().join("dev"));
    let error = provider.discover().unwrap_err();
    let reason = "failed to query media device information for";
    assert!(error.code == CODE_IO_OPEN && error.reason.starts_with(reason));
}

/// Synthetic: no entity or format list may exceed `MAX_ENUMERATION_ENTRIES`,
/// and one ISP node gets `MAX_DEVICE_ENUMERATIONS` queries in all.
#[test]
fn enumerations_are_bounded() {
    let entities = vec![(0, "vdma", SUBDEV); MAX_ENUMERATION_ENTRIES as usize + 1];
    let error = scan([real_media(&entities)], []).unwrap_err();
    assert_eq!(error.code, CODE_IO_OPEN);
    assert_eq!(
        error.reason,
        "media device /dev/media0 returned a malformed entity list"
    );

    let mut formats = nv12_isp(&[]);
    formats.formats = vec![(CAPTURE, 1); MAX_ENUMERATION_ENTRIES as usize + 1];
    let mut queries = nv12_isp(&[]);
    for format in [b"AR24", b"RGB3", b"GREY", b"YUYV"].map(fourcc) {
        queries = queries.format(CAPTURE, format);
        let sizes = (1..=900).map(|width| (format, raw_size_discrete(width, 480)));
        queries.sizes.extend(sizes);
    }
    let calls = queries.calls.clone();
    let budget = "enumeration (more than 4096 queries)";
    for (node, what) in [(formats, "format list"), (queries, budget)] {
        let reason = isp_failure([isp(node)]);
        assert_eq!(
            reason,
            format!("ISP node /dev/video0 returned a malformed {what}")
        );
    }
    let calls = calls.lock().unwrap();
    let enumerations = calls.iter().filter(|op| **op != VideoOp::QueryCap);
    assert_eq!(enumerations.count(), MAX_DEVICE_ENUMERATIONS as usize);
}

#[test]
fn listing_dev_maps_only_eacces_to_permission_denied() {
    let dev = Path::new("/dev");
    for (errno, code) in [
        (libc::EACCES, "io.permission_denied"),
        (libc::EPERM, "io.open"),
    ] {
        let error = io::Error::from_raw_os_error(errno);
        assert_eq!(listing_failure(dev, &error).code, code);
    }
}

/// The IMX477 board with its sensor sub-device answering `sensor`.
fn timed_imx477(sensor: Node<FakeSubdev>) -> Value {
    let subdev = ("v4l-subdev2", IMX477_DEVNODE, sensor);
    let records = scan_with([real_media(&[IMX477])], [isp(real_isp())], [subdev]);
    records.unwrap()[0].details.clone()
}

/// Transcribed graph and ISP, with the sensor values the DevKit reports:
/// the receiver, the timing, a 66.18 fps limit, and every ISP size offered at
/// the sensor's rates instead of the nominal 30/1.
#[test]
fn imx477_sensor_timing_sets_max_fps_and_rates() {
    let details = timed_imx477(Ok(imx477_subdev()));
    assert_eq!(details["csi_receiver"], "csidev-40c3000.csi");
    let timing = json!({"pixel_rate": 840_000_000_u64, "hblank_min": 9332,
                        "vblank_min": 48, "width": 1920, "height": 1080});
    assert_eq!(details["sensor_timing"], timing);
    assert_eq!(details["max_fps"], json!(66.18));
    let rates = [66, 60, 30, 25, 20, 15, 10, 5];
    let mut expected = Vec::new();
    for format in ["AR24", "NV12", "RGB3"] {
        for size in SIZES {
            let at = |rate| mode(format, size, (rate, 1), "sensor_timing");
            expected.extend(rates.map(at));
        }
    }
    assert_eq!(details["modes"], json!(expected));
}

/// Transcribed, then classified with Neat Core's rules: only NV12 at 30/1 is
/// supported; the other rates get the frame-rate reason.
#[test]
fn sensor_timing_rates_classified_by_core_rules() {
    let subdev = ("v4l-subdev2", IMX477_DEVNODE, Ok(imx477_subdev()));
    let mut records = scan_with([real_media(&[IMX477])], [isp(real_isp())], [subdev]).unwrap();
    let directory = TempDir::new();
    let rules = directory.path().join("neat-core.json");
    fs::write(&rules, core_rules().to_string()).unwrap();
    SupportStage::new(&rules).apply(&mut records, &mut Vec::new());
    let modes = records[0].details["modes"].as_array().unwrap();
    let supported: Vec<_> = (modes.iter())
        .filter(|mode| mode["supported"] == true)
        .map(|mode| (mode["format"].clone(), mode["framerate_num"].clone()))
        .collect();
    assert_eq!(
        json!(supported),
        json!([["NV12", 30], ["NV12", 30], ["NV12", 30]])
    );
    let fast = modes
        .iter()
        .find(|mode| mode["framerate_num"] == 66)
        .unwrap();
    assert_eq!(fast["supported"], false);
    let reason = fast["reason"].as_str().unwrap();
    assert!(
        reason.contains("NV12 output only"),
        "format is checked first: {reason}"
    );
    let nv12 = |mode: &&Value| mode["format"] == "NV12" && mode["framerate_num"] == 60;
    let reason = modes.iter().find(nv12).unwrap()["reason"].as_str().unwrap();
    assert!(reason.contains("30/1"), "{reason}");
}

/// Synthetic: a sensor without a readable sub-device, format or control
/// keeps its receiver and the nominal modes, and the scan succeeds.
#[test]
fn sensor_without_timing_omits_it_without_failing() {
    let without = |control| {
        let mut sensor = imx477_subdev();
        sensor.controls.retain(|(id, ..)| *id != control);
        sensor.values.retain(|(id, _)| *id != control);
        Ok(sensor)
    };
    let mut disabled = imx477_subdev();
    disabled.controls[1].2 = V4L2_CTRL_FLAG_DISABLED;
    let mut negative = imx477_subdev();
    negative.controls[0].1 = -1;
    let mut other_pad = imx477_subdev();
    other_pad.format = Some((1, 1920, 1080));
    let mut empty = imx477_subdev();
    empty.format = Some((0, 0, 0));
    let zero_rate = subdev(0, (9332, 48), (1920, 1080));
    let cases = [
        Err(libc::EACCES),
        without(V4L2_CID_HBLANK),
        without(V4L2_CID_VBLANK),
        without(V4L2_CID_PIXEL_RATE),
        Ok(disabled),
        Ok(negative),
        Ok(other_pad),
        Ok(empty),
        Ok(zero_rate),
    ];
    let nominal_modes = imx477([isp(real_isp())])["modes"].clone();
    for (case, sensor) in cases.into_iter().enumerate() {
        let details = timed_imx477(sensor);
        assert_eq!(details["csi_receiver"], "csidev-40c3000.csi", "case {case}");
        assert!(details.get("sensor_timing").is_none(), "case {case}");
        assert!(details.get("max_fps").is_none(), "case {case}");
        assert_eq!(details["modes"], nominal_modes, "case {case}");
    }
    // No `/sys/dev/char` entry for the sub-device: nothing is opened.
    assert!(imx477([isp(real_isp())]).get("sensor_timing").is_none());
    // The device number names a video node, not a sub-device: not opened.
    let video = ("video5", IMX477_DEVNODE, Ok(imx477_subdev()));
    let records = scan_with([real_media(&[IMX477])], [isp(real_isp())], [video]);
    assert!(records.unwrap()[0].details.get("sensor_timing").is_none());
}

/// Synthetic: a graph without links or interfaces (or one whose links do not
/// fit) has no receiver and no timing; a media API older than 4.19 has no pad
/// index, so the receiver is reported but the timing is not.
#[test]
fn graph_without_links_or_pad_index_degrades() {
    let bare = media(SIMA_MEDIA_DRIVER, "platform:csi2video@1", &[IMX477]);
    let subdev = ("v4l-subdev2", IMX477_DEVNODE, Ok(imx477_subdev()));
    let details = scan_with([bare], [isp(real_isp())], [subdev]).unwrap()[0]
        .details
        .clone();
    assert!(details.get("csi_receiver").is_none());
    assert!(details.get("sensor_timing").is_none());

    let mut crowded = real_media(&[IMX477]).unwrap();
    let filler = MediaV2Link::default();
    crowded
        .1
        .links
        .extend(vec![filler; MAX_ENUMERATION_ENTRIES as usize]);
    let subdev = ("v4l-subdev2", IMX477_DEVNODE, Ok(imx477_subdev()));
    let records = scan_with([Ok(crowded)], [isp(real_isp())], [subdev]).unwrap();
    assert_eq!(
        ids(&records),
        ["camera:imx477 5-001a"],
        "entities still read"
    );
    assert!(records[0].details.get("csi_receiver").is_none());

    let mut old = real_media(&[IMX477]).unwrap();
    old.0.versions[0] = (4 << 16) | (18 << 8);
    let subdev = ("v4l-subdev2", IMX477_DEVNODE, Ok(imx477_subdev()));
    let details = scan_with([Ok(old)], [isp(real_isp())], [subdev]).unwrap()[0]
        .details
        .clone();
    assert_eq!(details["csi_receiver"], "csidev-40c3000.csi");
    assert!(details.get("sensor_timing").is_none());
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
    graph = graph
        .link((40, 0), (18, 0), false)
        .link((40, 1), (17, 0), true);
    graph = graph.interface(40, MEDIA_INTF_T_V4L_SUBDEV, (81, 7));
    let mut ov_sensor = subdev(58_333_000, (1896, 4), (640, 480));
    ov_sensor.format = Some((1, 640, 480)); // The linked pad.
    let subdevs = [
        ("v4l-subdev2", IMX477_DEVNODE, Ok(imx477_subdev())),
        ("v4l-subdev7", (81, 7), Ok(ov_sensor)),
    ];
    let records = scan_with([Ok(graph)], [isp(real_isp())], subdevs).unwrap();
    assert_eq!(
        ids(&records),
        ["camera:imx477 5-001a", "camera:ov5647 4-0036"]
    );
    let field = |key| Vec::from_iter(records.iter().map(|r| r.details[key].clone()));
    assert_eq!(
        field("csi_receiver"),
        ["csidev-40c3000.csi", "csidev-40d0000.csi"]
    );
    // 58333000 / ((640 + 1896) * (480 + 4)) = 47.52
    assert_eq!(field("max_fps"), [json!(66.18), json!(47.52)]);
    let first_rate = |r: &Record| r.details["modes"][0]["framerate_num"].clone();
    assert_eq!(records.iter().map(first_rate).collect::<Vec<_>>(), [66, 48]);
}

/// Synthetic: rates the ISP reports are kept; only nominal sizes expand.
#[test]
fn isp_intervals_are_kept_when_timing_is_known() {
    let nv12 = fourcc(b"NV12");
    let node = nv12_isp(&SIZES[..2]).interval(nv12, 1920, 1080, raw_interval_discrete(1, 60));
    let subdev = ("v4l-subdev2", IMX477_DEVNODE, Ok(imx477_subdev()));
    let records = scan_with([real_media(&[IMX477])], [isp(node)], [subdev]).unwrap();
    let mut expected = vec![mode("NV12", SIZES[0], (60, 1), "isp")];
    let at = |rate| mode("NV12", SIZES[1], (rate, 1), "sensor_timing");
    expected.extend([66, 60, 30, 25, 20, 15, 10, 5].map(at));
    assert_eq!(records[0].details["modes"], json!(expected));
}

#[test]
fn sensor_rates_follow_insight_rule() {
    assert_eq!(sensor_rates(66.18), [66, 60, 30, 25, 20, 15, 10, 5]);
    assert_eq!(sensor_rates(30.0), [30, 25, 20, 15, 10, 5]);
    assert_eq!(sensor_rates(120.0), [120, 60, 30, 25, 20, 15, 10, 5]);
    assert_eq!(sensor_rates(29.6), [30, 25, 20, 15, 10, 5], "snaps up");
    assert_eq!(sensor_rates(29.4), [29, 25, 20, 15, 10, 5]);
    assert_eq!(sensor_rates(7.0), [7, 5]);
    assert_eq!(sensor_rates(0.2), [1], "at least 1");
    let timing = |width, height| SensorTiming {
        pixel_rate: 840_000_000,
        hblank_min: 9332,
        vblank_min: 48,
        width,
        height,
    };
    assert_eq!(timing(1920, 1080).max_fps(), 66.18);
    assert_eq!(timing(4056, 3040).max_fps(), 20.32);
}
