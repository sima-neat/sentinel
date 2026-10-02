//! Tests for the MIPI camera provider.
//!
//! No camera hardware is needed: `/dev` and `/sys` are temporary directories
//! and media and video nodes are served by `FakeBackend`. Each test is
//! labelled:
//!
//! * **transcribed** — the device answers are transcribed from a real DevKit
//!   capture (`media-ctl -p` and `v4l2-ctl --list-formats-ext` on a Modalix
//!   DevKit running kernel 6.18.3, kept in Insight's
//!   `tests/fixtures/peripherals/`). Details the capture does not show (which
//!   `/dev/videoN` the ISP is, other sysfs names) are synthetic and say so.
//! * **synthetic** — format-faithful answers built to cover another member of
//!   the class (more sensors, more media devices, other drivers, failures).

use super::ioctl::*;
use super::*;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "sentinel-mipi-provider-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, AtomicOrdering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create fixture directory");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_file(path: &Path, value: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

fn copy_into(destination: &mut [u8], value: &str) {
    destination[..value.len()].copy_from_slice(value.as_bytes());
}

// ---------------------------------------------------------------------------
// Fake media node
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaOp {
    DeviceInfo,
    Topology,
}

#[derive(Debug, Clone, Default)]
struct FakeMedia {
    info: MediaDeviceInfo,
    entities: Vec<MediaV2Entity>,
    version: u64,
    fail: Option<(MediaOp, i32)>,
    /// Fill calls still to answer with ENOSPC (the graph grew).
    enospc: Arc<Mutex<u32>>,
    /// Fill calls still to answer with a newer topology version.
    races: Arc<Mutex<u32>>,
    calls: Arc<Mutex<Vec<MediaOp>>>,
}

impl FakeMedia {
    fn new(driver: &str, model: &str, bus_info: &str) -> Self {
        let mut info = MediaDeviceInfo::default();
        copy_into(&mut info.driver, driver);
        copy_into(&mut info.model, model);
        copy_into(&mut info.bus_info, bus_info);
        Self {
            info,
            version: 7,
            ..Self::default()
        }
    }

    fn entity(mut self, id: u32, name: &str, function: u32) -> Self {
        let mut entity = MediaV2Entity {
            id,
            function,
            ..MediaV2Entity::default()
        };
        copy_into(&mut entity.name, name);
        self.entities.push(entity);
        self
    }

    fn failing(mut self, op: MediaOp, errno: i32) -> Self {
        self.fail = Some((op, errno));
        self
    }

    fn check(&self, op: MediaOp) -> io::Result<()> {
        self.calls.lock().unwrap().push(op);
        match self.fail {
            Some((failing, errno)) if failing == op => Err(io::Error::from_raw_os_error(errno)),
            _ => Ok(()),
        }
    }
}

fn take_one(counter: &Mutex<u32>) -> bool {
    let mut remaining = counter.lock().unwrap();
    if *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    true
}

impl MediaNode for FakeMedia {
    fn device_info(&mut self, value: &mut MediaDeviceInfo) -> io::Result<()> {
        self.check(MediaOp::DeviceInfo)?;
        *value = self.info;
        Ok(())
    }

    fn topology(
        &mut self,
        value: &mut MediaV2Topology,
        entities: &mut [MediaV2Entity],
    ) -> io::Result<()> {
        self.check(MediaOp::Topology)?;
        *value = MediaV2Topology {
            topology_version: self.version,
            num_entities: self.entities.len() as u32,
            ..MediaV2Topology::default()
        };
        if entities.is_empty() {
            return Ok(());
        }
        if take_one(&self.enospc) || entities.len() < self.entities.len() {
            return Err(io::Error::from_raw_os_error(libc::ENOSPC));
        }
        if take_one(&self.races) {
            value.topology_version += 1;
        }
        entities[..self.entities.len()].copy_from_slice(&self.entities);
        Ok(())
    }
}

/// `media_device_info` of the SiMa capture media device (transcribed).
fn sima_media(bus_info: &str) -> FakeMedia {
    FakeMedia::new(SIMA_MEDIA_DRIVER, "SiMa.ai Capture Media Device", bus_info)
}

/// The non-sensor part of the real graph: entities 1, 5 and 16 of
/// `media_ctl_imx477_real.txt` (transcribed). `media-ctl` prints
/// `type Node subtype V4L` for MEDIA_ENT_F_IO_V4L and
/// `type V4L2 subdev subtype Unknown` for MEDIA_ENT_F_V4L2_SUBDEV_UNKNOWN.
fn real_capture_path(media: FakeMedia) -> FakeMedia {
    media
        .entity(1, "raw-capture.1.0", MEDIA_ENT_F_IO_V4L)
        .entity(5, "vdma.1", MEDIA_ENT_F_V4L2_SUBDEV_UNKNOWN)
        .entity(16, "csidev-40c3000.csi", MEDIA_ENT_F_V4L2_SUBDEV_UNKNOWN)
}

/// `media_ctl_imx477_real.txt` (transcribed): one SiMa media device whose
/// only sensor (`subtype Sensor`) is entity 33, `imx477 5-001a`.
fn real_imx477_media() -> FakeMedia {
    real_capture_path(sima_media("platform:csi2video@1")).entity(
        33,
        "imx477 5-001a",
        MEDIA_ENT_F_CAM_SENSOR,
    )
}

// ---------------------------------------------------------------------------
// Fake video node
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VideoOp {
    QueryCap,
    EnumFmt,
    EnumFrameSizes,
    EnumFrameIntervals,
}

#[derive(Debug, Clone, Default)]
struct FakeVideo {
    capability: Capability,
    /// `(buffer type, pixel format)` in driver order.
    formats: Vec<(u32, u32)>,
    /// `(pixel format, frame size)` in driver order.
    sizes: Vec<(u32, FrmSizeEnum)>,
    /// `((pixel format, width, height), interval)` in driver order.
    intervals: Vec<((u32, u32, u32), FrmIvalEnum)>,
    fail: Option<(VideoOp, i32)>,
}

fn einval() -> io::Error {
    io::Error::from_raw_os_error(libc::EINVAL)
}

impl FakeVideo {
    fn new(card: &str, capabilities: u32, device_caps: u32) -> Self {
        let mut capability = Capability {
            capabilities,
            device_caps,
            ..Capability::default()
        };
        copy_into(&mut capability.card, card);
        Self {
            capability,
            ..Self::default()
        }
    }

    fn format(mut self, buffer_type: u32, pixel_format: u32) -> Self {
        self.formats.push((buffer_type, pixel_format));
        self
    }

    fn size(mut self, pixel_format: u32, width: u32, height: u32) -> Self {
        self.sizes.push((
            pixel_format,
            FrmSizeEnum {
                kind: V4L2_FRMSIZE_TYPE_DISCRETE,
                data: [width, height, 0, 0, 0, 0],
                ..FrmSizeEnum::default()
            },
        ));
        self
    }

    fn interval(mut self, pixel_format: u32, width: u32, height: u32, value: FrmIvalEnum) -> Self {
        self.intervals.push(((pixel_format, width, height), value));
        self
    }

    fn failing(mut self, op: VideoOp, errno: i32) -> Self {
        self.fail = Some((op, errno));
        self
    }

    fn check(&self, op: VideoOp) -> io::Result<()> {
        match self.fail {
            Some((failing, errno)) if failing == op => Err(io::Error::from_raw_os_error(errno)),
            _ => Ok(()),
        }
    }
}

impl VideoNode for FakeVideo {
    fn query_capability(&mut self, value: &mut Capability) -> io::Result<()> {
        self.check(VideoOp::QueryCap)?;
        *value = self.capability;
        Ok(())
    }

    fn enum_format(&mut self, value: &mut FmtDesc) -> io::Result<()> {
        self.check(VideoOp::EnumFmt)?;
        let found = self
            .formats
            .iter()
            .filter(|(buffer_type, _)| *buffer_type == value.buf_type)
            .nth(value.index as usize)
            .ok_or_else(einval)?;
        value.pixelformat = found.1;
        Ok(())
    }

    fn enum_frame_size(&mut self, value: &mut FrmSizeEnum) -> io::Result<()> {
        self.check(VideoOp::EnumFrameSizes)?;
        let found = self
            .sizes
            .iter()
            .filter(|(pixel_format, _)| *pixel_format == value.pixel_format)
            .nth(value.index as usize)
            .ok_or_else(einval)?;
        value.kind = found.1.kind;
        value.data = found.1.data;
        Ok(())
    }

    fn enum_frame_interval(&mut self, value: &mut FrmIvalEnum) -> io::Result<()> {
        self.check(VideoOp::EnumFrameIntervals)?;
        let key = (value.pixel_format, value.width, value.height);
        let found = self
            .intervals
            .iter()
            .filter(|(candidate, _)| *candidate == key)
            .nth(value.index as usize)
            .ok_or_else(einval)?;
        value.kind = found.1.kind;
        value.data = found.1.data;
        Ok(())
    }
}

fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}

