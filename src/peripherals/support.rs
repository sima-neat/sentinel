//! Applies Neat Core's camera support rules to the catalog. Providers report
//! only what the kernel reports; whether Core's `CameraInput` accepts a mode
//! is Core's decision, shipped by Core as a rules file and applied here, in
//! the peripherals thread, before the catalog is compared and published.

use std::ffi::{CString, OsString};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{Issue, Record};

pub const DEFAULT_RULES_PATH: &str = "/usr/share/simaai-sentinel/support/neat-core.json";
pub const RULES_FORMAT: u32 = 1;
const ISSUE_PROVIDER: &str = "support";
const NOT_INSTALLED: &str =
    "Neat Core is not installed, so CameraInput support for this mode is unknown.";
const RANGE_REASON: &str =
    "Size ranges are advisory; CameraInput support is reported only for discrete sizes.";

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesFile {
    pub format: u32,
    /// Who wrote the rules, for example `neat-core 0.4.0`.
    pub source: String,
    pub camera: CameraRules,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraRules {
    pub backends: Accept<String>,
    pub formats: Accept<String>,
    pub framerates: Accept<Rate>,
    /// When present, a mode must be an ISP output size (`isp_output: true`).
    #[serde(default)]
    pub isp_output: Option<Reasoned>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accept<T> {
    pub accept: Vec<T>,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rate {
    pub num: u64,
    pub den: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reasoned {
    pub reason: String,
}

/// Published as the catalog's top-level `support` field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SupportStatus {
    /// `applied`, `not_installed`, `invalid`, or `stale` (an invalid update;
    /// the previous rules are still applied).
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub path: String,
}

/// Holds the last valid rules so a broken update cannot erase classification.
pub struct SupportStage {
    path: PathBuf,
    last_good: Option<RulesFile>,
}

impl SupportStage {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            last_good: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Re-read the rules file and fill `supported` / `reason` on every camera
    /// mode. A rules problem becomes an issue; it never fails the scan.
    pub fn apply(&mut self, devices: &mut [Record], issues: &mut Vec<Issue>) -> SupportStatus {
        let path = self.path.display().to_string();
        let state = match load(&self.path) {
            Ok(Some(rules)) => {
                self.last_good = Some(rules);
                "applied"
            }
            Ok(None) => {
                self.last_good = None;
                "not_installed"
            }
            Err(reason) => {
                issues.push(Issue {
                    provider: ISSUE_PROVIDER.into(),
                    code: "peripherals.invalid_support_rules".into(),
                    reason: format!("{path}: {reason}"),
                    retained_last_good: self.last_good.is_some(),
                });
                // Keep classifying with the previous rules rather than none.
                if self.last_good.is_some() {
                    "stale"
                } else {
                    "invalid"
                }
            }
        };
        for device in devices.iter_mut().filter(|device| device.kind == "camera") {
            classify_camera(&mut device.details, self.last_good.as_ref());
        }
        SupportStatus {
            state: state.into(),
            source: self.last_good.as_ref().map(|rules| rules.source.clone()),
            path,
        }
    }
}

fn load(path: &Path) -> Result<Option<RulesFile>, String> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read failed: {error}")),
    };
    // Check the format before the shape: a newer format is likely to have a
    // different shape, and the useful answer is "update Sentinel".
    let value: Value =
        serde_json::from_slice(&data).map_err(|error| format!("invalid rules: {error}"))?;
    let format = value.get("format").and_then(Value::as_u64);
    if format.is_some_and(|format| format > u64::from(RULES_FORMAT)) {
        return Err(format!(
            "Neat Core's rules use format {}, but this Sentinel reads format {RULES_FORMAT}. \
Update Sentinel: sima-cli neat install sentinel",
            format.unwrap_or_default()
        ));
    }
    let rules: RulesFile =
        serde_json::from_value(value).map_err(|error| format!("invalid rules: {error}"))?;
    if rules.format != RULES_FORMAT {
        return Err(format!("unsupported rules format {}", rules.format));
    }
    if rules.source.is_empty() {
        return Err("source must not be empty".into());
    }
    Ok(Some(rules))
}

fn classify_camera(details: &mut Value, rules: Option<&RulesFile>) {
    let backend = details["backend"].as_str().unwrap_or_default().to_string();
    let Some(modes) = details.get_mut("modes").and_then(Value::as_array_mut) else {
        return;
    };
    for mode in modes.iter_mut().filter(|mode| mode.is_object()) {
        let verdict = match rules {
            None => Err(NOT_INSTALLED.to_string()),
            Some(rules) => evaluate(&rules.camera, &backend, mode),
        };
        mode["supported"] = Value::Bool(verdict.is_ok());
        mode["reason"] = Value::String(verdict.err().unwrap_or_default());
    }
}

/// Rules are checked in a fixed order; the first failure is the reason.
fn evaluate(rules: &CameraRules, backend: &str, mode: &Value) -> Result<(), String> {
    if !rules
        .backends
        .accept
        .iter()
        .any(|accepted| accepted == backend)
    {
        return Err(rules.backends.reason.clone());
    }
    let format = mode["format"].as_str().unwrap_or_default();
    if !rules
        .formats
        .accept
        .iter()
        .any(|accepted| accepted == format)
    {
        return Err(rules.formats.reason.clone());
    }
    let rate = (
        mode["framerate_num"].as_u64().unwrap_or(0),
        mode["framerate_den"].as_u64().unwrap_or(0),
    );
    if !rules
        .framerates
        .accept
        .iter()
        .any(|accepted| same_rate((accepted.num, accepted.den), rate))
    {
        return Err(rules.framerates.reason.clone());
    }
    if mode.get("size_range").is_some() {
        return Err(RANGE_REASON.into());
    }
    if let Some(isp_output) = &rules.isp_output {
        if mode["isp_output"].as_bool() != Some(true) {
            return Err(isp_output.reason.clone());
        }
    }
    Ok(())
}

/// 30/1 and 60/2 are the same rate.
fn same_rate(left: (u64, u64), right: (u64, u64)) -> bool {
    left.1 != 0 && right.1 != 0 && left.0 * right.1 == right.0 * left.1
}

/// Wakes the peripherals thread when Core installs, replaces or removes its
/// rules file, so a Core upgrade is reflected without a hardware rescan.
pub struct RulesWatch {
    fd: OwnedFd,
    file_name: OsString,
}

impl RulesWatch {
    /// Watch the rules file's directory. Package managers replace files by
    /// rename, so the directory, not the file, is watched.
    pub fn open(path: &Path) -> io::Result<Self> {
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        let file_name = path
            .file_name()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "rules path has no file name")
            })?
            .to_os_string();
        // SAFETY: plain descriptor creation; the result is checked.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: raw is a freshly created, owned descriptor.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let c_directory = CString::new(directory.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "rules directory contains NUL")
        })?;
        let mask = libc::IN_CLOSE_WRITE
            | libc::IN_MOVED_TO
            | libc::IN_MOVED_FROM
            | libc::IN_CREATE
            | libc::IN_DELETE;
        // SAFETY: valid descriptor and NUL-terminated path.
        if unsafe { libc::inotify_add_watch(raw, c_directory.as_ptr(), mask) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd, file_name })
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Read every queued event; true if one concerned the rules file or the
    /// kernel dropped events.
    pub fn drain(&self) -> io::Result<bool> {
        let mut changed = false;
        let mut buffer = [0u8; 4096];
        loop {
            // SAFETY: buffer is valid for its full length.
            let count = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    buffer.as_mut_ptr() as *mut libc::c_void,
                    buffer.len(),
                )
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                return match error.raw_os_error() {
                    Some(libc::EAGAIN) => Ok(changed),
                    Some(libc::EINTR) => continue,
                    _ => Err(error),
                };
            }
            changed |= events_concern(&buffer[..count as usize], self.file_name.as_bytes());
        }
    }
}

