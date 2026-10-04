//! The read-only V4L2 ioctl surface shared by the camera providers.
//!
//! Only query ioctls are declared here (`VIDIOC_QUERYCAP`, `VIDIOC_ENUM_FMT`,
//! `VIDIOC_ENUM_FRAMESIZES`, `VIDIOC_ENUM_FRAMEINTERVALS`). Nothing in this
//! module can set a format, request buffers, or start streaming, so discovery
//! never changes the state of a camera another process may be using.
//!
//! The structures mirror `<linux/videodev2.h>` byte for byte. Kernel unions are
//! represented as `[u32; 6]` payloads, so no `unsafe` union reads are needed.

use std::fs::{File, OpenOptions};
use std::io;
use std::ops::ControlFlow;
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

/// Why [`enumerate`] stopped short.
pub(crate) enum EnumerationError {
    /// What the device returned malformed, e.g. `frame size` or
    /// `format list`.
    Malformed(String),
    /// The enumeration ioctl failed with an error other than `EINVAL`.
    Failed(io::Error),
}

/// Enumerate one V4L2 `list`: issue `query` for index 0, 1, ... until the
/// driver ends the list with `EINVAL`, decoding each answer. `decode` returns
/// `None` for a malformed entry and `Break` for an entry that must be the last
/// (a stepwise or continuous range). Each query is charged to `budget`, the
/// queries left for the device, and a list holds at most
/// [`MAX_ENUMERATION_ENTRIES`].
pub(crate) fn enumerate<T, D>(
    budget: &mut u32,
    list: &str,
    mut query: impl FnMut(u32) -> io::Result<T>,
    mut decode: impl FnMut(T) -> Option<ControlFlow<D, D>>,
) -> Result<Vec<D>, EnumerationError> {
    let mut entries = Vec::new();
    for index in 0.. {
        if index >= MAX_ENUMERATION_ENTRIES {
            return Err(EnumerationError::Malformed(format!("{list} list")));
        }
        *budget = budget.checked_sub(1).ok_or_else(|| {
            let what = format!("enumeration (more than {MAX_DEVICE_ENUMERATIONS} queries)");
            EnumerationError::Malformed(what)
        })?;
        let value = match query(index) {
            Ok(value) => value,
            Err(error) if error.raw_os_error() == Some(libc::EINVAL) => break,
            Err(error) => return Err(EnumerationError::Failed(error)),
        };
        match decode(value) {
            Some(ControlFlow::Continue(entry)) => entries.push(entry),
            Some(ControlFlow::Break(entry)) => {
                entries.push(entry);
                break;
            }
            None => return Err(EnumerationError::Malformed(list.to_string())),
        }
    }
    Ok(entries)
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
// Values from <linux/videodev2.h> (identical on aarch64 and x86_64).
const _: () = assert!(
    VIDIOC_QUERYCAP == 0x8068_5600
        && VIDIOC_ENUM_FMT == 0xc040_5602
        && VIDIOC_ENUM_FRAMESIZES == 0xc02c_564a
        && VIDIOC_ENUM_FRAMEINTERVALS == 0xc034_564b
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

/// A fake video node shared by the provider tests: it answers from its lists
/// in driver order, counts every ioctl, and fails `fail.0` with errno `fail.1`.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Debug, Clone, Default)]
    pub(crate) struct FakeVideoNode {
        pub capability: Capability,
        /// `(buffer type, pixel format, description)`.
        pub formats: Vec<(u32, u32, &'static str)>,
        pub sizes: Vec<(u32, FrmSizeEnum)>,
        /// `((pixel format, width, height), interval)`.
        pub intervals: Vec<((u32, u32, u32), FrmIvalEnum)>,
        pub fail: Option<(u32, i32)>,
        pub calls: Arc<AtomicUsize>,
    }

    impl FakeVideoNode {
        pub(crate) fn new(card: &str, capabilities: u32, device_caps: u32) -> Self {
            let mut node = Self::default();
            node.capability.capabilities = capabilities;
            node.capability.device_caps = device_caps;
            node.capability.card[..card.len()].copy_from_slice(card.as_bytes());
            node
        }

        fn answer<T>(&self, request: u32, found: Option<T>) -> io::Result<T> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let found = match self.fail {
                Some((failing, errno)) if failing == request => Err(errno),
                _ => found.ok_or(libc::EINVAL),
            };
            found.map_err(io::Error::from_raw_os_error)
        }
    }

    fn nth<K: PartialEq, V: Copy>(list: &[(K, V)], key: K, index: u32) -> Option<V> {
        let mut matching = list.iter().filter(|(candidate, _)| *candidate == key);
        matching.nth(index as usize).map(|(_, value)| *value)
    }

    impl VideoNode for FakeVideoNode {
        fn query_capability(&mut self, value: &mut Capability) -> io::Result<()> {
            *value = self.answer(VIDIOC_QUERYCAP, Some(self.capability))?;
            Ok(())
        }

        fn enum_format(&mut self, value: &mut FmtDesc) -> io::Result<()> {
            let mut matching = self.formats.iter().filter(|f| f.0 == value.buf_type);
            let found = matching.nth(value.index as usize).copied();
            let (_, pixel_format, text) = self.answer(VIDIOC_ENUM_FMT, found)?;
            value.pixelformat = pixel_format;
            value.description[..text.len()].copy_from_slice(text.as_bytes());
            Ok(())
        }

        fn enum_frame_size(&mut self, value: &mut FrmSizeEnum) -> io::Result<()> {
            let found = nth(&self.sizes, value.pixel_format, value.index);
            let size = self.answer(VIDIOC_ENUM_FRAMESIZES, found)?;
            (value.kind, value.data) = (size.kind, size.data);
            Ok(())
        }

        fn enum_frame_interval(&mut self, value: &mut FrmIvalEnum) -> io::Result<()> {
            let key = (value.pixel_format, value.width, value.height);
            let found = nth(&self.intervals, key, value.index);
            let interval = self.answer(VIDIOC_ENUM_FRAMEINTERVALS, found)?;
            (value.kind, value.data) = (interval.kind, interval.data);
            Ok(())
        }
    }

    pub(crate) fn fourcc(code: &[u8; 4]) -> u32 {
        u32::from_le_bytes(*code)
    }

    /// A frame size or interval of `kind` with its `<linux/videodev2.h>` data.
    pub(crate) fn raw_size(kind: u32, data: [u32; 6]) -> FrmSizeEnum {
        let mut size = FrmSizeEnum::default();
        (size.kind, size.data) = (kind, data);
        size
    }

    pub(crate) fn raw_interval(kind: u32, data: [u32; 6]) -> FrmIvalEnum {
        let mut interval = FrmIvalEnum::default();
        (interval.kind, interval.data) = (kind, data);
        interval
    }

    pub(crate) fn discrete_size(width: u32, height: u32) -> FrmSizeEnum {
        raw_size(V4L2_FRMSIZE_TYPE_DISCRETE, [width, height, 0, 0, 0, 0])
    }

    pub(crate) fn discrete_interval(numerator: u32, denominator: u32) -> FrmIvalEnum {
        let data = [numerator, denominator, 0, 0, 0, 0];
        raw_interval(V4L2_FRMIVAL_TYPE_DISCRETE, data)
    }
}