const MPLANE: u32 = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
const REAL_ISP_SIZES: [(u32, u32); 3] = [(1920, 1080), (2048, 1080), (2432, 2048)];

/// `v4l2_isp_out_real.txt` (transcribed): card `arm-isp-out`, capabilities
/// 0x85201000 / device caps 0x05201000 (multiplanar capture), formats AR24,
/// RGB3 and NV12 in that order, each listing every discrete size eight times,
/// and no frame intervals.
fn real_isp_node() -> FakeVideo {
    let mut node = FakeVideo::new(ISP_CARD_NAME, 0x8520_1000, 0x0520_1000);
    copy_into(&mut node.capability.driver, "arm-camera-isp");
    copy_into(&mut node.capability.bus_info, "platform: isp-v4l2-0-0-1");
    for code in [b"AR24", b"RGB3", b"NV12"] {
        let format = fourcc(code);
        node = node.format(MPLANE, format);
        for (width, height) in REAL_ISP_SIZES {
            for _ in 0..8 {
                node = node.size(format, width, height);
            }
        }
    }
    node
}

fn discrete_interval(numerator: u32, denominator: u32) -> FrmIvalEnum {
    FrmIvalEnum {
        kind: V4L2_FRMIVAL_TYPE_DISCRETE,
        data: [numerator, denominator, 0, 0, 0, 0],
        ..FrmIvalEnum::default()
    }
}

// ---------------------------------------------------------------------------
// Fake backend and board fixture
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct FakeBackend {
    media: HashMap<PathBuf, Result<FakeMedia, i32>>,
    video: HashMap<PathBuf, Result<FakeVideo, i32>>,
    opened: Arc<Mutex<Vec<PathBuf>>>,
}

