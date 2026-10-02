use std::ffi::{CStr, CString};
use std::fs;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use super::model::{Provider, ProviderError, Record};
use super::scan::valid_type_token;

pub const PROTOCOL_VERSION: u32 = 1;
const DEFAULT_TIMEOUT_MS: u64 = 4_000;
const MAX_TIMEOUT_MS: u64 = 30_000;
const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 8 * 1024;

/// `<providers dir>/<name>.json`, describing one out-of-process provider.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub protocol: u32,
    pub name: String,
    pub exec: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub subsystems: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    pub user: String,
}

/// Who may own provider manifests and executables. Production requires root
/// so an unprivileged user cannot make the root daemon run their code.
#[derive(Debug, Clone, Copy)]
pub struct Trust {
    pub owner_uid: u32,
}

impl Trust {
    pub fn root() -> Self {
        Self { owner_uid: 0 }
    }
}

/// Load every `*.json` manifest in `dir`. A missing directory means no
/// external providers; a bad manifest is reported, never fatal.
pub fn load_providers(dir: &Path, trust: Trust) -> (Vec<ExternalProvider>, Vec<(PathBuf, String)>) {
    let mut providers = Vec::new();
    let mut rejected = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return (providers, rejected);
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    for path in paths {
        match load_manifest(&path, trust) {
            Ok(manifest) => providers.push(ExternalProvider::new(manifest)),
            Err(reason) => rejected.push((path, reason)),
        }
    }
    (providers, rejected)
}

fn load_manifest(path: &Path, trust: Trust) -> Result<Manifest, String> {
    check_trusted(path, trust)?;
    let data = fs::read(path).map_err(|error| format!("read failed: {error}"))?;
    let manifest: Manifest =
        serde_json::from_slice(&data).map_err(|error| format!("invalid manifest: {error}"))?;
    if manifest.protocol != PROTOCOL_VERSION {
        return Err(format!(
            "unsupported provider protocol {}",
            manifest.protocol
        ));
    }
    if manifest.name.is_empty() || manifest.user.is_empty() || manifest.subsystems.is_empty() {
        return Err("name, user and at least one subsystem are required".into());
    }
    if !manifest.exec.is_absolute() {
        return Err("exec must be an absolute path".into());
    }
    check_trusted(&manifest.exec, trust)?;
    Ok(manifest)
}

fn check_trusted(path: &Path, trust: Trust) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if metadata.uid() != trust.owner_uid {
        return Err(format!(
            "{} must be owned by uid {}",
            path.display(),
            trust.owner_uid
        ));
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(format!(
            "{} must not be writable by group or others",
            path.display()
        ));
    }
    Ok(())
}

/// Runs a provider executable for each scan: as the manifest's user, with
/// stdin closed, killed (with its process group) at the time limit, and with
/// bounded output. Its stdout must be one provider protocol v1 document.
pub struct ExternalProvider {
    manifest: Manifest,
}

impl ExternalProvider {
    pub fn new(manifest: Manifest) -> Self {
        Self { manifest }
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(
            self.manifest
                .timeout_ms
                .unwrap_or(DEFAULT_TIMEOUT_MS)
                .clamp(1, MAX_TIMEOUT_MS),
        )
    }

