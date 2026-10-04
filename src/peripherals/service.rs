use std::fs;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::catalog::{Catalog, DEFAULT_CHANGE_CAPACITY};
use super::model::{CatalogDocument, Provider};
use super::scan::Scanner;
use super::support::{RulesWatch, SupportStage};
use super::uevent::UeventSocket;

pub const DEBOUNCE: Duration = Duration::from_millis(250);
/// Discovery yields to camera pipelines on a busy board. Scan threads inherit
/// this from the peripherals thread.
pub const NICE: libc::c_int = 10;
/// A steady stream of events cannot postpone a scan by more than this.
pub const MAX_EVENT_WAIT: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Schedule {
    completed: u64,
    scanning: bool,
    refresh: bool,
    stop: bool,
    /// False once the thread has exited, so refreshes are refused instead of
    /// promising a scan that will never run.
    alive: bool,
}

/// Shared between the peripherals thread and its callers (API, daemon).
pub struct Control {
    schedule: Mutex<Schedule>,
    wake_read: OwnedFd,
    wake_write: OwnedFd,
}

impl Control {
    fn new() -> io::Result<Self> {
        let mut fds = [0; 2];
        // SAFETY: fds has room for the two descriptors pipe2 returns.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both descriptors were just created and are owned here.
        let (wake_read, wake_write) =
            unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        Ok(Self {
            schedule: Mutex::new(Schedule::default()),
            wake_read,
            wake_write,
        })
    }

    /// The schedule, even if a thread panicked while holding it.
    fn schedule(&self) -> MutexGuard<'_, Schedule> {
        self.schedule.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ask for a scan now. Returns the `scan_sequence` whose completion
    /// guarantees the result reflects the hardware as of this call, or `None`
    /// if the peripherals thread is no longer running.
    pub fn request_refresh(&self) -> Option<u64> {
        let target = {
            let mut schedule = self.schedule();
            if !schedule.alive {
                return None;
            }
            schedule.refresh = true;
            schedule.completed + if schedule.scanning { 2 } else { 1 }
        };
        self.wake();
        Some(target)
    }

    fn request_stop(&self) {
        self.schedule().stop = true;
        self.wake();
    }

    fn wake(&self) {
        let byte = 1u8;
        // SAFETY: writing one byte from a live buffer. A full pipe already
        // guarantees a pending wake-up, so EAGAIN is ignored.
        unsafe {
            libc::write(
                self.wake_write.as_raw_fd(),
                &byte as *const u8 as *const libc::c_void,
                1,
            );
        }
    }

    fn drain_wake(&self) {
        let mut buffer = [0u8; 64];
        // SAFETY: reading into a live buffer from a non-blocking pipe.
        while unsafe {
            libc::read(
                self.wake_read.as_raw_fd(),
                buffer.as_mut_ptr() as *mut libc::c_void,
                buffer.len(),
            )
        } > 0
        {}
    }
}

pub struct Config {
    pub catalog_path: PathBuf,
    pub support_rules_path: PathBuf,
    pub instance_id: String,
    pub debounce: Duration,
    /// Tests turn this off so host hot-plug events cannot trigger scans.
    pub listen_for_uevents: bool,
}

pub struct PeripheralsHandle {
    control: Arc<Control>,
    thread: JoinHandle<()>,
}

impl PeripheralsHandle {
    pub fn control(&self) -> Arc<Control> {
        self.control.clone()
    }

    pub fn stop(self) {
        self.control.request_stop();
        if self.thread.join().is_err() {
            eprintln!("Sentinel peripherals thread panicked");
        }
    }
}

