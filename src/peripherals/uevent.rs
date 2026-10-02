use std::collections::BTreeSet;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

/// Kernel uevents, sent as soon as a device appears or disappears.
const KERNEL_GROUP: u32 = 1;
/// The same events re-broadcast by udev after its rules have run, so device
/// nodes already carry their final permissions.
const UDEV_GROUP: u32 = 2;
const RECEIVE_BUFFER_BYTES: libc::c_int = 1024 * 1024;

/// What draining the socket found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Drained {
    /// At least one event matched a watched subsystem.
    pub matched: bool,
    /// The kernel dropped events; the caller must assume anything changed.
    pub overflowed: bool,
}

/// A non-blocking NETLINK_KOBJECT_UEVENT socket. Messages are only treated as
/// a hint to rescan; their content is never trusted as catalog data.
pub struct UeventSocket {
    fd: OwnedFd,
    subsystems: BTreeSet<String>,
}

impl UeventSocket {
    pub fn open(subsystems: impl IntoIterator<Item = String>) -> io::Result<Self> {
        // SAFETY: plain socket creation; the result is checked below.
        let raw = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                libc::NETLINK_KOBJECT_UEVENT,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: raw is a freshly created, owned descriptor.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let size = RECEIVE_BUFFER_BYTES;
        // SAFETY: size outlives the call. SO_RCVBUFFORCE needs CAP_NET_ADMIN;
        // fall back to the capped SO_RCVBUF without it.
        unsafe {
            let pointer = &size as *const libc::c_int as *const libc::c_void;
            let length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            if libc::setsockopt(raw, libc::SOL_SOCKET, libc::SO_RCVBUFFORCE, pointer, length) != 0 {
                libc::setsockopt(raw, libc::SOL_SOCKET, libc::SO_RCVBUF, pointer, length);
            }
        }
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        address.nl_groups = KERNEL_GROUP | UDEV_GROUP;
        // SAFETY: address is a valid sockaddr_nl for the call's duration.
        let bound = unsafe {
            libc::bind(
                raw,
                &address as *const libc::sockaddr_nl as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        };
        if bound != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            fd,
            subsystems: subsystems.into_iter().collect(),
        })
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Read every queued message without blocking.
    pub fn drain(&self) -> io::Result<Drained> {
        let mut drained = Drained::default();
        let mut buffer = [0u8; 8192];
        loop {
            // SAFETY: buffer is valid for its full length.
            let count = unsafe {
                libc::recv(
                    self.fd.as_raw_fd(),
                    buffer.as_mut_ptr() as *mut libc::c_void,
                    buffer.len(),
                    0,
                )
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::EAGAIN) => return Ok(drained),
                    Some(libc::EINTR) => continue,
                    Some(libc::ENOBUFS) => drained.overflowed = true,
                    _ => return Err(error),
                }
                continue;
            }
            if let Some(subsystem) = message_subsystem(&buffer[..count as usize]) {
                if self.subsystems.contains(subsystem) {
                    drained.matched = true;
                }
            }
        }
    }
}

/// Both the kernel format (`add@/devpath\0KEY=VALUE\0...`) and udev's format
/// (binary header, then `KEY=VALUE\0...`) carry NUL-separated properties.
pub fn message_subsystem(message: &[u8]) -> Option<&str> {
    message
        .split(|&byte| byte == 0)
        .find_map(|field| field.strip_prefix(b"SUBSYSTEM="))
        .and_then(|value| std::str::from_utf8(value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsystem_is_found_in_kernel_messages() {
        let message = b"add@/devices/usb1/1-2/1-2.3:1.0/video4linux/video2\0ACTION=add\0DEVPATH=/devices/usb1/1-2/1-2.3:1.0/video4linux/video2\0SUBSYSTEM=video4linux\0DEVNAME=video2\0SEQNUM=4127\0";
        assert_eq!(message_subsystem(message), Some("video4linux"));
    }

    #[test]
    fn subsystem_is_found_after_a_udev_binary_header() {
        let mut message = b"libudev\0\xfe\xed\xca\xfe\x28\0\0\0".to_vec();
        message.extend_from_slice(b"ACTION=remove\0SUBSYSTEM=sound\0DEVPATH=/devices/x\0");
        assert_eq!(message_subsystem(&message), Some("sound"));
    }

    #[test]
    fn messages_without_a_subsystem_are_ignored() {
        assert_eq!(message_subsystem(b"ACTION=add\0DEVPATH=/x\0"), None);
        assert_eq!(message_subsystem(b""), None);
    }
}
