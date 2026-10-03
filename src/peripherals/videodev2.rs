//! The read-only V4L2 ioctl surface shared by the camera providers.
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

pub(crate) const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
#[cfg(test)]
pub(crate) const V4L2_CAP_VIDEO_OUTPUT: u32 = 0x0000_0002;
pub(crate) const V4L2_CAP_VIDEO_CAPTURE_MPLANE: u32 = 0x0000_1000;
pub(crate) const V4L2_CAP_VIDEO_M2M_MPLANE: u32 = 0x0000_4000;
pub(crate) const V4L2_CAP_VIDEO_M2M: u32 = 0x0000_8000;
#[cfg(test)]
pub(crate) const V4L2_CAP_META_CAPTURE: u32 = 0x0080_0000;
pub(crate) const V4L2_CAP_DEVICE_CAPS: u32 = 0x8000_0000;

pub(crate) const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
pub(crate) const V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE: u32 = 9;

pub(crate) const V4L2_FRMSIZE_TYPE_DISCRETE: u32 = 1;
pub(crate) const V4L2_FRMSIZE_TYPE_CONTINUOUS: u32 = 2;
pub(crate) const V4L2_FRMSIZE_TYPE_STEPWISE: u32 = 3;

pub(crate) const V4L2_FRMIVAL_TYPE_DISCRETE: u32 = 1;
pub(crate) const V4L2_FRMIVAL_TYPE_CONTINUOUS: u32 = 2;
pub(crate) const V4L2_FRMIVAL_TYPE_STEPWISE: u32 = 3;

/// A driver that never ends an enumeration with EINVAL would otherwise keep
/// the in-process peripherals thread busy forever.
pub(crate) const MAX_ENUMERATION_ENTRIES: u32 = 1024;

/// Enumeration ioctls one device may answer in one scan. The per-list cap
/// alone still lets formats x sizes x intervals reach about 10^9 queries on a
/// broken or hostile device, stalling the peripherals thread. A large UVC
/// camera (a few formats, tens of sizes, a handful of rates each) needs on the
/// order of 1000 queries and the Modalix ISP about 100. Every reported format,
/// size, mode and frame interval costs at least one query, so the budget also
/// bounds what one device can add to the catalog.
pub(crate) const MAX_DEVICE_ENUMERATIONS: u32 = 4096;

/// The enumeration ioctls left for one device.
pub(crate) struct EnumerationBudget {
    remaining: u32,
}

impl EnumerationBudget {
    pub(crate) fn new() -> Self {
        Self {
            remaining: MAX_DEVICE_ENUMERATIONS,
        }
    }

    /// Account for one enumeration ioctl. Once the budget is spent, the error
    /// names what the device returned too much of, for the provider's
    /// malformed-result message.
    pub(crate) fn spend(&mut self) -> Result<(), String> {
        if self.remaining == 0 {
            return Err(format!(
                "enumeration (more than {MAX_DEVICE_ENUMERATIONS} queries)"
            ));
        }
        self.remaining -= 1;
        Ok(())
    }
}

/// `struct v4l2_capability`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Capability {
    pub driver: [u8; 16],
    pub card: [u8; 32],
    pub bus_info: [u8; 32],
    pub version: u32,
    pub capabilities: u32,
    pub device_caps: u32,
    pub reserved: [u32; 3],
}
const _: () = assert!(std::mem::size_of::<Capability>() == 104);

/// `struct v4l2_fmtdesc`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FmtDesc {
    pub index: u32,
    pub buf_type: u32,
    pub flags: u32,
    pub description: [u8; 32],
    pub pixelformat: u32,
    pub mbus_code: u32,
    pub reserved: [u32; 3],
}
const _: () = assert!(std::mem::size_of::<FmtDesc>() == 64);

/// `struct v4l2_frmsizeenum`. `data` holds the `discrete`
/// (`width`, `height`) or `stepwise` (`min_width`, `max_width`, `step_width`,
/// `min_height`, `max_height`, `step_height`) union member.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FrmSizeEnum {
    pub index: u32,
    pub pixel_format: u32,
    pub kind: u32,
    pub data: [u32; 6],
    pub reserved: [u32; 2],
}
const _: () = assert!(std::mem::size_of::<FrmSizeEnum>() == 44);

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
pub(crate) struct FrmIvalEnum {
    pub index: u32,
    pub pixel_format: u32,
    pub width: u32,
    pub height: u32,
    pub kind: u32,
    pub data: [u32; 6],
    pub reserved: [u32; 2],
}
const _: () = assert!(std::mem::size_of::<FrmIvalEnum>() == 52);

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

pub(crate) const IOC_WRITE: u32 = 1;
pub(crate) const IOC_READ: u32 = 2;

/// The generic Linux `_IOC` encoding (identical on aarch64 and x86_64).
pub(crate) const fn ioc(direction: u32, kind: u8, number: u32, size: usize) -> u32 {
    (direction << 30) | ((size as u32) << 16) | ((kind as u32) << 8) | number
}

