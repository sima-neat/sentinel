//! Small helpers shared by the kernel-backed discovery providers: provider
//! error codes, sysfs text reads, kernel string decoding, and errno handling.

use std::fs;
use std::io;
use std::path::Path;

use super::ProviderError;

pub(crate) const CODE_IO_OPEN: &str = "io.open";
pub(crate) const CODE_PERMISSION_DENIED: &str = "io.permission_denied";
pub(crate) const CODE_DISCOVERY_FAILED: &str = "peripherals.discovery_failed";

/// `value` without leading or trailing C-locale (`isspace`) whitespace.
pub(crate) fn trim_c_space(value: &str) -> &str {
    value.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r'))
}

/// A sysfs attribute as trimmed text; `None` when it cannot be read.
pub(crate) fn read_text_file(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(trim_c_space(&String::from_utf8_lossy(&bytes)).to_string())
}

/// A NUL-terminated kernel string, untrimmed.
pub(crate) fn bounded_string(data: &[u8]) -> String {
    let length = data
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(data.len());
    String::from_utf8_lossy(&data[..length]).into_owned()
}

/// The OS error text without Rust's ` (os error N)` suffix, as `strerror`
/// prints it.
pub(crate) fn os_message(error: &io::Error) -> String {
    let text = error.to_string();
    match error.raw_os_error() {
        Some(code) => text
            .strip_suffix(&format!(" (os error {code})"))
            .map(str::to_owned)
            .unwrap_or(text),
        None => text,
    }
}

pub(crate) fn errno_of(error: &io::Error) -> i32 {
    error.raw_os_error().unwrap_or(0)
}

/// `what path: strerror`, coded `io.permission_denied` for EACCES, and for
/// EPERM only when `eperm` is set (opening a device node; sysfs and directory
/// reads say EPERM for other reasons), otherwise `io.open`.
pub(crate) fn io_error(what: &str, path: &Path, error: &io::Error, eperm: bool) -> ProviderError {
    let code = match errno_of(error) {
        libc::EACCES => CODE_PERMISSION_DENIED,
        libc::EPERM if eperm => CODE_PERMISSION_DENIED,
        _ => CODE_IO_OPEN,
    };
    let reason = format!("{what} {}: {}", path.display(), os_message(error));
    ProviderError::new(code, reason)
}

/// The device node or sysfs entry vanished (hot-unplug race).
pub(crate) fn disappeared(errno: i32) -> bool {
    errno == libc::ENOENT || errno == libc::ENODEV || errno == libc::ENXIO
}

/// Whether the sysfs entry or device node at `path` is gone. A device whose
/// probe failed and whose entry has gone since was unplugged mid-scan, even
/// when the failing call did not say so (a removed sysfs attribute reads as
/// nothing; a driver may answer `EIO`). Any other error cannot tell, so the
/// device counts as present and its failure stands.
pub(crate) fn vanished(path: &Path) -> bool {
    fs::metadata(path).is_err_and(|error| matches!(errno_of(&error), libc::ENOENT | libc::ENOTDIR))
}

/// Fixtures shared by the peripheral tests.
#[cfg(test)]
pub(crate) mod testing {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::peripherals::{Peripheral, ProviderError};

    /// A fresh directory under the system temp directory, removed on drop.
    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        pub(crate) fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let name = format!("sentinel-peripherals-test-{}-{n}", std::process::id());
            let path = std::env::temp_dir().join(name);
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Write `value` to `path`, creating parent directories.
    pub(crate) fn write_file(path: &Path, value: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }

    /// A scan of a tree under `root`, with its paths as on a board
    /// (`/dev/video0`, not `/tmp/…/dev/video0`).
    pub(crate) fn on_board(
        root: &Path,
        result: Result<Vec<Peripheral>, ProviderError>,
    ) -> Result<Vec<Peripheral>, ProviderError> {
        let strip = |text: String| text.replace(root.to_str().unwrap(), "");
        let devices = result.map_err(|e| ProviderError::new(e.code, strip(e.reason)))?;
        let text = strip(serde_json::to_string(&devices).unwrap());
        Ok(serde_json::from_str(&text).unwrap())
    }
}
