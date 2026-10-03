//! The read-only ioctls of MIPI discovery: `MEDIA_IOC_DEVICE_INFO` and
//! `MEDIA_IOC_G_TOPOLOGY` on media devices, and `VIDIOC_SUBDEV_G_FMT`,
//! `VIDIOC_QUERY_EXT_CTRL` and `VIDIOC_G_EXT_CTRLS` on the sensor sub-device.
//! Nothing here can set up links, set a format or control, or stream. The
//! structures mirror `<linux/media.h>`, `<linux/v4l2-subdev.h>` and
//! `<linux/videodev2.h>` byte for byte; the V4L2 node queries are the shared
//! surface in `videodev2`.

use std::io;
use std::mem::{offset_of, size_of};
use std::path::Path;

use crate::peripherals::videodev2::{
    ioc, open_read_only, SystemNode, VideoNode, IOC_READ, IOC_WRITE,
};

pub const MEDIA_ENT_F_CAM_SENSOR: u32 = 0x0002_0001;
pub const MEDIA_PAD_FL_SOURCE: u32 = 1 << 1;
pub const MEDIA_LNK_FL_ENABLED: u32 = 1 << 0;
pub const MEDIA_LNK_FL_LINK_TYPE: u32 = 0xf << 28;
pub const MEDIA_LNK_FL_DATA_LINK: u32 = 0;
pub const MEDIA_LNK_FL_INTERFACE_LINK: u32 = 1 << 28;
pub const MEDIA_INTF_T_V4L_SUBDEV: u32 = 0x0203;
/// `MEDIA_V2_PAD_HAS_INDEX`: `media_v2_pad.index` is valid from media API 4.19.
pub const MEDIA_V2_PAD_INDEX_VERSION: u32 = (4 << 16) | (19 << 8);

pub const V4L2_CID_VBLANK: u32 = 0x009e_0901;
pub const V4L2_CID_HBLANK: u32 = 0x009e_0902;
pub const V4L2_CID_PIXEL_RATE: u32 = 0x009f_0902;
pub const V4L2_CTRL_FLAG_DISABLED: u32 = 0x0001;
pub const V4L2_SUBDEV_FORMAT_ACTIVE: u32 = 1;

/// `struct media_device_info`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MediaDeviceInfo {
    pub driver: [u8; 16],
    pub model: [u8; 32],
    pub serial: [u8; 40],
    pub bus_info: [u8; 32],
    pub versions: [u32; 3],
    pub reserved: [u32; 31],
}

impl Default for MediaDeviceInfo {
    fn default() -> Self {
        Self {
            driver: [0; 16],
            model: [0; 32],
            serial: [0; 40],
            bus_info: [0; 32],
            versions: [0; 3],
            reserved: [0; 31],
        }
    }
}

/// `struct media_v2_topology` (declared `packed`; every member is naturally
/// aligned, so the `repr(C)` layout is identical). A null array pointer makes
/// the kernel skip that array.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct MediaV2Topology {
    topology_version: u64,
    num_entities: u32,
    reserved1: u32,
    ptr_entities: u64,
    num_interfaces: u32,
    reserved2: u32,
    ptr_interfaces: u64,
    num_pads: u32,
    reserved3: u32,
    ptr_pads: u64,
    num_links: u32,
    reserved4: u32,
    ptr_links: u64,
}

/// `struct media_v2_entity` (declared `packed`; every member is 4-byte
/// aligned, so the `repr(C)` layout is identical).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MediaV2Entity {
    pub id: u32,
    pub name: [u8; 64],
    pub function: u32,
    pub flags: u32,
    pub reserved: [u32; 5],
}

impl Default for MediaV2Entity {
    fn default() -> Self {
        Self {
            id: 0,
            name: [0; 64],
            function: 0,
            flags: 0,
            reserved: [0; 5],
        }
    }
}

/// `struct media_v2_interface`; `data` is the `devnode` / `raw` union.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaV2Interface {
    pub id: u32,
    pub intf_type: u32,
    pub flags: u32,
    pub reserved: [u32; 9],
    pub data: [u32; 16],
}

impl MediaV2Interface {
    /// `devnode.major`, `devnode.minor`.
    pub fn devnode(&self) -> (u32, u32) {
        (self.data[0], self.data[1])
    }
}

/// `struct media_v2_pad`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaV2Pad {
    pub id: u32,
    pub entity_id: u32,
    pub flags: u32,
    pub index: u32,
    pub reserved: [u32; 4],
}

/// `struct media_v2_link`. A data link joins two pad ids; an interface link
/// joins an interface id (`source_id`) to an entity id (`sink_id`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaV2Link {
    pub id: u32,
    pub source_id: u32,
    pub sink_id: u32,
    pub flags: u32,
    pub reserved: [u32; 6],
}