fn open_fake<T: Clone>(
    nodes: &HashMap<PathBuf, Result<T, i32>>,
    opened: &Mutex<Vec<PathBuf>>,
    path: &Path,
) -> io::Result<T> {
    opened.lock().unwrap().push(path.to_path_buf());
    match nodes.get(path) {
        Some(Ok(node)) => Ok(node.clone()),
        Some(Err(errno)) => Err(io::Error::from_raw_os_error(*errno)),
        None => Err(io::Error::from_raw_os_error(libc::ENOENT)),
    }
}

impl Backend for FakeBackend {
    fn open_media(&self, path: &Path) -> io::Result<Box<dyn MediaNode>> {
        Ok(Box::new(open_fake(&self.media, &self.opened, path)?))
    }

    fn open_video(&self, path: &Path) -> io::Result<Box<dyn VideoNode>> {
        Ok(Box::new(open_fake(&self.video, &self.opened, path)?))
    }
}

/// A temporary `/sys` + `/dev` pair and the fake nodes behind it.
struct Board {
    _directory: TempDir,
    sys: PathBuf,
    dev: PathBuf,
    backend: FakeBackend,
}

impl Board {
    fn new() -> Self {
        let directory = TempDir::new();
        let sys = directory.0.join("sys");
        let dev = directory.0.join("dev");
        fs::create_dir_all(sys.join("class/video4linux")).unwrap();
        fs::create_dir_all(&dev).unwrap();
        Self {
            _directory: directory,
            sys,
            dev,
            backend: FakeBackend::default(),
        }
    }

    fn dev_path(&self, name: &str) -> PathBuf {
        self.dev.join(name)
    }

    fn dev_string(&self, name: &str) -> String {
        self.dev_path(name).to_string_lossy().into_owned()
    }

    fn media(mut self, name: &str, media: FakeMedia) -> Self {
        write_file(&self.dev_path(name), "");
        self.backend.media.insert(self.dev_path(name), Ok(media));
        self
    }

    fn media_error(mut self, name: &str, errno: i32) -> Self {
        write_file(&self.dev_path(name), "");
        self.backend.media.insert(self.dev_path(name), Err(errno));
        self
    }

    /// A `/sys/class/video4linux/<entry>` with the given sysfs `name`, plus its
    /// `/dev` node served by `node` (or failing to open with `errno`).
    fn video(mut self, entry: &str, sysfs_name: &str, node: Result<FakeVideo, i32>) -> Self {
        write_file(
            &self.sys.join("class/video4linux").join(entry).join("name"),
            &format!("{sysfs_name}\n"),
        );
        write_file(&self.dev_path(entry), "");
        self.backend.video.insert(self.dev_path(entry), node);
        self
    }

    fn isp(self, entry: &str, node: FakeVideo) -> Self {
        self.video(entry, ISP_SYSFS_NAME, Ok(node))
    }

    fn discover(&self) -> Result<Vec<Record>, ProviderError> {
        MipiProvider::with_backend(&self.sys, &self.dev, Box::new(self.backend.clone())).discover()
    }

    fn opened(&self) -> Vec<PathBuf> {
        self.backend.opened.lock().unwrap().clone()
    }
}

fn ids(records: &[Record]) -> Vec<&str> {
    records.iter().map(|record| record.id.as_str()).collect()
}