/// Publish a `starting` catalog, then run the event loop on its own thread.
pub fn spawn(config: Config, providers: Vec<Box<dyn Provider>>) -> Result<PeripheralsHandle> {
    let control = Arc::new(Control::new().context("create peripherals wake pipe")?);
    let mut catalog = Catalog::new(config.instance_id.clone(), DEFAULT_CHANGE_CAPACITY);
    let scanner = Scanner::new(
        providers,
        SupportStage::new(config.support_rules_path.clone()),
    );
    let uevents = match config
        .listen_for_uevents
        .then(|| UeventSocket::open(scanner.subsystems()))
    {
        None => None,
        Some(Ok(socket)) => Some(socket),
        Some(Err(error)) => {
            eprintln!("Sentinel cannot watch hot-plug events: {error}");
            catalog.apply_error(
                "peripherals.monitor_failed",
                &format!(
                    "hot-plug events are unavailable; only explicit refreshes rescan: {error}"
                ),
            );
            None
        }
    };
    // Without the directory (Core not installed yet), rules are still re-read
    // on every scan; Core's installer also requests a refresh.
    let rules_watch = RulesWatch::open(scanner.support_path())
        .map_err(|error| {
            eprintln!(
                "Sentinel is not watching {}: {error}",
                scanner.support_path().display()
            )
        })
        .ok();
    publish(&config.catalog_path, &catalog.document())?;
    control.schedule().alive = true;
    let thread_control = control.clone();
    let thread = thread::Builder::new()
        .name("peripherals".into())
        .spawn(move || {
            run(
                config,
                thread_control,
                catalog,
                scanner,
                uevents,
                rules_watch,
            )
        })
        .context("spawn peripherals thread")?;
    Ok(PeripheralsHandle { control, thread })
}

/// Marks the thread dead when it exits for any reason, including a panic.
struct AliveGuard(Arc<Control>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.schedule().alive = false;
    }
}

fn run(
    config: Config,
    control: Arc<Control>,
    mut catalog: Catalog,
    mut scanner: Scanner,
    mut uevents: Option<UeventSocket>,
    mut rules_watch: Option<RulesWatch>,
) {
    let _alive = AliveGuard(control.clone());
    lower_priority();
    // The initial scan runs immediately; afterwards the thread sleeps in
    // poll() until a uevent, a rules change, a refresh request, or shutdown.
    let mut due = Some(Instant::now());
    let mut reclassify_due: Option<Instant> = None;
    // A refresh is waiting on `due`: later events may not postpone it.
    let mut refresh_pending = false;
    // When the current burst of events began, to cap how long it can delay.
    let mut burst_start: Option<Instant> = None;
    // Refresh requests within one debounce window of the last scan share the
    // next scan, so a client that spams refresh cannot keep the thread busy.
    let mut last_scan_end: Option<Instant> = None;
    loop {
        // A pending scan re-reads the rules, so it supersedes a reclassify.
        let next = due.or(reclassify_due);
        let timeout = next.map_or(-1, |at| {
            at.saturating_duration_since(Instant::now())
                .as_millis()
                .min(i32::MAX as u128) as libc::c_int
        });
        let mut fds = [
            libc::pollfd {
                fd: control.wake_read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: uevents.as_ref().map_or(-1, UeventSocket::as_raw_fd),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: rules_watch.as_ref().map_or(-1, RulesWatch::as_raw_fd),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: fds is a valid array of three pollfd structures.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 3, timeout) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                eprintln!("Sentinel peripherals thread stopped: poll failed: {error}");
                catalog.apply_error(
                    "peripherals.monitor_failed",
                    &format!("waiting for events failed: {error}"),
                );
                let _ = publish(&config.catalog_path, &catalog.document());
                return;
            }
        }
        if fds[0].revents != 0 {
            control.drain_wake();
        }
        {
            let mut schedule = control.schedule();
            if schedule.stop {
                drop(schedule);
                // Like the metrics cache, leave no catalog that looks live.
                catalog.apply_error("peripherals.stopped", "The Sentinel daemon has stopped.");
                let _ = publish(&config.catalog_path, &catalog.document());
                return;
            }
            if std::mem::take(&mut schedule.refresh) {
                let earliest = last_scan_end.map_or_else(Instant::now, |end| {
                    (end + config.debounce).max(Instant::now())
                });
                due = Some(due.map_or(earliest, |at| at.min(earliest)));
                refresh_pending = true;
            }
        }
        if fds[1].revents != 0 {
            if let Some(socket) = &uevents {
                match socket.drain() {
                    Ok(drained) if drained.matched || drained.overflowed => {
                        // Trailing debounce, capped so a flapping device cannot
                        // postpone the scan forever; never delays a refresh.
                        let now = Instant::now();
                        let start = *burst_start.get_or_insert(now);
                        due = Some(scan_due_after_event(
                            now,
                            start,
                            config.debounce,
                            due,
                            refresh_pending,
                        ));
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("Sentinel stopped receiving hot-plug events: {error}");
                        catalog.apply_error(
                            "peripherals.monitor_failed",
                            &format!("hot-plug events stopped: {error}"),
                        );
                        uevents = None;
                        let _ = publish(&config.catalog_path, &catalog.document());
                    }
                }
            }
        }
        if fds[2].revents != 0 {
            if let Some(watch) = &rules_watch {
                match watch.drain() {
                    // A package install writes in steps; wait for it to settle.
                    Ok(true) => reclassify_due = Some(Instant::now() + config.debounce),
                    Ok(false) => {}
                    Err(error) => {
                        eprintln!("Sentinel stopped watching support rules: {error}");
                        rules_watch = None;
                    }
                }
            }
        }
        if due.is_none() && reclassify_due.is_some_and(|at| Instant::now() >= at) {
            reclassify_due = None;
            if let Err(error) = scanner.reclassify(&mut catalog) {
                eprintln!("Sentinel peripheral reclassification rejected: {error}");
            }
            if let Err(error) = publish(&config.catalog_path, &catalog.document()) {
                eprintln!("Sentinel peripheral catalog write failed: {error:#}");
            }
        }
        if due.is_some_and(|at| Instant::now() >= at) {
            due = None;
            refresh_pending = false;
            burst_start = None;
            // A scan re-reads the rules too.
            reclassify_due = None;
            control.schedule().scanning = true;
            if let Err(error) = scanner.scan(&mut catalog) {
                eprintln!("Sentinel peripheral scan rejected: {error}");
            }
            {
                let mut schedule = control.schedule();
                schedule.scanning = false;
                schedule.completed = catalog.scan_sequence();
            }
            if let Err(error) = publish(&config.catalog_path, &catalog.document()) {
                eprintln!("Sentinel peripheral catalog write failed: {error:#}");
            }
            last_scan_end = Some(Instant::now());
        }
    }
}