    fn spawn(&self) -> Result<Child, ProviderError> {
        let identity = resolve_user(&self.manifest.user)?;
        let mut command = Command::new(&self.manifest.exec);
        command
            .args(&self.manifest.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("HOME", &identity.home)
            .current_dir("/");
        let switch_user = current_uid() != identity.uid;
        // SAFETY: only async-signal-safe system calls run between fork and
        // exec; the group list was resolved before forking.
        unsafe {
            command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if switch_user {
                    if libc::setgroups(identity.groups.len() as _, identity.groups.as_ptr()) != 0
                        || libc::setgid(identity.gid) != 0
                        || libc::setuid(identity.uid) != 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        command.spawn().map_err(|error| {
            ProviderError::new(
                "peripherals.provider_unavailable",
                format!("could not start {}: {error}", self.manifest.exec.display()),
            )
        })
    }
}

impl Provider for ExternalProvider {
    fn name(&self) -> &str {
        &self.manifest.name
    }

    fn subsystems(&self) -> &[String] {
        &self.manifest.subsystems
    }

    fn discover(&mut self) -> Result<Vec<Record>, ProviderError> {
        let mut child = self.spawn()?;
        let output = collect_output(&mut child, self.timeout());
        let pid = child.id() as libc::pid_t;
        let status = match output {
            Ok(_) => child.wait().ok(),
            Err(_) => {
                // SAFETY: the child leads its own process group (setpgid above).
                unsafe { libc::kill(-pid, libc::SIGKILL) };
                let _ = child.wait();
                None
            }
        };
        let (stdout, stderr) = output?;
        parse_response(&self.manifest.name, &stdout).map_err(|error| {
            let detail = String::from_utf8_lossy(&stderr);
            let detail = detail.trim();
            match (status.and_then(|status| status.code()), detail.is_empty()) {
                (Some(0), _) | (_, true) => error,
                (code, false) => ProviderError::new(
                    "peripherals.discovery_failed",
                    format!(
                        "provider exited with status {}: {}",
                        code.map_or("signal".to_string(), |code| code.to_string()),
                        last_line(detail)
                    ),
                ),
            }
        })
    }
}

/// Stands in for a manifest that could not be loaded, so the problem is
/// visible as a catalog issue on every scan instead of only in the journal.
pub struct RejectedManifest {
    name: String,
    reason: String,
}

impl RejectedManifest {
    pub fn new(path: &Path, reason: String) -> Self {
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        Self {
            name: format!("manifest:{stem}"),
            reason: format!("{}: {reason}", path.display()),
        }
    }
}

impl Provider for RejectedManifest {
    fn name(&self) -> &str {
        &self.name
    }

    fn subsystems(&self) -> &[String] {
        &[]
    }

    fn discover(&mut self) -> Result<Vec<Record>, ProviderError> {
        Err(ProviderError::new(
            "peripherals.provider_rejected",
            self.reason.clone(),
        ))
    }
}

type Output = (Vec<u8>, Vec<u8>);

fn collect_output(child: &mut Child, timeout: Duration) -> Result<Output, ProviderError> {
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let deadline = Instant::now() + timeout;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_open, mut err_open) = (true, true);
    let mut buffer = [0u8; 16 * 1024];
    while out_open || err_open {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ProviderError::new(
                "peripherals.provider_timeout",
                format!("provider did not finish within {} ms", timeout.as_millis()),
            ));
        }
        let mut fds = [
            libc::pollfd {
                fd: if out_open { stdout.as_raw_fd() } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if err_open { stderr.as_raw_fd() } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let wait_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        // SAFETY: fds is a valid array of two pollfd structures.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, wait_ms) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(ProviderError::new(
                "peripherals.discovery_failed",
                format!("waiting for provider output failed: {error}"),
            ));
        }
        if out_open && fds[0].revents != 0 {
            match stdout.read(&mut buffer) {
                Ok(0) | Err(_) => out_open = false,
                Ok(count) => {
                    out.extend_from_slice(&buffer[..count]);
                    if out.len() > MAX_STDOUT_BYTES {
                        return Err(ProviderError::new(
                            "peripherals.invalid_provider_result",
                            "provider output exceeds 4 MiB",
                        ));
                    }
                }
            }
        }
        if err_open && fds[1].revents != 0 {
            match stderr.read(&mut buffer) {
                Ok(0) | Err(_) => err_open = false,
                Ok(count) => {
                    err.extend_from_slice(&buffer[..count]);
                    if err.len() > MAX_STDERR_BYTES {
                        err.drain(..err.len() - MAX_STDERR_BYTES);
                    }
                }
            }
        }
    }
    Ok((out, err))
}

#[derive(Deserialize)]
struct Response {
    schema_version: u32,
    ok: bool,
    #[serde(default)]
    records: Option<Vec<WireRecord>>,
    #[serde(default)]
    error: Option<WireError>,
}

