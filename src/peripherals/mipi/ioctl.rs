//! The read-only media-controller and V4L2 ioctl surface used by MIPI camera
//! discovery.
//!
//! Only query ioctls are declared here: `MEDIA_IOC_DEVICE_INFO` and
//! `MEDIA_IOC_G_TOPOLOGY` on `/dev/media*`, and `VIDIOC_QUERYCAP`,
//! `VIDIOC_ENUM_FMT`, `VIDIOC_ENUM_FRAMESIZES` and `VIDIOC_ENUM_FRAMEINTERVALS`
//! on the ISP video node. Nothing in this module can set up media links, set a
//! subdevice or video format, request buffers, or start streaming.
//!
//! The structures mirror `<linux/media.h>` byte for byte. The V4L2 video-node
//! queries are the shared surface in `peripherals::videodev2`.

use std::io;
use std::path::Path;

use crate::peripherals::videodev2::{
    ioc, open_read_only, SystemNode, VideoNode, IOC_READ, IOC_WRITE,
};

/// `MEDIA_ENT_F_CAM_SENSOR`.
pub const MEDIA_ENT_F_CAM_SENSOR: u32 = 0x0002_0001;
#[cfg(test)]
pub const MEDIA_ENT_F_IO_V4L: u32 = 0x0001_0001;
#[cfg(test)]
pub const MEDIA_ENT_F_V4L2_SUBDEV_UNKNOWN: u32 = 0x0002_0000;

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
const _: () = assert!(std::mem::size_of::<MediaDeviceInfo>() == 256);

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
const _: () = assert!(std::mem::size_of::<MediaV2Topology>() == 72);

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
const _: () = assert!(std::mem::size_of::<MediaV2Entity>() == 96);

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

/// Character-device backend: `open(O_RDONLY | O_NONBLOCK | O_CLOEXEC)` and
/// query ioctls only.
pub struct SystemBackend;

impl Backend for SystemBackend {
    fn open_media(&self, path: &Path) -> io::Result<Box<dyn MediaNode>> {
        Ok(Box::new(open_read_only(path)?))
    }

    fn open_video(&self, path: &Path) -> io::Result<Box<dyn VideoNode>> {
        Ok(Box::new(open_read_only(path)?))
    }
}

impl MediaNode for SystemNode {
    fn device_info(&mut self, value: &mut MediaDeviceInfo) -> io::Result<()> {
        // SAFETY: MEDIA_IOC_DEVICE_INFO encodes `struct media_device_info`.
        unsafe { self.ioctl(MEDIA_IOC_DEVICE_INFO, value) }
    }

    fn topology(
        &mut self,
        value: &mut MediaV2Topology,
        entities: &mut [MediaV2Entity],
    ) -> io::Result<()> {
        // The kernel writes at most `num_entities` entries to `ptr_entities`
        // (and fails with ENOSPC if more exist), so tying both to the slice
        // keeps the write in bounds. All other arrays stay null. A slice too
        // long to describe is refused rather than silently understated.
        let num_entities = u32::try_from(entities.len())
            .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
        *value = MediaV2Topology {
            num_entities,
            ptr_entities: if entities.is_empty() {
                0
            } else {
                entities.as_mut_ptr() as u64
            },
            ..MediaV2Topology::default()
        };
        // SAFETY: MEDIA_IOC_G_TOPOLOGY encodes `struct media_v2_topology`;
        // `ptr_entities` is null or points to `num_entities` entries of
        // `entities`, which outlives the call, and every other array is null.
        unsafe { self.ioctl(MEDIA_IOC_G_TOPOLOGY, value) }
    }
}
