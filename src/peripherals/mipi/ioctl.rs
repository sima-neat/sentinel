//! The read-only media-controller and V4L2 ioctl surface used by MIPI camera
//! discovery.
//!
//! Only query ioctls are declared here: `MEDIA_IOC_DEVICE_INFO` and
//! `MEDIA_IOC_G_TOPOLOGY` on `/dev/media*`, and `VIDIOC_QUERYCAP`,
//! `VIDIOC_ENUM_FMT`, `VIDIOC_ENUM_FRAMESIZES` and `VIDIOC_ENUM_FRAMEINTERVALS`
//! on the ISP video node. Nothing in this module can set up media links, set a
//! subdevice or video format, request buffers, or start streaming.
//!
//! The structures mirror `<linux/media.h>` and `<linux/videodev2.h>` byte for
//! byte. The V4L2 declarations repeat those of the `v4l2` provider, whose ioctl
//! module is private to it.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// `MEDIA_ENT_F_CAM_SENSOR`.
pub const MEDIA_ENT_F_CAM_SENSOR: u32 = 0x0002_0001;
#[cfg(test)]
pub const MEDIA_ENT_F_IO_V4L: u32 = 0x0001_0001;
#[cfg(test)]
pub const MEDIA_ENT_F_V4L2_SUBDEV_UNKNOWN: u32 = 0x0002_0000;

pub const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
pub const V4L2_CAP_VIDEO_CAPTURE_MPLANE: u32 = 0x0000_1000;

pub const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
pub const V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE: u32 = 9;

pub const V4L2_FRMSIZE_TYPE_DISCRETE: u32 = 1;

pub const V4L2_FRMIVAL_TYPE_DISCRETE: u32 = 1;
#[cfg(test)]
pub const V4L2_FRMIVAL_TYPE_CONTINUOUS: u32 = 2;

/// `struct media_device_info`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MediaDeviceInfo {
    pub driver: [u8; 16],
    pub model: [u8; 32],
    pub serial: [u8; 40],
    pub bus_info: [u8; 32],
    pub media_version: u32,
    pub hw_revision: u32,
    pub driver_version: u32,
    pub reserved: [u32; 31],
}

impl Default for MediaDeviceInfo {
    fn default() -> Self {
        Self {
            driver: [0; 16],
            model: [0; 32],
            serial: [0; 40],
            bus_info: [0; 32],
            media_version: 0,
            hw_revision: 0,
            driver_version: 0,
            reserved: [0; 31],
        }
    }
}

/// `struct media_v2_topology`. Only the entity array is ever requested; the
/// interface, pad and link pointers stay null so the kernel skips them.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaV2Topology {
    pub topology_version: u64,
    pub num_entities: u32,
    pub reserved1: u32,
    pub ptr_entities: u64,
    pub num_interfaces: u32,
    pub reserved2: u32,
    pub ptr_interfaces: u64,
    pub num_pads: u32,
    pub reserved3: u32,
    pub ptr_pads: u64,
    pub num_links: u32,
    pub reserved4: u32,
    pub ptr_links: u64,
}

/// `struct media_v2_entity` (declared `packed` in the header; every member is
/// 4-byte aligned, so the `repr(C)` layout is identical).
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

/// `struct v4l2_capability`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Capability {
    pub driver: [u8; 16],
    pub card: [u8; 32],
    pub bus_info: [u8; 32],
    pub version: u32,
    pub capabilities: u32,
    pub device_caps: u32,
    pub reserved: [u32; 3],
}

/// `struct v4l2_fmtdesc`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FmtDesc {
    pub index: u32,
    pub buf_type: u32,
    pub flags: u32,
    pub description: [u8; 32],
    pub pixelformat: u32,
    pub mbus_code: u32,
    pub reserved: [u32; 3],
}

/// `struct v4l2_frmsizeenum`; `data` holds the `discrete` or `stepwise` union
/// member.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrmSizeEnum {
    pub index: u32,
    pub pixel_format: u32,
    pub kind: u32,
    pub data: [u32; 6],
    pub reserved: [u32; 2],
}

impl FrmSizeEnum {
    pub fn discrete_width(&self) -> u32 {
        self.data[0]
    }
    pub fn discrete_height(&self) -> u32 {
        self.data[1]
    }
}

/// `struct v4l2_frmivalenum`; `data` holds the `discrete` fraction or the
/// `stepwise` fractions.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrmIvalEnum {
    pub index: u32,
    pub pixel_format: u32,
    pub width: u32,
    pub height: u32,
    pub kind: u32,
    pub data: [u32; 6],
    pub reserved: [u32; 2],
}

impl FrmIvalEnum {
    pub fn discrete(&self) -> (u32, u32) {
        (self.data[0], self.data[1])
    }
}

const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

const fn ioc(direction: u32, kind: u8, number: u32, size: usize) -> u32 {
    (direction << 30) | ((size as u32) << 16) | ((kind as u32) << 8) | number
}

pub const MEDIA_IOC_DEVICE_INFO: u32 = ioc(
    IOC_READ | IOC_WRITE,
    b'|',
    0x00,
    std::mem::size_of::<MediaDeviceInfo>(),
);
pub const MEDIA_IOC_G_TOPOLOGY: u32 = ioc(
    IOC_READ | IOC_WRITE,
    b'|',
    0x04,
    std::mem::size_of::<MediaV2Topology>(),
);