#[derive(Deserialize)]
struct WireRecord {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    provider: String,
    details: Value,
}

#[derive(Deserialize)]
struct WireError {
    code: String,
    reason: String,
}

fn parse_response(provider: &str, stdout: &[u8]) -> Result<Vec<Record>, ProviderError> {
    let invalid =
        |reason: String| ProviderError::new("peripherals.invalid_provider_result", reason);
    let response: Response = serde_json::from_slice(stdout)
        .map_err(|error| invalid(format!("{provider} returned invalid JSON: {error}")))?;
    if response.schema_version != PROTOCOL_VERSION {
        return Err(invalid(format!(
            "{provider} returned unsupported schema_version {}",
            response.schema_version
        )));
    }
    if !response.ok {
        let error = response
            .error
            .ok_or_else(|| invalid(format!("{provider} reported failure without an error")))?;
        return Err(ProviderError::new(error.code, error.reason));
    }
    let records = response
        .records
        .ok_or_else(|| invalid(format!("{provider} returned no records array")))?;
    records
        .into_iter()
        .map(|record| {
            if !valid_type_token(&record.kind) {
                return Err(invalid(format!(
                    "{provider} returned invalid type {:?}",
                    record.kind
                )));
            }
            Ok(Record {
                id: record.id,
                kind: record.kind,
                provider: record.provider,
                details: record.details,
            })
        })
        .collect()
}

fn last_line(text: &str) -> &str {
    text.lines().last().unwrap_or(text)
}

struct Identity {
    uid: libc::uid_t,
    gid: libc::gid_t,
    groups: Vec<libc::gid_t>,
    home: String,
}

fn current_uid() -> libc::uid_t {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() }
}