fn modes_of(record: &Record) -> Vec<(String, u64, u64, u64, u64, String)> {
    record.details["modes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|mode| {
            (
                mode["format"].as_str().unwrap().to_string(),
                mode["width"].as_u64().unwrap(),
                mode["height"].as_u64().unwrap(),
                mode["framerate_num"].as_u64().unwrap(),
                mode["framerate_den"].as_u64().unwrap(),
                mode["framerate_source"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn nominal(format: &str, width: u64, height: u64) -> (String, u64, u64, u64, u64, String) {
    (format.into(), width, height, 30, 1, "nominal".into())
}

/// The nine modes of the real ISP (format, then width, then height).
fn real_isp_modes() -> Vec<(String, u64, u64, u64, u64, String)> {
    let mut modes = Vec::new();
    for format in ["AR24", "NV12", "RGB3"] {
        for (width, height) in REAL_ISP_SIZES {
            modes.push(nominal(format, width.into(), height.into()));
        }
    }
    modes
}

// ---------------------------------------------------------------------------
// Real board
// ---------------------------------------------------------------------------

/// Transcribed: `media_ctl_imx477_real.txt` + `v4l2_isp_out_real.txt`.
/// Synthetic: the ISP's placement at `video1`, and `video0`'s sysfs name.
#[test]
fn real_imx477_devkit_board_produces_one_camera_with_isp_modes() {
    let board = Board::new()
        .media("media0", real_imx477_media())
        .video(
            "video0",
            "raw-capture.1.0",
            Ok(FakeVideo::new("raw-capture", 0, 0)),
        )
        .isp("video1", real_isp_node());
    let records = board.discover().unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.id, "camera:imx477 5-001a");
    assert_eq!(record.kind, "camera");
    assert_eq!(record.provider, "daemon.camera.mipi");

    let mode = |format: &str, width: u32, height: u32| {
        json!({
            "format": format, "width": width, "height": height,
            "framerate_num": 30, "framerate_den": 1,
            "framerate_source": "nominal", "isp_output": true,
        })
    };
    let mut modes = Vec::new();
    for format in ["AR24", "NV12", "RGB3"] {
        for (width, height) in REAL_ISP_SIZES {
            modes.push(mode(format, width, height));
        }
    }
    assert_eq!(
        record.details,
        json!({
            "camera_name": "imx477 5-001a",
            "model": "imx477",
            "backend": "mipi",
            "connection": "mipi-csi2",
            "media_device": board.dev_string("media0"),
            "bus_info": "platform:csi2video@1",
            "availability": {"state": "unknown", "reason": AVAILABILITY_REASON},
            "modes": modes,
            "isp": {
                "state": "available",
                "device_path": board.dev_string("video1"),
                "device_paths": [board.dev_string("video1")],
            },
        })
    );
    // The raw capture node is never opened: only the media device and the
    // node whose sysfs name is the ISP output name.
    assert_eq!(
        board.opened(),
        vec![board.dev_path("media0"), board.dev_path("video1")]
    );
}

/// Transcribed: `media_ctl_no_sensor.txt` (a SiMa media device with no sensor
/// bound, which libcamera locks as a zombie).
#[test]
fn media_device_without_sensor_yields_no_camera_and_skips_the_isp() {
    let board = Board::new()
        .media(
            "media0",
            real_capture_path(sima_media("platform:csi2video@1")),
        )
        .isp("video1", real_isp_node());
    assert!(board.discover().unwrap().is_empty());
    assert_eq!(board.opened(), vec![board.dev_path("media0")]);
}

// ---------------------------------------------------------------------------
// Class coverage
// ---------------------------------------------------------------------------

/// Synthetic graph; both names are transcribed from real captures
/// (`imx477 5-001a`, and `econ-imx568-fpga 5-0042` from `cam_info_imx568.txt`).
#[test]
fn two_sensors_on_one_media_device() {
    let media = real_imx477_media().entity(40, "econ-imx568-fpga 5-0042", MEDIA_ENT_F_CAM_SENSOR);
    let board = Board::new()
        .media("media0", media)
        .isp("video1", real_isp_node());
    let records = board.discover().unwrap();
    assert_eq!(
        ids(&records),
        ["camera:econ-imx568-fpga 5-0042", "camera:imx477 5-001a"]
    );
    assert_eq!(records[0].details["model"], "econ-imx568-fpga");
    assert_eq!(records[1].details["model"], "imx477");
    for record in &records {
        assert_eq!(record.details["media_device"], board.dev_string("media0"));
        assert_eq!(modes_of(record), real_isp_modes());
    }
}

/// Synthetic: two CSI-2 media devices, one sensor each.
#[test]
fn two_sima_media_devices() {
    let board = Board::new()
        .media("media0", real_imx477_media())
        .media(
            "media1",
            sima_media("platform:csi2video@2").entity(9, "imx477 6-001a", MEDIA_ENT_F_CAM_SENSOR),
        )
        .isp("video1", real_isp_node());
    let records = board.discover().unwrap();
    assert_eq!(
        ids(&records),
        ["camera:imx477 5-001a", "camera:imx477 6-001a"]
    );
    assert_eq!(
        records[0].details["media_device"],
        board.dev_string("media0")
    );
    assert_eq!(records[0].details["bus_info"], "platform:csi2video@1");
    assert_eq!(
        records[1].details["media_device"],
        board.dev_string("media1")
    );
    assert_eq!(records[1].details["bus_info"], "platform:csi2video@2");
}

/// Synthetic: uvcvideo registers its camera input terminal as
/// MEDIA_ENT_F_CAM_SENSOR too; the device must be ignored on its driver name
/// without reading its topology.
#[test]
fn non_sima_media_device_is_ignored() {
    let uvc = FakeMedia::new("uvcvideo", "HD Pro Webcam C920", "usb-xhci-hcd.0-1")
        .entity(1, "HD Pro Webcam C920", MEDIA_ENT_F_IO_V4L)
        .entity(3, "Camera 1", MEDIA_ENT_F_CAM_SENSOR);
    let uvc_calls = uvc.calls.clone();
    let board = Board::new()
        .media("media0", uvc)
        .media("media1", real_imx477_media())
        .isp("video2", real_isp_node());
    let records = board.discover().unwrap();
    assert_eq!(ids(&records), ["camera:imx477 5-001a"]);
    assert_eq!(*uvc_calls.lock().unwrap(), [MediaOp::DeviceInfo]);

    let only_uvc = Board::new().media(
        "media0",
        FakeMedia::new("uvcvideo", "C920", "usb-1").entity(3, "Camera 1", MEDIA_ENT_F_CAM_SENSOR),
    );
    assert!(only_uvc.discover().unwrap().is_empty());
}

/// Synthetic: a sensor entity whose name has no space.
#[test]
fn entity_name_without_space_uses_the_whole_name_as_model() {
    let board = Board::new()
        .media(
            "media0",
            sima_media("platform:csi2video@1").entity(2, "ov5647", MEDIA_ENT_F_CAM_SENSOR),
        )
        .isp("video1", real_isp_node());
    let records = board.discover().unwrap();
    assert_eq!(records[0].id, "camera:ov5647");
    assert_eq!(records[0].details["camera_name"], "ov5647");
    assert_eq!(records[0].details["model"], "ov5647");
}

/// Synthetic: a sensor without an ISP output node keeps its record with no
/// modes.
#[test]
fn missing_isp_node_degrades_to_empty_modes() {
    // No ISP-named node at all.
    let board = Board::new().media("media0", real_imx477_media()).video(
        "video0",
        "raw-capture.1.0",
        Ok(FakeVideo::new("raw-capture", 0, 0)),
    );
    let records = board.discover().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].details["modes"], json!([]));
    assert_eq!(
        records[0].details["isp"],
        json!({"state": "unavailable", "reason": "no Modalix ISP output node was found"})
    );
    assert_eq!(board.opened(), vec![board.dev_path("media0")]);

    // No video4linux class directory.
    let board = Board::new().media("media0", real_imx477_media());
    fs::remove_dir(board.sys.join("class/video4linux")).unwrap();
    let records = board.discover().unwrap();
    assert_eq!(records[0].details["modes"], json!([]));
    assert_eq!(records[0].details["isp"]["state"], "unavailable");
    let reason = records[0].details["isp"]["reason"].as_str().unwrap();
    assert!(reason.starts_with("could not read "), "{reason}");
    assert!(!reason.contains("os error"), "{reason}");
}

/// Synthetic: every way the ISP node can fail to answer degrades the records
/// instead of failing the scan, permission denied included.
#[test]
fn unreadable_isp_node_degrades_to_empty_modes() {
    let eio = os_message(&io::Error::from_raw_os_error(libc::EIO));
    let cases: Vec<(Result<FakeVideo, i32>, String)> = vec![
        (
            Err(libc::EACCES),
            format!(
                "could not open {{}}: {}",
                os_message(&io::Error::from_raw_os_error(libc::EACCES))
            ),
        ),
        (
            Ok(real_isp_node().failing(VideoOp::QueryCap, libc::EIO)),
            format!("VIDIOC_QUERYCAP failed for {{}}: {eio}"),
        ),
        (
            Ok(real_isp_node().failing(VideoOp::EnumFmt, libc::EIO)),
            format!("VIDIOC_ENUM_FMT failed for {{}}: {eio}"),
        ),
        (
            Ok(real_isp_node().failing(VideoOp::EnumFrameSizes, libc::EIO)),
            format!("VIDIOC_ENUM_FRAMESIZES failed for {{}}: {eio}"),
        ),
        (
            Ok(real_isp_node().failing(VideoOp::EnumFrameIntervals, libc::EIO)),
            format!("VIDIOC_ENUM_FRAMEINTERVALS failed for {{}}: {eio}"),
        ),
    ];
    for (node, reason) in cases {
        let board =
            Board::new()
                .media("media0", real_imx477_media())
                .video("video1", ISP_SYSFS_NAME, node);
        let records = board.discover().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].details["modes"], json!([]));
        assert_eq!(
            records[0].details["isp"],
            json!({
                "state": "unavailable",
                "reason": reason.replace("{}", &board.dev_string("video1")),
            })
        );
    }
}

