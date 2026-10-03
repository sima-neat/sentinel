//! How the V4L2 camera provider opens nodes. The query ioctls themselves are
//! the shared, read-only surface in `peripherals::videodev2`.

use std::io;
use std::path::Path;

use crate::peripherals::videodev2::{open_read_only, VideoNode};

/// Opens V4L2 nodes. The production implementation opens character devices;
/// tests substitute a fake that serves synthetic devices.
pub trait Backend: Send {
    fn open(&self, path: &Path) -> io::Result<Box<dyn VideoNode>>;
}

/// Character-device backend: `open(O_RDONLY | O_NONBLOCK | O_CLOEXEC)` and
/// query ioctls only.
pub struct SystemBackend;

impl Backend for SystemBackend {
    fn open(&self, path: &Path) -> io::Result<Box<dyn VideoNode>> {
        Ok(Box::new(open_read_only(path)?))
    }
}