fn resolve_user(name: &str) -> Result<Identity, ProviderError> {
    let unavailable =
        |reason: String| ProviderError::new("peripherals.provider_unavailable", reason);
    let c_name = CString::new(name).map_err(|_| unavailable("invalid user name".into()))?;
    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: all pointers reference live, correctly sized buffers.
    let status = unsafe {
        libc::getpwnam_r(
            c_name.as_ptr(),
            &mut passwd,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return Err(unavailable(format!(
            "provider user {name:?} does not exist"
        )));
    }
    let home = unsafe { CStr::from_ptr(passwd.pw_dir) }
        .to_string_lossy()
        .into_owned();
    let mut groups = vec![0 as libc::gid_t; 64];
    let mut count = groups.len() as libc::c_int;
    // SAFETY: groups holds `count` entries; getgrouplist updates count.
    let found = unsafe {
        libc::getgrouplist(
            c_name.as_ptr(),
            passwd.pw_gid,
            groups.as_mut_ptr(),
            &mut count,
        )
    };
    if found < 0 {
        groups.resize(count.max(0) as usize, 0);
        // SAFETY: as above, with the size the first call reported.
        if unsafe {
            libc::getgrouplist(
                c_name.as_ptr(),
                passwd.pw_gid,
                groups.as_mut_ptr(),
                &mut count,
            )
        } < 0
        {
            return Err(unavailable(format!("could not resolve groups of {name:?}")));
        }
    }
    groups.truncate(count.max(0) as usize);
    Ok(Identity {
        uid: passwd.pw_uid,
        gid: passwd.pw_gid,
        groups,
        home,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "sentinel-external-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
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

    fn current_user() -> String {
        // SAFETY: getpwuid returns a pointer into static storage or null.
        let entry = unsafe { libc::getpwuid(libc::getuid()) };
        assert!(!entry.is_null());
        unsafe { CStr::from_ptr((*entry).pw_name) }
            .to_string_lossy()
            .into_owned()
    }

    fn trust() -> Trust {
        Trust {
            owner_uid: current_uid(),
        }
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn manifest(dir: &Path, exec: &Path, timeout_ms: u64) -> PathBuf {
        let path = dir.join("test.json");
        let document = serde_json::json!({
            "protocol": 1,
            "name": "test.camera",
            "exec": exec,
            "subsystems": ["video4linux", "media"],
            "timeout_ms": timeout_ms,
            "user": current_user(),
        });
        fs::write(&path, document.to_string()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        path
    }

    fn make_provider(body: &str, timeout_ms: u64) -> (TempDir, ExternalProvider) {
        let dir = TempDir::new();
        let exec = script(&dir.0, "provider", body);
        manifest(&dir.0, &exec, timeout_ms);
        let (mut providers, rejected) = load_providers(&dir.0, trust());
        assert!(rejected.is_empty(), "{rejected:?}");
        (dir, providers.remove(0))
    }

    #[test]
    fn successful_response_becomes_records() {
        let (_dir, mut provider) = make_provider(
            r#"echo '{"schema_version":1,"ok":true,"records":[{"id":"camera:imx477 5-001a","type":"camera","provider":"test.camera","details":{"camera_name":"imx477 5-001a"}}]}'"#,
            2_000,
        );
        assert_eq!(provider.subsystems(), ["video4linux", "media"]);
        let records = provider.discover().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].details["camera_name"], "imx477 5-001a");
    }

    #[test]
    fn structured_failure_is_passed_through() {
        let (_dir, mut provider) = make_provider(
            r#"echo '{"schema_version":1,"ok":false,"error":{"code":"io.permission_denied","reason":"permission denied accessing /dev/media0"}}'; exit 2"#,
            2_000,
        );
        let error = provider.discover().unwrap_err();
        assert_eq!(error.code, "io.permission_denied");
        assert!(error.reason.contains("/dev/media0"));
    }

    #[test]
    fn hung_provider_is_killed_at_the_time_limit() {
        let (_dir, mut provider) = make_provider("sleep 30", 200);
        let started = Instant::now();
        let error = provider.discover().unwrap_err();
        assert_eq!(error.code, "peripherals.provider_timeout");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn crash_without_json_reports_status_and_stderr() {
        let (_dir, mut provider) =
            make_provider("echo 'libcamera: pipeline failed' >&2; exit 3", 2_000);
        let error = provider.discover().unwrap_err();
        assert_eq!(error.code, "peripherals.discovery_failed");
        assert!(error.reason.contains("status 3"), "{}", error.reason);
        assert!(error.reason.contains("pipeline failed"));
    }

    #[test]
    fn malformed_output_is_an_invalid_result() {
        let (_dir, mut provider) = make_provider("echo not-json", 2_000);
        assert_eq!(
            provider.discover().unwrap_err().code,
            "peripherals.invalid_provider_result"
        );
        let (_dir, mut provider) = make_provider(
            r#"echo '{"schema_version":2,"ok":true,"records":[]}'"#,
            2_000,
        );
        assert!(provider
            .discover()
            .unwrap_err()
            .reason
            .contains("schema_version 2"));
    }

    #[test]
    fn untrusted_or_invalid_manifests_are_rejected() {
        let dir = TempDir::new();
        let exec = script(&dir.0, "provider", "true");
        let path = manifest(&dir.0, &exec, 1_000);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        let (providers, rejected) = load_providers(&dir.0, trust());
        assert!(providers.is_empty());
        assert!(rejected[0].1.contains("writable"));

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let (_, rejected) = load_providers(
            &dir.0,
            Trust {
                owner_uid: u32::MAX,
            },
        );
        assert!(rejected[0].1.contains("owned by"));

        fs::write(
            &path,
            r#"{"protocol":1,"name":"x","exec":"relative","subsystems":["media"],"user":"root"}"#,
        )
        .unwrap();
        let (_, rejected) = load_providers(&dir.0, trust());
        assert!(rejected[0].1.contains("absolute"));
    }

    #[test]
    fn missing_directory_means_no_external_providers() {
        let (providers, rejected) = load_providers(Path::new("/nonexistent/sentinel"), trust());
        assert!(providers.is_empty() && rejected.is_empty());
    }
}