/// Synthetic: discrete ISP frame intervals replace the nominal 30/1; a size
/// without intervals, a range-only size, and a driver without the ioctl keep
/// the nominal rate.
#[test]
fn isp_frame_intervals_replace_the_nominal_rate() {
    let nv12 = fourcc(b"NV12");
    let node = FakeVideo::new(ISP_CARD_NAME, 0, V4L2_CAP_VIDEO_CAPTURE_MPLANE)
        .format(MPLANE, nv12)
        .size(nv12, 1920, 1080)
        .size(nv12, 2048, 1080)
        .size(nv12, 2432, 2048)
        .interval(nv12, 1920, 1080, discrete_interval(1, 60))
        .interval(nv12, 1920, 1080, discrete_interval(1001, 30000))
        .interval(nv12, 1920, 1080, discrete_interval(1, 30))
        .interval(
            nv12,
            2432,
            2048,
            FrmIvalEnum {
                kind: V4L2_FRMIVAL_TYPE_CONTINUOUS,
                data: [1, 60, 1, 1, 1, 1],
                ..FrmIvalEnum::default()
            },
        );
    let board = Board::new()
        .media("media0", real_imx477_media())
        .isp("video1", node.clone());
    let records = board.discover().unwrap();
    let isp = |format: &str, width, height, num, den| {
        (
            format.to_string(),
            width,
            height,
            num,
            den,
            "isp".to_string(),
        )
    };
    assert_eq!(
        modes_of(&records[0]),
        [
            isp("NV12", 1920, 1080, 30, 1),
            isp("NV12", 1920, 1080, 60, 1),
            isp("NV12", 1920, 1080, 30000, 1001),
            nominal("NV12", 2048, 1080),
            nominal("NV12", 2432, 2048),
        ]
    );

    let board = Board::new().media("media0", real_imx477_media()).isp(
        "video1",
        node.failing(VideoOp::EnumFrameIntervals, libc::ENOTTY),
    );
    let records = board.discover().unwrap();
    assert_eq!(
        modes_of(&records[0]),
        [
            nominal("NV12", 1920, 1080),
            nominal("NV12", 2048, 1080),
            nominal("NV12", 2432, 2048),
        ]
    );

    let malformed = FakeVideo::new(ISP_CARD_NAME, 0, V4L2_CAP_VIDEO_CAPTURE_MPLANE)
        .format(MPLANE, nv12)
        .size(nv12, 1920, 1080)
        .interval(nv12, 1920, 1080, discrete_interval(0, 30));
    let board = Board::new()
        .media("media0", real_imx477_media())
        .isp("video1", malformed);
    let records = board.discover().unwrap();
    assert_eq!(
        records[0].details["isp"]["reason"],
        format!(
            "ISP node {} returned a malformed frame interval",
            board.dev_string("video1")
        )
    );
}

