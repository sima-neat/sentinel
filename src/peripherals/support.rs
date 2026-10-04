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
    if let Some(format) = format.filter(|&format| format > u64::from(RULES_FORMAT)) {
        return Err(format!(
            "Neat Core's rules use format {format}, but this Sentinel reads format {RULES_FORMAT}. \
Update Sentinel: sima-cli neat install sentinel"
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
    let cross = |a: u64, b: u64| u128::from(a) * u128::from(b);
    left.1 != 0 && right.1 != 0 && cross(left.0, right.1) == cross(right.0, left.1)
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
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
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
            | libc::IN_DELETE
            | libc::IN_DELETE_SELF
            | libc::IN_MOVE_SELF;
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
            changed |= events_concern(&buffer[..count as usize], self.file_name.as_bytes())?;
        }
    }
}

/// Parse a buffer of `struct inotify_event` records.
fn events_concern(mut buffer: &[u8], file_name: &[u8]) -> io::Result<bool> {
    const HEADER: usize = 16;
    let mut concerned = false;
    while buffer.len() >= HEADER {
        let mask = u32::from_ne_bytes(buffer[4..8].try_into().unwrap());
        let length = u32::from_ne_bytes(buffer[12..16].try_into().unwrap()) as usize;
        let Some(name) = buffer.get(HEADER..HEADER + length) else {
            break;
        };
        if mask & (libc::IN_IGNORED | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "support rules directory watch was invalidated",
            ));
        }
        let name = name.split(|&byte| byte == 0).next().unwrap_or_default();
        if mask & libc::IN_Q_OVERFLOW != 0 || name == file_name {
            concerned = true;
        }
        buffer = &buffer[HEADER + length..];
    }
    Ok(concerned)
}

