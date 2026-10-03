//! The read-only media-controller ioctls of MIPI discovery: `MEDIA_IOC_DEVICE_INFO`
//! and `MEDIA_IOC_G_TOPOLOGY` (entities only). Nothing here can set up links,
//! set a format, or stream. The structures mirror `<linux/media.h>` byte for
//! byte; the V4L2 node queries are the shared surface in `videodev2`.

use std::io;
use std::mem::{offset_of, size_of};
use std::path::Path;

use crate::peripherals::videodev2::{
    ioc, open_read_only, SystemNode, VideoNode, IOC_READ, IOC_WRITE,
};

pub const MEDIA_ENT_F_CAM_SENSOR: u32 = 0x0002_0001;

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

/// `struct media_v2_topology`. `other` holds the interface, pad and link
/// counts and pointers, which stay zero so the kernel skips those arrays.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct MediaV2Topology {
    topology_version: u64,
    num_entities: u32,
    reserved1: u32,
    ptr_entities: u64,
    other: [u64; 6],
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

const IOWR: u32 = IOC_READ | IOC_WRITE;
const MEDIA_IOC_DEVICE_INFO: u32 = ioc(IOWR, b'|', 0x00, size_of::<MediaDeviceInfo>());
const MEDIA_IOC_G_TOPOLOGY: u32 = ioc(IOWR, b'|', 0x04, size_of::<MediaV2Topology>());

// The ABI of <linux/media.h>, with ioctl numbers computed by a C program.
const _: () =
    assert!(size_of::<MediaDeviceInfo>() == 256 && offset_of!(MediaDeviceInfo, bus_info) == 88);
const _: () =
    assert!(size_of::<MediaV2Topology>() == 72 && offset_of!(MediaV2Topology, ptr_entities) == 16);
const _: () =
    assert!(size_of::<MediaV2Entity>() == 96 && offset_of!(MediaV2Entity, function) == 68);
const _: () = assert!(MEDIA_IOC_DEVICE_INFO == 0xc100_7c00 && MEDIA_IOC_G_TOPOLOGY == 0xc048_7c04);

/// Opens media and video nodes; tests substitute synthetic devices.
pub trait Backend: Send {
    fn open_media(&self, path: &Path) -> io::Result<Box<dyn MediaNode>>;
    fn open_video(&self, path: &Path) -> io::Result<Box<dyn VideoNode>>;
}

/// The query ioctls of one open `/dev/media*` node.
pub trait MediaNode {
    fn device_info(&mut self, value: &mut MediaDeviceInfo) -> io::Result<()>;
    /// Fills `entities` with the graph's entities in one `MEDIA_IOC_G_TOPOLOGY`
    /// call and returns how many there are; `ENOSPC` when they do not fit.
    fn entities(&mut self, entities: &mut [MediaV2Entity]) -> io::Result<usize>;
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
}

impl MediaNode for SystemNode {
    fn device_info(&mut self, value: &mut MediaDeviceInfo) -> io::Result<()> {
        // SAFETY: MEDIA_IOC_DEVICE_INFO encodes `struct media_device_info`.
        unsafe { self.ioctl(MEDIA_IOC_DEVICE_INFO, value) }
    }

    fn entities(&mut self, entities: &mut [MediaV2Entity]) -> io::Result<usize> {
        let mut topology = MediaV2Topology {
            // Truncation could only understate the buffer, never overrun it.
            num_entities: entities.len() as u32,
            ptr_entities: entities.as_mut_ptr() as u64,
            ..MediaV2Topology::default()
        };
        // SAFETY: MEDIA_IOC_G_TOPOLOGY encodes `struct media_v2_topology`; the
        // kernel writes at most `num_entities` entries to `ptr_entities`, which
        // points into `entities` for the duration of the call; all other
        // arrays are null.
        unsafe { self.ioctl(MEDIA_IOC_G_TOPOLOGY, &mut topology)? };
        Ok(topology.num_entities as usize)
    }
}