/// Synthetic: the C++ `probe_isp_sizes` behaviour. Every node whose sysfs name
/// is the ISP output name is probed in sorted order; one with another card or
/// no discrete size is skipped; the rest contribute only the modes they all
/// share; any failing node makes the ISP unavailable.
#[test]
fn multiple_isp_nodes_report_the_modes_they_share() {
    let nv12 = fourcc(b"NV12");
    let second = FakeVideo::new(ISP_CARD_NAME, 0, V4L2_CAP_VIDEO_CAPTURE_MPLANE)
        .format(MPLANE, nv12)
        .size(nv12, 3840, 2160)
        .size(nv12, 2048, 1080)
        .size(nv12, 1920, 1080);
    let mut wrong_card = real_isp_node();
    wrong_card.capability.card = [0; 32];
    copy_into(&mut wrong_card.capability.card, "arm-isp-raw");
    let no_formats = FakeVideo::new(ISP_CARD_NAME, 0, V4L2_CAP_VIDEO_CAPTURE_MPLANE);

    let board = Board::new()
        .media("media0", real_imx477_media())
        .isp("video1", real_isp_node())
        .isp("video2", second.clone())
        .isp("video3", wrong_card)
        .isp("video4", no_formats);
    let records = board.discover().unwrap();
    assert_eq!(
        modes_of(&records[0]),
        [nominal("NV12", 1920, 1080), nominal("NV12", 2048, 1080)]
    );
    assert_eq!(
        records[0].details["isp"],
        json!({
            "state": "available",
            "device_path": board.dev_string("video1"),
            "device_paths": [board.dev_string("video1"), board.dev_string("video2")],
        })
    );
    // All four ISP-named nodes were opened; the rejected ones contribute nothing.
    assert_eq!(board.opened().len(), 5);

    let disjoint = FakeVideo::new(ISP_CARD_NAME, 0, V4L2_CAP_VIDEO_CAPTURE_MPLANE)
        .format(MPLANE, nv12)
        .size(nv12, 640, 480);
    let board = Board::new()
        .media("media0", real_imx477_media())
        .isp("video1", real_isp_node())
        .isp("video2", disjoint);
    let records = board.discover().unwrap();
    assert_eq!(records[0].details["modes"], json!([]));
    assert_eq!(
        records[0].details["isp"]["reason"],
        "Modalix ISP output nodes reported no common discrete sizes"
    );

    let board = Board::new()
        .media("media0", real_imx477_media())
        .isp("video1", real_isp_node())
        .isp("video2", second.failing(VideoOp::EnumFmt, libc::EIO));
    let records = board.discover().unwrap();
    assert_eq!(records[0].details["modes"], json!([]));
    assert!(records[0].details["isp"]["reason"]
        .as_str()
        .unwrap()
        .starts_with("VIDIOC_ENUM_FMT failed for"));
}

/// Synthetic: a single-planar ISP uses the single-planar buffer type (and
/// `capabilities` when `device_caps` is zero, as the C++ does); fourccs are
/// printed as the v4l2 provider prints them, without NV12M -> NV12 mapping.
#[test]
fn single_planar_isp_and_fourcc_spelling() {
    let nm12 = fourcc(b"NM12");
    let unprintable = 0x0000_0001;
    let node = FakeVideo::new(ISP_CARD_NAME, V4L2_CAP_VIDEO_CAPTURE, 0)
        .format(V4L2_BUF_TYPE_VIDEO_CAPTURE, nm12)
        .format(V4L2_BUF_TYPE_VIDEO_CAPTURE, unprintable)
        .format(MPLANE, fourcc(b"NV12"))
        .size(nm12, 1920, 1080)
        .size(unprintable, 640, 480);
    let board = Board::new()
        .media("media0", real_imx477_media())
        .isp("video1", node);
    let records = board.discover().unwrap();
    assert_eq!(
        modes_of(&records[0]),
        [nominal("0x00000001", 640, 480), nominal("NM12", 1920, 1080)]
    );
}

/// Synthetic: renumbering `/dev/mediaN` (another media device probing first)
/// changes only the routing metadata.
#[test]
fn media_node_renumbering_keeps_ids() {
    let first = Board::new()
        .media("media0", real_imx477_media())
        .isp("video1", real_isp_node());
    let second = Board::new()
        .media("media0", FakeMedia::new("uvcvideo", "C920", "usb-1"))
        .media("media3", real_imx477_media())
        .isp("video1", real_isp_node());
    let before = first.discover().unwrap();
    let after = second.discover().unwrap();
    assert_eq!(ids(&before), ids(&after));
    assert_eq!(
        after[0].details["media_device"],
        second.dev_string("media3")
    );
    let without_media = |record: &Record| {
        let mut details = record.details.clone();
        details.as_object_mut().unwrap().remove("media_device");
        details["isp"].as_object_mut().unwrap().clear();
        details
    };
    assert_eq!(without_media(&before[0]), without_media(&after[0]));
}

/// Synthetic: permission denied on a media device fails the scan with the
/// permission code, whether at open or at the first ioctl.
#[test]
fn permission_denied_on_media_device_fails_with_permission_code() {
    for errno in [libc::EACCES, libc::EPERM] {
        let board = Board::new().media_error("media0", errno);
        let error = board.discover().unwrap_err();
        assert_eq!(error.code, "io.permission_denied");
        assert_eq!(
            error.reason,
            format!(
                "failed to open media device {}: {}",
                board.dev_string("media0"),
                os_message(&io::Error::from_raw_os_error(errno))
            )
        );
    }
    let board = Board::new().media(
        "media0",
        real_imx477_media().failing(MediaOp::DeviceInfo, libc::EACCES),
    );
    assert_eq!(board.discover().unwrap_err().code, "io.permission_denied");
}