/// The rules Neat Core ships (`src/peripherals/sentinel-support-rules.json.in`
/// in Core), matching its current CameraInput, for tests across Sentinel.
#[cfg(test)]
pub(crate) fn core_rules() -> Value {
    serde_json::json!({
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peripherals::scan::testing::record;
    use crate::peripherals::sysutil::testing::TempDir;
    use serde_json::json;

    fn camera(backend: &str, modes: Value) -> Record {
        let details = json!({"backend": backend, "modes": modes});
        record("test", &format!("camera:{backend}"), details)
    }

    fn mode(format: &str, width: u32, rate: (u32, u32), isp_output: bool) -> Value {
        json!({"format": format, "width": width, "height": width * 9 / 16,
               "framerate_num": rate.0, "framerate_den": rate.1, "isp_output": isp_output})
    }

    fn verdicts(record: &Record) -> Vec<(bool, Value)> {
        let modes = record.details["modes"].as_array().unwrap().iter();
        let verdict = |mode: &Value| (mode["supported"] == true, mode["reason"].clone());
        modes.map(verdict).collect()
    }

    /// Rules are checked in a fixed order and the first failure is the
    /// reason; 60/2 is 30/1; ranges are never supported; only cameras are
    /// classified.
    #[test]
    fn core_rules_classify_each_mode_with_the_first_failing_reason() {
        let dir = TempDir::new();
        let path = dir.path().join("neat-core.json");
        fs::write(&path, core_rules().to_string()).unwrap();
        let range = json!({"format": "NV12", "framerate_num": 30, "framerate_den": 1,
            "size_range": {"min_width": 640, "min_height": 480, "max_width": 1920,
                           "max_height": 1080, "step_width": 16, "step_height": 8}});
        let mipi = json!([
            mode("NV12", 1920, (30, 1), true),
            mode("NV12", 1280, (30, 1), false),
            mode("RGB", 1920, (15, 1), false),
            mode("NV12", 1920, (60, 2), true),
            mode("NV12", 1920, (15, 1), false),
            range
        ]);
        let usb = camera("v4l2", json!([mode("YUYV", 1920, (15, 1), false)]));
        let mut microphone = camera("mipi", json!([{}]));
        microphone.kind = "microphone".into();
        let mut devices = [camera("mipi", mipi), usb, microphone];
        let mut issues = Vec::new();
        let status = SupportStage::new(&path).apply(&mut devices, &mut issues);
        assert_eq!(status.state, "applied");
        assert_eq!(status.source.unwrap(), "neat-core 0.4.0");
        assert!(issues.is_empty());
        let rules = core_rules();
        let reason = |rule: &str| rules["camera"][rule]["reason"].clone();
        let expected = [
            (true, json!("")),
            (false, reason("isp_output")),
            (false, reason("formats")),
            (true, json!("")),
            (false, reason("framerates")),
            (false, json!(RANGE_REASON)),
        ];
        assert_eq!(verdicts(&devices[0]), expected);
        assert_eq!(verdicts(&devices[1]), [(false, reason("backends"))]);
        assert_eq!(devices[2].details["modes"], json!([{}]), "not a camera");
    }

    /// Every rules-file state. Without valid rules every mode is unknown;
    /// `stale` (an invalid update) keeps the previous rules, and a newer
    /// format asks for a Sentinel update.
    #[test]
    fn rules_file_states() {
        let dir = TempDir::new();
        let path = dir.path().join("neat-core.json");
        let mut stage = SupportStage::new(&path);
        // `state source supported[, issue]` after writing `contents` ("" removes
        // the file), and the issues.
        let mut apply = |contents: &str| {
            let _ = fs::remove_file(&path);
            if !contents.is_empty() {
                fs::write(&path, contents).unwrap();
            }
            let mut devices = [camera("mipi", json!([mode("NV12", 1920, (30, 1), true)]))];
            let mut issues = Vec::new();
            let status = stage.apply(&mut devices, &mut issues);
            assert_eq!(status.path, path.display().to_string());
            let (supported, reason) = verdicts(&devices[0]).remove(0);
            assert_eq!(reason, if supported { "" } else { NOT_INSTALLED });
            let source = status.source.unwrap_or("-".into());
            let mut text = format!("{} {source} {supported}", status.state);
            for issue in &issues {
                let (provider, code) = (&issue.provider, &issue.code);
                let retained = issue.retained_last_good;
                text += &format!(", {provider} {code} retained={retained}");
            }
            (text, issues)
        };
        assert_eq!(apply("").0, "not_installed - false");
        let issue = "support peripherals.invalid_support_rules";
        let expected = format!("invalid - false, {issue} retained=false");
        assert_eq!(apply("not json").0, expected);
        let (text, _) = apply(&core_rules().to_string());
        assert_eq!(text, "applied neat-core 0.4.0 true");
        let (text, issues) = apply(r#"{"format": 2, "source": "x", "camera": {}}"#);
        let expected = format!("stale neat-core 0.4.0 true, {issue} retained=true");
        assert_eq!(text, expected);
        assert!(issues[0].reason.contains("format 2"));
        assert!(issues[0].reason.contains("sima-cli neat install sentinel"));
        assert_eq!(apply("").0, "not_installed - false");
    }

    #[test]
    fn watch_reports_changes_to_the_rules_file_only() {
        assert!(RulesWatch::open(Path::new("/nonexistent/sentinel/neat-core.json")).is_err());
        RulesWatch::open(Path::new("neat-core.json")).unwrap();
        let dir = TempDir::new();
        let path = dir.path().join("neat-core.json");
        let watch = RulesWatch::open(&path).unwrap();
        assert!(!watch.drain().unwrap());
        fs::write(dir.path().join("other.json"), "{}").unwrap();
        assert!(!watch.drain().unwrap(), "unrelated files are ignored");
        let staged = dir.path().join("neat-core.json.dpkg-new");
        fs::write(&staged, core_rules().to_string()).unwrap();
        fs::rename(&staged, &path).unwrap();
        assert!(watch.drain().unwrap(), "a package-manager rename");
        fs::remove_file(&path).unwrap();
        assert!(watch.drain().unwrap(), "removal is seen");

        fs::remove_dir_all(dir.path()).unwrap();
        let error = watch.drain().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

        // Renaming a watched directory keeps the watch attached to the old
        // inode and emits IN_MOVE_SELF without necessarily emitting
        // IN_IGNORED. The service must reopen against the replacement path.
        let root = TempDir::new();
        let directory = root.path().join("support");
        fs::create_dir(&directory).unwrap();
        let watch = RulesWatch::open(&directory.join("neat-core.json")).unwrap();
        fs::rename(&directory, root.path().join("support.old")).unwrap();
        fs::create_dir(&directory).unwrap();
        let error = watch.drain().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}