/// One media graph. Each array's length is the room offered to the kernel
/// (empty: not requested) and, after a read, the number of elements.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub entities: Vec<MediaV2Entity>,
    pub interfaces: Vec<MediaV2Interface>,
    pub pads: Vec<MediaV2Pad>,
    pub links: Vec<MediaV2Link>,
}

/// `struct v4l2_mbus_framefmt`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MbusFrameFmt {
    pub width: u32,
    pub height: u32,
    pub code: u32,
    pub field: u32,
    pub colorspace: u32,
    pub encoding: [u16; 4],
    pub reserved: [u16; 10],
}

/// `struct v4l2_subdev_format`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SubdevFormat {
    pub which: u32,
    pub pad: u32,
    pub format: MbusFrameFmt,
    pub stream: u32,
    pub reserved: [u32; 7],
}

/// `struct v4l2_query_ext_ctrl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct QueryExtCtrl {
    pub id: u32,
    pub kind: u32,
    pub name: [u8; 32],
    pub minimum: i64,
    pub maximum: i64,
    pub step: u64,
    pub default_value: i64,
    pub flags: u32,
    pub elem_size: u32,
    pub elems: u32,
    pub nr_of_dims: u32,
    pub dims: [u32; 4],
    pub reserved: [u32; 32],
}

/// `struct v4l2_ext_control` (declared `packed`); `value` is the `value64`
/// member of its union.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
struct ExtControl {
    id: u32,
    size: u32,
    reserved2: u32,
    value: i64,
}

/// `struct v4l2_ext_controls`; `which` 0 is `V4L2_CTRL_WHICH_CUR_VAL`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct ExtControls {
    which: u32,
    count: u32,
    error_idx: u32,
    request_fd: i32,
    reserved: u32,
    controls: u64,
}

const IOWR: u32 = IOC_READ | IOC_WRITE;
const MEDIA_IOC_DEVICE_INFO: u32 = ioc(IOWR, b'|', 0x00, size_of::<MediaDeviceInfo>());
const MEDIA_IOC_G_TOPOLOGY: u32 = ioc(IOWR, b'|', 0x04, size_of::<MediaV2Topology>());
const VIDIOC_SUBDEV_G_FMT: u32 = ioc(IOWR, b'V', 4, size_of::<SubdevFormat>());
const VIDIOC_G_EXT_CTRLS: u32 = ioc(IOWR, b'V', 71, size_of::<ExtControls>());
const VIDIOC_QUERY_EXT_CTRL: u32 = ioc(IOWR, b'V', 103, size_of::<QueryExtCtrl>());

// The ABI of <linux/media.h>, with ioctl numbers computed by a C program.
const _: () =
    assert!(size_of::<MediaDeviceInfo>() == 256 && offset_of!(MediaDeviceInfo, bus_info) == 88);
const _: () =
    assert!(size_of::<MediaV2Topology>() == 72 && offset_of!(MediaV2Topology, ptr_entities) == 16);
const _: () =
    assert!(size_of::<MediaV2Entity>() == 96 && offset_of!(MediaV2Entity, function) == 68);
const _: () = assert!(MEDIA_IOC_DEVICE_INFO == 0xc100_7c00 && MEDIA_IOC_G_TOPOLOGY == 0xc048_7c04);
const _: () = assert!(
    size_of::<MediaV2Interface>() == 112
        && offset_of!(MediaV2Interface, data) == 48
        && size_of::<MediaV2Pad>() == 32
        && size_of::<MediaV2Link>() == 40
);
const _: () = assert!(size_of::<SubdevFormat>() == 88 && size_of::<MbusFrameFmt>() == 48);
const _: () = assert!(size_of::<QueryExtCtrl>() == 232 && offset_of!(QueryExtCtrl, minimum) == 40);
const _: () = assert!(
    size_of::<ExtControls>() == 32
        && offset_of!(ExtControls, controls) == 24
        && size_of::<ExtControl>() == 20
        && offset_of!(ExtControl, value) == 12
);
const _: () = assert!(
    VIDIOC_SUBDEV_G_FMT == 0xc058_5604
        && VIDIOC_G_EXT_CTRLS == 0xc020_5647
        && VIDIOC_QUERY_EXT_CTRL == 0xc0e8_5667
);

/// Opens media, video and sub-device nodes; tests substitute synthetic devices.
pub trait Backend: Send {
    fn open_media(&self, path: &Path) -> io::Result<Box<dyn MediaNode>>;
    fn open_video(&self, path: &Path) -> io::Result<Box<dyn VideoNode>>;
    fn open_subdev(&self, path: &Path) -> io::Result<Box<dyn SubdevNode>>;
}

/// The query ioctls of one open `/dev/media*` node.
pub trait MediaNode {
    fn device_info(&mut self, value: &mut MediaDeviceInfo) -> io::Result<()>;
    /// Reads every non-empty array of `graph` in one `MEDIA_IOC_G_TOPOLOGY`
    /// call and truncates each to its count; `ENOSPC` when one does not fit.
    fn topology(&mut self, graph: &mut Graph) -> io::Result<()>;
}