/// Synthetic: other media failures fail with `io.open`; a node that vanished
/// mid-scan is skipped.
#[test]
fn media_errors_fail_with_io_open_and_vanished_devices_are_skipped() {
    let eio = os_message(&io::Error::from_raw_os_error(libc::EIO));
    for (media, action) in [
        (
            real_imx477_media().failing(MediaOp::DeviceInfo, libc::EIO),
            "failed to query media device information for",
        ),
        (
            real_imx477_media().failing(MediaOp::Topology, libc::EIO),
            "failed to read the media topology of",
        ),
    ] {
        let board = Board::new().media("media0", media);
        let error = board.discover().unwrap_err();
        assert_eq!(error.code, "io.open");
        assert_eq!(
            error.reason,
            format!("{action} {}: {eio}", board.dev_string("media0"))
        );
    }

    let board = Board::new()
        .media_error("media0", libc::ENODEV)
        .media(
            "media1",
            real_imx477_media().failing(MediaOp::Topology, libc::ENXIO),
        )
        .media(
            "media2",
            sima_media("platform:csi2video@2").entity(9, "imx477 6-001a", MEDIA_ENT_F_CAM_SENSOR),
        )
        .isp("video1", real_isp_node());
    // media3 is listed in /dev but has no backing node (unplugged): ENOENT.
    write_file(&board.dev_path("media3"), "");
    assert_eq!(ids(&board.discover().unwrap()), ["camera:imx477 6-001a"]);
}

/// Synthetic: two sensors with one name would make the camera name ambiguous.
#[test]
fn duplicate_sensor_names_fail_discovery() {
    let board = Board::new()
        .media("media0", real_imx477_media())
        .media("media1", real_imx477_media())
        .isp("video1", real_isp_node());
    let error = board.discover().unwrap_err();
    assert_eq!(error.code, "peripherals.discovery_failed");
    assert!(error.reason.contains("imx477 5-001a"), "{}", error.reason);
    assert!(
        error.reason.contains(&board.dev_string("media1")),
        "{}",
        error.reason
    );
}

/// Synthetic: a sensor entity without a name cannot be addressed.
#[test]
fn unnamed_sensor_entity_fails_discovery() {
    let board = Board::new().media(
        "media0",
        sima_media("platform:csi2video@1").entity(4, "", MEDIA_ENT_F_CAM_SENSOR),
    );
    let error = board.discover().unwrap_err();
    assert_eq!(error.code, "peripherals.discovery_failed");
    assert!(error.reason.contains("unnamed sensor entity 4"));
}

/// Synthetic: the topology is re-read when it changes between the count and
/// fill calls, and a graph that never settles fails the scan.
#[test]
fn topology_changes_during_discovery_are_retried() {
    let media = real_imx477_media();
    *media.enospc.lock().unwrap() = 1;
    *media.races.lock().unwrap() = 1;
    let board = Board::new()
        .media("media0", media)
        .isp("video1", real_isp_node());
    assert_eq!(ids(&board.discover().unwrap()), ["camera:imx477 5-001a"]);

    let media = real_imx477_media();
    *media.races.lock().unwrap() = u32::MAX;
    let board = Board::new().media("media0", media);
    let error = board.discover().unwrap_err();
    assert_eq!(error.code, "io.open");
    assert!(
        error
            .reason
            .ends_with("kept changing its topology during discovery"),
        "{}",
        error.reason
    );
}

/// Synthetic: no enumeration may exceed `MAX_ENUMERATION_ENTRIES`.
#[test]
fn enumerations_are_capped() {
    let mut media = sima_media("platform:csi2video@1");
    for id in 0..=MAX_ENUMERATION_ENTRIES {
        media = media.entity(id, "vdma", MEDIA_ENT_F_V4L2_SUBDEV_UNKNOWN);
    }
    let board = Board::new().media("media0", media);
    let error = board.discover().unwrap_err();
    assert_eq!(error.code, "io.open");
    assert_eq!(
        error.reason,
        format!(
            "media device {} returned a malformed entity list",
            board.dev_string("media0")
        )
    );

    let nv12 = fourcc(b"NV12");
    let base = || FakeVideo::new(ISP_CARD_NAME, 0, V4L2_CAP_VIDEO_CAPTURE_MPLANE);
    let mut endless_formats = base();
    let mut endless_sizes = base().format(MPLANE, nv12);
    let mut endless_intervals = base().format(MPLANE, nv12).size(nv12, 1920, 1080);
    for _ in 0..=MAX_ENUMERATION_ENTRIES {
        endless_formats = endless_formats.format(MPLANE, nv12);
        endless_sizes = endless_sizes.size(nv12, 1920, 1080);
        endless_intervals = endless_intervals.interval(nv12, 1920, 1080, discrete_interval(1, 30));
    }
    for (node, what) in [
        (endless_formats, "format list"),
        (endless_sizes, "frame size list"),
        (endless_intervals, "frame interval list"),
    ] {
        let board = Board::new()
            .media("media0", real_imx477_media())
            .isp("video1", node);
        let records = board.discover().unwrap();
        assert_eq!(
            records[0].details["isp"]["reason"],
            format!(
                "ISP node {} returned a malformed {what}",
                board.dev_string("video1")
            )
        );
    }
}