pub(crate) const VIDIOC_QUERYCAP: u32 = ioc(IOC_READ, b'V', 0, std::mem::size_of::<Capability>());
pub(crate) const VIDIOC_ENUM_FMT: u32 = ioc(
    IOC_READ | IOC_WRITE,
    b'V',
    2,
    std::mem::size_of::<FmtDesc>(),
);
pub(crate) const VIDIOC_ENUM_FRAMESIZES: u32 = ioc(
    IOC_READ | IOC_WRITE,
    b'V',
    74,
    std::mem::size_of::<FrmSizeEnum>(),
);
pub(crate) const VIDIOC_ENUM_FRAMEINTERVALS: u32 = ioc(
    IOC_READ | IOC_WRITE,
    b'V',
    75,
    std::mem::size_of::<FrmIvalEnum>(),
);

/// The capabilities of the opened node: `device_caps` when the driver sets
/// `V4L2_CAP_DEVICE_CAPS` in `capabilities` (as `<linux/videodev2.h>`
/// documents), otherwise the aggregate `capabilities` of the physical device.
pub(crate) fn effective_capabilities(capability: &Capability) -> u32 {
    if capability.capabilities & V4L2_CAP_DEVICE_CAPS != 0 {
        capability.device_caps
    } else {
        capability.capabilities
    }
}

/// The fourcc as four printable characters, or `0x%08x` otherwise.
pub(crate) fn fourcc_string(value: u32) -> String {
    let bytes = value.to_le_bytes();
    if bytes.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
        bytes.iter().map(|&byte| char::from(byte)).collect()
    } else {
        format!("0x{value:08x}")
    }
}

/// The query ioctls of one open V4L2 video node. Every method fills its
/// argument in place and returns the raw OS error on failure (`EINVAL` ends an
/// enumeration).
pub(crate) trait VideoNode {
    fn query_capability(&mut self, value: &mut Capability) -> io::Result<()>;
    fn enum_format(&mut self, value: &mut FmtDesc) -> io::Result<()>;
    fn enum_frame_size(&mut self, value: &mut FrmSizeEnum) -> io::Result<()>;
    fn enum_frame_interval(&mut self, value: &mut FrmIvalEnum) -> io::Result<()>;
}

/// `open(O_RDONLY | O_NONBLOCK | O_CLOEXEC)` of a character device.
pub(crate) fn open_read_only(path: &Path) -> io::Result<SystemNode> {
    // `read(true)` without write selects O_RDONLY; std always adds O_CLOEXEC,
    // which is repeated here so the contract is explicit.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    Ok(SystemNode { file })
}

/// An open character device that only ever receives query ioctls.
pub(crate) struct SystemNode {
    file: File,
}