/// When to scan after a hot-plug event: a trailing debounce, capped at
/// `MAX_EVENT_WAIT` after the burst began, and never later than a refresh
/// that is already waiting.
fn scan_due_after_event(
    now: Instant,
    burst_start: Instant,
    debounce: Duration,
    due: Option<Instant>,
    refresh_pending: bool,
) -> Instant {
    let event_due = (now + debounce).min(burst_start + MAX_EVENT_WAIT);
    match due {
        Some(at) if refresh_pending => at.min(event_due),
        _ => event_due,
    }
}

fn lower_priority() {
    // SAFETY: gettid has no arguments; get/setpriority target only this
    // thread. Raise niceness relative to the daemon's own, capped at 19.
    let result = unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        let current = libc::getpriority(libc::PRIO_PROCESS, tid);
        libc::setpriority(libc::PRIO_PROCESS, tid, (current + NICE).min(19))
    };
    if result != 0 {
        eprintln!(
            "Sentinel could not lower the peripherals thread priority: {}",
            io::Error::last_os_error()
        );
    }
}

/// Replace the catalog file atomically so readers never see a partial write.
pub fn publish(path: &Path, document: &CatalogDocument) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("peripherals.json")
    ));
    {
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        serde_json::to_writer(&mut file, document).context("serialize peripheral catalog")?;
        file.write_all(b"\n")?;
    }
    fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

pub fn read(path: &Path) -> Result<CatalogDocument> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&data).with_context(|| format!("parse {}", path.display()))
}