/// Synthetic: no `/dev`, or a `/dev` without `mediaN` nodes, is not an error.
#[test]
fn no_media_devices_is_an_empty_result() {
    let directory = TempDir::new();
    let missing = MipiProvider::with_roots(directory.0.join("no-sys"), directory.0.join("no-dev"))
        .discover()
        .unwrap();
    assert!(missing.is_empty());

    let board = Board::new().isp("video1", real_isp_node());
    for name in ["media", "mediafoo", "media0x", "v4l-subdev0"] {
        write_file(&board.dev_path(name), "");
    }
    assert!(board.discover().unwrap().is_empty());
    assert!(board.opened().is_empty());
}

#[test]
fn provider_metadata() {
    let provider = MipiProvider::with_roots("/nonexistent/sys", "/nonexistent/dev");
    assert_eq!(provider.name(), "daemon.camera.mipi");
    assert_eq!(
        provider.subsystems(),
        ["media".to_string(), "video4linux".to_string()]
    );
}

#[test]
fn ioctl_abi_matches_linux_media_h() {
    // Values computed from <linux/media.h> and <linux/videodev2.h> with a C
    // program (generic _IOC encoding, identical on aarch64 and x86_64).
    assert_eq!(std::mem::size_of::<MediaDeviceInfo>(), 256);
    assert_eq!(std::mem::size_of::<MediaV2Topology>(), 72);
    assert_eq!(std::mem::size_of::<MediaV2Entity>(), 96);
    assert_eq!(std::mem::offset_of!(MediaDeviceInfo, bus_info), 88);
    assert_eq!(std::mem::offset_of!(MediaV2Topology, num_entities), 8);
    assert_eq!(std::mem::offset_of!(MediaV2Topology, ptr_entities), 16);
    assert_eq!(std::mem::offset_of!(MediaV2Entity, name), 4);
    assert_eq!(std::mem::offset_of!(MediaV2Entity, function), 68);
    assert_eq!(MEDIA_IOC_DEVICE_INFO, 0xc100_7c00);
    assert_eq!(MEDIA_IOC_G_TOPOLOGY, 0xc048_7c04);
    assert_eq!(MEDIA_ENT_F_CAM_SENSOR, 0x0002_0001);
    assert_eq!(MEDIA_ENT_F_IO_V4L, 0x0001_0001);
    assert_eq!(MEDIA_ENT_F_V4L2_SUBDEV_UNKNOWN, 0x0002_0000);
    assert_eq!(std::mem::size_of::<Capability>(), 104);
    assert_eq!(std::mem::size_of::<FmtDesc>(), 64);
    assert_eq!(std::mem::size_of::<FrmSizeEnum>(), 44);
    assert_eq!(std::mem::size_of::<FrmIvalEnum>(), 52);
    assert_eq!(VIDIOC_QUERYCAP, 0x8068_5600);
    assert_eq!(VIDIOC_ENUM_FMT, 0xc040_5602);
    assert_eq!(VIDIOC_ENUM_FRAMESIZES, 0xc02c_564a);
    assert_eq!(VIDIOC_ENUM_FRAMEINTERVALS, 0xc034_564b);
}

#[test]
fn system_backend_reports_non_media_file_as_unreadable() {
    // Exercises the real open/ioctl path: a regular file opens read-only but
    // rejects MEDIA_IOC_DEVICE_INFO with ENOTTY.
    let directory = TempDir::new();
    let dev = directory.0.join("dev");
    write_file(&dev.join("media0"), "not a device");
    let error = MipiProvider::with_roots(directory.0.join("sys"), &dev)
        .discover()
        .unwrap_err();
    assert_eq!(error.code, "io.open");
    assert_eq!(
        error.reason,
        format!(
            "failed to query media device information for {}: {}",
            dev.join("media0").display(),
            os_message(&io::Error::from_raw_os_error(libc::ENOTTY))
        )
    );
}

#[test]
fn real_imx477_modes_classified_by_core_rules() {
    // Transcribed from a real DevKit capture, then classified the way the
    // peripherals thread does with the rules Neat Core installs.
    let board = Board::new()
        .media("media0", real_imx477_media())
        .video(
            "video0",
            "raw-capture.1.0",
            Ok(FakeVideo::new("raw-capture", 0, 0)),
        )
        .isp("video1", real_isp_node());
    let mut records = board.discover().unwrap();
    let dir = TempDir::new();
    let rules = dir.0.join("neat-core.json");
    fs::write(
        &rules,
        crate::peripherals::support::core_rules().to_string(),
    )
    .unwrap();
    let mut issues = Vec::new();
    let status =
        crate::peripherals::support::SupportStage::new(&rules).apply(&mut records, &mut issues);
    assert_eq!(status.state, "applied");
    assert!(issues.is_empty());
    let supported: Vec<String> = records[0].details["modes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|mode| mode["supported"] == true)
        .map(|mode| {
            format!(
                "{} {}x{}",
                mode["format"].as_str().unwrap(),
                mode["width"],
                mode["height"]
            )
        })
        .collect();
    assert_eq!(
        supported,
        ["NV12 1920x1080", "NV12 2048x1080", "NV12 2432x2048"],
        "only NV12 at the ISP output sizes"
    );
    let rgb = &records[0].details["modes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|mode| mode["format"] == "RGB3")
        .unwrap()["reason"];
    assert!(rgb.as_str().unwrap().contains("NV12 output only"));
}
