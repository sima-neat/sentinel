//! Small helpers shared by the kernel-backed discovery providers: provider
//! error codes, sysfs text reads, kernel string decoding, and errno handling.

use std::fs;
use std::io;
use std::path::Path;

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

/// The device node or sysfs entry vanished (hot-unplug race).
pub(crate) fn disappeared(errno: i32) -> bool {
    errno == libc::ENOENT || errno == libc::ENODEV || errno == libc::ENXIO
}

/// Temporary-directory fixtures shared by the provider tests.
#[cfg(test)]
pub(crate) mod testing {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh directory under the system temp directory, removed on drop.
    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        pub(crate) fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "sentinel-peripherals-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create fixture directory");
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
}