/// Parse a buffer of `struct inotify_event` records.
fn events_concern(mut buffer: &[u8], file_name: &[u8]) -> bool {
    const HEADER: usize = 16;
    let mut concerned = false;
    while buffer.len() >= HEADER {
        let mask = u32::from_ne_bytes(buffer[4..8].try_into().unwrap());
        let length = u32::from_ne_bytes(buffer[12..16].try_into().unwrap()) as usize;
        let Some(name) = buffer.get(HEADER..HEADER + length) else {
            break;
        };
        let name = name.split(|&byte| byte == 0).next().unwrap_or_default();
        if mask & libc::IN_Q_OVERFLOW != 0 || name == file_name {
            concerned = true;
        }
        buffer = &buffer[HEADER + length..];
    }
    concerned
}

/// The rules Neat Core ships (`src/peripherals/sentinel-support-rules.json.in`
/// in Core), for tests elsewhere in Sentinel.
#[cfg(test)]
pub(crate) fn core_rules() -> Value {
    tests::core_rules()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keeps temporary directories distinct when tests start in the same instant.
    static UNIQUE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// The rules Neat Core 0.4 would ship, matching its current CameraInput.
    pub fn core_rules() -> Value {
        json!({
            "format": 1,
            "source": "neat-core 0.4.0",
            "camera": {
                "backends": {"accept": ["mipi"], "reason": "CameraInput currently accepts MIPI cameras only; direct V4L2 capture is not supported."},
                "formats": {"accept": ["NV12"], "reason": "CameraInput's current camera-memory path supports NV12 output only."},
                "framerates": {"accept": [{"num": 30, "den": 1}], "reason": "This mode does not advertise CameraInput's 30/1 frame rate."},
                "isp_output": {"reason": "This resolution is not an ISP output size on this board."}
            }
        })
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "sentinel-support-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                UNIQUE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn camera(backend: &str, modes: Value) -> Record {
        Record {
            id: format!("camera:{backend}"),
            kind: "camera".into(),
            provider: "test".into(),
            details: json!({"backend": backend, "modes": modes}),
        }
    }

    fn mode(format: &str, width: u32, rate: (u32, u32), isp_output: bool) -> Value {
        json!({"format": format, "width": width, "height": width * 9 / 16,
               "framerate_num": rate.0, "framerate_den": rate.1, "isp_output": isp_output})
    }

    fn verdicts(record: &Record) -> Vec<(bool, String)> {
        record.details["modes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|mode| {
                (
                    mode["supported"].as_bool().unwrap(),
                    mode["reason"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn core_rules_classify_each_mode_with_the_first_failing_reason() {
        let dir = TempDir::new();
        let path = dir.0.join("neat-core.json");
        fs::write(&path, core_rules().to_string()).unwrap();
        let mut stage = SupportStage::new(&path);
        let mut devices = vec![
            camera(
                "mipi",
                json!([
                    mode("NV12", 1920, (30, 1), true),
                    mode("NV12", 1280, (30, 1), false),
                    mode("RGB", 1920, (30, 1), true),
                    mode("NV12", 1920, (60, 2), true),
                    mode("NV12", 1920, (15, 1), true),
                    {"format": "NV12", "framerate_num": 30, "framerate_den": 1,
                     "size_range": {"min_width": 640, "min_height": 480, "max_width": 1920,
                                    "max_height": 1080, "step_width": 16, "step_height": 8}}
                ]),
            ),
            camera("v4l2", json!([mode("NV12", 1920, (30, 1), true)])),
        ];
        let mut issues = Vec::new();
        let status = stage.apply(&mut devices, &mut issues);
        assert_eq!(status.state, "applied");
        assert_eq!(status.source.as_deref(), Some("neat-core 0.4.0"));
        assert!(issues.is_empty());

        let mipi = verdicts(&devices[0]);
        assert_eq!(mipi[0], (true, String::new()));
        assert!(mipi[1].1.contains("not an ISP output size"));
        assert!(mipi[2].1.contains("NV12 output only"));
        assert_eq!(mipi[3], (true, String::new()), "60/2 equals 30/1");
        assert!(mipi[4].1.contains("30/1 frame rate"));
        assert!(mipi[5].1.contains("Size ranges are advisory"));
        assert!(verdicts(&devices[1])[0].1.contains("MIPI cameras only"));
    }

    #[test]
    fn missing_rules_mark_every_mode_unknown_with_a_reason() {
        let dir = TempDir::new();
        let mut stage = SupportStage::new(dir.0.join("neat-core.json"));
        let mut devices = vec![camera("mipi", json!([mode("NV12", 1920, (30, 1), true)]))];
        let mut issues = Vec::new();
        let status = stage.apply(&mut devices, &mut issues);
        assert_eq!(status.state, "not_installed");
        assert!(status.source.is_none());
        assert!(issues.is_empty());
        assert_eq!(verdicts(&devices[0])[0], (false, NOT_INSTALLED.into()));
    }

    #[test]
    fn invalid_update_keeps_the_last_good_rules_and_reports_an_issue() {
        let dir = TempDir::new();
        let path = dir.0.join("neat-core.json");
        fs::write(&path, core_rules().to_string()).unwrap();
        let mut stage = SupportStage::new(&path);
        let mut devices = vec![camera("mipi", json!([mode("NV12", 1920, (30, 1), true)]))];
        stage.apply(&mut devices, &mut Vec::new());

        fs::write(&path, r#"{"format": 2, "source": "x", "camera": {}}"#).unwrap();
        let mut issues = Vec::new();
        let status = stage.apply(&mut devices, &mut issues);
        assert_eq!(status.state, "stale");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "peripherals.invalid_support_rules");
        assert!(issues[0].reason.contains("format 2"));
        assert!(issues[0].reason.contains("sima-cli neat install sentinel"));
        assert!(issues[0].retained_last_good);
        assert_eq!(verdicts(&devices[0])[0], (true, String::new()));
    }

    #[test]
    fn invalid_rules_without_a_previous_version_report_invalid() {
        let dir = TempDir::new();
        let path = dir.0.join("neat-core.json");
        fs::write(&path, "not json").unwrap();
        let mut stage = SupportStage::new(&path);
        let mut devices = vec![camera("mipi", json!([mode("NV12", 1920, (30, 1), true)]))];
        let mut issues = Vec::new();
        let status = stage.apply(&mut devices, &mut issues);
        assert_eq!(status.state, "invalid");
        assert!(!issues[0].retained_last_good);
        assert!(!verdicts(&devices[0])[0].0);
    }

    #[test]
    fn non_camera_records_are_left_alone() {
        let dir = TempDir::new();
        let mut stage = SupportStage::new(dir.0.join("missing.json"));
        let mut devices = vec![Record {
            id: "microphone:x".into(),
            kind: "microphone".into(),
            provider: "test".into(),
            details: json!({"modes": [{"format": "S16_LE"}]}),
        }];
        stage.apply(&mut devices, &mut Vec::new());
        assert!(devices[0].details["modes"][0].get("supported").is_none());
    }

    #[test]
    fn watch_reports_changes_to_the_rules_file_only() {
        let dir = TempDir::new();
        let path = dir.0.join("neat-core.json");
        let watch = RulesWatch::open(&path).unwrap();
        assert!(!watch.drain().unwrap());
        fs::write(dir.0.join("other.json"), "{}").unwrap();
        assert!(!watch.drain().unwrap(), "unrelated files are ignored");
        let staged = dir.0.join("neat-core.json.dpkg-new");
        fs::write(&staged, core_rules().to_string()).unwrap();
        fs::rename(&staged, &path).unwrap();
        assert!(
            watch.drain().unwrap(),
            "a package-manager style rename is seen"
        );
        fs::remove_file(&path).unwrap();
        assert!(watch.drain().unwrap(), "removal is seen");
    }

    #[test]
    fn watch_needs_an_existing_directory() {
        assert!(RulesWatch::open(Path::new("/nonexistent/sentinel/neat-core.json")).is_err());
    }
}
