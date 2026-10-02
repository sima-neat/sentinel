//! The read-only V4L2 ioctl surface used by camera discovery.
//!
//! Only query ioctls are declared here (`VIDIOC_QUERYCAP`, `VIDIOC_ENUM_FMT`,
//! `VIDIOC_ENUM_FRAMESIZES`, `VIDIOC_ENUM_FRAMEINTERVALS`). Nothing in this
//! module can set a format, request buffers, or start streaming, so discovery
//! never changes the state of a camera another process may be using.
//!
//! The structures mirror `<linux/videodev2.h>` byte for byte. Kernel unions are
//! represented as `[u32; 6]` payloads with typed accessors so that no `unsafe`
//! union reads are needed.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
#[cfg(test)]
pub const V4L2_CAP_VIDEO_OUTPUT: u32 = 0x0000_0002;
pub const V4L2_CAP_VIDEO_CAPTURE_MPLANE: u32 = 0x0000_1000;
pub const V4L2_CAP_VIDEO_M2M_MPLANE: u32 = 0x0000_4000;
pub const V4L2_CAP_VIDEO_M2M: u32 = 0x0000_8000;
#[cfg(test)]
pub const V4L2_CAP_META_CAPTURE: u32 = 0x0080_0000;
pub const V4L2_CAP_DEVICE_CAPS: u32 = 0x8000_0000;

pub const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
pub const V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE: u32 = 9;

pub const V4L2_FRMSIZE_TYPE_DISCRETE: u32 = 1;
pub const V4L2_FRMSIZE_TYPE_CONTINUOUS: u32 = 2;
pub const V4L2_FRMSIZE_TYPE_STEPWISE: u32 = 3;

pub const V4L2_FRMIVAL_TYPE_DISCRETE: u32 = 1;
pub const V4L2_FRMIVAL_TYPE_CONTINUOUS: u32 = 2;
pub const V4L2_FRMIVAL_TYPE_STEPWISE: u32 = 3;

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

/// `struct v4l2_frmsizeenum`. `data` holds the `discrete`
/// (`width`, `height`) or `stepwise` (`min_width`, `max_width`, `step_width`,
/// `min_height`, `max_height`, `step_height`) union member.
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
    pub fn min_width(&self) -> u32 {
        self.data[0]
    }
    pub fn max_width(&self) -> u32 {
        self.data[1]
    }
    pub fn step_width(&self) -> u32 {
        self.data[2]
    }
    pub fn min_height(&self) -> u32 {
        self.data[3]
    }
    pub fn max_height(&self) -> u32 {
        self.data[4]
    }
    pub fn step_height(&self) -> u32 {
        self.data[5]
    }
}

/// `struct v4l2_frmivalenum`. `data` holds the `discrete` fraction
/// (`numerator`, `denominator`) or the `stepwise` `min`, `max`, `step`
/// fractions.
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
    pub fn min(&self) -> (u32, u32) {
        (self.data[0], self.data[1])
    }
    pub fn max(&self) -> (u32, u32) {
        (self.data[2], self.data[3])
    }
    pub fn step(&self) -> (u32, u32) {
        (self.data[4], self.data[5])
    }
}

const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

const fn ioc(direction: u32, number: u32, size: usize) -> u32 {
    (direction << 30) | ((size as u32) << 16) | ((b'V' as u32) << 8) | number
}

pub const VIDIOC_QUERYCAP: u32 = ioc(IOC_READ, 0, std::mem::size_of::<Capability>());
pub const VIDIOC_ENUM_FMT: u32 = ioc(IOC_READ | IOC_WRITE, 2, std::mem::size_of::<FmtDesc>());
pub const VIDIOC_ENUM_FRAMESIZES: u32 =
    ioc(IOC_READ | IOC_WRITE, 74, std::mem::size_of::<FrmSizeEnum>());
pub const VIDIOC_ENUM_FRAMEINTERVALS: u32 =
    ioc(IOC_READ | IOC_WRITE, 75, std::mem::size_of::<FrmIvalEnum>());

/// Opens V4L2 nodes. The production implementation opens character devices;
/// tests substitute a fake that serves synthetic devices.
pub trait Backend: Send {
    fn open(&self, path: &Path) -> io::Result<Box<dyn Node>>;
}

/// The query ioctls of one open node. Every method fills its argument in place
/// and returns the raw OS error on failure (`EINVAL` ends an enumeration).
pub trait Node {
    fn query_capability(&mut self, value: &mut Capability) -> io::Result<()>;
    fn enum_format(&mut self, value: &mut FmtDesc) -> io::Result<()>;
    fn enum_frame_size(&mut self, value: &mut FrmSizeEnum) -> io::Result<()>;
    fn enum_frame_interval(&mut self, value: &mut FrmIvalEnum) -> io::Result<()>;
}

/// Character-device backend: `open(O_RDONLY | O_NONBLOCK | O_CLOEXEC)` and
/// query ioctls only.
pub struct SystemBackend;

impl Backend for SystemBackend {
    fn open(&self, path: &Path) -> io::Result<Box<dyn Node>> {
        // `read(true)` without write selects O_RDONLY; std always adds
        // O_CLOEXEC, which is repeated here so the contract is explicit.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        Ok(Box::new(SystemNode { file }))
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
            // the duration of the call.
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

impl Node for SystemNode {
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