pub const VIDIOC_QUERYCAP: u32 = ioc(IOC_READ, b'V', 0, std::mem::size_of::<Capability>());
pub const VIDIOC_ENUM_FMT: u32 = ioc(
    IOC_READ | IOC_WRITE,
    b'V',
    2,
    std::mem::size_of::<FmtDesc>(),
);
pub const VIDIOC_ENUM_FRAMESIZES: u32 = ioc(
    IOC_READ | IOC_WRITE,
    b'V',
    74,
    std::mem::size_of::<FrmSizeEnum>(),
);
pub const VIDIOC_ENUM_FRAMEINTERVALS: u32 = ioc(
    IOC_READ | IOC_WRITE,
    b'V',
    75,
    std::mem::size_of::<FrmIvalEnum>(),
);

/// Opens media and video nodes. The production implementation opens character
/// devices; tests substitute a fake that serves synthetic devices.
pub trait Backend: Send {
    fn open_media(&self, path: &Path) -> io::Result<Box<dyn MediaNode>>;
    fn open_video(&self, path: &Path) -> io::Result<Box<dyn VideoNode>>;
}

/// The query ioctls of one open `/dev/media*` node.
pub trait MediaNode {
    fn device_info(&mut self, value: &mut MediaDeviceInfo) -> io::Result<()>;
    /// `MEDIA_IOC_G_TOPOLOGY` for entities only. The kernel writes at most
    /// `entities.len()` entries (none when the slice is empty, which only
    /// counts) and sets `value.num_entities` to the total entity count.
    fn topology(
        &mut self,
        value: &mut MediaV2Topology,
        entities: &mut [MediaV2Entity],
    ) -> io::Result<()>;
}

/// The query ioctls of one open V4L2 video node. Every method fills its
/// argument in place and returns the raw OS error on failure (`EINVAL` ends an
/// enumeration).
pub trait VideoNode {
    fn query_capability(&mut self, value: &mut Capability) -> io::Result<()>;
    fn enum_format(&mut self, value: &mut FmtDesc) -> io::Result<()>;
    fn enum_frame_size(&mut self, value: &mut FrmSizeEnum) -> io::Result<()>;
    fn enum_frame_interval(&mut self, value: &mut FrmIvalEnum) -> io::Result<()>;
}

/// Character-device backend: `open(O_RDONLY | O_NONBLOCK | O_CLOEXEC)` and
/// query ioctls only.
pub struct SystemBackend;

fn open_read_only(path: &Path) -> io::Result<SystemNode> {
    // `read(true)` without write selects O_RDONLY; std always adds O_CLOEXEC,
    // which is repeated here so the contract is explicit.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    Ok(SystemNode { file })
}

impl Backend for SystemBackend {
    fn open_media(&self, path: &Path) -> io::Result<Box<dyn MediaNode>> {
        Ok(Box::new(open_read_only(path)?))
    }

    fn open_video(&self, path: &Path) -> io::Result<Box<dyn VideoNode>> {
        Ok(Box::new(open_read_only(path)?))
    }
}

struct SystemNode {
    file: File,
}

impl SystemNode {
    fn ioctl<T>(&mut self, request: u32, argument: &mut T) -> io::Result<()> {
        loop {
            // SAFETY: `request` encodes the size of `T`, `T` is a `repr(C)`
            // mirror of the kernel structure, and the descriptor is open for
            // the duration of the call. Any user pointer inside `T` is set up
            // by the caller to reference memory that outlives the call.
            let result = unsafe {
                libc::ioctl(
                    self.file.as_raw_fd(),
                    request as _,
                    argument as *mut T as *mut libc::c_void,
                )
            };
            if result >= 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINTR) {
                return Err(error);
            }
        }
    }
}

impl MediaNode for SystemNode {
    fn device_info(&mut self, value: &mut MediaDeviceInfo) -> io::Result<()> {
        self.ioctl(MEDIA_IOC_DEVICE_INFO, value)
    }

    fn topology(
        &mut self,
        value: &mut MediaV2Topology,
        entities: &mut [MediaV2Entity],
    ) -> io::Result<()> {
        // The kernel writes at most `num_entities` entries to `ptr_entities`
        // (and fails with ENOSPC if more exist), so tying both to the slice
        // keeps the write in bounds. All other arrays stay null.
        *value = MediaV2Topology {
            num_entities: u32::try_from(entities.len()).unwrap_or(u32::MAX),
            ptr_entities: if entities.is_empty() {
                0
            } else {
                entities.as_mut_ptr() as u64
            },
            ..MediaV2Topology::default()
        };
        self.ioctl(MEDIA_IOC_G_TOPOLOGY, value)
    }
}

impl VideoNode for SystemNode {
    fn query_capability(&mut self, value: &mut Capability) -> io::Result<()> {
        self.ioctl(VIDIOC_QUERYCAP, value)
    }
    fn enum_format(&mut self, value: &mut FmtDesc) -> io::Result<()> {
        self.ioctl(VIDIOC_ENUM_FMT, value)
    }
    fn enum_frame_size(&mut self, value: &mut FrmSizeEnum) -> io::Result<()> {
        self.ioctl(VIDIOC_ENUM_FRAMESIZES, value)
    }
    fn enum_frame_interval(&mut self, value: &mut FrmIvalEnum) -> io::Result<()> {
        self.ioctl(VIDIOC_ENUM_FRAMEINTERVALS, value)
    }
}