impl SystemNode {
    /// Issue `request` with `argument`, retrying on `EINTR`.
    ///
    /// # Safety
    ///
    /// `request` must encode the size of `T`, `T` must be a `repr(C)` mirror
    /// of the kernel structure that `request` reads and writes, and any user
    /// pointer inside `argument` must reference memory that is valid for the
    /// kernel to access as that structure describes for the duration of the
    /// call.
    pub(crate) unsafe fn ioctl<T>(&mut self, request: u32, argument: &mut T) -> io::Result<()> {
        loop {
            // SAFETY: the caller upholds the request/argument contract above,
            // and the descriptor is open for the duration of the call.
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

impl VideoNode for SystemNode {
    fn query_capability(&mut self, value: &mut Capability) -> io::Result<()> {
        // SAFETY: VIDIOC_QUERYCAP encodes `struct v4l2_capability`.
        unsafe { self.ioctl(VIDIOC_QUERYCAP, value) }
    }
    fn enum_format(&mut self, value: &mut FmtDesc) -> io::Result<()> {
        // SAFETY: VIDIOC_ENUM_FMT encodes `struct v4l2_fmtdesc`.
        unsafe { self.ioctl(VIDIOC_ENUM_FMT, value) }
    }
    fn enum_frame_size(&mut self, value: &mut FrmSizeEnum) -> io::Result<()> {
        // SAFETY: VIDIOC_ENUM_FRAMESIZES encodes `struct v4l2_frmsizeenum`.
        unsafe { self.ioctl(VIDIOC_ENUM_FRAMESIZES, value) }
    }
    fn enum_frame_interval(&mut self, value: &mut FrmIvalEnum) -> io::Result<()> {
        // SAFETY: VIDIOC_ENUM_FRAMEINTERVALS encodes `struct v4l2_frmivalenum`.
        unsafe { self.ioctl(VIDIOC_ENUM_FRAMEINTERVALS, value) }
    }
}

/// A fake video node shared by the provider tests.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum VideoOp {
        QueryCap,
        EnumFmt,
        EnumFrameSizes,
        EnumFrameIntervals,
    }

    /// Serves synthetic answers and records every ioctl it receives.
    #[derive(Debug, Clone, Default)]
    pub(crate) struct FakeVideoNode {
        pub capability: Capability,
        /// `(buffer type, pixel format)` in driver order.
        pub formats: Vec<(u32, u32)>,
        /// `(pixel format, frame size)` in driver order.
        pub sizes: Vec<(u32, FrmSizeEnum)>,
        /// `((pixel format, width, height), interval)` in driver order.
        pub intervals: Vec<((u32, u32, u32), FrmIvalEnum)>,
        pub fail: Option<(VideoOp, i32)>,
        pub calls: Arc<Mutex<Vec<VideoOp>>>,
    }

    impl FakeVideoNode {
        pub(crate) fn new(card: &str, capabilities: u32, device_caps: u32) -> Self {
            let mut capability = Capability {
                capabilities,
                device_caps,
                ..Capability::default()
            };
            capability.card[..card.len()].copy_from_slice(card.as_bytes());
            Self {
                capability,
                ..Self::default()
            }
        }

        pub(crate) fn format(mut self, buffer_type: u32, pixel_format: u32) -> Self {
            self.formats.push((buffer_type, pixel_format));
            self
        }

        pub(crate) fn size(mut self, pixel_format: u32, size: FrmSizeEnum) -> Self {
            self.sizes.push((pixel_format, size));
            self
        }

        pub(crate) fn interval(
            mut self,
            pixel_format: u32,
            width: u32,
            height: u32,
            value: FrmIvalEnum,
        ) -> Self {
            self.intervals.push(((pixel_format, width, height), value));
            self
        }

        pub(crate) fn failing(mut self, op: VideoOp, errno: i32) -> Self {
            self.fail = Some((op, errno));
            self
        }

        fn check(&self, op: VideoOp) -> io::Result<()> {
            self.calls.lock().unwrap().push(op);
            match self.fail {
                Some((failing, errno)) if failing == op => Err(io::Error::from_raw_os_error(errno)),
                _ => Ok(()),
            }
        }
    }

    fn einval() -> io::Error {
        io::Error::from_raw_os_error(libc::EINVAL)
    }

    impl VideoNode for FakeVideoNode {
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

    pub(crate) fn fourcc(code: &[u8; 4]) -> u32 {
        u32::from_le_bytes(*code)
    }

    pub(crate) fn raw_size_discrete(width: u32, height: u32) -> FrmSizeEnum {
        FrmSizeEnum {
            kind: V4L2_FRMSIZE_TYPE_DISCRETE,
            data: [width, height, 0, 0, 0, 0],
            ..FrmSizeEnum::default()
        }
    }

    pub(crate) fn raw_interval_discrete(numerator: u32, denominator: u32) -> FrmIvalEnum {
        FrmIvalEnum {
            kind: V4L2_FRMIVAL_TYPE_DISCRETE,
            data: [numerator, denominator, 0, 0, 0, 0],
            ..FrmIvalEnum::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_abi_matches_linux_videodev2() {
        // Values from <linux/videodev2.h> (generic _IOC encoding, identical on
        // aarch64 and x86_64). Structure sizes are checked at compile time.
        assert_eq!(VIDIOC_QUERYCAP, 0x8068_5600);
        assert_eq!(VIDIOC_ENUM_FMT, 0xc040_5602);
        assert_eq!(VIDIOC_ENUM_FRAMESIZES, 0xc02c_564a);
        assert_eq!(VIDIOC_ENUM_FRAMEINTERVALS, 0xc034_564b);
    }

    #[test]
    fn enumeration_budget_allows_exactly_its_limit() {
        let mut budget = EnumerationBudget::new();
        for _ in 0..MAX_DEVICE_ENUMERATIONS {
            budget.spend().unwrap();
        }
        assert_eq!(
            budget.spend().unwrap_err(),
            "enumeration (more than 4096 queries)"
        );
    }

    #[test]
    fn effective_capabilities_follow_the_device_caps_flag() {
        let capability = |capabilities, device_caps| Capability {
            capabilities,
            device_caps,
            ..Capability::default()
        };
        let mplane = V4L2_CAP_VIDEO_CAPTURE_MPLANE;
        assert_eq!(
            effective_capabilities(&capability(V4L2_CAP_DEVICE_CAPS | mplane, mplane)),
            mplane
        );
        // Flag set: device_caps is authoritative even when it is zero.
        assert_eq!(
            effective_capabilities(&capability(V4L2_CAP_DEVICE_CAPS | mplane, 0)),
            0
        );
        // Flag clear: device_caps is not valid and is ignored.
        assert_eq!(
            effective_capabilities(&capability(V4L2_CAP_VIDEO_CAPTURE, mplane)),
            V4L2_CAP_VIDEO_CAPTURE
        );
    }
}