/// A new identifier per daemon start, so clients can detect restarts.
pub fn new_instance_id() -> String {
    fs::read_to_string("/proc/sys/kernel/random/uuid")
        .map(|uuid| uuid.trim().to_string())
        .unwrap_or_else(|_| {
            format!(
                "{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peripherals::model::{ProviderError, Record};
    use crate::peripherals::sysutil::testing::TempDir;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting {
        calls: Arc<AtomicUsize>,
        subsystems: Vec<String>,
    }

    impl Provider for Counting {
        fn name(&self) -> &str {
            "test.camera"
        }
        fn subsystems(&self) -> &[String] {
            &self.subsystems
        }
        fn discover(&mut self) -> Result<Vec<Record>, ProviderError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok((0..=call)
                .map(|index| Record {
                    id: format!("camera:{index}"),
                    kind: "camera".into(),
                    provider: "test.camera".into(),
                    details: json!({}),
                })
                .collect())
        }
    }

    /// Start the thread on `<root>/peripherals.json`, with uevents off.
    fn start(
        root: &Path,
        debounce_ms: u64,
        providers: Vec<Box<dyn Provider>>,
    ) -> (PathBuf, PeripheralsHandle) {
        let path = root.join("peripherals.json");
        let config = Config {
            catalog_path: path.clone(),
            support_rules_path: root.join("support/neat-core.json"),
            instance_id: "test-instance".into(),
            debounce: Duration::from_millis(debounce_ms),
            listen_for_uevents: false,
        };
        (path, spawn(config, providers).unwrap())
    }

    fn wait_for(path: &Path, predicate: impl Fn(&CatalogDocument) -> bool) -> CatalogDocument {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(document) = read(path) {
                if predicate(&document) {
                    return document;
                }
            }
            assert!(
                Instant::now() < deadline,
                "catalog never reached the expected state"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn publishes_initial_scan_and_honours_refresh_targets() {
        let root = TempDir::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let counting = Counting {
            calls: calls.clone(),
            subsystems: vec!["video4linux".into()],
        };
        let (path, handle) = start(root.path(), 10, vec![Box::new(counting)]);

        let first = wait_for(&path, |document| document.ready);
        assert_eq!(first.instance_id, "test-instance");
        assert_eq!((first.revision, first.scan_sequence), (1, 1));
        assert_eq!(first.devices.len(), 1);

        let target = handle.control().request_refresh().unwrap();
        assert_eq!(target, 2);
        let refreshed = wait_for(&path, |document| document.scan_sequence >= target);
        assert_eq!(refreshed.revision, 2);
        assert_eq!(refreshed.devices.len(), 2);
        assert_eq!(refreshed.changes.last().unwrap().kind, "added");

        handle.stop();
        assert_eq!(calls.load(Ordering::SeqCst), 2, "no scan without a trigger");
    }

    struct OneMipiCamera;

    impl Provider for OneMipiCamera {
        fn name(&self) -> &str {
            "test.mipi"
        }
        fn subsystems(&self) -> &[String] {
            &[]
        }
        fn discover(&mut self) -> Result<Vec<Record>, ProviderError> {
            Ok(vec![Record {
                id: "camera:imx477 5-001a".into(),
                kind: "camera".into(),
                provider: "test.mipi".into(),
                details: json!({"backend": "mipi", "modes": [{
                    "format": "NV12", "width": 1920, "height": 1080,
                    "framerate_num": 30, "framerate_den": 1, "isp_output": true
                }]}),
            }])
        }
    }

    #[test]
    fn installing_core_rules_reclassifies_without_rescanning() {
        let root = TempDir::new();
        let rules_dir = root.path().join("support");
        fs::create_dir_all(&rules_dir).unwrap();
        let (path, handle) = start(root.path(), 10, vec![Box::new(OneMipiCamera)]);

        let before = wait_for(&path, |document| document.ready);
        assert_eq!(before.support.as_ref().unwrap().state, "not_installed");
        assert_eq!(before.devices[0]["camera"]["modes"][0]["supported"], false);

        let staged = rules_dir.join("neat-core.json.dpkg-new");
        fs::write(
            &staged,
            json!({"format": 1, "source": "neat-core 0.4.0", "camera": {
                "backends": {"accept": ["mipi"], "reason": "MIPI only."},
                "formats": {"accept": ["NV12"], "reason": "NV12 only."},
                "framerates": {"accept": [{"num": 30, "den": 1}], "reason": "30/1 only."},
                "isp_output": {"reason": "Not an ISP output size."}
            }})
            .to_string(),
        )
        .unwrap();
        fs::rename(&staged, rules_dir.join("neat-core.json")).unwrap();

        let after = wait_for(&path, |document| document.revision > before.revision);
        assert_eq!(after.devices[0]["camera"]["modes"][0]["supported"], true);
        assert_eq!(
            after.support.as_ref().unwrap().source.as_deref(),
            Some("neat-core 0.4.0")
        );
        assert_eq!(
            after.scan_sequence, before.scan_sequence,
            "no hardware rescan"
        );
        assert_eq!(after.changes.last().unwrap().kind, "changed");

        handle.stop();
    }

    struct NiceProbe(Arc<std::sync::Mutex<Option<i32>>>);

    impl Provider for NiceProbe {
        fn name(&self) -> &str {
            "test.nice"
        }
        fn subsystems(&self) -> &[String] {
            &[]
        }
        fn discover(&mut self) -> Result<Vec<Record>, ProviderError> {
            // SAFETY: reads this thread's own scheduling priority.
            let nice = unsafe {
                libc::getpriority(
                    libc::PRIO_PROCESS,
                    libc::syscall(libc::SYS_gettid) as libc::id_t,
                )
            };
            *self.0.lock().unwrap() = Some(nice);
            Ok(Vec::new())
        }
    }

    #[test]
    fn scans_run_at_lower_priority() {
        let root = TempDir::new();
        let seen = Arc::new(std::sync::Mutex::new(None));
        let (path, handle) = start(root.path(), 10, vec![Box::new(NiceProbe(seen.clone()))]);
        wait_for(&path, |document| document.ready);
        handle.stop();
        // The process may already run niced; discovery adds NICE on top.
        let base = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
        assert_eq!(*seen.lock().unwrap(), Some((base + NICE).min(19)));
    }

    #[test]
    fn a_burst_of_refresh_requests_shares_one_scan() {
        let root = TempDir::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let counting = Counting {
            calls: calls.clone(),
            subsystems: vec![],
        };
        let (path, handle) = start(root.path(), 100, vec![Box::new(counting)]);
        wait_for(&path, |document| document.ready);
        let control = handle.control();
        let mut target = 0;
        for _ in 0..200 {
            target = control.request_refresh().unwrap();
        }
        let document = wait_for(&path, |document| document.scan_sequence >= target);
        handle.stop();
        assert!(
            target <= 3,
            "200 requests promise at most two more scans, got {target}"
        );
        assert!(
            calls.load(Ordering::SeqCst) <= 3,
            "200 refresh requests ran {} scans",
            calls.load(Ordering::SeqCst)
        );
        assert!(document.scan_sequence <= 3);
    }

    #[test]
    fn events_cannot_postpone_a_scan_forever_or_delay_a_refresh() {
        let start = Instant::now();
        let debounce = Duration::from_millis(250);
        // Trailing debounce inside a burst.
        assert_eq!(
            scan_due_after_event(start, start, debounce, None, false),
            start + debounce
        );
        // An event stream 900 ms in is capped at one second after it began.
        let late = start + Duration::from_millis(900);
        assert_eq!(
            scan_due_after_event(late, start, debounce, Some(late), false),
            start + MAX_EVENT_WAIT
        );
        // A refresh due now is not pushed back by a new event.
        assert_eq!(
            scan_due_after_event(start, start, debounce, Some(start), true),
            start
        );
    }

    #[test]
    fn stopping_marks_the_catalog_stopped_and_refuses_refreshes() {
        let root = TempDir::new();
        let (path, handle) = start(root.path(), 10, vec![]);
        wait_for(&path, |document| document.ready);
        let control = handle.control();
        handle.stop();
        let document = read(&path).unwrap();
        assert_eq!(document.state, "degraded");
        assert_eq!(document.error.unwrap()["code"], "peripherals.stopped");
        assert_eq!(control.request_refresh(), None);
    }
}