/// The query ioctls of one open `/dev/v4l-subdev*` node.
pub trait SubdevNode {
    /// `VIDIOC_SUBDEV_G_FMT`.
    fn format(&mut self, value: &mut SubdevFormat) -> io::Result<()>;
    /// `VIDIOC_QUERY_EXT_CTRL` of `value.id`.
    fn query_control(&mut self, value: &mut QueryExtCtrl) -> io::Result<()>;
    /// The current value of one 64-bit control (`VIDIOC_G_EXT_CTRLS`).
    fn control_value(&mut self, id: u32) -> io::Result<i64>;
}

/// `open(O_RDONLY | O_NONBLOCK | O_CLOEXEC)` and query ioctls only.
pub struct SystemBackend;

impl Backend for SystemBackend {
    fn open_media(&self, path: &Path) -> io::Result<Box<dyn MediaNode>> {
        Ok(Box::new(open_read_only(path)?))
    }

    fn open_video(&self, path: &Path) -> io::Result<Box<dyn VideoNode>> {
        Ok(Box::new(open_read_only(path)?))
    }

    fn open_subdev(&self, path: &Path) -> io::Result<Box<dyn SubdevNode>> {
        Ok(Box::new(open_read_only(path)?))
    }
}

/// The array pointer and room offered for `items`: null when empty, so the
/// kernel skips it. Truncation could only understate the room, never overrun.
fn array<T>(items: &mut [T]) -> (u64, u32) {
    match items.is_empty() {
        true => (0, 0),
        false => (items.as_mut_ptr() as u64, items.len() as u32),
    }
}

impl MediaNode for SystemNode {
    fn device_info(&mut self, value: &mut MediaDeviceInfo) -> io::Result<()> {
        // SAFETY: MEDIA_IOC_DEVICE_INFO encodes `struct media_device_info`.
        unsafe { self.ioctl(MEDIA_IOC_DEVICE_INFO, value) }
    }

    fn topology(&mut self, graph: &mut Graph) -> io::Result<()> {
        let mut topology = MediaV2Topology::default();
        (topology.ptr_entities, topology.num_entities) = array(&mut graph.entities);
        (topology.ptr_interfaces, topology.num_interfaces) = array(&mut graph.interfaces);
        (topology.ptr_pads, topology.num_pads) = array(&mut graph.pads);
        (topology.ptr_links, topology.num_links) = array(&mut graph.links);
        let requested = topology;
        // SAFETY: MEDIA_IOC_G_TOPOLOGY encodes `struct media_v2_topology`; for
        // each non-null array the kernel writes at most `num_*` entries to
        // `ptr_*`, which points into the matching vector of `graph` for the
        // duration of the call; null arrays are skipped.
        unsafe { self.ioctl(MEDIA_IOC_G_TOPOLOGY, &mut topology)? };
        let count = |found: u32, room: u32| found.min(room) as usize;
        let entities = count(topology.num_entities, requested.num_entities);
        graph.entities.truncate(entities);
        let interfaces = count(topology.num_interfaces, requested.num_interfaces);
        graph.interfaces.truncate(interfaces);
        graph
            .pads
            .truncate(count(topology.num_pads, requested.num_pads));
        graph
            .links
            .truncate(count(topology.num_links, requested.num_links));
        Ok(())
    }
}

impl SubdevNode for SystemNode {
    fn format(&mut self, value: &mut SubdevFormat) -> io::Result<()> {
        // SAFETY: VIDIOC_SUBDEV_G_FMT encodes `struct v4l2_subdev_format`.
        unsafe { self.ioctl(VIDIOC_SUBDEV_G_FMT, value) }
    }

    fn query_control(&mut self, value: &mut QueryExtCtrl) -> io::Result<()> {
        // SAFETY: VIDIOC_QUERY_EXT_CTRL encodes `struct v4l2_query_ext_ctrl`.
        unsafe { self.ioctl(VIDIOC_QUERY_EXT_CTRL, value) }
    }

    fn control_value(&mut self, id: u32) -> io::Result<i64> {
        let mut control = ExtControl {
            id,
            ..ExtControl::default()
        };
        let mut controls = ExtControls {
            count: 1,
            controls: &mut control as *mut ExtControl as u64,
            ..ExtControls::default()
        };
        // SAFETY: VIDIOC_G_EXT_CTRLS encodes `struct v4l2_ext_controls`;
        // `controls` points to one `struct v4l2_ext_control` that outlives the
        // call, and a 64-bit control's value is written in place.
        unsafe { self.ioctl(VIDIOC_G_EXT_CTRLS, &mut controls)? };
        Ok(control.value)
    }
}
